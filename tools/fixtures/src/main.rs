//! `fixtures` CLI (W2, PLAN.md section 4).
//!
//! - `fixtures fetch --chain mainnet|sepolia [--out DIR] [--periods N] [--finality-only]`: runs the stdlib fetcher
//!   `tools/fixtures/fetch.py` with `python3 -I` (one implementation of the Beacon API fetch, used before this
//!   binary could be built while the shared target was locked; see TRACK-W2-NOTES.md).
//! - `fixtures fabricate [--like mainnet] [--seed S] [--out DIR]`: writes a fabricated chain that impersonates the
//!   fetched chain (same chain id, genesis validators root, forks, checkpoint slot; fresh keys), with a manifest.

use std::{path::PathBuf, process::Command};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use fixtures::{
    fabricate::{FabChain, FabParams},
    fixtures_dir, real,
};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(
    name = "fixtures",
    about = "Beacon light-client fixtures and the fabricated chain (beacon-veil W2)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fetch bootstrap, updates_by_range, finality/optimistic updates (JSON + SSZ) with a manifest.
    Fetch {
        #[arg(long)]
        chain: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 4)]
        periods: u32,
        #[arg(long)]
        finality_only: bool,
    },
    /// Write a fabricated chain impersonating a fetched one.
    Fabricate {
        #[arg(long, default_value = "mainnet")]
        like: String,
        #[arg(long, default_value = "beacon-veil/vs1/v1")]
        seed: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Fetch {
            chain,
            out,
            periods,
            finality_only,
        } => {
            let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fetch.py");
            let out = out.unwrap_or_else(fixtures_dir);
            let mut c = Command::new("python3");
            c.arg("-I")
                .arg(&script)
                .args(["--chain", &chain, "--periods", &periods.to_string()])
                .arg("--out")
                .arg(&out);
            if finality_only {
                c.arg("--finality-only");
            }
            let st = c.status().context("running python3")?;
            if !st.success() {
                bail!("fetch.py failed: {st}");
            }
        }
        Cmd::Fabricate { like, seed, out } => {
            let real = real::load(&like, vec![attest_core::AccountId::new([0; 32])], 0)?;
            let chain = FabChain::build(FabParams::impersonating(&real.config, seed.as_bytes()));
            let out = out.unwrap_or_else(|| fixtures_dir().join(format!("fabricated-{like}")));
            std::fs::create_dir_all(&out)?;
            let mut files = serde_json::Map::new();
            let mut put = |name: &str, bytes: &[u8]| -> Result<()> {
                std::fs::write(out.join(name), bytes)?;
                files.insert(
                    name.into(),
                    serde_json::json!({"bytes": bytes.len(), "sha256": hex::encode(Sha256::digest(bytes))}),
                );
                Ok(())
            };
            put("bootstrap.ssz", &chain.bootstrap.ssz())?;
            for (i, u) in chain.updates.iter().enumerate() {
                let kind = if u.next.is_some() { "update" } else { "finality_update" };
                put(&format!("{i}_{kind}_{}.ssz", u.finalized.beacon.slot), &u.ssz())?;
            }
            let s = &chain.state;
            let state = serde_json::json!({
                "state_root": s.state_root, "receipts_root": s.receipts_root, "address": s.address,
                "nonce": s.nonce, "balance": s.balance, "storage_root": s.storage_root, "code_hash": s.code_hash,
                "account_proof": s.account_proof.iter().map(|n| format!("0x{}", hex::encode(n))).collect::<Vec<_>>(),
                "slot_key": s.slot_key, "slot_value": s.slot_value,
                "storage_proof": s.storage_proof.iter().map(|n| format!("0x{}", hex::encode(n))).collect::<Vec<_>>(),
            });
            put("state.json", serde_json::to_string_pretty(&state)?.as_bytes())?;
            let manifest = serde_json::json!({
                "what": "fabricated chain (RFP-021 Verification Security 1): fresh BLS keys, self-consistent; NOT Ethereum",
                "impersonates": like, "seed": seed, "chain_id": chain.params.chain_id,
                "genesis_validators_root": chain.params.genesis_validators_root,
                "checkpoint_slot": chain.params.checkpoint_slot, "checkpoint_root": chain.checkpoint_root,
                "committee_roots": chain.keys.iter().map(|k| serde_json::json!({"period": k.period, "root": k.root()})).collect::<Vec<_>>(),
                "real_checkpoint_root": format!("0x{}", hex::encode(real.config.checkpoint_root)),
                "files": files,
            });
            std::fs::write(
                out.join("manifest.json"),
                serde_json::to_string_pretty(&manifest)? + "\n",
            )?;
            eprintln!("fabricated chain written to {}", out.display());
        }
    }
    Ok(())
}
