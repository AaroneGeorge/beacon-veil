//! `cost`: deterministic cycle measurements for the cost table (`make cost-exec`).
//!
//! ```text
//! cost exec-lc [--lc-elf P] [--attest-elf P] --json OUT
//! cost sizes --json OUT
//! cost bench-bls --elf P --out DIR
//! cost exec-v3 --v3-elf P --trace-elf P --attest-elf P --json OUT
//! ```
//!
//! `exec-lc` runs the real mainnet light-client chain (mainnet fixtures, via `e2e_chain::mainnet_chain`) through both
//! guests in the RISC Zero 3.0.5 executor, each handler once per step with a `ReceiveInput` frame, as LEZ runs an
//! actor: `lc_verify::Verify` (the private leg on Path A, the whole update on Path B0) and the public `attest`
//! handlers `Init`, `WriteRecord`, `Apply`, `CheckHeader`. No sequencer and no proof: `user_cycles` is
//! deterministic for a given ELF and input and is what LEZ meters as execution gas (1 gas per cycle, LEZ rc3
//! `lez/programs/fee/core/src/market.rs`). Every handler's result is checked against the native `attest_core`
//! result, so a row is only written for a run that did what the chain would do.
//!
//! `sizes` prints the borsh sizes of the stored states and of the messages (state growth, wire bytes).
//!
//! Default ELFs: `$CARGO_TARGET_DIR/riscv-guest/guest_methods/<pkg>/riscv32im-risc0-zkvm-elf/release/<bin>`.

mod bench_bls;
mod exec_v3;

use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, AttestMessage, HeaderRecord, InstanceState, LcVerifyMessage, instance_id, lc, record_id};
use lee_core::{
    account::{Actor, ActorState},
    program::{ReceiveInput, Transition},
};
use risc0_zkvm::{ExecutorEnv, default_executor};
use serde_json::{Value, json};

fn guest_path(pkg: &str, bin: &str) -> String {
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
    format!("{target}/riscv-guest/guest_methods/{pkg}/riscv32im-risc0-zkvm-elf/release/{bin}")
}

struct Guest {
    path: String,
    elf_bytes: Option<usize>,
    program: Vec<u8>,
    image_id: String,
}

fn load(path: &str) -> Result<Guest> {
    let raw = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let (elf_bytes, program) = if path.ends_with(".bin") {
        (None, raw)
    } else {
        (
            Some(raw.len()),
            risc0_binfmt::ProgramBinary::new(&raw, risc0_zkos_v1compat::V1COMPAT_ELF).encode(),
        )
    };
    let image_id = risc0_binfmt::compute_image_id(&program).map_err(|e| anyhow!("image id of {path}: {e}"))?;
    Ok(Guest {
        path: path.into(),
        elf_bytes,
        program,
        image_id: image_id.to_string(),
    })
}

struct Run {
    user_cycles: u64,
    total_cycles: u64,
    segments: usize,
    segment_po2: Vec<u32>,
    ms: u128,
    transition: Transition,
}

/// `COST_DUMP_DIR`: also write each input frame to `<dir>/<label>.input` (stdin bytes for `r0vm --initial-input`, used
/// to time lift and join on a small multi-segment run in a proof window).
fn execute(g: &Guest, input: &ReceiveInput, label: &str) -> Result<Run> {
    let frame = lee_core::to_frame(&borsh::to_vec(input)?);
    if let Ok(dir) = std::env::var("COST_DUMP_DIR") {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(format!("{dir}/{label}.input"), &frame)?;
    }
    let env = ExecutorEnv::builder().write_slice(&frame).build()?;
    let t = Instant::now();
    let info = default_executor().execute(env, &g.program)?;
    let ms = t.elapsed().as_millis();
    let journal = lee_core::from_frame(&info.journal.bytes).ok_or_else(|| anyhow!("journal frame"))?;
    let transition: Transition = borsh::from_slice(journal).context("transition")?;
    Ok(Run {
        user_cycles: info.cycles(),
        total_cycles: info.segments.iter().map(|s| 1u64 << s.po2).sum(),
        segments: info.segments.len(),
        segment_po2: info.segments.iter().map(|s| s.po2).collect(),
        ms,
        transition,
    })
}

