//! Runs the measurement guest in the RISC Zero 3.1.0 executor (r0vm over IPC, `RISC0_SERVER_PATH`). No proof.
//!
//!   burn_chain_bench_host image-id
//!   burn_chain_bench_host execute --dir BLOCK_DIR --pinned P --tip T --burn-block K --tx I \
//!       --bridge HEX32 --instance HEX32 --note HEX32
//!
//! BLOCK_DIR holds `<id>.borsh` served block bytes. The pinned hash is recomputed from block P's bytes. The same
//! checks run natively first; the guest's journal must equal the native result. Prints one JSON object. A zero
//! `--note` skips the burn finder (measures the hash chain and the parse of block K only).

use anyhow::{Context as _, Result, bail, ensure};
use borsh::{BorshDeserialize, BorshSerialize};
use burn_chain_bench_methods::{BURN_CHAIN_BENCH_ELF, BURN_CHAIN_BENCH_ID};
use lez_chain::{AccountId, Tip};
use risc0_zkvm::{Digest, ExecutorEnv, default_executor};
use serde_json::json;

#[derive(BorshSerialize)]
struct Input {
    pinned_id: u64,
    pinned_hash: [u8; 32],
    blocks: Vec<Vec<u8>>,
    burn_pos: u32,
    tx_index: u32,
    bridge: [u8; 32],
    instance: [u8; 32],
    note: [u8; 32],
}

#[derive(BorshDeserialize, Debug)]
struct Output {
    tip_id: u64,
    tip_hash: [u8; 32],
    burn_block_id: u64,
    marks: [u64; 4],
}

fn h32(s: &str) -> Result<[u8; 32]> {
    hex::decode(s.trim_start_matches("0x"))?
        .try_into()
        .ok()
        .context("32 bytes")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    match args.get(1).map(String::as_str) {
        Some("image-id") => println!("0x{}", hex::encode(Digest::from(BURN_CHAIN_BENCH_ID).as_bytes())),
        Some("execute") => {
            let need = |k: &str| get(k).with_context(|| format!("missing {k}"));
            let dir = std::path::PathBuf::from(need("--dir")?);
            let (p, t, k): (u64, u64, u64) = (
                need("--pinned")?.parse()?,
                need("--tip")?.parse()?,
                need("--burn-block")?.parse()?,
            );
            ensure!(p < k && k <= t, "need pinned < burn block <= tip");
            let read = |i: u64| std::fs::read(dir.join(format!("{i}.borsh"))).with_context(|| format!("block {i}"));
            let pinned_hash = lez_chain::block_hash(&read(p)?).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let blocks: Vec<Vec<u8>> = (p + 1..=t).map(read).collect::<Result<_>>()?;
            let input = Input {
                pinned_id: p,
                pinned_hash,
                burn_pos: u32::try_from(k - p - 1)?,
                tx_index: need("--tx")?.parse()?,
                bridge: h32(&need("--bridge")?)?,
                instance: h32(&need("--instance")?)?,
                note: h32(&need("--note")?)?,
                blocks,
            };
            // Native run of the same checks.
            let tip = lez_chain::verify_chain(
                Tip {
                    block_id: p,
                    hash: pinned_hash,
                },
                input.blocks.iter().map(Vec::as_slice),
            )
            .map_err(|e| anyhow::anyhow!("native chain: {e:?}"))?;
            let block =
                lez_chain::parse_block(&input.blocks[input.burn_pos as usize]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            // A zero note: no finder (parse cost of a block without a burn).
            let burn = if input.note == [0; 32] {
                None
            } else {
                Some(
                    lez_chain::find_burn(
                        &block,
                        input.tx_index as usize,
                        &AccountId(input.bridge),
                        &AccountId(input.instance),
                        &input.note,
                    )
                    .map_err(|e| anyhow::anyhow!("native find_burn: {e:?}"))?,
                )
            };

            let bytes = borsh::to_vec(&input)?;
            let env = ExecutorEnv::builder().write_slice(&bytes).build()?;
            let start = std::time::Instant::now();
            let session = default_executor().execute(env, BURN_CHAIN_BENCH_ELF)?;
            let secs = start.elapsed().as_secs_f64();
            let out = Output::try_from_slice(&session.journal.bytes)?;
            if out.tip_id != tip.block_id || out.tip_hash != tip.hash || out.burn_block_id != k {
                bail!("journal {out:?} differs from native tip {tip:?}");
            }
            let block_bytes: Vec<usize> = input.blocks.iter().map(Vec::len).collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "image_id": format!("0x{}", hex::encode(Digest::from(BURN_CHAIN_BENCH_ID).as_bytes())),
                    "pinned": p, "tip": t, "burn_block": k, "tx": input.tx_index, "blocks_hashed": block_bytes.len(),
                    "block_bytes": block_bytes, "input_bytes": bytes.len(),
                    "burn": format!("{burn:?}"),
                    "tip_hash": hex::encode(tip.hash),
                    "cycles_user_total": session.cycles(),
                    "segments": session.segments.len(),
                    "marks": {"after_input": out.marks[0], "after_chain": out.marks[1], "after_parse": out.marks[2], "after_find": out.marks[3]},
                    "cycles_chain_check": out.marks[1] - out.marks[0],
                    "cycles_parse_burn_block": out.marks[2] - out.marks[1],
                    "cycles_find_burn": out.marks[3] - out.marks[2],
                    "executor_seconds": secs,
                    "mode": "executor only (no proof)",
                }))?
            );
        }
        _ => bail!(
            "usage: burn_chain_bench_host image-id | execute --dir D --pinned P --tip T --burn-block K --tx I --bridge H --instance H --note H"
        ),
    }
    Ok(())
}
