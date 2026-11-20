//! `cost exec-v3 --v3-elf P --trace-elf P --attest-elf P --json OUT`: every step of the staged public verifier
//! (`lc_verify_v3`) in the RISC Zero 3.0.5 executor, as LEZ runs an actor (a `ReceiveInput` frame per step, the job
//! state carried from step to step), on real mainnet and Sepolia updates and on fabricated updates at exactly 342
//! signers. `Finish` runs with its chained `attest.Apply` and `WriteRecord`, executed by the `attest` guest on the
//! instance state, so a step's transaction total is what one public transaction meters.
//!
//! Per step: `metered_cycles` (user cycles of the deployed build of `lc_verify_v3`, the gas a public transaction is
//! charged for it), the phases inside it (from a measurement build with cycle marks, `--features cycle-trace`), their
//! sum and the difference (program entry, message and state decode, state encode, journal), and for `Finish` the
//! chained `attest` cycles. Every executor result is checked against V1 (native Helios 0.12.0): same pre-store,
//! post-store, head and record, and the `attest` instance head after `Apply` equals V1's.
//!
//! Output is deterministic for given ELFs and fixtures (no times).

use anyhow::{Context, Result, anyhow, bail, ensure};
use attest_core::{
    AccountId, AttestMessage, InstanceConfig, InstanceState, LcVerifyV3Message, instance_id,
    lc::{self, StoreWitness},
    lc_v3::{self, StoreWitnessV3},
    rules,
};
use lee_core::{
    account::{Actor, ActorState},
    program::ReceiveInput,
};
use risc0_zkvm::{ExecutorEnv, default_executor};
use serde_json::{Value, json};

use crate::{Guest, load, state_of, write_record};

const CAP: u64 = 10_000_000;
/// Gas limit the staged submitter declares per step (`attest_host::V3_STEP_GAS`).
const DECLARED: u64 = 9_000_000;
/// Gas declared per step (`attest_host::V3_STEP_GAS`, `V3_MILLER_GAS`, `V3_FINISH_GAS`).
fn declared(step: &str) -> u64 {
    match step {
        "Miller" => 7_000_000,
        "Finish" => 5_500_000,
        _ => DECLARED,
    }
}
/// The plan's ceiling per step: 90% of the cap.
const STEP_LIMIT: u64 = 9_000_000;
/// `user ELF` size limit for deployment (20 segments of 96 KiB).
const ELF_CAP: usize = 20 * 96 * 1024;

const ATTEST: AccountId = AccountId::new([0xA7; 32]);
const V3: AccountId = AccountId::new([0x33; 32]);

struct Exec {
    cycles: u64,
    padded: u64,
    segments: usize,
    post_state: Vec<u8>,
    sync: Vec<(Actor, Vec<u8>)>,
    stderr: String,
}

