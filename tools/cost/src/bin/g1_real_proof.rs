//! G1 real-proof leg (W6; PLAN.md section 1 G1, 3.4): on `seq-real` (RISC0_DEV_MODE unset), `Init` a mainnet
//! instance from the real bootstrap (public, no proof), then submit ONE update through Path A with a real proof: the
//! wallet proves `lc_verify` (succinct, through the external r0vm) and the sequencer verifies the receipt and applies
//! `Apply` publicly. Prints one line `G1REAL {json}` with the instance, tx ids, block id and transaction bytes.
//!
//! ```text
//! g1_real_proof [--step update[0]] [--instance BASE58]
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
    let step = arg("--step").unwrap_or_else(|| "update[0]".into());

    let ids = DeployedIds::from_env().context("deploy ids")?;
    let chain = e2e_chain::mainnet_chain(vec![ids.lc_verify])?;
    let idx = chain
        .updates
        .iter()
        .position(|(l, _)| *l == step)
        .ok_or_else(|| anyhow!("unknown step {step}"))?;
    let t0 = std::time::Instant::now();

    let (instance, init_tx) = match arg("--instance") {
        Some(s) => (AccountId::from_str(&s).map_err(|e| anyhow!("instance: {e:?}"))?, None),
        None => {
            if idx != 0 {
                bail!("{step} needs the earlier steps applied: pass --instance of an instance that has them");
            }
            let config = fresh_config(&ids, &chain.config).await?;
            let (instance, out) = attest_host::init(&ids, &config, &chain.bootstrap)
                .await
                .context("init")?;
            println!("init: instance {instance} tx {}", out.tx);
            (instance, Some(out.tx))
        }
    };
    let before = attest_host::read_head(&ids, &instance)
        .await?
        .ok_or_else(|| anyhow!("instance missing"))?;
    println!("head before: slot {} period {}", before.slot, before.period);

    let t = std::time::Instant::now();
    let out = attest_host::submit_update(&ids, &instance, &chain.updates[idx].1, SubmitMode::Private)
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
        "step": step, "instance": instance.to_string(), "init_tx": init_tx, "tx": out.tx, "block_id": block_id,
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
