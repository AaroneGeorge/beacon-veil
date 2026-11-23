//! `v3_onchain`: the staged public verifier (`lc_verify_v3`) as real public transactions on a running sequencer, and
//! the read-back check of every recorded transaction.
//!
//! ```text
//! v3_onchain run --network mainnet|sepolia|floor --v3-elf P --attest-elf P --out run.json
//! v3_onchain check run.json [run.json ...]
//! v3_onchain check-images <release IMAGE_IDS>
//! BV_ALLOW_REVERT=1 v3_onchain refusals --out refusals.json     # refused steps as real transactions
//! BV_ALLOW_REVERT=1 v3_onchain refusals-onchain --v3-elf P --attest-elf P --out refusals-onchain.json
//!                                                               # tampered updates of the differential test
//! v3_onchain annotate --v3-elf P --seq-log P FILE...            # read-only: payer, fee, codes into the records
//! ```
//!
//! Environment as `attest_host` (`BV_IDS`, `LEE_WALLET_HOME_DIR`, `BV_STORE_DIR`, `BV_PAYER`).
//!
//! `run` initialises a fresh instance (`verifiers = [lc_verify_v3]`) from the fixture checkpoint of the network and
//! submits each fixture update with `attest_host::submit_update_staged` (Begin, Miller, Finish, one public
//! transaction each). For every transaction it records the hash, the block, the transaction's position, whether it
//! reverted, the block's charged execution gas (from the block's fee summary) and how many charged transactions the
//! block holds, and the metered cycles of the same step re-executed in the RISC Zero executor with the deployed ELFs
//! (`lc_verify_v3` itself, plus the chained `attest.Apply` and `WriteRecord` for `Finish`). After each update the
//! instance head on chain must equal V1's (Helios 0.12.0 unmodified, native) head for the same input.
//!
//! `refusals-onchain` sends tampered updates taken from the differential test (`tests/diff_helios`) to two fresh
//! instances, step by step up to the step where the program logic refuses them, and records each transaction, the
//! exit code of the deployed ELF re-executed on the same message and job state, and that the instance state did not
//! move; then one untampered update per instance applies (positive control). The cases are [`SPECS`].
//!
//! `check` re-reads every recorded transaction from the sequencer RPC: same block, same position, not reverted, same
//! program and step, same charged gas in the block's fee summary; and the instance head equals the recorded one.
//! Prints PASS or the first mismatch.

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail, ensure};
use attest_core::{
    AccountId, InstanceConfig, InstanceState, LcVerifyV3Message, instance_id,
    lc::{self, StoreWitness},
};
use attest_host::DeployedIds;
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{Actor, ActorState},
    program::ReceiveInput,
};
use risc0_zkvm::{ExecutorEnv, default_executor};
use serde_json::{Value, json};

const CAP: u64 = 10_000_000;
const STEP_LIMIT: u64 = 9_000_000;

// ---- fee summary of a block (layout of LEZ rc3 `fee_core::Message`, borsh) ----
#[derive(BorshSerialize, BorshDeserialize)]
struct BlockFeeSummary {
    gas_used_exec: u64,
    gas_used_stor: u64,
    revenue_base: u128,
    revenue_tip: u128,
}

#[derive(BorshSerialize, BorshDeserialize)]
enum FeeMessage {
    Distribute {
        summary: BlockFeeSummary,
        payout: u128,
        producer: AccountId,
    },
    Refund {
        amount: u128,
        payer: AccountId,
    },
}

struct TxInBlock {
    block: u64,
    position: usize,
    reverted: bool,
    timestamp: u64,
    block_gas_exec: Option<u64>,
    block_gas_stor: Option<u64>,
    charged_txs: usize,
    to: Actor,
    message: Vec<u8>,
    /// `(payer, declared gas)` of the transaction's fee declaration.
    fee: Option<(AccountId, u64)>,
}

async fn tx_in_block(w: &attest_host::Wallet, hash: &str) -> Result<TxInBlock> {
    let h = common::HashType::from_str(hash).map_err(|e| anyhow!("tx hash {hash}: {e:?}"))?;
    let (_, block_id) = w.poll_transaction(h).await.map_err(|e| anyhow!("tx {hash}: {e:#}"))?;
    let block = w
        .get_block(block_id)
        .await?
        .ok_or_else(|| anyhow!("block {block_id} not found"))?;
    let position = block
        .body
        .transactions
        .iter()
        .position(|t| t.hash() == h)
        .ok_or_else(|| anyhow!("tx {hash} not in block {block_id}"))?;
    let fee_program = AccountId::from_builtin_program_name(b"fee");
    let (mut gas_exec, mut gas_stor, mut charged) = (None, None, 0usize);
    let mut root = None;
    for (i, t) in block.body.transactions.iter().enumerate() {
        match t {
            common::transaction::LeeTransaction::Public(p) => {
                let ex = &p.message.execution;
                if i == position {
                    root = Some((
                        ex.root.to,
                        ex.root.message.clone(),
                        ex.fee.as_ref().map(|f| (f.payer, f.gas_limit)),
                    ));
                }
                if ex.fee.is_some() {
                    charged += 1;
                } else if ex.root.to.program_account_id == fee_program
                    && let Ok(FeeMessage::Distribute { summary, .. }) = borsh::from_slice(&ex.root.message)
                {
                    gas_exec = Some(summary.gas_used_exec);
                    gas_stor = Some(summary.gas_used_stor);
                }
            }
            common::transaction::LeeTransaction::PrivacyPreserving(_) => charged += 1,
        }
    }
    let (to, message, fee) = root.ok_or_else(|| anyhow!("tx {hash} is not a public transaction"))?;
    Ok(TxInBlock {
        block: block_id,
        position,
        reverted: block.body.is_reverted(position),
        timestamp: block.header.timestamp,
        block_gas_exec: gas_exec,
        block_gas_stor: gas_stor,
        charged_txs: charged,
        to,
        message,
        fee,
    })
}

// ---- executor ----
struct Guest {
    program: Vec<u8>,
    image_id: String,
}

fn load(path: &str) -> Result<Guest> {
    let raw = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let program = if path.ends_with(".bin") {
        raw
    } else {
        risc0_binfmt::ProgramBinary::new(&raw, risc0_zkos_v1compat::V1COMPAT_ELF).encode()
    };
    let image_id = risc0_binfmt::compute_image_id(&program).map_err(|e| anyhow!("{e}"))?;
    Ok(Guest {
        program,
        image_id: image_id.to_string(),
    })
}

struct Exec {
    cycles: u64,
    post_state: Vec<u8>,
    sync: Vec<(Actor, Vec<u8>)>,
}

fn exec(g: &Guest, input: &ReceiveInput) -> Result<Exec> {
    let frame = lee_core::to_frame(&borsh::to_vec(input)?);
    let env = ExecutorEnv::builder().write_slice(&frame).build()?;
    let info = default_executor().execute(env, &g.program)?;
    let journal =
        lee_core::from_frame(&info.journal.bytes).ok_or_else(|| anyhow!("no journal (exit {:?})", info.exit_code))?;
    let t: lee_core::program::Transition = borsh::from_slice(journal)?;
    Ok(Exec {
        cycles: info.cycles(),
        post_state: t.response.post_state.as_ref().map(|s| s.to_vec()).unwrap_or_default(),
        sync: t
            .response
            .sync_messages
            .iter()
            .map(|m| (m.to, m.message.clone()))
            .collect(),
    })
}

/// Metered cycles of one step as the sequencer runs it: the v3 program on the job account, and for `Finish` the
/// chained `attest.Apply` on the instance and `WriteRecord` on the record account.
#[allow(clippy::too_many_arguments)]
fn metered(
    v3g: &Guest,
    atg: &Guest,
    ids: (&AccountId, &AccountId),
    job: AccountId,
    job_state: &[u8],
    instance: AccountId,
    instance_state: &[u8],
    message: &[u8],
) -> Result<(u64, u64, Vec<u8>)> {
    let (attest, v3) = ids;
    let e = exec(
        v3g,
        &ReceiveInput {
            receiver: Actor::new(job, *v3),
            from: None,
            is_authorized: false,
            pre_state: ActorState::from(job_state.to_vec()),
            message: message.to_vec(),
        },
    )?;
    let mut chained = 0;
    for (to, m) in &e.sync {
        ensure!(
            *to == Actor::new(instance, *attest),
            "sync message to an unexpected actor"
        );
        let a = exec(
            atg,
            &ReceiveInput {
                receiver: *to,
                from: Some(Actor::new(job, *v3)),
                is_authorized: false,
                pre_state: ActorState::from(instance_state.to_vec()),
                message: m.clone(),
            },
        )?;
        chained += a.cycles;
        for (rto, rm) in &a.sync {
            let r = exec(
                atg,
                &ReceiveInput {
                    receiver: *rto,
                    from: Some(*to),
                    is_authorized: false,
                    pre_state: ActorState::empty(),
                    message: rm.clone(),
                },
            )?;
            chained += r.cycles;
        }
    }
    Ok((e.cycles, chained, e.post_state))
}

fn step_name(msg: &[u8]) -> String {
    match borsh::from_slice::<LcVerifyV3Message>(msg) {
        Ok(LcVerifyV3Message::Begin { .. }) => "Begin",
        Ok(LcVerifyV3Message::Miller) => "Miller",
        Ok(LcVerifyV3Message::Finish) => "Finish",
        Ok(LcVerifyV3Message::VerifyOnce { .. }) => "VerifyOnce",
        Ok(LcVerifyV3Message::BeginPrepare { .. }) => "BeginPrepare",
        Ok(LcVerifyV3Message::BeginKeys { .. }) => "BeginKeys",
        Err(_) => "?",
    }
    .to_string()
}

/// `(config, bootstrap, [(label, update)])` of one test network.
type NetUpdates = (InstanceConfig, Vec<u8>, Vec<(String, Vec<u8>)>);

/// `(job seed, job id, record slot, witness, absent keys)` of one update.
type JobInputs = (
    [u8; 32],
    AccountId,
    u64,
    attest_core::lc_v3::StoreWitnessV3,
    Vec<[u8; 96]>,
);

/// Fresh public data (fetched 18-19 Nov 2026): the bootstrap at the first slot of a period (`fixtures/pub/<net>`), the
/// best updates of that period and the next (`fixtures/diff/<net>/updates_*`), and the finality updates of the next
/// period (`fixtures/diff/<net>/finality_update_*`), oldest first.
struct PubData {
    config: InstanceConfig,
    bootstrap: Vec<u8>,
    period_update: Vec<u8>,
    handoff_update: Vec<u8>,
    finality: Vec<(String, Vec<u8>)>,
}

