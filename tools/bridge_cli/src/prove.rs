//! Guest burn_proof: native check, executor run (dev mode, cycles) or a real Groth16 proof (proving windows only,
//! PLAN.md 3.4). Writes `journal.hex`, `seal.hex` and `meta.json` into the output directory; never the witness.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use bridge_program::{
    hash::{burn_leaf, burn_note, burn_nullifier, commitment, dest_binding, leaf, sha},
    journal::{BurnWitness, prove_burn},
    tree::{IncrementalTree, path_for},
};
use risc0_zkvm::{ExecutorEnv, ProverOpts, ReceiptClaim, default_executor, default_prover, sha::Digestible as _};
use serde_json::json;

use crate::hex0x;

pub fn run(witness: &BurnWitness, out: &Path, mode: &str) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let journal = prove_burn(witness).encode();
    std::fs::write(out.join("journal.hex"), hex0x(&journal))?;
    let mut meta = json!({
        "mode": mode,
        "journal_sha256": hex0x(&sha(&[&journal])),
        "recipient": hex0x(&witness.recipient),
        "token": hex0x(&witness.token),
        "denom": witness.denom.to_string(),
        "fee": witness.fee.to_string(),
        "leaf_index": witness.leaf_index,
        "nullifier": hex0x(&burn_nullifier(&witness.burn_secret)),
    });
    if mode == "open" {
        // Failsafe L2: arguments of Vault.releaseOpen. They reveal the burn secret (the user's own link): opt-in only.
        use std::os::unix::fs::OpenOptionsExt as _;
        let p = out.join("open.env");
        let path = witness.path.iter().map(|h| hex0x(h)).collect::<Vec<_>>().join(",");
        let text = format!(
            "BURN_SECRET={}\nRECIPIENT={}\nFEE={}\nTOKEN={}\nDENOM={}\nLEAF_INDEX={}\nBURN_PATH={}\n",
            hex0x(&witness.burn_secret),
            hex0x(&witness.recipient),
            witness.fee,
            hex0x(&witness.token),
            witness.denom,
            witness.leaf_index,
            path
        );
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&p)?;
        std::io::Write::write_all(&mut f, text.as_bytes())?;
        meta["open_env"] = json!(p.display().to_string());
        meta["note"] = json!("open.env reveals the burn secret: use only for releaseOpen, by explicit choice");
    } else if mode == "witness" {
        // For the separate RISC Zero 3.1.0 prover (guests/burn_proof/r31). Holds the burn secret: owner-only file.
        use std::os::unix::fs::OpenOptionsExt as _;
        let p = out.join("witness.borsh");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&p)?;
        std::io::Write::write_all(&mut f, &borsh::to_vec(witness)?)?;
        meta["witness_file"] = json!(p.display().to_string());
        meta["note"] = json!("witness.borsh holds the burn secret; delete it after proving");
    } else if mode != "none" {
        let elf = std::fs::read(attest_host::guests_dir().join("burn_proof.bin")).context("burn_proof.bin")?;
        let image_id = risc0_zkvm::compute_image_id(&elf)?;
        meta["image_id"] = json!(hex0x(image_id.as_bytes()));
        let input = borsh::to_vec(witness)?;
        let env = ExecutorEnv::builder().write(&input)?.build()?;
        let started = std::time::Instant::now();
        match mode {
            "execute" => {
                let session = default_executor().execute(env, &elf)?;
                if session.journal.bytes != journal {
                    bail!("executor journal differs from the native statement");
                }
                meta["cycles_user"] = json!(session.cycles());
                meta["segments"] = json!(session.segments.len());
                // Seal for RiscZeroMockVerifier(0x00000000): selector || claim digest (anvil only).
                let claim = ReceiptClaim::ok(image_id, journal.to_vec()).digest();
                let mut seal = vec![0_u8; 4];
                seal.extend_from_slice(claim.as_bytes());
                std::fs::write(out.join("seal.hex"), hex0x(&seal))?;
                meta["seal"] = json!("mock (RiscZeroMockVerifier selector 0x00000000), not a proof");
            }
            "groth16" => {
                if std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v != "0" && !v.is_empty()) {
                    bail!("groth16 needs RISC0_DEV_MODE unset (and a PLAN.md 3.4 proving window)");
                }
                let info = default_prover().prove_with_opts(env, &elf, &ProverOpts::groth16())?;
                info.receipt.verify(image_id)?;
                if info.receipt.journal.bytes != journal {
                    bail!("receipt journal differs from the native statement");
                }
                let g = info.receipt.inner.groth16()?;
                // risc0-ethereum encode_seal: verifier selector (first 4 bytes of the parameters digest) || seal.
                let mut seal = g.verifier_parameters.as_bytes()[..4].to_vec();
                seal.extend_from_slice(&g.seal);
                std::fs::write(out.join("seal.hex"), hex0x(&seal))?;
                meta["selector"] = json!(hex0x(&seal[..4]));
                meta["seal_bytes"] = json!(seal.len());
                meta["cycles_user"] = json!(info.stats.user_cycles);
                meta["cycles_total"] = json!(info.stats.total_cycles);
                meta["segments"] = json!(info.stats.segments);
            }
            other => bail!("--mode none|open|witness|execute|groth16, not {other}"),
        }
        meta["seconds"] = json!(started.elapsed().as_secs_f64());
    }
    let text = serde_json::to_string_pretty(&meta)? + "\n";
    std::fs::write(out.join("meta.json"), &text)?;
    print!("{text}");
    Ok(())
}

/// Vectors that `contracts/test/BridgeVectors.t.sol` checks against Solidity: leaf hash, tree roots, journal bytes.
pub fn vectors(out: &Path) -> Result<()> {
    let token: bridge_program::Addr = [0x11; 20];
    let denom = 10_u128.pow(16);
    let c = commitment(&[1; 32], &dest_binding(&[2; 32], &[3; 32]), &token, denom);
    let leaves: Vec<[u8; 32]> = (0_u8..5).map(|i| leaf(&token, denom, &sha(&[&[i]]))).collect();
    let mut t = IncrementalTree::default();
    let mut roots = Vec::new();
    for l in &leaves {
        t.insert(*l);
        roots.push(hex0x(&t.root()));
    }
    let (_, path) = path_for(&leaves, 3);
    let note = burn_note(&[9; 32], &[0x22; 20], 5);
    let w = BurnWitness {
        burn_secret: [9; 32],
        recipient: [0x22; 20],
        fee: 5,
        token,
        denom,
        leaf_index: 0,
        path: path_for(&[burn_leaf(&token, denom, &note)], 0).1,
        chain_id: 31_337,
        vault: [0x33; 20],
        lez_program: [0x44; 32],
        lez_instance: [0x55; 32],
    };
    let j = prove_burn(&w);
    let v = json!({
        "token": hex0x(&token),
        "denom": denom.to_string(),
        "commitment": hex0x(&c),
        "leaf": hex0x(&leaf(&token, denom, &c)),
        "leaves_commitments": (0_u8..5).map(|i| hex0x(&sha(&[&[i]]))).collect::<Vec<_>>(),
        "roots": roots,
        "path3": path.iter().map(|p| hex0x(p)).collect::<Vec<_>>(),
        "journal": hex0x(&j.encode()),
        "journal_root": hex0x(&j.accumulator_root),
        "journal_nullifier": hex0x(&j.nullifier),
    });
    std::fs::write(out, serde_json::to_string_pretty(&v)? + "\n")?;
    println!("wrote {}", out.display());
    Ok(())
}
