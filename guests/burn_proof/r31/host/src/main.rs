//! Host prover for guest `burn_proof` on RISC Zero 3.1.0 (GHSA-g6qg-9j8p-7j87). Proves through the r0vm 3.1.0 server
//! (`RISC0_SERVER_PATH`). Input: the borsh `BurnWitness` file that `bridge_cli prove --mode witness` writes. Output:
//! `journal.hex`, `seal.hex` (risc0-ethereum encoding: verifier selector || seal) and `meta.json`. The witness holds a
//! secret: it is read, never copied.
//!
//!   burn_prover_r31 image-id
//!   burn_prover_r31 prove --witness FILE --out DIR --mode execute|groth16

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use burn_methods_r31::{BURN_PROOF_ELF, BURN_PROOF_ID};
use risc0_zkvm::{Digest, ExecutorEnv, ProverOpts, ReceiptClaim, default_executor, default_prover, sha::Digestible as _};
use serde_json::json;

fn hex0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let image_id = Digest::from(BURN_PROOF_ID);
    match args.get(1).map(String::as_str) {
        Some("image-id") => {
            println!("{}", hex0x(image_id.as_bytes()));
            Ok(())
        }
        Some("prove") => {
            let get = |k: &str| -> Option<String> {
                args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned()
            };
            let witness = std::fs::read(get("--witness").context("--witness FILE")?)?;
            let out = PathBuf::from(get("--out").context("--out DIR")?);
            let mode = get("--mode").unwrap_or_else(|| "execute".into());
            std::fs::create_dir_all(&out)?;
            let env = ExecutorEnv::builder().write(&witness)?.build()?;
            let started = std::time::Instant::now();
            let mut meta = json!({"risc0_zkvm": "3.1.0", "image_id": hex0x(image_id.as_bytes()), "mode": mode});
            let (journal, seal) = match mode.as_str() {
                "execute" => {
                    let session = default_executor().execute(env, BURN_PROOF_ELF)?;
                    meta["cycles_user"] = json!(session.cycles());
                    meta["segments"] = json!(session.segments.len());
                    let journal = session.journal.bytes;
                    // Seal for RiscZeroMockVerifier(0x00000000): selector || claim digest (anvil only).
                    let claim = ReceiptClaim::ok(image_id, journal.clone()).digest();
                    let mut seal = vec![0_u8; 4];
                    seal.extend_from_slice(claim.as_bytes());
                    meta["seal"] = json!("mock (RiscZeroMockVerifier selector 0x00000000), not a proof");
                    (journal, seal)
                }
                "groth16" => {
                    if std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v != "0" && !v.is_empty()) {
                        bail!("groth16 needs RISC0_DEV_MODE unset (and a PLAN.md 3.4 proving window)");
                    }
                    let info = default_prover().prove_with_opts(env, BURN_PROOF_ELF, &ProverOpts::groth16())?;
                    info.receipt.verify(image_id)?;
                    let g = info.receipt.inner.groth16()?;
                    let mut seal = g.verifier_parameters.as_bytes()[..4].to_vec();
                    seal.extend_from_slice(&g.seal);
                    meta["selector"] = json!(hex0x(&seal[..4]));
                    meta["seal_bytes"] = json!(seal.len());
                    meta["cycles_user"] = json!(info.stats.user_cycles);
                    meta["cycles_total"] = json!(info.stats.total_cycles);
                    meta["segments"] = json!(info.stats.segments);
                    (info.receipt.journal.bytes, seal)
                }
                other => bail!("--mode execute|groth16, not {other}"),
            };
            meta["seconds"] = json!(started.elapsed().as_secs_f64());
            std::fs::write(out.join("journal.hex"), hex0x(&journal))?;
            std::fs::write(out.join("seal.hex"), hex0x(&seal))?;
            let text = serde_json::to_string_pretty(&meta)? + "\n";
            std::fs::write(out.join("meta.json"), &text)?;
            print!("{text}");
            Ok(())
        }
        _ => bail!("usage: burn_prover_r31 image-id | prove --witness FILE --out DIR --mode execute|groth16"),
    }
}