fn pub_data(network: &str, v3: AccountId) -> Result<PubData> {
    let (chain_id, bootstrap, range, first, period) = match network {
        "mainnet" => (1u64, "bootstrap_15679488.ssz", "updates_1904_12.ssz", 1904u64, 1914u64),
        "sepolia" => (11_155_111, "bootstrap_11599872.ssz", "updates_1406_12.ssz", 1406, 1416),
        "hoodi" => (560_048, "bootstrap_4399104.ssz", "updates_530_9.ssz", 530, 537),
        _ => bail!("network: mainnet | sepolia | hoodi"),
    };
    let dir = format!("pub/{network}");
    let bootstrap = e2e_chain::fixture(&dir, bootstrap)?;
    let config = e2e_chain::config_from_beacon(
        chain_id,
        &e2e_chain::fixture(&dir, "genesis.json")?,
        &e2e_chain::fixture(&dir, "fork_schedule.json")?,
        &bootstrap,
        vec![v3],
        0,
    )?;
    let diff = format!("diff/{network}");
    let ups = e2e_chain::split_updates_by_range(&e2e_chain::fixture(&diff, range)?)?;
    let at = |p: u64| -> Result<Vec<u8>> {
        ups.get(usize::try_from(p - first)?)
            .cloned()
            .ok_or_else(|| anyhow!("no update for period {p}"))
    };
    let mut names: Vec<String> = std::fs::read_dir(e2e_chain::fixtures_dir().join(&diff))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("finality_update_") && n.ends_with(".ssz"))
        .collect();
    names.sort();
    let finality = names
        .into_iter()
        .map(|n| Ok((n.trim_end_matches(".ssz").to_string(), e2e_chain::fixture(&diff, &n)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(PubData {
        config,
        bootstrap,
        period_update: at(period)?,
        handoff_update: at(period + 1)?,
        finality,
    })
}

/// A fabricated chain (our own keys) impersonating the network of `real`, checkpoint at the first slot of `period`
/// plus two epochs.
fn fab_chain(real: &InstanceConfig, period: u64, seed: &[u8]) -> fixtures::fabricate::FabChain {
    use fixtures::fabricate::{FabChain, FabParams, SLOTS_PER_EPOCH, SLOTS_PER_PERIOD};
    let mut c = real.clone();
    c.checkpoint_slot = period * SLOTS_PER_PERIOD + 2 * SLOTS_PER_EPOCH;
    FabChain::build(FabParams::impersonating(&c, seed))
}

fn net_updates(network: &str, v3: AccountId) -> Result<NetUpdates> {
    match network {
        "mainnet" | "sepolia" | "hoodi" => {
            let d = pub_data(network, v3)?;
            let p = calc_period(&d.config, &d.period_update)?;
            let mut ups = vec![
                (format!("update_{p}"), d.period_update.clone()),
                (format!("update_{}", p + 1), d.handoff_update.clone()),
            ];
            // The newest finality update that applies after the two.
            let (mut w, _) = lc::init_store(&d.config, &d.bootstrap).map_err(|e| anyhow!("init: {e}"))?;
            for (_, u) in &ups {
                w = lc::verify_and_apply(&d.config, &w, u)
                    .map_err(|e| anyhow!("{network}: {e}"))?
                    .new_witness();
            }
            let f = d
                .finality
                .iter()
                .rev()
                .find(|(_, u)| lc::verify_and_apply(&d.config, &w, u).is_ok())
                .ok_or_else(|| anyhow!("{network}: no finality update applies after the period updates"))?;
            ups.push(f.clone());
            Ok((d.config, d.bootstrap, ups))
        }
        "floor" => {
            use fixtures::fabricate::bits_first;
            let real = pub_data("mainnet", v3)?;
            let chain = fab_chain(&real.config, 1914, b"beacon-veil/v3/floor");
            let mut u0 = chain.updates[0].clone();
            u0.bits = bits_first(342);
            chain.resign(&mut u0);
            Ok((
                chain.config(vec![v3], 0),
                chain.bootstrap.ssz(),
                vec![("period_update_342".into(), u0.ssz())],
            ))
        }
        _ => bail!("network: mainnet | sepolia | hoodi | floor"),
    }
}

/// Sync period of an update's signature slot.
fn calc_period(config: &InstanceConfig, u: &[u8]) -> Result<u64> {
    let f = lc::forks(config).map_err(|e| anyhow!("{e}"))?;
    Ok(lc::decode_update(u, &f).map_err(|e| anyhow!("{e}"))?.signature_slot() / 8192)
}

/// A config whose instance does not exist yet on this chain (the instance address is the PDA of the config).
async fn fresh_config(ids: &DeployedIds, base: &InstanceConfig) -> Result<InstanceConfig> {
    let mut c = base.clone();
    for k in 0..1000u16 {
        c.min_participants = 342 + k;
        if attest_host::read_head(ids, &instance_id(&ids.attest, &c))
            .await?
            .is_none()
        {
            return Ok(c);
        }
    }
    bail!("no fresh config")
}

fn kind_of(label: &str, out: &lc::UpdateOutcome) -> &'static str {
    if label.starts_with("finality") {
        "finality"
    } else if out.handoff {
        "handoff"
    } else {
        "period"
    }
}

async fn run(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let network = arg("--network").context("--network")?;
    let out_path = arg("--out").context("--out")?;
    let v3g = load(&arg("--v3-elf").context("--v3-elf")?)?;
    let atg = load(&arg("--attest-elf").context("--attest-elf")?)?;
    let ids = DeployedIds::from_env()?;
    let v3 = attest_host::deployed_program("lc_verify_v3")?;
    let (base, bootstrap, updates) = net_updates(&network, v3)?;
    let config = fresh_config(&ids, &base).await?;
    let w = attest_host::open_wallet()?;

    let (instance, init) = attest_host::init(&ids, &config, &bootstrap).await?;
    let ib = tx_in_block(&w, &init.tx).await?;
    println!("{network}: init instance {instance} tx {} block {}", init.tx, ib.block);
    let mut rows = Vec::new();
    let mut witness: StoreWitness = lc::init_store(&config, &bootstrap)
        .map_err(|e| anyhow!("init store: {e}"))?
        .0;
    for (label, u) in &updates {
        let native = lc::verify_and_apply(&config, &witness, u).map_err(|e| anyhow!("{label}: V1 refused: {e}"))?;
        let kind = kind_of(label, &native);
        let state_before = attest_host::read_instance(&ids, &instance)
            .await?
            .ok_or_else(|| anyhow!("instance missing"))?;
        let instance_bytes = borsh::to_vec(&state_before)?;
        let staged = attest_host::submit_update_staged(&ids, v3, &instance, u).await?;
        let head = attest_host::read_head(&ids, &instance)
            .await?
            .ok_or_else(|| anyhow!("instance missing"))?;
        // attest.Apply replaces the whole head when the finalized slot moves; when the update only teaches the next
        // committee it keeps the head and sets the new store commitment.
        let want_ok = if native.new_slot {
            head == native.head
        } else {
            head.store_commitment == native.post_store && head.slot == native.head.slot
        };
        ensure!(want_ok, "{label}: on-chain head differs from V1");
        // Re-execute each step with the deployed ELFs on the same inputs.
        let mut job_state: Vec<u8> = Vec::new();
        let mut steps = Vec::new();
        for (name, o) in &staged.steps {
            let t = tx_in_block(&w, &o.tx).await?;
            ensure!(!t.reverted, "{label} {name}: reverted");
            ensure!(
                t.to == Actor::new(staged.job, v3),
                "{label} {name}: not sent to the job account"
            );
            ensure!(
                step_name(&t.message) == *name,
                "{label} {name}: message is {}",
                step_name(&t.message)
            );
            let (cycles, chained, post) = metered(
                &v3g,
                &atg,
                (&ids.attest, &v3),
                staged.job,
                &job_state,
                instance,
                &instance_bytes,
                &t.message,
            )?;
            job_state = post;
            let total = cycles + chained;
            let alone = t.charged_txs == 1;
            if alone {
                ensure!(
                    t.block_gas_exec == Some(total),
                    "{label} {name}: block charged {:?} gas, executor metered {total}",
                    t.block_gas_exec
                );
            }
            println!(
                "{network} {label} ({kind}) {name}: tx {} block {} charged {:?} metered {total}",
                o.tx, t.block, t.block_gas_exec
            );
            steps.push(json!({
                "step": name, "tx": o.tx, "block": t.block, "position": t.position, "block_timestamp_ms": t.timestamp,
                "reverted": t.reverted, "declared_gas": o.gas,
                "block_gas_used_exec": t.block_gas_exec, "block_gas_used_stor": t.block_gas_stor,
                "charged_txs_in_block": t.charged_txs,
                "charged_gas": if alone { json!(t.block_gas_exec) } else { Value::Null },
                "metered_cycles_v3": cycles, "metered_cycles_chained_attest": chained, "metered_cycles_total": total,
                "share_of_cap": format!("{:.1}%", total as f64 * 100.0 / CAP as f64),
                "within_step_limit": total <= STEP_LIMIT,
                "message_bytes": t.message.len(),
            }));
        }
        rows.push(json!({
            "update": label, "kind": kind, "participation": native.participation,
            "signature_slot": native.signature_slot, "job": staged.job.to_string(), "four_steps": staged.four_steps,
            "txs_per_update": staged.steps.len(),
            "head_after": {"slot": head.slot, "period": head.period, "beacon_root": hex::encode(head.beacon_root),
                           "store_commitment": hex::encode(head.store_commitment)},
            "head_equals_v1": want_ok,
            "finalized_slot_moved": native.new_slot,
            "steps": steps,
        }));
        witness = native.new_witness();
    }
    let st: InstanceState = attest_host::read_instance(&ids, &instance)
        .await?
        .ok_or_else(|| anyhow!("instance"))?;
    let all: Vec<&Value> = rows
        .iter()
        .flat_map(|r| r["steps"].as_array().into_iter().flatten())
        .collect();
    let max_charged = all.iter().filter_map(|s| s["charged_gas"].as_u64()).max();
    let doc = json!({
        "tool": "tools/cost v3_onchain run", "network": network,
        "what": "lc_verify_v3 updates as real public transactions on seq-pub (LEZ v0.4.0-rc3 standalone, RISC0_DEV_MODE unset: public transactions are executed by the sequencer, no proofs involved)",
        "sequencer": "seq-pub",
        "programs": {"attest": ids.attest.to_string(), "lc_verify": ids.lc_verify.to_string(), "lc_verify_v3": v3.to_string()},
        "executor_images": {"lc_verify_v3": v3g.image_id, "attest": atg.image_id},
        "instance": instance.to_string(), "config_chain_id": config.chain_id, "checkpoint_slot": config.checkpoint_slot,
        "init": {"tx": init.tx, "block": ib.block},
        "updates": rows,
        "txs": all.len(), "max_charged_gas": max_charged,
        "every_step_within_step_limit": all.iter().all(|s| s["within_step_limit"] == true),
        "final_head": {"slot": st.head.slot, "period": st.head.period, "store_commitment": hex::encode(st.head.store_commitment)},
    });
    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!(
        "wrote {out_path}: {} transactions, max charged gas {max_charged:?}",
        all.len()
    );
    Ok(())
}

async fn check(paths: &[String]) -> Result<()> {
    let w = attest_host::open_wallet()?;
    let ids = DeployedIds::from_env()?;
    let mut n = 0;
    for p in paths {
        let doc: Value = serde_json::from_slice(&std::fs::read(p).with_context(|| p.clone())?)?;
        if doc.get("cases").is_some() {
            n += check_refusals(&w, &ids, p, &doc).await?;
            continue;
        }
        let v3 = AccountId::from_str(doc["programs"]["lc_verify_v3"].as_str().unwrap_or_default())
            .map_err(|e| anyhow!("{p}: lc_verify_v3 id: {e:?}"))?;
        let instance = AccountId::from_str(doc["instance"].as_str().unwrap_or_default())
            .map_err(|e| anyhow!("{p}: instance: {e:?}"))?;
        for u in doc["updates"].as_array().into_iter().flatten() {
            let job = AccountId::from_str(u["job"].as_str().unwrap_or_default()).map_err(|e| anyhow!("job: {e:?}"))?;
            for s in u["steps"].as_array().into_iter().flatten() {
                let hash = s["tx"].as_str().unwrap_or_default();
                let what = format!("{p} {} {} tx {hash}", u["update"], s["step"]);
                let t = tx_in_block(&w, hash).await.with_context(|| what.clone())?;
                ensure!(
                    Some(t.block) == s["block"].as_u64(),
                    "{what}: block {} recorded {}",
                    t.block,
                    s["block"]
                );
                ensure!(
                    Some(t.position as u64) == s["position"].as_u64(),
                    "{what}: position differs"
                );
                ensure!(!t.reverted, "{what}: reverted");
                ensure!(t.to == Actor::new(job, v3), "{what}: receiver differs");
                ensure!(
                    Some(step_name(&t.message).as_str()) == s["step"].as_str(),
                    "{what}: step differs"
                );
                ensure!(
                    t.block_gas_exec == s["block_gas_used_exec"].as_u64(),
                    "{what}: block gas differs"
                );
                n += 1;
            }
        }
        let head = attest_host::read_head(&ids, &instance)
            .await?
            .ok_or_else(|| anyhow!("{p}: instance missing"))?;
        ensure!(
            hex::encode(head.store_commitment) == doc["final_head"]["store_commitment"].as_str().unwrap_or_default()
                && Some(head.slot) == doc["final_head"]["slot"].as_u64(),
            "{p}: instance head differs from the recorded final head"
        );
    }
    println!("PASS: {n} transactions re-read from the sequencer match the records");
    Ok(())
}

/// Sends one raw V3 step (the wallet must allow reverts: `BV_ALLOW_REVERT=1`); returns the tx hash and whether the
/// transaction landed reverted, with the refusal text.
async fn send_step(
    ids: &DeployedIds,
    v3: AccountId,
    seed: [u8; 32],
    instance: &AccountId,
    record_slot: u64,
    msg: &LcVerifyV3Message,
    what: &str,
) -> Result<(String, bool, String)> {
    let mut w = attest_host::open_wallet()?;
    let mut accounts = vec![attest_host::pda(v3, seed).select_program_actor_state(v3)];
    if matches!(msg, LcVerifyV3Message::Finish) {
        accounts
            .push(attest_host::wallet::AccountIdentity::PublicNoSign(*instance).select_program_actor_state(ids.attest));
        accounts.push(
            attest_host::pda(ids.attest, attest_core::record_seed(instance, record_slot))
                .select_program_actor_state(ids.attest),
        );
    }
    match attest_host::send_public(&mut w, accounts, borsh::to_vec(msg)?, what).await {
        Ok(o) => Ok((o.tx, false, String::new())),
        Err(e) => {
            let text = format!("{e:#}");
            let hash = text
                .split("tx ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .filter(|h| h.len() == 64)
                .ok_or_else(|| anyhow!("{what}: not sent: {text}"))?;
            Ok((hash.to_string(), true, text))
        }
    }
}

async fn refusals(args: &[String]) -> Result<()> {
    use attest_core::lc_v3;
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out_path = arg("--out").context("--out")?;
    ensure!(
        std::env::var("BV_ALLOW_REVERT").is_ok_and(|v| v == "1"),
        "set BV_ALLOW_REVERT=1: refused steps must reach the chain"
    );
    let ids = DeployedIds::from_env()?;
    let v3 = attest_host::deployed_program("lc_verify_v3")?;
    let w = attest_host::open_wallet()?;
    let d = pub_data("mainnet", v3)?;
    let config = fresh_config(&ids, &d.config).await?;
    let (instance, _) = attest_host::init(&ids, &config, &d.bootstrap).await?;
    let mut witness = lc::init_store(&config, &d.bootstrap).map_err(|e| anyhow!("{e}"))?.0;
    let mut rows = Vec::new();
    let head_now = || {
        let ids = ids.clone();
        async move {
            attest_host::read_head(&ids, &instance)
                .await?
                .ok_or_else(|| anyhow!("head"))
        }
    };
    let job_of = |w: &StoreWitness, u: &[u8]| -> Result<JobInputs> {
        let n = lc::verify_and_apply(&config, w, u).map_err(|e| anyhow!("{e}"))?;
        let w3 = lc_v3::StoreWitnessV3::from_v1(w, n.signature_slot);
        let keys = lc_v3::absent_keys(&w3.committee_ssz, u);
        let seed = lc_v3::job_seed(&instance, &n.pre_store, u);
        Ok((seed, lc_v3::job_id(&v3, &seed), n.head.slot, w3, keys))
    };
    let begin =
        |config: &InstanceConfig, instance: AccountId, w3: &lc_v3::StoreWitnessV3, u: &[u8], keys: &[[u8; 96]]| {
            LcVerifyV3Message::Begin {
                attest_program: ids.attest,
                instance,
                config: config.clone(),
                witness: w3.clone(),
                update_ssz: u.to_vec(),
                absent_keys: keys.to_vec(),
            }
        };
    let mut record =
        |case: &str, want: &str, hash: &str, reverted: bool, text: &str, t: Option<&TxInBlock>, unchanged: bool| {
            match t {
                Some(t) => println!(
                    "{case}: tx {hash} block {} reverted {reverted} charged {:?}",
                    t.block, t.block_gas_exec
                ),
                None => println!("{case}: not included: {}", text.lines().last().unwrap_or_default()),
            }
            rows.push(json!({
            "case": case, "expected_code": want, "tx": if t.is_some() { json!(hash) } else { Value::Null },
            "included": t.is_some(), "block": t.map(|t| t.block), "reverted": reverted,
            "wallet_message": text.lines().last().unwrap_or_default(),
            "block_gas_used_exec": t.and_then(|t| t.block_gas_exec), "charged_txs_in_block": t.map(|t| t.charged_txs),
            "charged_gas": t.filter(|t| t.charged_txs == 1).and_then(|t| t.block_gas_exec),
            "head_unchanged": unchanged,
        }));
        };
    let sig_of = |u: &[u8]| -> Result<Vec<u8>> {
        use ssz::Encode;
        let f = lc::forks(&config).map_err(|e| anyhow!("{e}"))?;
        Ok(match lc::decode_update(u, &f).map_err(|e| anyhow!("{e}"))? {
            lc::AnyUpdate::Full(x) => x.sync_aggregate().sync_committee_signature.as_ssz_bytes(),
            lc::AnyUpdate::Finality(x) => x.sync_aggregate().sync_committee_signature.as_ssz_bytes(),
        })
    };

    // The period update through all three steps (the job is then done).
    let u0 = d.period_update.clone();
    let (seed0, _, slot0, w30, keys0) = job_of(&witness, &u0)?;
    attest_host::submit_update_staged(&ids, v3, &instance, &u0).await?;
    witness = lc::verify_and_apply(&config, &witness, &u0)
        .map_err(|e| anyhow!("{e}"))?
        .new_witness();
    // 1. Begin again on the done job: E14 (exit code).
    let before = head_now().await?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seed0,
        &instance,
        slot0,
        &begin(&config, instance, &w30, &u0, &keys0),
        "duplicate begin",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    record(
        "Begin again on a done job",
        "E14 (exit code 14)",
        &h,
        rev,
        &text,
        Some(&t),
        before == head_now().await?,
    );
    // 2. Miller on the done job: E14.
    let before = head_now().await?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seed0,
        &instance,
        slot0,
        &LcVerifyV3Message::Miller,
        "miller on a done job",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    record(
        "Miller on a done job",
        "E14 (exit code 14)",
        &h,
        rev,
        &text,
        Some(&t),
        before == head_now().await?,
    );

    // 3. Finish before Miller on the handoff update's job: E01; then the job completes normally.
    let u1 = d.handoff_update.clone();
    let (seed1, _, slot1, w31, keys1) = job_of(&witness, &u1)?;
    let (hb, revb, _) = send_step(
        &ids,
        v3,
        seed1,
        &instance,
        slot1,
        &begin(&config, instance, &w31, &u1, &keys1),
        "begin",
    )
    .await?;
    ensure!(!revb, "Begin of the handoff update reverted ({hb})");
    let before = head_now().await?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seed1,
        &instance,
        slot1,
        &LcVerifyV3Message::Finish,
        "finish before miller",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    record(
        "Finish before Miller",
        "E01 (exit code 1)",
        &h,
        rev,
        &text,
        Some(&t),
        before == head_now().await?,
    );
    attest_host::submit_update_staged(&ids, v3, &instance, &u1).await?;
    witness = lc::verify_and_apply(&config, &witness, &u1)
        .map_err(|e| anyhow!("{e}"))?
        .new_witness();

    // Two finality updates that both apply on this store, with different results.
    let valid: Vec<&(String, Vec<u8>)> = d
        .finality
        .iter()
        .filter(|(_, u)| lc::verify_and_apply(&config, &witness, u).is_ok())
        .collect();
    ensure!(
        valid.len() >= 2,
        "need two finality updates that apply on the store after the handoff"
    );
    let (fa, fb) = (&valid[0].1, &valid[valid.len() - 1].1);

    // 4. Forged signature (the older finality update carrying the period update's signature): Begin and Miller land,
    //    Finish refuses E03 and the head does not move.
    let own = sig_of(fa)?;
    let at = fa
        .windows(96)
        .position(|x| x == own.as_slice())
        .ok_or_else(|| anyhow!("signature bytes"))?;
    let mut forged = fa.clone();
    forged[at..at + 96].copy_from_slice(&sig_of(&u0)?);
    ensure!(lc::verify_and_apply(&config, &witness, &forged).err() == Some(attest_core::EsaError::E03BadSignature));
    let na = lc::verify_and_apply(&config, &witness, fa).map_err(|e| anyhow!("{e}"))?;
    let w3f = lc_v3::StoreWitnessV3::from_v1(&witness, na.signature_slot);
    let keysf = lc_v3::absent_keys(&w3f.committee_ssz, &forged);
    let seedf = lc_v3::job_seed(&instance, &na.pre_store, &forged);
    let (hb, revb, _) = send_step(
        &ids,
        v3,
        seedf,
        &instance,
        na.head.slot,
        &begin(&config, instance, &w3f, &forged, &keysf),
        "forged begin",
    )
    .await?;
    let (hm, revm, _) = send_step(
        &ids,
        v3,
        seedf,
        &instance,
        na.head.slot,
        &LcVerifyV3Message::Miller,
        "forged miller",
    )
    .await?;
    ensure!(!revb && !revm, "forged Begin/Miller reverted ({hb}, {hm})");
    let before = head_now().await?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seedf,
        &instance,
        na.head.slot,
        &LcVerifyV3Message::Finish,
        "forged finish",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    record(
        "forged signature, refused at Finish",
        "E03 (exit code 3)",
        &h,
        rev,
        &text,
        Some(&t),
        before == head_now().await?,
    );

    // 5. Stale job: the older finality update's job runs Begin and Miller; the newer one moves the store first; the
    //    stale job's Finish sends an Apply that attest refuses (E13, panic).
    let (seeds, _, slots, w3s, keyss) = job_of(&witness, fa)?;
    let (hb, revb, _) = send_step(
        &ids,
        v3,
        seeds,
        &instance,
        slots,
        &begin(&config, instance, &w3s, fa, &keyss),
        "stale begin",
    )
    .await?;
    let (hm, revm, _) = send_step(
        &ids,
        v3,
        seeds,
        &instance,
        slots,
        &LcVerifyV3Message::Miller,
        "stale miller",
    )
    .await?;
    ensure!(!revb && !revm, "stale Begin/Miller reverted ({hb}, {hm})");
    attest_host::submit_update_staged(&ids, v3, &instance, fb).await?;
    witness = lc::verify_and_apply(&config, &witness, fb)
        .map_err(|e| anyhow!("{e}"))?
        .new_witness();
    let before = head_now().await?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seeds,
        &instance,
        slots,
        &LcVerifyV3Message::Finish,
        "stale finish",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    record(
        "a job whose store has moved (stale): attest.Apply refuses",
        "E13 (attest panic ESA:E13)",
        &h,
        rev,
        &text,
        Some(&t),
        before == head_now().await?,
    );

    // 6. A Sepolia update sent to this mainnet instance: refused at Begin.
    let sep = pub_data("sepolia", v3)?;
    let us = sep.period_update.clone();
    let native = lc::verify_and_apply(&config, &witness, &us).err();
    let w3x = lc_v3::StoreWitnessV3::from_v1(&witness, 0);
    let prex = w3x.commitment().map_err(|e| anyhow!("{e}"))?;
    let seedx = lc_v3::job_seed(&instance, &prex, &us);
    let before = head_now().await?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seedx,
        &instance,
        0,
        &begin(&config, instance, &w3x, &us, &[]),
        "other network",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    record(
        "a Sepolia update sent to a mainnet instance",
        &format!(
            "E01 (exit code 1; V1 natively: {})",
            native.map_or("accepts", |e| e.code())
        ),
        &h,
        rev,
        &text,
        Some(&t),
        before == head_now().await?,
    );

    // 7. An instance whose store is older than the 14-day bound: its Begin is outside the timestamp window.
    let fab = fab_chain(&d.config, 1900, b"beacon-veil/v3/old-store");
    let old_cfg = fresh_config(&ids, &fab.config(vec![v3], 0)).await?;
    let (old_instance, _) = attest_host::init(&ids, &old_cfg, &fab.bootstrap.ssz()).await?;
    let (ow, _) = lc::init_store(&old_cfg, &fab.bootstrap.ssz()).map_err(|e| anyhow!("{e}"))?;
    let uo = fab.updates[0].ssz();
    let no = lc::verify_and_apply(&old_cfg, &ow, &uo).map_err(|e| anyhow!("{e}"))?;
    let w3o = lc_v3::StoreWitnessV3::from_v1(&ow, no.signature_slot);
    let keyso = lc_v3::absent_keys(&w3o.committee_ssz, &uo);
    let seedo = lc_v3::job_seed(&old_instance, &no.pre_store, &uo);
    let before = attest_host::read_head(&ids, &old_instance).await?;
    let r = send_step(
        &ids,
        v3,
        seedo,
        &old_instance,
        no.head.slot,
        &begin(&old_cfg, old_instance, &w3o, &uo, &keyso),
        "old store begin",
    )
    .await;
    let after = attest_host::read_head(&ids, &old_instance).await?;
    match r {
        Ok((h, rev, text)) => {
            let t = tx_in_block(&w, &h).await?;
            record(
                "Begin on an instance whose store is older than 14 days",
                "outside the timestamp window",
                &h,
                rev,
                &text,
                Some(&t),
                before == after,
            );
        }
        Err(e) => record(
            "Begin on an instance whose store is older than 14 days",
            "outside the timestamp window",
            "",
            true,
            &format!("{e:#}"),
            None,
            before == after,
        ),
    }

    let doc = json!({
        "tool": "tools/cost v3_onchain refusals", "sequencer": "seq-pub", "instance": instance.to_string(),
        "programs": {"attest": ids.attest.to_string(), "lc_verify_v3": v3.to_string()},
        "data": "fixtures/pub/mainnet bootstrap_15679488 (period 1914), fixtures/diff/mainnet updates 1914 and 1915 and finality updates, fixtures/diff/sepolia update 1416; fabricated chain (checkpoint period 1900) for the old store",
        "note": "lc_verify_v3 refuses by exit code (charged the cycles used); attest.Apply refuses by panic (charged the declared gas). charged_gas is the block's charged execution gas when the block holds one charged transaction.",
        "rows": rows,
    });
    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!("wrote {out_path}");
    Ok(())
}

// ---- refusals-onchain: tampered updates of the differential test as real transactions ----

/// Where the update of a refusal case comes from.
#[derive(Clone, Copy)]
enum Source {
    /// The differential test's case of this id (`tests/diff_helios`, `evidence/diff/cases.tsv`), bytes unchanged.
    Diff(&'static str),
    /// The differential test's `fab_against_real_store` construction (our own keys impersonating mainnet, seed
    /// `beacon-veil/diff/v1`), built at the period of instance A's store so that it is checked against that store's
    /// committee. The differential case itself is at period 1911, where instance A has no store.
    FabricatedAtStorePeriod,
}

/// One refusal case. `instance`: `A`, a fresh instance on the real mainnet checkpoint of `fixtures/pub/mainnet`
/// (store period 1914; the untampered update `mainnet/p1914/handoff/valid` is the period update it accepts), or `B`, a
/// fresh instance on the differential test's fabricated chain whose store is the store of the `fabricated/*` cases.
/// `after_control`: sent after the instance's positive control applied. `step` and `code`: where the program logic
/// refuses it; checked natively before anything is sent, then on chain.
struct Spec {
    instance: char,
    source: Source,
    after_control: bool,
    step: &'static str,
    code: attest_core::EsaError,
    what: &'static str,
}

const SPECS: [Spec; 10] = {
    use Source::{Diff, FabricatedAtStorePeriod};
    use attest_core::EsaError::{
        E01Malformed, E02NotFinalised, E03BadSignature, E04LowParticipation, E05HandoffNotVerified, E14AlreadyApplied,
        E15HeaderMismatch,
    };
    [
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/finalized_header_zeroed"),
            after_control: false,
            step: "Begin",
            code: E02NotFinalised,
            what: "finalized header zeroed (unmodified Helios 0.12.0 verify passes this one)",
        },
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/next_committee_branch_node_changed"),
            after_control: false,
            step: "Begin",
            code: E05HandoffNotVerified,
            what: "next sync committee branch tampered (one node changed)",
        },
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/finalized_execution_changed"),
            after_control: false,
            step: "Begin",
            code: E15HeaderMismatch,
            what: "finalized execution payload header changed (no longer matches its branch)",
        },
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/signature_slot_not_after_attested"),
            after_control: false,
            step: "Begin",
            code: E01Malformed,
            what: "signature slot set to the attested slot (not after it)",
        },
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/sig_infinity"),
            after_control: false,
            step: "Miller",
            code: E03BadSignature,
            what: "aggregate signature replaced by the point at infinity",
        },
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/bit_cleared"),
            after_control: false,
            step: "Finish",
            code: E03BadSignature,
            what: "one participation bit cleared: 510 claimed signers, the aggregate signature is of 511",
        },
        Spec {
            instance: 'A',
            source: FabricatedAtStorePeriod,
            after_control: false,
            step: "Finish",
            code: E03BadSignature,
            what: "an update from a fabricated chain (our own keys, all 512 signed) sent to the real mainnet instance",
        },
        Spec {
            instance: 'A',
            source: Diff("mainnet/p1914/handoff/replay_after_apply"),
            after_control: true,
            step: "Begin",
            code: E14AlreadyApplied,
            what: "replay: the update the positive control applied, sent again on the store it produced",
        },
        Spec {
            instance: 'B',
            source: Diff("fabricated/participation341"),
            after_control: false,
            step: "Begin",
            code: E04LowParticipation,
            what: "341 of 512 signers, correctly signed by the chain's committee (below the 342 floor)",
        },
        Spec {
            instance: 'B',
            source: Diff("fabricated/empty_finalized_junk_branch"),
            after_control: false,
            step: "Begin",
            code: E02NotFinalised,
            what: "empty finalized header with a non-zero finality branch, correctly signed (Helios 0.12.0 verify passes this one)",
        },
    ]
};

