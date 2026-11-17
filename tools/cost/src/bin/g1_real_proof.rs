//! G1 real-proof leg: on `seq-real` (RISC0_DEV_MODE unset), `Init` a mainnet
//! instance from the real bootstrap (public, no proof), then submit ONE update through Path A with a real proof: the
//! wallet proves `lc_verify` (succinct, through the external r0vm) and the sequencer verifies the receipt and applies
//! `Apply` publicly. Prints one line `G1REAL {json}` with the instance, tx ids, block id and transaction bytes.
//!
//! ```text
//! g1_real_proof [--bootstrap bootstrap_15631808.ssz] [--step finality_update_15637644] [--verifier lc_verify|lc_verify_v2]
//!               [--instance BASE58 | --min-participants N] [--init-only]
//! ```
//! - `--step`: which fixture step to prove. Default `update[0]` (76.4M cycles, the cheapest step reachable from the
//!   bootstrap). `update[1]` (handoff, 88.2M) needs `update[0]` applied on the same instance first.
//! - `--instance`: reuse an instance (window 2 continues the instance of window 1); else a fresh config is chosen.
//!
//! Run it only through `scripts/gate/g1_real_proof.sh` (which wraps it in `tools/cost/prove_job.sh`).

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, InstanceConfig};
use attest_host::{DeployedIds, SubmitMode};
use serde_json::json;

async fn fresh_config(ids: &DeployedIds, base: &InstanceConfig) -> Result<InstanceConfig> {
    let mut c = base.clone();
    for k in 0..1000u16 {
        c.min_participants = 342 + k;
        if attest_host::read_head(ids, &attest_core::instance_id(&ids.attest, &c))
            .await?
            .is_none()
        {
            return Ok(c);
        }
    }
    bail!("no fresh config")
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0") {
        bail!("RISC0_DEV_MODE is set: this binary is for the real-proof run only");
    }
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let flag = |k: &str| args.iter().any(|a| a == k);
    // --check-images FILE: read each [programs] header under the program loader and check that its on-chain state
    // holds the image id listed for that program in FILE (a guests_release IMAGE_IDS file).
    if let Some(f) = arg("--check-images") {
        return check_images(&f).await;
    }
    let step = arg("--step").unwrap_or_else(|| "finality_update_15637644".into());
    let verifier_name = arg("--verifier").unwrap_or_else(|| "lc_verify".into());

    let ids = DeployedIds::from_env().context("deploy ids")?;
    let mut verifiers = vec![ids.lc_verify];
    if let Ok(v2) = attest_host::deployed_program("lc_verify_v2") {
        verifiers.push(v2);
    }
    let verifier = if verifier_name == "lc_verify" { ids.lc_verify } else { attest_host::deployed_program(&verifier_name)? };
    let mut chain = e2e_chain::mainnet_chain(verifiers.clone())?;
    // --bootstrap NAME: a mainnet fixture bootstrap (e.g. bootstrap_15631808.ssz, period 1908) instead of 15605760,
    // so a finality update of the same period is the first update.
    if let Some(b) = arg("--bootstrap") {
        let bootstrap = e2e_chain::fixture("mainnet", &b)?;
        chain.config = e2e_chain::config_from_beacon(
            1,
            &e2e_chain::fixture("mainnet", "genesis.json")?,
            &e2e_chain::fixture("mainnet", "fork_schedule.json")?,
            &bootstrap,
            verifiers.clone(),
            0,
        )?;
        chain.bootstrap = bootstrap;
    }
    let idx = chain.updates.iter().position(|(l, _)| *l == step).ok_or_else(|| anyhow!("unknown step {step}"))?;
    let t0 = std::time::Instant::now();

    let (instance, init_tx) = match arg("--instance") {
        Some(s) => (AccountId::from_str(&s).map_err(|e| anyhow!("instance: {e:?}"))?, None),
        None => {
            let mut config = fresh_config(&ids, &chain.config).await?;
            if let Some(m) = arg("--min-participants") {
                config.min_participants = m.parse()?;
            }
            let (instance, out) = attest_host::init(&ids, &config, &chain.bootstrap).await.context("init")?;
            println!("init: instance {instance} tx {} (checkpoint slot {}, min_participants {})", out.tx,
                config.checkpoint_slot, config.min_participants);
            println!("G1INIT {}", json!({"instance": instance.to_string(), "init_tx": out.tx,
                "checkpoint_slot": config.checkpoint_slot, "min_participants": config.min_participants,
                "verifiers": verifiers.iter().map(ToString::to_string).collect::<Vec<_>>()}));
            (instance, Some(out.tx))
        }
    };
    if flag("--init-only") {
        return Ok(());
    }
    let before = attest_host::read_head(&ids, &instance)
        .await?
        .ok_or_else(|| anyhow!("instance missing"))?;
    println!("head before: slot {} period {}", before.slot, before.period);

    let t = std::time::Instant::now();
    let out = attest_host::submit_update_via(&ids, verifier, &instance, &chain.updates[idx].1, SubmitMode::Private)
        .await
        .with_context(|| format!("submit {step}"))?;
    let prove_and_submit_s = t.elapsed().as_secs_f64();
    let after = attest_host::read_head(&ids, &instance)
        .await?
        .ok_or_else(|| anyhow!("instance missing"))?;

    let w = attest_host::open_wallet()?;
    let hash = common::HashType::from_str(&out.tx).map_err(|e| anyhow!("tx hash: {e:?}"))?;
    let block_id = attest_host::confirm(&w, hash, "g1").await?;
    let block = w
        .get_block(block_id)
        .await?
        .ok_or_else(|| anyhow!("block {block_id}"))?;
    let tx = block
        .body
        .transactions
        .iter()
        .find(|t| t.hash() == hash)
        .ok_or_else(|| anyhow!("tx not in block"))?;
    let tx_bytes = borsh::to_vec(tx)?.len();
    let witness_bytes = match tx {
        common::transaction::LeeTransaction::PrivacyPreserving(p) => Some(borsh::to_vec(&p.witness_set)?.len()),
        _ => None,
    };

    let doc = json!({
        "step": step, "verifier": verifier_name, "verifier_id": verifier.to_string(), "instance": instance.to_string(), "init_tx": init_tx, "tx": out.tx, "block_id": block_id,
        "accepted": after.store_commitment != before.store_commitment,
        "head_before": {"slot": before.slot, "period": before.period},
        "head_after": {"slot": after.slot, "period": after.period, "exec_number": after.exec_number},
        "tx_wire_bytes": tx_bytes, "witness_set_bytes": witness_bytes,
        "prove_and_submit_s": prove_and_submit_s, "total_s": t0.elapsed().as_secs_f64(),
    });
    println!("G1REAL {doc}");
    if after.store_commitment == before.store_commitment {
        bail!("update not applied");
    }
    Ok(())
}