fn row(id: &str, program: &str, message: &str, step: &str, r: &Run, message_bytes: usize, extra: Value) -> Value {
    let mut v = json!({
        "id": id, "program": program, "message": message, "step": step,
        "user_cycles": r.user_cycles, "total_cycles_padded": r.total_cycles, "segments": r.segments,
        "segment_po2_max": r.segment_po2.iter().max(), "segment_po2_last": r.segment_po2.last(),
        "message_bytes": message_bytes, "executor_ms": r.ms,
    });
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    v
}

fn state_of(t: &Transition) -> Vec<u8> {
    // The new actor state the handler set (Response::set_state); empty for keep_state.
    t.response.post_state.as_ref().map(|s| s.to_vec()).unwrap_or_default()
}

fn exec_lc(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let lc_path = arg("--lc-elf").unwrap_or_else(|| guest_path("lc_verify_program", "lc_verify"));
    let attest_path = arg("--attest-elf").unwrap_or_else(|| guest_path("attest_program", "attest"));
    let out = arg("--json").context("--json OUT")?;
    let lcg = load(&lc_path)?;
    let atg = load(&attest_path)?;
    let lc2 = arg("--lc2-elf").map(|p| load(&p)).transpose()?;
    println!(
        "lc_verify {} image {} ({:?} ELF bytes)",
        lcg.path, lcg.image_id, lcg.elf_bytes
    );
    println!(
        "attest    {} image {} ({:?} ELF bytes)",
        atg.path, atg.image_id, atg.elf_bytes
    );

    // Placeholder program ids: the executor does not read chain state, and ids do not change cycle counts beyond
    // their fixed 32-byte encodings.
    let attest_program = AccountId::new([0xA7; 32]);
    let lc_program = AccountId::new([0x1C; 32]);
    let submitter_alias = AccountId::new([0x55; 32]);
    let chain = e2e_chain::mainnet_chain(vec![lc_program])?;
    let config = chain.config.clone();
    let instance = instance_id(&attest_program, &config);
    let mut rows = Vec::new();

    // ---- Init (public) ----
    let (mut witness, head0) = lc::init_store(&config, &chain.bootstrap).map_err(|e| anyhow!("native init: {e}"))?;
    let init_msg = AttestMessage::Init {
        config: config.clone(),
        bootstrap: chain.bootstrap.clone(),
    };
    let init_bytes = borsh::to_vec(&init_msg)?;
    let input = ReceiveInput {
        receiver: Actor::new(instance, attest_program),
        from: None,
        is_authorized: true,
        pre_state: ActorState::empty(),
        message: init_bytes.clone(),
    };
    let r = execute(&atg, &input, "attest.init")?;
    let mut state = state_of(&r.transition);
    let st: InstanceState = borsh::from_slice(&state).context("Init state")?;
    if st.head != head0 {
        bail!("Init: executor head differs from native");
    }
    let sm = r
        .transition
        .response
        .sync_messages
        .first()
        .ok_or_else(|| anyhow!("Init sent no WriteRecord"))?;
    let AttestMessage::WriteRecord { record: cp_record } = borsh::from_slice(&sm.message)? else {
        bail!("Init: not WriteRecord")
    };
    println!(
        "Init: user cycles {} ({} segments, {} ms)",
        r.user_cycles, r.segments, r.ms
    );
    rows.push(row(
        "attest.init",
        "attest",
        "Init",
        "bootstrap_15605760",
        &r,
        init_bytes.len(),
        json!({
        "bootstrap_bytes": chain.bootstrap.len(), "path": "public", "instance_state_bytes": state.len()}),
    ));
    let r = write_record(&atg, attest_program, instance, &cp_record)?;
    rows.push(row(
        "attest.write_record.checkpoint",
        "attest",
        "WriteRecord",
        "bootstrap_15605760",
        &r,
        borsh::to_vec(&AttestMessage::WriteRecord {
            record: cp_record.clone(),
        })?
        .len(),
        json!({"path": "public (sync from Init)", "record_state_bytes": borsh::to_vec(&cp_record)?.len()}),
    ));

    // ---- updates ----
    for (label, update) in &chain.updates {
        let native = lc::verify_and_apply(&config, &witness, update).map_err(|e| anyhow!("{label}: {e}"))?;
        let verify = LcVerifyMessage::Verify {
            attest_program,
            instance,
            config: config.clone(),
            store_witness: borsh::to_vec(&witness)?,
            update_ssz: update.clone(),
        };
        let vbytes = borsh::to_vec(&verify)?;
        let input = ReceiveInput {
            receiver: Actor::new(submitter_alias, lc_program),
            from: None,
            is_authorized: true,
            pre_state: ActorState::empty(),
            message: vbytes.clone(),
        };
        let r = execute(&lcg, &input, &format!("lc_verify.{label}"))?;
        let sm = r
            .transition
            .response
            .sync_messages
            .first()
            .ok_or_else(|| anyhow!("{label}: no Apply"))?;
        let apply_bytes = sm.message.clone();
        let AttestMessage::Apply {
            pre_store,
            post_store,
            head,
            record,
        } = borsh::from_slice(&apply_bytes)?
        else {
            bail!("{label}: not Apply")
        };
        if (pre_store, post_store, &head, &record)
            != (native.pre_store, native.post_store, &native.head, &native.record)
        {
            bail!("{label}: executor Apply differs from native");
        }
        println!(
            "{label}: Verify user cycles {} ({} segments, {} ms)",
            r.user_cycles, r.segments, r.ms
        );
        rows.push(row(
            &format!("lc_verify.v1.verify.{label}"),
            "lc_verify",
            "Verify (V1)",
            label,
            &r,
            vbytes.len(),
            json!({"variant": "V1", "update_bytes": update.len(), "participation": native.participation,
                   "handoff": native.handoff, "new_slot": native.new_slot, "period": head.period,
                   "head_slot": head.slot, "path": "private on Path A; public on B0 (over the cap)"}),
        ));

        if let Some(g2) = &lc2 {
            let r2 = execute(g2, &input, &format!("lc_verify_v2.{label}"))?;
            let sm2 = r2
                .transition
                .response
                .sync_messages
                .first()
                .ok_or_else(|| anyhow!("{label}: V2 no Apply"))?;
            if sm2.message != apply_bytes {
                bail!("{label}: V2 Apply differs from V1");
            }
            println!(
                "{label}: Verify V2 user cycles {} ({} segments)",
                r2.user_cycles, r2.segments
            );
            rows.push(row(
                &format!("lc_verify.v2.verify.{label}"),
                "lc_verify_v2",
                "Verify (V2)",
                label,
                &r2,
                vbytes.len(),
                json!({"variant": "V2", "update_bytes": update.len(), "participation": native.participation,
                "handoff": native.handoff, "path": "private on Path A"}),
            ));
        }
        // Apply, as the instance actor receives it: from the private lc_verify actor.
        let input = ReceiveInput {
            receiver: Actor::new(instance, attest_program),
            from: Some(Actor::new(submitter_alias, lc_program)),
            is_authorized: false,
            pre_state: ActorState::from(state.clone()),
            message: apply_bytes.clone(),
        };
        let r = execute(&atg, &input, &format!("attest.apply.{label}"))?;
        let new_state = state_of(&r.transition);
        let st: InstanceState = borsh::from_slice(&new_state).context("Apply state")?;
        if st.head.store_commitment != native.post_store {
            bail!("{label}: Apply state differs from native");
        }
        let wrote = r.transition.response.sync_messages.first().cloned();
        println!("{label}: Apply user cycles {} ({} ms)", r.user_cycles, r.ms);
        rows.push(row(
            &format!("attest.apply.{label}"),
            "attest",
            "Apply",
            label,
            &r,
            apply_bytes.len(),
            json!({"path": "public (sync from the private Verify)", "writes_record": wrote.is_some()}),
        ));
        state = new_state;
        if let Some(sm) = wrote {
            let AttestMessage::WriteRecord { record } = borsh::from_slice(&sm.message)? else {
                bail!("not WriteRecord")
            };
            let r = write_record(&atg, attest_program, instance, &record)?;
            rows.push(row(
                &format!("attest.write_record.{label}"),
                "attest",
                "WriteRecord",
                label,
                &r,
                sm.message.len(),
                json!({"path": "public (sync from Apply)",
                "record_state_bytes": borsh::to_vec(&record)?.len()}),
            ));
            if label.starts_with("finality") {
                let r = check_header(&atg, attest_program, instance, &record)?;
                rows.push(row(
                    "attest.check_header",
                    "attest",
                    "CheckHeader",
                    label,
                    &r,
                    borsh::to_vec(&AttestMessage::CheckHeader {
                        record_hash: record.hash(),
                    })?
                    .len(),
                    json!({"path": "public (sync from Attest)"}),
                ));
            }
        }
        witness = native.new_witness();
    }

    let doc = json!({
        "tool": "tools/cost exec-lc", "measured": true,
        "executor": "RISC Zero 3.0.5 executor (no proof); user_cycles = LEZ execution gas for public handlers",
        "fixtures": "fixtures/mainnet: bootstrap_15605760, updates_1905_4, finality_update_15637644",
        "guests": {
            "lc_verify": {"path": lcg.path, "elf_bytes": lcg.elf_bytes, "program_binary_bytes": lcg.program.len(), "image_id": lcg.image_id},
            "lc_verify_v2": lc2.as_ref().map(|g| json!({"path": g.path, "elf_bytes": g.elf_bytes, "program_binary_bytes": g.program.len(), "image_id": g.image_id})),
            "attest": {"path": atg.path, "elf_bytes": atg.elf_bytes, "program_binary_bytes": atg.program.len(), "image_id": atg.image_id},
        },
        "rows": rows,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!("wrote {out}");
    Ok(())
}

fn write_record(atg: &Guest, attest_program: AccountId, instance: AccountId, record: &HeaderRecord) -> Result<Run> {
    let input = ReceiveInput {
        receiver: Actor::new(record_id(&attest_program, &instance, record.slot), attest_program),
        from: Some(Actor::new(instance, attest_program)),
        is_authorized: false,
        pre_state: ActorState::empty(),
        message: borsh::to_vec(&AttestMessage::WriteRecord { record: record.clone() })?,
    };
    let r = execute(atg, &input, "attest.write_record")?;
    if state_of(&r.transition) != borsh::to_vec(record)? {
        bail!("WriteRecord: state differs from borsh(record)");
    }
    Ok(r)
}

fn check_header(atg: &Guest, attest_program: AccountId, instance: AccountId, record: &HeaderRecord) -> Result<Run> {
    let input = ReceiveInput {
        receiver: Actor::new(record_id(&attest_program, &instance, record.slot), attest_program),
        from: Some(Actor::new(AccountId::new([0x33; 32]), attest_program)),
        is_authorized: false,
        pre_state: ActorState::from(borsh::to_vec(record)?),
        message: borsh::to_vec(&AttestMessage::CheckHeader {
            record_hash: record.hash(),
        })?,
    };
    execute(atg, &input, "attest.check_header")
}

fn sizes(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out = arg("--json").context("--json OUT")?;
    let chain = e2e_chain::mainnet_chain(vec![AccountId::new([0x1C; 32])])?;
    let (witness, head) = lc::init_store(&chain.config, &chain.bootstrap).map_err(|e| anyhow!("init: {e}"))?;
    let state = InstanceState {
        config: chain.config.clone(),
        head,
    };
    let record = HeaderRecord {
        chain_id: 1,
        slot: 0,
        beacon_root: [0; 32],
        exec_number: 0,
        exec_hash: [0; 32],
        exec_state_root: [0; 32],
        exec_receipts_root: [0; 32],
    };
    let witness_bytes = borsh::to_vec(&witness)?.len();
    let doc = json!({
        "tool": "tools/cost sizes", "measured": true, "what": "borsh sizes (bytes) of stored states and inputs",
        "instance_state_bytes_mainnet": borsh::to_vec(&state)?.len(),
        "header_record_bytes": borsh::to_vec(&record)?.len(),
        "store_witness_bytes": witness_bytes,
        "bootstrap_bytes": chain.bootstrap.len(),
        "update_bytes": chain.updates.iter().map(|(l, u)| json!({"step": l, "bytes": u.len()})).collect::<Vec<_>>(),
    });
    std::fs::write(&out, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!("wrote {out}");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("exec-lc") => exec_lc(&args[2..]),
        Some("sizes") => sizes(&args[2..]),
        Some("bench-bls") => bench_bls::bench_bls(&args[2..]),
        Some("exec-v3") => exec_v3::exec_v3(&args[2..]),
        _ => bail!(
            "usage: cost exec-lc [--lc-elf P] [--attest-elf P] --json OUT | cost sizes --json OUT | cost bench-bls --elf P --out DIR | cost exec-v3 --v3-elf P --trace-elf P --attest-elf P --json OUT"
        ),
    }
}