const STEPS: [&str; 3] = ["Begin", "Miller", "Finish"];
const DIFF_FAB_SEED: &[u8] = b"beacon-veil/diff/v1";

/// One instance of the run: its config (before [`fresh_config`] picks `min_participants`), bootstrap and where they
/// come from.
struct Base {
    config: InstanceConfig,
    bootstrap: Vec<u8>,
    source: String,
}

/// Instances A and B and the differential test's cases.
fn bases(v3: AccountId) -> Result<(Base, Base, Vec<diff_helios::Case>)> {
    use fixtures::fabricate::SLOTS_PER_PERIOD;
    let diff = diff_helios::all_cases()?;
    let case = |id: &str| -> Result<&diff_helios::Case> {
        diff.iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow!("no differential case {id}"))
    };
    let d = pub_data("mainnet", v3)?;
    ensure!(
        case("mainnet/p1914/handoff/valid")?.update == d.period_update,
        "the differential test's mainnet/p1914 update is not the period update of fixtures/pub/mainnet"
    );
    let a = Base {
        config: d.config,
        bootstrap: d.bootstrap,
        source: "fixtures/pub/mainnet bootstrap_15679488 (real mainnet checkpoint, first slot of period 1914)".into(),
    };
    // The differential test's fabricated chain: the store of its `fabricated/*` cases is the chain's bootstrap.
    let w0 = &case("fabricated/chain0")?.witness;
    let period = w0.finalized.slot / SLOTS_PER_PERIOD;
    let net = diff_helios::load::load_network("mainnet")?;
    let chain = fab_chain(&net.config, period, DIFF_FAB_SEED);
    let config = chain.config(vec![v3], 0);
    let bootstrap = chain.bootstrap.ssz();
    let (w, _) = lc::init_store(&config, &bootstrap).map_err(|e| anyhow!("fabricated bootstrap: {e}"))?;
    ensure!(
        w == *w0,
        "the fabricated chain's store differs from the differential cases' store"
    );
    let b = Base {
        config,
        bootstrap,
        source: format!(
            "the differential test's fabricated chain (tests/diff_helios cases.rs fabricated_cases: FabChain seed beacon-veil/diff/v1, impersonating mainnet, checkpoint in period {period}); the store equals the store of the fabricated/* cases"
        ),
    };
    Ok((a, b, diff))
}

