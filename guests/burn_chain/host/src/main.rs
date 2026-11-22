//! Host of guest `burn_chain` (RISC Zero 3.1.0, r0vm over IPC, `RISC0_SERVER_PATH`).
//!
//!   burn_chain_host image-id
//!   burn_chain_host prove --mode execute|groth16 --witness-json FILE --dir BLOCK_DIR [--pinned-dir DIR] \
//!       --pinned P --tip T --burn-block K --tx I --out OUT_DIR [--segment-po2 N]
//!
//! `--witness-json`: `{burn_secret, recipient, fee, token, denom, chain_id, vault, lez_program, lez_instance}` (hex
//! strings, decimal strings for fee, denom and chain_id). BLOCK_DIR holds `<id>.borsh` served block bytes for P+1..T;
//! the pinned hash is recomputed from block P's bytes in `--pinned-dir` (default BLOCK_DIR). The statement runs
//! natively first; the guest's journal must equal it. `--mode execute` runs the executor only (no proof) and writes a
//! mock seal for `RiscZeroMockVerifier(0x00000000)` (anvil only); `--mode groth16` proves for real (proving windows
//! only, `RISC0_DEV_MODE` unset). OUT_DIR gets `journal.hex`, `seal.hex` (verifier selector || seal) and `meta.json`
//! (image id, cycles, segments, sizes). `--segment-po2 N` sets the executor's segment limit (default 20): a smaller
//! segment lowers the prover's peak memory, adds segments and leaves the image id, journal and seal format unchanged.
//! The witness file holds the burn secret: keep it out of the repository unless
//! it is a test secret, and delete it after proving.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use burn_chain_methods::{BURN_CHAIN_ELF, BURN_CHAIN_ID};
use lez_chain::statement::{ChainJournal, ChainWitness, prove_burn_chain};
use risc0_zkvm::{
    Digest, ExecutorEnv, ProverOpts, ReceiptClaim, default_executor, default_prover, sha::Digestible as _,
};
use serde_json::{Value, json};

fn hexn<const N: usize>(v: &Value, k: &str) -> Result<[u8; N]> {
    let s = v[k].as_str().with_context(|| format!("witness field {k}"))?;
    hex::decode(s.trim_start_matches("0x"))?
        .try_into()
        .map_err(|_| anyhow!("{k}: {N} bytes"))
}

fn num<T: std::str::FromStr>(v: &Value, k: &str) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Ok(v[k].as_str().with_context(|| format!("witness field {k}"))?.parse()?)
}

fn image_id() -> String {
    format!("0x{}", hex::encode(Digest::from(BURN_CHAIN_ID).as_bytes()))
}