fn run(g: &Guest, input: &ReceiveInput) -> Result<Exec> {
    let frame = lee_core::to_frame(&borsh::to_vec(input)?);
    let mut err = Vec::new();
    let info = {
        let env = ExecutorEnv::builder().write_slice(&frame).stderr(&mut err).build()?;
        default_executor().execute(env, &g.program)?
    };
    let journal = lee_core::from_frame(&info.journal.bytes)
        .ok_or_else(|| anyhow!("no journal: the step was refused (exit {:?})", info.exit_code))?;
    let t: lee_core::program::Transition = borsh::from_slice(journal).context("transition")?;
    Ok(Exec {
        cycles: info.cycles(),
        padded: info.segments.iter().map(|s| 1u64 << s.po2).sum(),
        segments: info.segments.len(),
        post_state: state_of(&t),
        sync: t
            .response
            .sync_messages
            .iter()
            .map(|m| (m.to, m.message.clone()))
            .collect(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

/// `(label, delta)` from `v3-mark <label> <cycles>` lines, between `start` and `end`.
fn marks(stderr: &str) -> (Marks, u64) {
    let mut out = Vec::new();
    let mut last = None;
    let mut first = None;
    for line in stderr.lines() {
        let mut p = line.split_whitespace();
        if p.next() != Some("v3-mark") {
            continue;
        }
        let (Some(label), Some(Ok(c))) = (p.next(), p.next().map(str::parse::<u64>)) else {
            continue;
        };
        if let Some(l) = last {
            out.push((label.to_string(), c - l));
        } else {
            first = Some(c);
        }
        last = Some(c);
    }
    let total = match (first, last) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    (out, total)
}

struct Ctx<'a> {
    v3: &'a Guest,
    trace: &'a Guest,
    attest: &'a Guest,
}

/// `(phase, cycles)` marks of one step.
type Marks = Vec<(String, u64)>;

/// Runs one step on both builds; checks that the measurement build did exactly what the deployed build did.
fn step(c: &Ctx, job: AccountId, pre_state: &[u8], msg: &LcVerifyV3Message) -> Result<(Exec, Marks, u64)> {
    let input = ReceiveInput {
        receiver: Actor::new(job, V3),
        from: None,
        is_authorized: false,
        pre_state: ActorState::from(pre_state.to_vec()),
        message: borsh::to_vec(msg)?,
    };
    let e = run(c.v3, &input)?;
    let t = run(c.trace, &input)?;
    ensure!(
        t.post_state == e.post_state && t.sync == e.sync,
        "measurement build differs from the deployed build"
    );
    let (m, sum) = marks(&t.stderr);
    Ok((e, m, sum))
}

#[allow(clippy::too_many_arguments)]
fn update_rows(
    c: &Ctx,
    network: &str,
    label: &str,
    kind: &str,
    config: &InstanceConfig,
    w: &StoreWitness,
    u: &[u8],
    state: &mut Vec<u8>,
    four_steps: bool,
    rows: &mut Vec<Value>,
) -> Result<()> {
    let native = lc::verify_and_apply(config, w, u).map_err(|e| anyhow!("{network} {label}: V1 refused: {e}"))?;
    let instance = instance_id(&ATTEST, config);
    let w3 = StoreWitnessV3::from_v1(w, native.signature_slot);
    let absent_keys = lc_v3::absent_keys(&w3.committee_ssz, u);
    let job = lc_v3::job_id(&V3, &lc_v3::job_seed(&instance, &native.pre_store, u));
    let mut msgs: Vec<(&str, LcVerifyV3Message)> = Vec::new();
    if four_steps {
        msgs.push((
            "BeginPrepare",
            LcVerifyV3Message::BeginPrepare {
                attest_program: ATTEST,
                instance,
                config: config.clone(),
                witness: w3.clone(),
                update_ssz: u.to_vec(),
            },
        ));
        msgs.push((
            "BeginKeys",
            LcVerifyV3Message::BeginKeys {
                committee_ssz: w3.committee_ssz.clone(),
                absent_keys: absent_keys.clone(),
            },
        ));
    } else {
        msgs.push((
            "Begin",
            LcVerifyV3Message::Begin {
                attest_program: ATTEST,
                instance,
                config: config.clone(),
                witness: w3.clone(),
                update_ssz: u.to_vec(),
                absent_keys: absent_keys.clone(),
            },
        ));
    }
    msgs.push(("Miller", LcVerifyV3Message::Miller));
    msgs.push(("Finish", LcVerifyV3Message::Finish));
    let mut job_state = Vec::new();
    for (name, msg) in &msgs {
        let (e, m, sum) = step(c, job, &job_state, msg)?;
        job_state = e.post_state.clone();
        let mut chained = 0u64;
        let mut chained_rows = Vec::new();
        if *name == "Finish" {
            let (to, apply_bytes) = e.sync.first().cloned().ok_or_else(|| anyhow!("Finish sent no Apply"))?;
            ensure!(to == Actor::new(instance, ATTEST), "Apply to the wrong actor");
            let AttestMessage::Apply {
                pre_store,
                post_store,
                head,
                record,
            } = borsh::from_slice(&apply_bytes)?
            else {
                bail!("Finish: not Apply")
            };
            ensure!(
                (pre_store, post_store, &head, &record)
                    == (native.pre_store, native.post_store, &native.head, &native.record),
                "{network} {label}: executor Apply differs from V1"
            );
            let input = ReceiveInput {
                receiver: Actor::new(instance, ATTEST),
                from: Some(Actor::new(job, V3)),
                is_authorized: false,
                pre_state: ActorState::from(state.clone()),
                message: apply_bytes,
            };
            let a = run(c.attest, &input)?;
            let st: InstanceState = borsh::from_slice(&a.post_state).context("Apply state")?;
            ensure!(
                st.head == native.head,
                "{network} {label}: instance head after Apply differs from V1"
            );
            chained += a.cycles;
            chained_rows.push(json!({"program": "attest", "message": "Apply", "cycles": a.cycles}));
            if let Some((_, wr)) = a.sync.first() {
                let AttestMessage::WriteRecord { record } = borsh::from_slice(wr)? else {
                    bail!("not WriteRecord")
                };
                let r = write_record(c.attest, ATTEST, instance, &record)?;
                chained += r.user_cycles;
                chained_rows.push(json!({"program": "attest", "message": "WriteRecord", "cycles": r.user_cycles}));
            }
            *state = a.post_state;
        }
        let total = e.cycles + chained;
        let mut phases = serde_json::Map::new();
        for (k, v) in &m {
            phases.insert(k.clone(), json!(v));
        }
        println!(
            "{network} {label} ({kind}) {name}: {} cycles (tx total {total})",
            e.cycles
        );
        rows.push(json!({
            "network": network, "update": label, "kind": kind, "participation": native.participation,
            "layout": if four_steps { "four steps" } else { "three steps" },
            "step": name, "metered_cycles": e.cycles,
            "total_cycles_padded": e.padded, "segments": e.segments,
            "phases": Value::Object(phases), "marked_cycles": sum, "unmarked_cycles": e.cycles.saturating_sub(sum),
            "chained": chained_rows, "tx_total_cycles": total,
            "declared_gas": declared(name), "expected_charged_gas": total,
            "share_of_cap": format!("{:.1}%", total as f64 * 100.0 / CAP as f64),
            "within_step_limit": total <= STEP_LIMIT,
            "job_state_bytes": e.post_state.len(),
            "message_bytes": borsh::to_vec(msg)?.len(),
        }));
    }
    let job: lc_v3::JobV3 = borsh::from_slice(&job_state).context("job")?;
    ensure!(job.stage == lc_v3::stage::DONE, "job not done");
    Ok(())
}

fn chain_rows(c: &Ctx, network: &str, chain: &e2e_chain::FixtureChain, rows: &mut Vec<Value>) -> Result<()> {
    let config = &chain.config;
    let instance = instance_id(&ATTEST, config);
    let (st, _) = rules::init(&ATTEST, &instance, &[], config, &chain.bootstrap).map_err(|e| anyhow!("init: {e}"))?;
    let mut state = borsh::to_vec(&st)?;
    let (mut w, _) = lc::init_store(config, &chain.bootstrap).map_err(|e| anyhow!("init store: {e}"))?;
    for (label, u) in &chain.updates {
        let native = lc::verify_and_apply(config, &w, u).map_err(|e| anyhow!("{label}: {e}"))?;
        let kind = if label.starts_with("finality") {
            "finality"
        } else if native.handoff {
            "handoff"
        } else {
            "period"
        };
        update_rows(c, network, label, kind, config, &w, u, &mut state, false, rows)?;
        w = native.new_witness();
    }
    Ok(())
}

fn floor_rows(c: &Ctx, rows: &mut Vec<Value>) -> Result<()> {
    use fixtures::fabricate::{FabChain, FabParams, bits_first};
    let real = e2e_chain::mainnet_chain(vec![V3])?;
    let chain = FabChain::build(FabParams::impersonating(&real.config, b"beacon-veil/v3/floor"));
    let config = chain.config(vec![V3], 0);
    let instance = instance_id(&ATTEST, &config);
    let bootstrap = chain.bootstrap.ssz();
    let (w0, _) = lc::init_store(&config, &bootstrap).map_err(|e| anyhow!("fab init: {e}"))?;
    let (st, _) = rules::init(&ATTEST, &instance, &[], &config, &bootstrap).map_err(|e| anyhow!("init: {e}"))?;
    // Period update carrying the next committee, 342 signers (the heaviest Begin: two committee roots, 170 absent
    // keys); three steps, then the same update as four steps.
    let mut u0 = chain.updates[0].clone();
    u0.bits = bits_first(342);
    chain.resign(&mut u0);
    for four in [false, true] {
        let mut state = borsh::to_vec(&st)?;
        update_rows(
            c,
            "fabricated",
            "period_update_342",
            "period",
            &config,
            &w0,
            &u0.ssz(),
            &mut state,
            four,
            rows,
        )?;
    }
    // Finality update at 342 signers, on the store after the chain's period and handoff updates.
    let mut state = borsh::to_vec(&st)?;
    let mut w = w0.clone();
    for (i, u) in chain.updates[..2].iter().enumerate() {
        let n = lc::verify_and_apply(&config, &w, &u.ssz()).map_err(|e| anyhow!("fab {i}: {e}"))?;
        let r = rules::apply(
            &state,
            Some(V3),
            n.pre_store,
            n.post_store,
            n.head.clone(),
            n.record.clone(),
        )
        .map_err(|e| anyhow!("fab apply {i}: {e}"))?;
        state = borsh::to_vec(&r.new_state)?;
        w = n.new_witness();
    }
    let mut u2 = chain.updates[2].clone();
    u2.bits = bits_first(342);
    chain.resign(&mut u2);
    update_rows(
        c,
        "fabricated",
        "finality_update_342",
        "finality",
        &config,
        &w,
        &u2.ssz(),
        &mut state,
        false,
        rows,
    )?;
    Ok(())
}

pub fn exec_v3(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let v3 = load(&arg("--v3-elf").context("--v3-elf P (lc_verify_v3 user ELF or .bin)")?)?;
    let trace = load(&arg("--trace-elf").context("--trace-elf P (lc_verify_v3 --features cycle-trace)")?)?;
    let attest = load(&arg("--attest-elf").context("--attest-elf P")?)?;
    let out = arg("--json").context("--json OUT")?;
    println!(
        "lc_verify_v3 {} image {} ({:?} user ELF bytes)",
        v3.path, v3.image_id, v3.elf_bytes
    );
    if let Some(n) = v3.elf_bytes {
        ensure!(n < ELF_CAP, "lc_verify_v3 user ELF {n} bytes >= {ELF_CAP}");
    }
    let c = Ctx {
        v3: &v3,
        trace: &trace,
        attest: &attest,
    };
    let mut rows = Vec::new();
    chain_rows(&c, "mainnet", &e2e_chain::mainnet_chain(vec![V3])?, &mut rows)?;
    chain_rows(&c, "sepolia", &e2e_chain::sepolia_chain(vec![V3])?, &mut rows)?;
    floor_rows(&c, &mut rows)?;
    let three: Vec<&Value> = rows.iter().filter(|r| r["layout"] == "three steps").collect();
    let max_of = |s: &str| {
        three
            .iter()
            .filter(|r| r["step"] == s)
            .map(|r| r["tx_total_cycles"].as_u64().unwrap_or(0))
            .max()
            .unwrap_or(0)
    };
    let worst = json!({"Begin": max_of("Begin"), "Miller": max_of("Miller"), "Finish": max_of("Finish")});
    let all_within = three.iter().all(|r| r["within_step_limit"] == true);
    let doc = json!({
        "tool": "tools/cost exec-v3", "measured": true,
        "executor": "RISC Zero 3.0.5 executor, no proof, no sequencer. metered_cycles = user cycles of the deployed build of lc_verify_v3 for one step (LEZ meters execution gas as user cycles); tx_total_cycles adds the chained attest.Apply and WriteRecord of Finish. Phases come from a measurement build of the same program with cycle marks (--features cycle-trace); unmarked_cycles = metered_cycles minus their sum (program entry, message and job-state decode and encode, journal).",
        "declared_gas_note": "declared_gas is what attest_host::submit_update_staged declares per step (Begin, BeginPrepare and BeginKeys 9,000,000; Miller 7,000,000; Finish 5,500,000); a step is charged the cycles it used; on-chain figures are recorded separately",
        "fixtures": "fixtures/mainnet (bootstrap_15605760, updates_1905_4, finality_update_15637644); fixtures/sepolia (bootstrap_11534336, updates_1408_4, finality_update_11561824); fabricated chain (tools/fixtures FabChain, seed beacon-veil/v3/floor) with participation bits set to the first 342 and re-signed",
        "guests": {
            "lc_verify_v3": {"path_kind": if v3.elf_bytes.is_some() { "user ELF" } else { ".bin" }, "image_id": v3.image_id, "user_elf_bytes": v3.elf_bytes, "user_elf_cap": ELF_CAP},
            "lc_verify_v3_trace": {"image_id": trace.image_id, "note": "measurement build, never deployed"},
            "attest": {"image_id": attest.image_id},
        },
        "cap": CAP, "step_limit": STEP_LIMIT,
        "worst_case_three_steps": worst,
        "every_three_step_row_within_step_limit": all_within,
        "rows": rows,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!("wrote {out}; worst case (three steps) {worst}; all within {STEP_LIMIT}: {all_within}");
    Ok(())
}
