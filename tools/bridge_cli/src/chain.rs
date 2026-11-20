//! Witness of guest `burn_chain` (guests/burn_chain, RISC Zero 3.1.0): burn `index` located in LEZ block bytes.
//!
//! Reads blocks `pinned ..= tip` from the sequencer (`getBlockRange`, 16 at a time, read only), checks each block's
//! hash and link with LEZ's own types, finds the burn transaction by its hash, and checks that its bytes carry this
//! burn's note (a public `Burn` root call, or a `PrivateToPublic` boundary `Append` from the bridge program). Writes
//! into OUT: `blocks/<id>.borsh` (served bytes), `witness.json` (mode 0600: holds the burn secret; delete it after
//! proving) and `chain.json` (no secret: ids, hashes and the prover command). Proving runs in `burn_chain_host`,
//! because the guest is on RISC Zero 3.1.0 and this CLI on 3.0.5 (as guest burn_proof and guests/burn_proof/r31).

use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};
use bridge_program::{B32, hash::burn_note, lez::BridgeMessage};
use common::{block::Block, transaction::LeeTransaction};
use lee_core::execution_state::BoundaryStep;
use serde_json::{Value, json};

use crate::{ConfigFile, hex0x, lezops};

/// Blocks per `getBlockRange` (the public read-only endpoint allows at most 16).
const RANGE: u64 = 16;

async fn rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let out: Value = http
        .post(url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(e) = out.get("error") {
        bail!("{method}: {e}");
    }
    out.get("result").cloned().context("no result")
}

/// The note the transaction's bytes carry for the bridge, if any.
fn notes_in(tx: &LeeTransaction, bridge: &attest_core::AccountId) -> Vec<B32> {
    let decode = |m: &[u8]| borsh::from_slice::<BridgeMessage>(m).ok();
    match tx {
        LeeTransaction::Public(p) => {
            let root = &p.message().execution.root;
            match decode(&root.message) {
                Some(BridgeMessage::Burn { note, .. }) if root.to.program_account_id == *bridge => vec![note],
                _ => vec![],
            }
        }
        LeeTransaction::PrivacyPreserving(pp) => pp
            .message()
            .execution
            .boundary
            .iter()
            .filter_map(|s| match s {
                BoundaryStep::PrivateToPublic(d)
                    if d.envelope.from.program_account_id == *bridge && d.envelope.to.program_account_id == *bridge =>
                {
                    match decode(&d.envelope.message) {
                        Some(BridgeMessage::Append { note, .. }) => Some(note),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect(),
    }
}

#[expect(clippy::too_many_arguments, reason = "one argument per CLI flag")]
pub async fn chain_witness(
    cf: &ConfigFile,
    key: &B32,
    index: u64,
    burn_tx: B32,
    pinned: u64,
    tip: Option<u64>,
    seq_rpc: &str,
    out: &Path,
) -> Result<()> {
    let w = lezops::burn_witness(cf, key, index).await?;
    let note = burn_note(&w.burn_secret, &w.recipient, w.fee);
    let http = reqwest::Client::new();
    let last = rpc(&http, seq_rpc, "getLastBlockId", json!([]))
        .await?
        .as_u64()
        .context("last block id")?;
    let tip = tip.unwrap_or(last);
    ensure!(pinned < tip && tip <= last, "need pinned < tip <= last block ({last})");

    let mut blocks: Vec<Block> = Vec::new();
    let mut lo = pinned;
    while lo <= tip {
        let hi = (lo + RANGE - 1).min(tip);
        let got: Vec<Block> = serde_json::from_value(rpc(&http, seq_rpc, "getBlockRange", json!([lo, hi])).await?)?;
        ensure!(
            got.len() as u64 == hi - lo + 1,
            "getBlockRange {lo}..{hi}: {} blocks",
            got.len()
        );
        blocks.extend(got);
        lo = hi + 1;
    }
    let mut burn_at = None;
    for (i, b) in blocks.iter().enumerate() {
        let id = pinned + i as u64;
        ensure!(b.header.block_id == id, "block {id}: id {}", b.header.block_id);
        ensure!(
            b.recompute_hash() == b.header.hash,
            "block {id}: stored hash differs from recomputed"
        );
        if i > 0 {
            ensure!(
                b.header.prev_block_hash == blocks[i - 1].header.hash,
                "block {id}: broken link"
            );
        }
        if let Some(t) = b.body.transactions.iter().position(|t| t.hash().0 == burn_tx) {
            burn_at = Some((id, t));
        }
    }
    let (burn_block, tx_index) = burn_at.context("burn transaction not in blocks pinned+1 ..= tip")?;
    ensure!(
        burn_block > pinned,
        "the burn is in the pinned block itself: pin an earlier block"
    );
    let block = &blocks[usize::try_from(burn_block - pinned)?];
    ensure!(
        !block.body.is_reverted(tx_index),
        "the block lists the burn transaction as reverted"
    );
    ensure!(
        notes_in(&block.body.transactions[tx_index], &cf.bridge_program).contains(&note),
        "the transaction does not carry burn {index}'s note"
    );

    std::fs::create_dir_all(out.join("blocks"))?;
    for b in &blocks {
        std::fs::write(
            out.join(format!("blocks/{}.borsh", b.header.block_id)),
            borsh::to_vec(b)?,
        )?;
    }
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let text = serde_json::to_string_pretty(&json!({
            "burn_secret": hex::encode(w.burn_secret), "recipient": hex::encode(w.recipient), "fee": w.fee.to_string(),
            "token": hex::encode(w.token), "denom": w.denom.to_string(), "chain_id": w.chain_id.to_string(),
            "vault": hex::encode(w.vault), "lez_program": hex::encode(w.lez_program),
            "lez_instance": hex::encode(w.lez_instance),
        }))?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(out.join("witness.json"))?;
        std::io::Write::write_all(&mut f, text.as_bytes())?;
    }
    let dir = out.join("blocks");
    let summary = json!({
        "burn_index": index, "burn_tx": hex0x(&burn_tx), "pinned": pinned, "tip": tip,
        "burn_block": burn_block, "tx": tx_index,
        "pinned_hash": hex0x(&blocks[0].header.hash.0), "tip_hash": hex0x(&blocks.last().expect("blocks").header.hash.0),
        "burn_block_hash": hex0x(&block.header.hash.0), "note": hex0x(&note),
        "witness_file": out.join("witness.json").display().to_string(),
        "note_on_witness": "witness.json holds the burn secret; delete it after proving",
        "prove": format!(
            "burn_chain_host prove --mode execute|groth16 --witness-json {} --dir {} --pinned {pinned} --tip {tip} \
             --burn-block {burn_block} --tx {tx_index} --out <dir>",
            out.join("witness.json").display(),
            dir.display()
        ),
    });
    let text = serde_json::to_string_pretty(&summary)?;
    std::fs::write(out.join("chain.json"), format!("{text}\n"))?;
    println!("{text}");
    Ok(())
}
