//! `v3_onchain`: the staged public verifier (`lc_verify_v3`) as real public transactions on a running sequencer, and
//! the read-back check of every recorded transaction.
//!
//! ```text
//! v3_onchain run --network mainnet|sepolia|floor --v3-elf P --attest-elf P --out run.json
//! v3_onchain check run.json [run.json ...]
//! v3_onchain check-images <release IMAGE_IDS>
//! BV_ALLOW_REVERT=1 v3_onchain refusals --out refusals.json     # refused steps as real transactions
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
                    root = Some((ex.root.to, ex.root.message.clone()));
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
    let (to, message) = root.ok_or_else(|| anyhow!("tx {hash} is not a public transaction"))?;
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

fn net_updates(network: &str, v3: AccountId) -> Result<NetUpdates> {
    match network {
        "mainnet" | "sepolia" => {
            let c = if network == "mainnet" {
                e2e_chain::mainnet_chain(vec![v3])?
            } else {
                e2e_chain::sepolia_chain(vec![v3])?
            };
            Ok((c.config, c.bootstrap, c.updates))
        }
        "floor" => {
            use fixtures::fabricate::{FabChain, FabParams, bits_first};
            let real = e2e_chain::mainnet_chain(vec![v3])?;
            let chain = FabChain::build(FabParams::impersonating(&real.config, b"beacon-veil/v3/floor"));
            let mut u0 = chain.updates[0].clone();
            u0.bits = bits_first(342);
            chain.resign(&mut u0);
            Ok((
                chain.config(vec![v3], 0),
                chain.bootstrap.ssz(),
                vec![("period_update_342".into(), u0.ssz())],
            ))
        }
        _ => bail!("network: mainnet | sepolia | floor"),
    }
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
        ensure!(head == native.head, "{label}: on-chain head differs from V1");
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
            "head_equals_v1": true,
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

/// The `LightClientFinalityUpdate` inside an Electra `LightClientUpdate` (the update without its next committee and
/// branch; the two offsets move by the cut).
fn finality_part_electra(u: &[u8]) -> Result<Vec<u8>> {
    let cut = lc::COMMITTEE_SSZ_LEN + 6 * 32;
    ensure!(
        u.len() > 4 + cut && u32::from_le_bytes(u[..4].try_into()?) as usize == 4 + cut + 4 + 7 * 32 + 168,
        "not an Electra update"
    );
    let mut f = Vec::with_capacity(u.len() - cut);
    f.extend_from_slice(&u[..4]);
    f.extend_from_slice(&u[4 + cut..]);
    for at in [0usize, 4] {
        let o = u32::from_le_bytes(f[at..at + 4].try_into()?) - u32::try_from(cut)?;
        f[at..at + 4].copy_from_slice(&o.to_le_bytes());
    }
    Ok(f)
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
    let chain = e2e_chain::mainnet_chain(vec![v3])?;
    let config = fresh_config(&ids, &chain.config).await?;
    let (instance, _) = attest_host::init(&ids, &config, &chain.bootstrap).await?;
    let mut witness = lc::init_store(&config, &chain.bootstrap).map_err(|e| anyhow!("{e}"))?.0;
    let mut rows = Vec::new();
    let head_now = |ids: &DeployedIds| {
        let ids = ids.clone();
        async move { attest_host::read_head(&ids, &instance).await }
    };
    let job_of = |w: &StoreWitness, u: &[u8]| -> Result<JobInputs> {
        let n = lc::verify_and_apply(&config, w, u).map_err(|e| anyhow!("{e}"))?;
        let w3 = lc_v3::StoreWitnessV3::from_v1(w, n.signature_slot);
        let keys = lc_v3::absent_keys(&w3.committee_ssz, u);
        let seed = lc_v3::job_seed(&instance, &n.pre_store, u);
        Ok((seed, lc_v3::job_id(&v3, &seed), n.head.slot, w3, keys))
    };
    let begin = |w3: &lc_v3::StoreWitnessV3, u: &[u8], keys: &[[u8; 96]]| LcVerifyV3Message::Begin {
        attest_program: ids.attest,
        instance,
        config: config.clone(),
        witness: w3.clone(),
        update_ssz: u.to_vec(),
        absent_keys: keys.to_vec(),
    };
    let mut record = |case: &str,
                      want: &str,
                      hash: &str,
                      reverted: bool,
                      text: &str,
                      t: &TxInBlock,
                      before: &attest_core::Head,
                      after: &attest_core::Head| {
        println!(
            "{case}: tx {hash} block {} reverted {reverted} charged {:?} (charged txs {})",
            t.block, t.block_gas_exec, t.charged_txs
        );
        rows.push(json!({
            "case": case, "expected": want, "tx": hash, "block": t.block, "reverted": reverted,
            "refusal_text": text.lines().last().unwrap_or_default(),
            "block_gas_used_exec": t.block_gas_exec, "charged_txs_in_block": t.charged_txs,
            "charged_gas": if t.charged_txs == 1 { json!(t.block_gas_exec) } else { Value::Null },
            "head_unchanged": before == after, "head_slot": after.slot,
        }));
    };

    // update[0] through all three steps (the job is then done).
    let u0 = chain.updates[0].1.clone();
    let (seed0, _, slot0, w3, keys) = job_of(&witness, &u0)?;
    attest_host::submit_update_staged(&ids, v3, &instance, &u0).await?;
    witness = lc::verify_and_apply(&config, &witness, &u0)
        .map_err(|e| anyhow!("{e}"))?
        .new_witness();
    // 1. Begin again on the done job: E14 (exit code).
    let before = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seed0,
        &instance,
        slot0,
        &begin(&w3, &u0, &keys),
        "duplicate begin",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    let after = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    record(
        "duplicate Begin on a done job",
        "E14 (exit code 14)",
        &h,
        rev,
        &text,
        &t,
        &before,
        &after,
    );
    // 2. Miller on the done job: E14.
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seed0,
        &instance,
        slot0,
        &LcVerifyV3Message::Miller,
        "miller on done job",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    let after2 = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    record(
        "Miller on a done job",
        "E14 (exit code 14)",
        &h,
        rev,
        &text,
        &t,
        &after,
        &after2,
    );

    // 3. Finish before Miller on update[1]'s job: E01; then the job completes normally.
    let u1 = chain.updates[1].1.clone();
    let (seed1, _, slot1, w31, keys1) = job_of(&witness, &u1)?;
    let (hb, revb, _) = send_step(&ids, v3, seed1, &instance, slot1, &begin(&w31, &u1, &keys1), "begin u1").await?;
    ensure!(!revb, "Begin of update[1] reverted ({hb})");
    let before = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
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
    let after = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    record(
        "Finish before Miller",
        "E01 (exit code 1)",
        &h,
        rev,
        &text,
        &t,
        &before,
        &after,
    );
    attest_host::submit_update_staged(&ids, v3, &instance, &u1).await?;
    witness = lc::verify_and_apply(&config, &witness, &u1)
        .map_err(|e| anyhow!("{e}"))?
        .new_witness();

    // 4. Forged signature (update[2] carrying update[3]'s signature): Begin and Miller land, Finish refuses E03.
    let u2 = chain.updates[2].1.clone();
    let sig_of = |u: &[u8]| -> Result<Vec<u8>> {
        use ssz::Encode;
        let f = lc::forks(&config).map_err(|e| anyhow!("{e}"))?;
        Ok(match lc::decode_update(u, &f).map_err(|e| anyhow!("{e}"))? {
            lc::AnyUpdate::Full(x) => x.sync_aggregate().sync_committee_signature.as_ssz_bytes(),
            lc::AnyUpdate::Finality(x) => x.sync_aggregate().sync_committee_signature.as_ssz_bytes(),
        })
    };
    let own = sig_of(&u2)?;
    let at = u2
        .windows(96)
        .position(|x| x == own.as_slice())
        .ok_or_else(|| anyhow!("signature bytes"))?;
    let mut forged = u2.clone();
    forged[at..at + 96].copy_from_slice(&sig_of(&chain.updates[3].1)?);
    ensure!(lc::verify_and_apply(&config, &witness, &forged).err() == Some(attest_core::EsaError::E03BadSignature));
    let w3f = lc_v3::StoreWitnessV3::from_v1(
        &witness,
        lc::verify_and_apply(&config, &witness, &u2)
            .map_err(|e| anyhow!("{e}"))?
            .signature_slot,
    );
    let keysf = lc_v3::absent_keys(&w3f.committee_ssz, &forged);
    let pre = w3f.commitment().map_err(|e| anyhow!("{e}"))?;
    let seedf = lc_v3::job_seed(&instance, &pre, &forged);
    let slotf = lc::verify_and_apply(&config, &witness, &u2)
        .map_err(|e| anyhow!("{e}"))?
        .head
        .slot;
    let (hb, revb, _) = send_step(
        &ids,
        v3,
        seedf,
        &instance,
        slotf,
        &begin(&w3f, &forged, &keysf),
        "forged begin",
    )
    .await?;
    let (hm, revm, _) = send_step(
        &ids,
        v3,
        seedf,
        &instance,
        slotf,
        &LcVerifyV3Message::Miller,
        "forged miller",
    )
    .await?;
    ensure!(!revb && !revm, "forged Begin/Miller reverted ({hb}, {hm})");
    let before = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    let (h, rev, text) = send_step(
        &ids,
        v3,
        seedf,
        &instance,
        slotf,
        &LcVerifyV3Message::Finish,
        "forged finish",
    )
    .await?;
    let t = tx_in_block(&w, &h).await?;
    let after = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    record(
        "forged signature, refused at Finish",
        "E03 (exit code 3)",
        &h,
        rev,
        &text,
        &t,
        &before,
        &after,
    );

    // 5. Stale job: update[2]'s job runs Begin and Miller on store S; then the finality part of update[2] (a different
    //    update) moves the store; the stale job's Finish sends an Apply that attest refuses (E13, panic).
    let (seeds, _, slots, w3s, keyss) = job_of(&witness, &u2)?;
    let (hb, revb, _) = send_step(
        &ids,
        v3,
        seeds,
        &instance,
        slots,
        &begin(&w3s, &u2, &keyss),
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
    let fin = finality_part_electra(&u2)?;
    let moved = lc::verify_and_apply(&config, &witness, &fin).map_err(|e| anyhow!("finality part: {e}"))?;
    attest_host::submit_update_staged(&ids, v3, &instance, &fin).await?;
    witness = moved.new_witness();
    let before = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
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
    let after = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    record(
        "job whose store has moved (stale), refused by attest.Apply",
        "E13 (attest panic ESA:E13)",
        &h,
        rev,
        &text,
        &t,
        &before,
        &after,
    );

    // 6. A Sepolia update sent to this mainnet instance: refused at Begin.
    let sep = e2e_chain::sepolia_chain(vec![v3])?;
    let us = sep.updates[4].1.clone();
    let native = lc::verify_and_apply(&config, &witness, &us).err();
    let w3x = lc_v3::StoreWitnessV3::from_v1(&witness, 0);
    let prex = w3x.commitment().map_err(|e| anyhow!("{e}"))?;
    let seedx = lc_v3::job_seed(&instance, &prex, &us);
    let before = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    let (h, rev, text) = send_step(&ids, v3, seedx, &instance, 0, &begin(&w3x, &us, &[]), "other network").await?;
    let t = tx_in_block(&w, &h).await?;
    let after = head_now(&ids).await?.ok_or_else(|| anyhow!("head"))?;
    record(
        "Sepolia update sent to a mainnet instance",
        &format!("V1 native: {native:?}"),
        &h,
        rev,
        &text,
        &t,
        &before,
        &after,
    );

    let doc = json!({
        "tool": "tools/cost v3_onchain refusals", "sequencer": "seq-pub", "instance": instance.to_string(),
        "programs": {"attest": ids.attest.to_string(), "lc_verify_v3": v3.to_string()},
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