fn read_block(dir: &Path, id: u64) -> Result<Vec<u8>> {
    std::fs::read(dir.join(format!("{id}.borsh"))).with_context(|| format!("block {id}"))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let need = |k: &str| get(k).with_context(|| format!("missing {k}"));
    match args.get(1).map(String::as_str) {
        Some("image-id") => println!("{}", image_id()),
        Some("prove") => {
            let mode = get("--mode").unwrap_or_else(|| "execute".into());
            let wj: Value = serde_json::from_slice(&std::fs::read(need("--witness-json")?)?)?;
            let dir = PathBuf::from(need("--dir")?);
            let pinned_dir = get("--pinned-dir").map_or_else(|| dir.clone(), PathBuf::from);
            let out = PathBuf::from(need("--out")?);
            let (p, t, k): (u64, u64, u64) = (
                need("--pinned")?.parse()?,
                need("--tip")?.parse()?,
                need("--burn-block")?.parse()?,
            );
            ensure!(p < k && k <= t, "need pinned < burn block <= tip");
            let pinned_hash =
                lez_chain::block_hash(&read_block(&pinned_dir, p)?).map_err(|e| anyhow!("block {p}: {e:?}"))?;
            let witness = ChainWitness {
                burn_secret: hexn(&wj, "burn_secret")?,
                recipient: hexn(&wj, "recipient")?,
                fee: num(&wj, "fee")?,
                token: hexn(&wj, "token")?,
                denom: num(&wj, "denom")?,
                chain_id: num(&wj, "chain_id")?,
                vault: hexn(&wj, "vault")?,
                lez_program: hexn(&wj, "lez_program")?,
                lez_instance: hexn(&wj, "lez_instance")?,
                pinned_id: p,
                pinned_hash,
                blocks: (p + 1..=t).map(|i| read_block(&dir, i)).collect::<Result<_>>()?,
                burn_block_id: k,
                burn_tx: need("--tx")?.parse()?,
            };
            let native = prove_burn_chain(&witness).map_err(|e| anyhow!("native statement refused: {e:?}"))?;
            let input = borsh::to_vec(&witness)?;
            let po2: Option<u32> = get("--segment-po2").map(|s| s.parse()).transpose()?;
            let mut builder = ExecutorEnv::builder();
            builder.write_slice(&input);
            if let Some(n) = po2 {
                builder.segment_limit_po2(n);
            }
            let env = builder.build()?;
            let start = std::time::Instant::now();
            let image = Digest::from(BURN_CHAIN_ID);
            let (journal, seal, cycles, segments, seal_note) = match mode.as_str() {
                "execute" => {
                    let session = default_executor().execute(env, BURN_CHAIN_ELF)?;
                    let journal = session.journal.bytes.clone();
                    let mut seal = vec![0_u8; 4];
                    seal.extend_from_slice(ReceiptClaim::ok(image, journal.clone()).digest().as_bytes());
                    let note = "mock (RiscZeroMockVerifier selector 0x00000000), not a proof";
                    (journal, seal, session.cycles(), session.segments.len(), note)
                }
                "groth16" => {
                    if std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v != "0" && !v.is_empty()) {
                        bail!("groth16 needs RISC0_DEV_MODE unset (and a proving window)");
                    }
                    let info = default_prover().prove_with_opts(env, BURN_CHAIN_ELF, &ProverOpts::groth16())?;
                    info.receipt.verify(image)?;
                    let g = info.receipt.inner.groth16()?;
                    let mut seal = g.verifier_parameters.as_bytes()[..4].to_vec();
                    seal.extend_from_slice(&g.seal);
                    let stats = (info.stats.user_cycles, usize::try_from(info.stats.segments)?);
                    (
                        info.receipt.journal.bytes,
                        seal,
                        stats.0,
                        stats.1,
                        "groth16 (verifier selector || seal)",
                    )
                }
                other => bail!("--mode execute|groth16, not {other}"),
            };
            let secs = start.elapsed().as_secs_f64();
            if journal != native.encode() {
                bail!("guest journal differs from the native statement");
            }
            ensure!(
                ChainJournal::decode(&journal).as_ref() == Some(&native),
                "journal does not decode"
            );
            std::fs::create_dir_all(&out)?;
            std::fs::write(out.join("journal.hex"), format!("0x{}\n", hex::encode(&journal)))?;
            std::fs::write(out.join("seal.hex"), format!("0x{}\n", hex::encode(&seal)))?;
            let meta = json!({
                "guest": "burn_chain", "image_id": image_id(), "risc0": "3.1.0", "mode": mode, "seal": seal_note,
                "pinned": p, "tip": t, "burn_block": k, "tx": witness.burn_tx,
                "blocks": witness.blocks.len(),
                "block_bytes": witness.blocks.iter().map(Vec::len).collect::<Vec<_>>(),
                "input_bytes": input.len(), "journal_bytes": journal.len(), "journal_equals_native": true,
                "tip_hash": hex::encode(native.tip_hash), "nullifier": hex::encode(native.nullifier),
                "cycles_user": cycles, "segments": segments, "segment_po2": po2.unwrap_or(20), "seconds": secs,
            });
            let text = serde_json::to_string_pretty(&meta)?;
            std::fs::write(out.join("meta.json"), format!("{text}\n"))?;
            println!("{text}");
        }
        _ => bail!(
            "usage: burn_chain_host image-id | prove --mode execute|groth16 --witness-json F --dir D [--pinned-dir D] \
             --pinned P --tip T --burn-block K --tx I --out O [--segment-po2 N]"
        ),
    }
    Ok(())
}