/// A case ready to send: its messages, the job they go to and the native verdicts.
struct RefusalCase {
    key: String,
    instance: char,
    after_control: bool,
    what: &'static str,
    source: String,
    diff_expected: Option<String>,
    update: Vec<u8>,
    messages: [LcVerifyV3Message; 3],
    seed: [u8; 32],
    job: AccountId,
    record_slot: u64,
    refused_at: usize,
    code: attest_core::EsaError,
    v1: String,
    helios: String,
}

/// Positive control of one instance: an update that applies, sent after that instance's refusals.
struct Control {
    instance: char,
    source: String,
    update: Vec<u8>,
}

struct CaseSet {
    cases: Vec<RefusalCase>,
    controls: Vec<Control>,
}

/// Steps `Begin`, `Miller`, `Finish` through the program logic (`lc_v3::handle`, the code the deployed program runs)
/// on the job state carried between steps: the messages, job, record slot and the first refusal.
#[allow(clippy::type_complexity)]
fn staged_native(
    attest: AccountId,
    v3: AccountId,
    instance: AccountId,
    config: &InstanceConfig,
    witness: &StoreWitness,
    update: &[u8],
) -> (
    [LcVerifyV3Message; 3],
    [u8; 32],
    AccountId,
    u64,
    Option<(usize, attest_core::EsaError)>,
) {
    use attest_core::lc_v3;
    let (w3, keys) = diff_helios::v3_inputs(config, witness, update);
    let pre = w3.commitment().unwrap_or([0u8; 32]);
    let seed = lc_v3::job_seed(&instance, &pre, update);
    let job = lc_v3::job_id(&v3, &seed);
    let messages = [
        LcVerifyV3Message::Begin {
            attest_program: attest,
            instance,
            config: config.clone(),
            witness: w3,
            update_ssz: update.to_vec(),
            absent_keys: keys,
        },
        LcVerifyV3Message::Miller,
        LcVerifyV3Message::Finish,
    ];
    let (mut state, mut record_slot) = (Vec::new(), 0);
    for (i, m) in messages.iter().enumerate() {
        match lc_v3::handle(&v3, &job, &state, m.clone()) {
            Err(e) => return (messages, seed, job, record_slot, Some((i, e))),
            Ok(eff) => {
                record_slot = eff.job.outcome.head.slot;
                if let Some(s) = eff.state {
                    state = s;
                }
            }
        }
    }
    (messages, seed, job, record_slot, None)
}