async fn check_images(image_ids_file: &str) -> Result<()> {
    let text = std::fs::read_to_string(image_ids_file)?;
    let w = attest_host::open_wallet()?;
    let mut ok = true;
    for name in ["attest", "lc_verify", "lc_verify_v2"] {
        let Ok(header) = attest_host::deployed_program(name) else { continue };
        let want = text
            .lines()
            .find(|l| l.starts_with(&format!("{name}.bin:")))
            .and_then(|l| l.rsplit("image id ").next())
            .map(str::trim)
            .ok_or_else(|| anyhow!("{name} not in {image_ids_file}"))?;
        let want_bytes: Vec<u8> = (0..want.len()).step_by(2).map(|i| u8::from_str_radix(&want[i..i + 2], 16)).collect::<Result<_, _>>()?;
        let loader = lee_core::program::PROGRAM_LOADER_ACCOUNT_ID;
        let view = w.get_account_view(lee_core::account::Actor::new(header, loader)).await?;
        let state = view.map(|a| a.data.actor_state(loader).to_vec()).unwrap_or_default();
        let found = state.windows(32).any(|win| win == want_bytes.as_slice());
        println!("{name}: header {header} state {} bytes, image id {want}: {}", state.len(), if found { "MATCH" } else { "NOT FOUND" });
        ok &= found;
    }
    if !ok {
        bail!("an on-chain header does not hold the expected image id");
    }
    Ok(())
}