/// Every case and control, built and checked natively (no chain): each case must refuse at its [`Spec`] step with its
/// code, each control must be accepted by V1 and by the staged program logic.
fn prepare_cases(
    attest: AccountId,
    v3: AccountId,
    (a, b): (&InstanceConfig, &InstanceConfig),
    (a_boot, b_boot): (&[u8], &[u8]),
    diff: &[diff_helios::Case],
) -> Result<CaseSet> {
    use fixtures::fabricate::SLOTS_PER_PERIOD;
    let case = |id: &str| -> Result<&diff_helios::Case> {
        diff.iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow!("no differential case {id}"))
    };
    let store = |c: &InstanceConfig, boot: &[u8]| -> Result<StoreWitness> {
        Ok(lc::init_store(c, boot).map_err(|e| anyhow!("init store: {e}"))?.0)
    };
    let (wa, wb) = (store(a, a_boot)?, store(b, b_boot)?);
    let control_a = case("mainnet/p1914/handoff/valid")?.update.clone();
    // Instance B's control: 342 of 512 (the floor) when the instance's own minimum is not above it, else all 512.
    let control_b_id = if b.min_participants <= 342 {
        "fabricated/participation342"
    } else {
        "fabricated/chain0"
    };
    let control_b = case(control_b_id)?.update.clone();
    let wa_after = lc::verify_and_apply(a, &wa, &control_a)
        .map_err(|e| anyhow!("control A refused by V1: {e}"))?
        .new_witness();
    let mut cases = Vec::new();
    let mut n = [0usize; 2];
    for s in &SPECS {
        let (config, witness) = match (s.instance, s.after_control) {
            ('A', false) => (a, &wa),
            ('A', true) => (a, &wa_after),
            ('B', false) => (b, &wb),
            _ => bail!("no store for instance {} after its control", s.instance),
        };
        let (update, source, diff_expected) = match s.source {
            Source::Diff(id) => {
                let c = case(id)?;
                (c.update.clone(), id.to_string(), Some(c.expect.short()))
            }
            Source::FabricatedAtStorePeriod => {
                let period = wa.finalized.slot / SLOTS_PER_PERIOD;
                let chain = fab_chain(a, period, DIFF_FAB_SEED);
                (
                    chain.updates[0].ssz(),
                    format!(
                        "fabricated period update, as the differential class fab_against_real_store (FabChain seed beacon-veil/diff/v1 impersonating mainnet), built at period {period}, the period of this instance's store"
                    ),
                    None,
                )
            }
        };
        let instance = instance_id(&attest, config);
        let (messages, seed, job, record_slot, refusal) = staged_native(attest, v3, instance, config, witness, &update);
        let i = usize::from(s.instance == 'B');
        n[i] += 1;
        let key = format!("{}{}", s.instance, n[i]);
        let (at, code) = refusal.ok_or_else(|| anyhow!("{key} ({source}): the program logic accepts it"))?;
        ensure!(
            STEPS[at] == s.step && code == s.code,
            "{key} ({source}): refused at {} with {}, expected {} with {}",
            STEPS[at],
            code.code(),
            s.step,
            s.code.code()
        );
        if let Some(e) = &diff_expected {
            ensure!(
                *e == code.code(),
                "{key}: the differential case expects {e}, here {}",
                code.code()
            );
        }
        let v1 = lc::verify_and_apply(config, witness, &update)
            .err()
            .map_or("accept".to_string(), |e| e.code().to_string());
        let probe = diff_helios::Case {
            id: key.clone(),
            network: String::new(),
            fork: "",
            kind: diff_helios::Kind::Tampered,
            class: "",
            crossing: false,
            source: String::new(),
            config: config.clone(),
            witness: witness.clone(),
            update: update.clone(),
            expect: diff_helios::Expect::Refuse(code),
            divergence: None,
        };
        cases.push(RefusalCase {
            key,
            instance: s.instance,
            after_control: s.after_control,
            what: s.what,
            source,
            diff_expected,
            update,
            messages,
            seed,
            job,
            record_slot,
            refused_at: at,
            code,
            v1,
            helios: diff_helios::helios_raw(&probe).verify,
        });
    }
    for (name, config, witness, u) in [("A", a, &wa, &control_a), ("B", b, &wb, &control_b)] {
        let instance = instance_id(&attest, config);
        let (.., refusal) = staged_native(attest, v3, instance, config, witness, u);
        ensure!(
            refusal.is_none(),
            "control {name} refused by the program logic: {refusal:?}"
        );
        lc::verify_and_apply(config, witness, u).map_err(|e| anyhow!("control {name} refused by V1: {e}"))?;
    }
    Ok(CaseSet {
        cases,
        controls: vec![
            Control {
                instance: 'A',
                source: "mainnet/p1914/handoff/valid (the untampered update of the A cases)".into(),
                update: control_a,
            },
            Control {
                instance: 'B',
                source: format!(
                    "{control_b_id} (the untampered fabricated period update re-signed by the first 342 or all 512 members)"
                ),
                update: control_b,
            },
        ],
    })
}

/// The job account's state under `lc_verify_v3` (empty if the account does not exist).
async fn read_job(w: &attest_host::Wallet, v3: AccountId, job: AccountId) -> Result<Vec<u8>> {
    Ok(w.get_account_view(Actor::new(job, v3))
        .await?
        .map(|a| a.data.actor_state(v3).to_vec())
        .unwrap_or_default())
}

async fn instance_bytes(ids: &DeployedIds, instance: &AccountId) -> Result<Vec<u8>> {
    let st = attest_host::read_instance(ids, instance)
        .await?
        .ok_or_else(|| anyhow!("instance {instance} missing"))?;
    Ok(borsh::to_vec(&st)?)
}

fn head_json(h: &attest_core::Head) -> Value {
    json!({"slot": h.slot, "period": h.period, "beacon_root": hex::encode(h.beacon_root),
           "store_commitment": hex::encode(h.store_commitment)})
}

fn step_json(name: &str, hash: &str, t: &TxInBlock, cycles: u64, exit: &str) -> Value {
    let alone = t.charged_txs == 1;
    json!({
        "step": name, "tx": hash, "block": t.block, "position": t.position, "block_timestamp_ms": t.timestamp,
        "reverted": t.reverted, "payer": t.fee.map(|f| f.0.to_string()), "declared_gas": t.fee.map(|f| f.1),
        "block_gas_used_exec": t.block_gas_exec, "charged_txs_in_block": t.charged_txs,
        "charged_gas": if alone { json!(t.block_gas_exec) } else { Value::Null },
        "executor_lc_verify_v3": {"exit": exit, "metered_cycles": cycles},
        "charged_equals_executor": if alone { json!(t.block_gas_exec == Some(cycles)) } else { Value::Null },
    })
}

async fn refusals_onchain(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out_path = arg("--out").context("--out")?;
    let v3g = load(&arg("--v3-elf").context("--v3-elf")?)?;
    let atg = load(&arg("--attest-elf").context("--attest-elf")?)?;
    ensure!(
        std::env::var("BV_ALLOW_REVERT").is_ok_and(|v| v == "1"),
        "set BV_ALLOW_REVERT=1: refused steps must reach the chain"
    );
    let ids = DeployedIds::from_env()?;
    let v3 = attest_host::deployed_program("lc_verify_v3")?;
    let w = attest_host::open_wallet()?;
    let (a, b, diff) = bases(v3)?;
    let (ca, cb) = (
        fresh_config(&ids, &a.config).await?,
        fresh_config(&ids, &b.config).await?,
    );
    let set = prepare_cases(ids.attest, v3, (&ca, &cb), (&a.bootstrap, &b.bootstrap), &diff)?;
    println!(
        "prepared: {} refusal cases and {} controls, all as predicted natively",
        set.cases.len(),
        set.controls.len()
    );

    let mut instances = serde_json::Map::new();
    let mut case_rows = Vec::new();
    let mut control_rows = Vec::new();
    for (name, base, config) in [('A', &a, &ca), ('B', &b, &cb)] {
        let (instance, init) = attest_host::init(&ids, config, &base.bootstrap).await?;
        let ib = tx_in_block(&w, &init.tx).await?;
        println!("instance {name}: {instance} init tx {} block {}", init.tx, ib.block);
        let witness = lc::init_store(config, &base.bootstrap).map_err(|e| anyhow!("{e}"))?.0;
        let ours: Vec<&RefusalCase> = set.cases.iter().filter(|c| c.instance == name).collect();
        for c in ours.iter().filter(|c| !c.after_control) {
            case_rows.push(send_case(&ids, &w, &v3g, v3, instance, c).await?);
        }
        let control = set
            .controls
            .iter()
            .find(|c| c.instance == name)
            .ok_or_else(|| anyhow!("control {name}"))?;
        control_rows.push(send_control(&ids, &w, (&v3g, &atg), v3, instance, config, &witness, control).await?);
        for c in ours.iter().filter(|c| c.after_control) {
            case_rows.push(send_case(&ids, &w, &v3g, v3, instance, c).await?);
        }
        let st = attest_host::read_instance(&ids, &instance)
            .await?
            .ok_or_else(|| anyhow!("instance"))?;
        instances.insert(
            name.to_string(),
            json!({
                "instance": instance.to_string(), "source": base.source, "config_chain_id": config.chain_id,
                "checkpoint_slot": config.checkpoint_slot, "min_participants": config.min_participants,
                "participation_floor": config.min_participants.max(342), "verifiers": ["lc_verify_v3"],
                "init": {"tx": init.tx, "block": ib.block}, "final_head": head_json(&st.head),
            }),
        );
    }

    let refused_steps: Vec<&str> = case_rows.iter().filter_map(|r| r["refused_at"].as_str()).collect();
    let count = |s: &str| refused_steps.iter().filter(|x| **x == s).count();
    let txs = case_rows
        .iter()
        .chain(&control_rows)
        .map(|r| r["steps"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    let doc = json!({
        "tool": "tools/cost v3_onchain refusals-onchain",
        "what": "tampered light-client updates of the differential test (tests/diff_helios) sent to lc_verify_v3 on the public path (Begin, Miller, Finish; one public transaction each) on seq-pub (LEZ v0.4.0-rc3 standalone, RISC0_DEV_MODE unset: public transactions are executed by the sequencer, no proofs involved). Each case is sent step by step up to the step that refuses; the steps before it land, the refusing step is included and reverted, and the instance state is the same before and after the case. One untampered update per instance is then applied (positive control).",
        "sequencer": "seq-pub",
        "programs": {"attest": ids.attest.to_string(), "lc_verify_v3": v3.to_string()},
        "executor_images": {"lc_verify_v3": v3g.image_id, "attest": atg.image_id},
        "instances": instances,
        "cases": case_rows,
        "controls": control_rows,
        "summary": {
            "refusal_cases": case_rows.len(),
            "refused_at": {"Begin": count("Begin"), "Miller": count("Miller"), "Finish": count("Finish")},
            "every_refusal_reverted_with_predicted_exit_code": case_rows.iter().all(|r| r["refused_as_predicted"] == true),
            "instance_state_unchanged_after_every_refusal": case_rows.iter().all(|r| r["instance_state_unchanged"] == true),
            "controls_applied": control_rows.len(),
            "transactions": txs,
        },
        "note": "Codes: lc_verify_v3 refuses by exit code (the EsaError number), which LEZ charges by the cycles used; the sequencer does not log exit codes, so the code of each refused step is the exit of the deployed ELF re-executed in the RISC Zero executor on the same message and the job state read from the chain before the step (executor_lc_verify_v3.exit), and charged_gas equals its metered cycles when the step is alone in its block. native.v3_staged is the program logic (lc_v3::handle) on the same input; native.v1 is V1 (Helios 0.12.0 with our pre-checks) and native.helios_0_12_0_verify is unmodified Helios's verify_* on this instance's store and the same bytes. Refusal steps declare 10,000,000 gas (the refusal tool); the controls go through attest_host::submit_update_staged (9.0M, 7.0M, 5.5M).",
        "rerun": "scripts/seq.sh pub start (dev mode unset) and the release set deployed (docs/PUBLIC-PATH.md, How to re-run); export BV_IDS=deploy/ids-pub.toml LEE_WALLET_HOME_DIR=<wallet on seq-pub> BV_PAYER=<funded public account> BV_ALLOW_REVERT=1; cargo build -p cost --bin v3_onchain; v3_onchain refusals-onchain --v3-elf <release>/lc_verify_v3.bin --attest-elf <release>/attest.bin --out refusals-onchain.json. Each run makes two fresh instances, so it gives new transaction hashes. v3_onchain check refusals-onchain.json re-reads the recorded transactions.",
    });
    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!(
        "wrote {out_path}: {} refusal cases, {txs} transactions",
        case_rows.len()
    );
    Ok(())
}

fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
}

/// Sends one case step by step up to its refusing step and records every transaction.
async fn send_case(
    ids: &DeployedIds,
    w: &attest_host::Wallet,
    v3g: &Guest,
    v3: AccountId,
    instance: AccountId,
    c: &RefusalCase,
) -> Result<Value> {
    let before = instance_bytes(ids, &instance).await?;
    let mut steps = Vec::new();
    let mut ok = true;
    let mut job_unchanged = true;
    for (i, m) in c.messages.iter().enumerate().take(c.refused_at + 1) {
        let what = format!("{} {}", c.key, STEPS[i]);
        let pre_job = read_job(w, v3, c.job).await?;
        let (hash, reverted, text) = send_step(ids, v3, c.seed, &instance, c.record_slot, m, &what).await?;
        let t = tx_in_block(w, &hash).await?;
        let post_job = read_job(w, v3, c.job).await?;
        let (cycles, exit) = exec_any(
            v3g,
            &ReceiveInput {
                receiver: Actor::new(c.job, v3),
                from: None,
                is_authorized: false,
                pre_state: ActorState::from(pre_job.clone()),
                message: t.message.clone(),
            },
        )?;
        ensure!(
            t.to == Actor::new(c.job, v3) && step_name(&t.message) == STEPS[i],
            "{what}: the transaction is not this step on the job account"
        );
        let refusing = i == c.refused_at;
        let want_exit = if refusing {
            format!("Halted({})", c.code as u8)
        } else {
            "Halted(0)".to_string()
        };
        ok &= reverted == refusing && exit == want_exit;
        if refusing {
            job_unchanged = pre_job == post_job;
        }
        println!(
            "{what}: tx {hash} block {} reverted {reverted} exit {exit} charged {:?}",
            t.block, t.block_gas_exec
        );
        let mut s = step_json(STEPS[i], &hash, &t, cycles, &exit);
        if refusing {
            s["wallet_message"] = json!(text.lines().last().unwrap_or_default());
            s["job_state_unchanged"] = json!(job_unchanged);
        }
        steps.push(s);
    }
    let after = instance_bytes(ids, &instance).await?;
    let unchanged = before == after;
    let head = attest_core::rules::decode_instance(&after)
        .map_err(|e| anyhow!("{e}"))?
        .head;
    ensure!(unchanged, "{}: the instance state moved after a refused update", c.key);
    Ok(json!({
        "case": c.key, "instance": c.instance.to_string(), "what": c.what, "source": c.source,
        "diff_expected": c.diff_expected, "update_sha256": hex::encode(sha256(&c.update)),
        "update_bytes": c.update.len(), "job": c.job.to_string(), "after_control": c.after_control,
        "native": {"v3_staged": {"step": STEPS[c.refused_at], "code": c.code.code()}, "v1": c.v1,
                   "helios_0_12_0_verify": c.helios},
        "refused_at": STEPS[c.refused_at], "code": c.code.code(), "exit_code": c.code as u8,
        "refused_as_predicted": ok && job_unchanged,
        "steps": steps,
        "instance_state_unchanged": unchanged, "head": head_json(&head),
    }))
}

/// Sends the positive control through `attest_host::submit_update_staged` and checks the head against V1.
#[allow(clippy::too_many_arguments)]
async fn send_control(
    ids: &DeployedIds,
    w: &attest_host::Wallet,
    (v3g, atg): (&Guest, &Guest),
    v3: AccountId,
    instance: AccountId,
    config: &InstanceConfig,
    witness: &StoreWitness,
    c: &Control,
) -> Result<Value> {
    let name = c.instance;
    let native = lc::verify_and_apply(config, witness, &c.update).map_err(|e| anyhow!("control {name}: {e}"))?;
    let before = instance_bytes(ids, &instance).await?;
    let staged = attest_host::submit_update_staged(ids, v3, &instance, &c.update).await?;
    let head = attest_host::read_head(ids, &instance)
        .await?
        .ok_or_else(|| anyhow!("instance missing"))?;
    // As `run`: attest.Apply replaces the head when the finalized slot moves, else it only sets the commitment.
    let equal = if native.new_slot {
        head == native.head
    } else {
        head.store_commitment == native.post_store && head.slot == native.head.slot
    };
    ensure!(equal, "control {name}: on-chain head differs from V1");
    let mut job_state = Vec::new();
    let mut steps = Vec::new();
    for (step, o) in &staged.steps {
        let t = tx_in_block(w, &o.tx).await?;
        ensure!(!t.reverted, "control {name} {step}: reverted");
        let (cycles, chained, post) = metered(
            v3g,
            atg,
            (&ids.attest, &v3),
            staged.job,
            &job_state,
            instance,
            &before,
            &t.message,
        )?;
        job_state = post;
        let mut s = step_json(step, &o.tx, &t, cycles + chained, "Halted(0)");
        s["metered_cycles_chained_attest"] = json!(chained);
        println!(
            "control {name} {step}: tx {} block {} charged {:?}",
            o.tx, t.block, t.block_gas_exec
        );
        steps.push(s);
    }
    let head_before = attest_core::rules::decode_instance(&before)
        .map_err(|e| anyhow!("{e}"))?
        .head;
    Ok(json!({
        "case": format!("control {name}"), "instance": name.to_string(), "update": c.source,
        "update_sha256": hex::encode(sha256(&c.update)), "participation": native.participation,
        "job": staged.job.to_string(), "steps": steps, "applied": true,
        "head_before": head_json(&head_before), "head_after": head_json(&head), "head_equals_v1": equal,
        "finalized_slot_moved": native.new_slot,
    }))
}

/// `check` for a `refusals-onchain` record: every step at its block and position, reverted exactly when recorded,
/// sent to its job account; each instance's head equals the recorded final head.
async fn check_refusals(w: &attest_host::Wallet, ids: &DeployedIds, p: &str, doc: &Value) -> Result<usize> {
    let v3 = AccountId::from_str(doc["programs"]["lc_verify_v3"].as_str().unwrap_or_default())
        .map_err(|e| anyhow!("{p}: lc_verify_v3 id: {e:?}"))?;
    let mut n = 0;
    for r in doc["cases"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(doc["controls"].as_array().into_iter().flatten())
    {
        let job = AccountId::from_str(r["job"].as_str().unwrap_or_default()).map_err(|e| anyhow!("job: {e:?}"))?;
        for s in r["steps"].as_array().into_iter().flatten() {
            let hash = s["tx"].as_str().unwrap_or_default();
            let what = format!("{p} {} {} tx {hash}", r["case"], s["step"]);
            let t = tx_in_block(w, hash).await.with_context(|| what.clone())?;
            ensure!(
                Some(t.block) == s["block"].as_u64() && Some(t.position as u64) == s["position"].as_u64(),
                "{what}: block or position differs"
            );
            ensure!(
                Some(t.reverted) == s["reverted"].as_bool(),
                "{what}: reverted {} recorded {}",
                t.reverted,
                s["reverted"]
            );
            ensure!(t.to == Actor::new(job, v3), "{what}: receiver differs");
            ensure!(
                Some(step_name(&t.message).as_str()) == s["step"].as_str(),
                "{what}: step differs"
            );
            ensure!(
                t.block_gas_exec == s["block_gas_used_exec"].as_u64(),
                "{what}: block gas differs"
            );
            n += 1;
        }
    }
    for (name, i) in doc["instances"].as_object().into_iter().flatten() {
        let instance = AccountId::from_str(i["instance"].as_str().unwrap_or_default())
            .map_err(|e| anyhow!("{p}: instance {name}: {e:?}"))?;
        let head = attest_host::read_head(ids, &instance)
            .await?
            .ok_or_else(|| anyhow!("{p}: instance {name} missing"))?;
        ensure!(
            head_json(&head) == i["final_head"],
            "{p}: instance {name}: head differs from the recorded final head"
        );
    }
    Ok(n)
}

// ---- annotate (read-only: blocks over RPC, the executor, native code; no transaction) ----

/// One transaction of a block, as `annotate` needs it.
struct BlockTx {
    hash: String,
    to: Option<Actor>,
    message: Vec<u8>,
    fee: Option<(AccountId, u64)>,
    reverted: bool,
}

struct BlockView {
    txs: Vec<BlockTx>,
    timestamp: u64,
    gas_exec: Option<u64>,
    revenue: Option<u128>,
}

async fn block_view(w: &attest_host::Wallet, b: u64) -> Result<BlockView> {
    let block = w.get_block(b).await?.ok_or_else(|| anyhow!("block {b} not found"))?;
    let fee_program = AccountId::from_builtin_program_name(b"fee");
    let mut v = BlockView {
        txs: Vec::new(),
        timestamp: block.header.timestamp,
        gas_exec: None,
        revenue: None,
    };
    for (i, t) in block.body.transactions.iter().enumerate() {
        let reverted = block.body.is_reverted(i);
        match t {
            common::transaction::LeeTransaction::Public(p) => {
                let ex = &p.message.execution;
                if ex.fee.is_none()
                    && ex.root.to.program_account_id == fee_program
                    && let Ok(FeeMessage::Distribute { summary, .. }) = borsh::from_slice(&ex.root.message)
                {
                    v.gas_exec = Some(summary.gas_used_exec);
                    v.revenue = Some(summary.revenue_base + summary.revenue_tip);
                }
                v.txs.push(BlockTx {
                    hash: t.hash().to_string(),
                    to: Some(ex.root.to),
                    message: ex.root.message.clone(),
                    fee: ex.fee.as_ref().map(|f| (f.payer, f.gas_limit)),
                    reverted,
                });
            }
            common::transaction::LeeTransaction::PrivacyPreserving(_) => v.txs.push(BlockTx {
                hash: t.hash().to_string(),
                to: None,
                message: Vec::new(),
                fee: Some((AccountId::new([0; 32]), 0)),
                reverted,
            }),
        }
    }
    Ok(v)
}

/// Executor run that also reports a non-zero exit (a refusal by exit code).
fn exec_any(g: &Guest, input: &ReceiveInput) -> Result<(u64, String)> {
    let frame = lee_core::to_frame(&borsh::to_vec(input)?);
    let env = ExecutorEnv::builder().write_slice(&frame).build()?;
    let info = default_executor().execute(env, &g.program)?;
    Ok((info.cycles(), format!("{:?}", info.exit_code)))
}

/// The job state before block `before` at `receiver`: every earlier included, not reverted transaction to that actor,
/// replayed with the program logic.
fn job_state_before(chain: &[(u64, BlockView)], receiver: Actor, before: u64) -> Vec<u8> {
    let mut state = Vec::new();
    for (b, v) in chain {
        if *b >= before {
            break;
        }
        for t in &v.txs {
            if t.to == Some(receiver)
                && !t.reverted
                && let Ok(m) = borsh::from_slice::<LcVerifyV3Message>(&t.message)
                && let Ok(e) = attest_core::lc_v3::handle(&receiver.program_account_id, &receiver.account_id, &state, m)
                && let Some(s) = e.state
            {
                state = s;
            }
        }
    }
    state
}

/// `ESA:Exx` panic lines the sequencer printed while it built block `b`.
fn panics_in_block(log: &str, b: u64) -> Vec<String> {
    let start = format!("Publishing block {} ", b.saturating_sub(1));
    let end = format!("Publishing block {b} ");
    let mut on = false;
    let mut out = Vec::new();
    for l in log.lines() {
        if l.contains(&start) {
            on = true;
            continue;
        }
        if l.contains(&end) {
            break;
        }
        if on && l.starts_with("ESA:") {
            out.push(l.trim().to_string());
        }
    }
    out
}

async fn annotate(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let v3g = load(&arg("--v3-elf").context("--v3-elf")?)?;
    let seq_log = std::fs::read_to_string(arg("--seq-log").context("--seq-log")?)?;
    let files: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && (*i == 0 || !args[i - 1].starts_with("--")))
        .map(|(_, a)| a)
        .collect();
    let w = attest_host::open_wallet()?;
    let ids = DeployedIds::from_env()?;
    // Every block once.
    let mut last = 0u64;
    for f in &files {
        let doc: Value = serde_json::from_slice(&std::fs::read(f)?)?;
        let blocks: Vec<u64> = doc["updates"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|u| u["steps"].as_array().cloned().unwrap_or_default())
            .chain(doc["rows"].as_array().cloned().unwrap_or_default())
            .filter_map(|s| s["block"].as_u64())
            .collect();
        last = last.max(blocks.into_iter().max().unwrap_or(0));
    }
    let mut chain = Vec::new();
    for b in 1..=last {
        if let Ok(v) = block_view(&w, b).await {
            chain.push((b, v));
        }
    }
    let view = |b: u64| chain.iter().find(|(x, _)| *x == b).map(|(_, v)| v);
    for f in files {
        let mut doc: Value = serde_json::from_slice(&std::fs::read(f)?)?;
        let v3 = AccountId::from_str(doc["programs"]["lc_verify_v3"].as_str().unwrap_or_default())
            .map_err(|e| anyhow!("{f}: {e:?}"))?;
        if doc.get("updates").is_some() {
            // run.json
            let network = doc["network"].as_str().unwrap_or_default().to_string();
            let (config, bootstrap, ups) = net_updates(&network, v3)?;
            let mut witness = lc::init_store(&config, &bootstrap).map_err(|e| anyhow!("{e}"))?.0;
            let mut all_equal = true;
            for u in doc["updates"].as_array_mut().into_iter().flatten() {
                let label = u["update"].as_str().unwrap_or_default().to_string();
                let bytes = &ups
                    .iter()
                    .find(|(l, _)| *l == label)
                    .ok_or_else(|| anyhow!("{f}: no input {label}"))?
                    .1;
                let n = lc::verify_and_apply(&config, &witness, bytes).map_err(|e| anyhow!("{label}: {e}"))?;
                let h = &u["head_after"];
                let same_commitment = h["store_commitment"].as_str() == Some(hex::encode(n.post_store).as_str())
                    && h["slot"].as_u64() == Some(n.head.slot);
                let equal = if n.new_slot {
                    same_commitment && h["beacon_root"].as_str() == Some(hex::encode(n.head.beacon_root).as_str())
                } else {
                    same_commitment
                };
                all_equal &= equal;
                u["head_equals_v1"] = json!(equal);
                u["head_equals_v1_compared"] = json!(if n.new_slot {
                    "slot, beacon_root and store_commitment of the on-chain head after the update, against V1 (Helios 0.12.0, native) on the same input"
                } else {
                    "slot and store_commitment (the update only teaches the next committee: attest keeps the head and sets the new commitment), against V1 native"
                });
                u["finalized_slot_moved"] = json!(n.new_slot);
                witness = n.new_witness();
                for s in u["steps"].as_array_mut().into_iter().flatten() {
                    let b = s["block"].as_u64().unwrap_or(0);
                    let hash = s["tx"].as_str().unwrap_or_default().to_string();
                    let v = view(b).ok_or_else(|| anyhow!("block {b}"))?;
                    let t = v
                        .txs
                        .iter()
                        .find(|t| t.hash == hash)
                        .ok_or_else(|| anyhow!("{hash} not in block {b}"))?;
                    let (payer, declared) = t.fee.ok_or_else(|| anyhow!("{hash}: no fee declaration"))?;
                    s["payer"] = json!(payer.to_string());
                    s["declared_gas"] = json!(declared);
                    let charged: Vec<&BlockTx> = v.txs.iter().filter(|t| t.fee.is_some()).collect();
                    if charged.len() == 1 {
                        s["fee_paid"] = json!(v.revenue.map(|r| r.to_string()));
                        s["fee_note"] = json!(
                            "base + tip revenue of the block's fee summary; the step is the block's only charged transaction"
                        );
                    } else {
                        // Charge the other transactions by re-executing them on their job state.
                        let mut others = 0u64;
                        let mut notes = Vec::new();
                        for o in charged.iter().filter(|o| o.hash != hash) {
                            let to = o.to.ok_or_else(|| anyhow!("{}: not public", o.hash))?;
                            ensure!(
                                to.program_account_id == v3,
                                "{}: other transaction is not lc_verify_v3",
                                o.hash
                            );
                            let pre = job_state_before(&chain, to, b);
                            let (cycles, exit) = exec_any(
                                &v3g,
                                &ReceiveInput {
                                    receiver: to,
                                    from: None,
                                    is_authorized: false,
                                    pre_state: ActorState::from(pre),
                                    message: o.message.clone(),
                                },
                            )?;
                            ensure!(
                                !exit.contains("Halted(0)"),
                                "{}: other step succeeded; its chained calls are not metered here",
                                o.hash
                            );
                            others += cycles;
                            notes.push(json!({"tx": o.hash, "step": step_name(&o.message), "payer": o.fee.map(|f| f.0.to_string()), "metered_cycles": cycles, "exit": exit}));
                        }
                        let derived = v.gas_exec.map(|g| g - others);
                        s["charged_gas"] = json!(derived);
                        s["charged_gas_method"] = json!(
                            "block charged execution gas minus the other charged transactions of the block, each re-executed with the deployed lc_verify_v3 on its job state (a refusal by exit code is charged its cycles)"
                        );
                        s["other_charged_txs"] = json!(notes);
                        s["fee_paid"] = Value::Null;
                        s["fee_note"] = json!(
                            "the block holds more than one charged transaction; the block's fee revenue is not split"
                        );
                        s["within_step_limit"] = json!(derived.is_some_and(|d| d <= STEP_LIMIT));
                    }
                }
            }
            doc["head_equals_v1_every_update"] = json!(all_equal);
            doc["anyone_can_submit"] = json!(
                "each step is a public transaction any funded account can send (the job account is a PDA of lc_verify_v3; the program does not check the sender). Shown by evidence/g4-public/dry-run-20261120/: two followers with their own payers on one instance shared each update's job"
            );
            println!("{f}: head_equals_v1 {all_equal}");
        } else {
            // refusals.json
            let instance =
                AccountId::from_str(doc["instance"].as_str().unwrap_or_default()).map_err(|e| anyhow!("{e:?}"))?;
            let inst_state = attest_host::read_instance(&ids, &instance)
                .await?
                .ok_or_else(|| anyhow!("instance"))?;
            for r in doc["rows"].as_array_mut().into_iter().flatten() {
                let hash = r["tx"].as_str().unwrap_or_default().to_string();
                let b = r["block"].as_u64().unwrap_or(0);
                let v = view(b).ok_or_else(|| anyhow!("block {b}"))?;
                let t = v
                    .txs
                    .iter()
                    .find(|t| t.hash == hash)
                    .ok_or_else(|| anyhow!("{hash} not in block {b}"))?;
                let to = t.to.ok_or_else(|| anyhow!("not public"))?;
                let (payer, declared) = t.fee.ok_or_else(|| anyhow!("no fee"))?;
                if let Some(e) = r.get("expected").cloned() {
                    r["expected_code"] = e;
                    r.as_object_mut().map(|o| o.remove("expected"));
                }
                if let Some(e) = r.get("refusal_text").cloned() {
                    r["wallet_message"] = e;
                    r.as_object_mut().map(|o| o.remove("refusal_text"));
                }
                r["payer"] = json!(payer.to_string());
                r["declared_gas"] = json!(declared);
                let pre = job_state_before(&chain, to, b);
                let msg: LcVerifyV3Message = borsh::from_slice(&t.message)?;
                let native = attest_core::lc_v3::handle(&to.program_account_id, &to.account_id, &pre, msg);
                let (cycles, exit) = exec_any(
                    &v3g,
                    &ReceiveInput {
                        receiver: to,
                        from: None,
                        is_authorized: false,
                        pre_state: ActorState::from(pre.clone()),
                        message: t.message.clone(),
                    },
                )?;
                r["executor_lc_verify_v3"] = json!({"metered_cycles": cycles, "exit": exit});
                let panics = panics_in_block(&seq_log, b);
                let (code, observed) = match native {
                    Err(e) => (
                        e.code().to_string(),
                        format!(
                            "reverted; lc_verify_v3 ends with exit code {} (the sequencer does not log exit codes; the deployed ELF re-executed on the same input gives {exit} after {cycles} cycles, and the block charged {})",
                            e as u8,
                            v.gas_exec.map_or("an unknown amount".to_string(), |g| g.to_string())
                        ),
                    ),
                    Ok(eff) => match eff.apply {
                        Some((_, _, o)) => {
                            let st = borsh::to_vec(&inst_state)?;
                            let c =
                                attest_core::rules::apply(&st, Some(v3), o.pre_store, o.post_store, o.head, o.record)
                                    .err()
                                    .map_or("accepted".to_string(), |e| e.code().to_string());
                            (
                                c,
                                format!(
                                    "reverted; lc_verify_v3 accepts the step, the chained attest.Apply panics; sequencer log for block {b}: {panics:?}"
                                ),
                            )
                        }
                        None => {
                            let (from, to_ms) = eff.window_ms.unwrap_or((0, 0));
                            let outside = v.timestamp < from || v.timestamp >= to_ms;
                            (
                                "none (accepted by the program)".to_string(),
                                format!(
                                    "reverted; the program accepts the step natively and exits 0 ({exit}); its timestamp window is [{from}, {to_ms}) ms and the block time is {} ms ({}); no panic in the sequencer log for block {b}; LEZ checks the window at settlement and charges a miss the whole declared gas",
                                    v.timestamp,
                                    if outside {
                                        "outside the window"
                                    } else {
                                        "inside the window"
                                    }
                                ),
                            )
                        }
                    },
                };
                r["native_code"] = json!(code);
                r["observed"] = json!(observed);
            }
            doc["note"] = json!(
                "Codes: expected_code is what the case was built to show; native_code is lc_verify_v3's program logic (lc_v3::handle) on the same message and job state, then attest's rules for a chained Apply. lc_verify_v3 refuses by exit code (charged the cycles used); attest.Apply refuses by panic, and a timestamp-window miss is charged the whole declared gas. declared_gas is 10,000,000 here (the refusal tool); attest_host::submit_update_staged declares 9,000,000 for Begin, 7,000,000 for Miller and 5,500,000 for Finish."
            );
            println!("{f}: annotated");
        }
        std::fs::write(f, serde_json::to_string_pretty(&doc)? + "\n")?;
    }
    Ok(())
}

/// Each deployed program header (ids file `[programs]`) holds the image id of the release set.
async fn check_images(image_ids_file: &str) -> Result<()> {
    let text = std::fs::read_to_string(image_ids_file)?;
    let w = attest_host::open_wallet()?;
    let mut ok = true;
    for name in ["attest", "lc_verify", "lc_verify_v3", "consumer_erc20", "bridge"] {
        let Ok(header) = attest_host::deployed_program(name) else {
            continue;
        };
        let want = text
            .lines()
            .find(|l| l.starts_with(&format!("{name}.bin:")))
            .and_then(|l| l.rsplit("image id ").next())
            .map(str::trim)
            .ok_or_else(|| anyhow!("{name} not in {image_ids_file}"))?;
        let want_bytes = hex::decode(want)?;
        let loader = lee_core::program::PROGRAM_LOADER_ACCOUNT_ID;
        let view = w.get_account_view(Actor::new(header, loader)).await?;
        let state = view.map(|a| a.data.actor_state(loader).to_vec()).unwrap_or_default();
        let found = state.windows(32).any(|win| win == want_bytes.as_slice());
        println!(
            "{name}: header {header} image id {want}: {}",
            if found { "MATCH" } else { "NOT FOUND" }
        );
        ok &= found;
    }
    ensure!(ok, "an on-chain header does not hold the expected image id");
    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("run") => run(&args[2..]).await,
        Some("check") => check(&args[2..]).await,
        Some("refusals") => refusals(&args[2..]).await,
        Some("refusals-onchain") => refusals_onchain(&args[2..]).await,
        Some("annotate") => annotate(&args[2..]).await,
        Some("check-images") => check_images(args.get(2).map_or("", String::as_str)).await,
        _ => Err(anyhow!(
            "usage: v3_onchain run --network N --v3-elf P --attest-elf P --out F | v3_onchain check F..."
        )),
    };
    if let Err(e) = &r {
        eprintln!("FAIL: {e:#}");
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases of `refusals-onchain` without a chain: each refuses at its step with its code in the program logic
    /// (and with the code its differential case expects), and both controls apply.
    #[test]
    fn refusal_cases_refuse_natively_where_specified() {
        let (attest, v3) = (AccountId::new([0xA7; 32]), AccountId::new([0x33; 32]));
        let (a, b, diff) = bases(v3).expect("instances and differential cases");
        let (mut ca, mut cb) = (a.config.clone(), b.config.clone());
        ca.min_participants = 342;
        cb.min_participants = 342;
        let p = prepare_cases(attest, v3, (&ca, &cb), (&a.bootstrap, &b.bootstrap), &diff).expect("cases");
        assert_eq!(p.cases.len(), SPECS.len());
        let codes: std::collections::BTreeSet<&str> = p.cases.iter().map(|c| c.code.code()).collect();
        assert!(codes.len() >= 7, "distinct codes: {codes:?}");
        for step in STEPS {
            assert!(
                p.cases.iter().any(|c| STEPS[c.refused_at] == step),
                "no case refused at {step}"
            );
        }
        // Unmodified Helios's verify passes the zeroed finalized header and the empty finalized header with a junk
        // branch; the staged verifier refuses both at Begin.
        for k in ["A1", "B2"] {
            let c = p.cases.iter().find(|c| c.key == k).expect("case");
            assert_eq!(c.helios, "ok", "{k}");
            assert_eq!(c.v1, "E02", "{k}");
        }
        assert!(p.controls[1].source.starts_with("fabricated/participation342"));
    }
}
