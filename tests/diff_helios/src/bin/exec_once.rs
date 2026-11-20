//! One `VerifyOnce` message through the `lc_verify_v3` guest in the RISC Zero executor (no proof), from inputs saved
//! by `attest update --mode v3-private --save-inputs DIR` (`config_witness.borsh`, `update.ssz`): user cycles and
//! segments of the message a real-proof run proves, and the verdict.
//!
//!   cargo run -p diff_helios --features executor --bin exec_once -- --inputs DIR --elf <lc_verify_v3 ELF or .bin>

use anyhow::{Context, Result, bail};
use attest_core::{InstanceConfig, lc::StoreWitness};
use diff_helios::exec::ExecV3;
use serde_json::json;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let (Some(dir), Some(elf)) = (arg("--inputs"), arg("--elf")) else {
        bail!("usage: exec_once --inputs DIR --elf <lc_verify_v3 ELF or .bin>");
    };
    let dir = std::path::PathBuf::from(dir);
    let cw = std::fs::read(dir.join("config_witness.borsh")).context("config_witness.borsh")?;
    let (config, witness): (InstanceConfig, StoreWitness) = borsh::from_slice(&cw).context("decode inputs")?;
    let update = std::fs::read(dir.join("update.ssz")).context("update.ssz")?;
    let guest = ExecV3::load(elf.as_ref())?;
    let r = guest.execute(&config, &witness, &update);
    println!(
        "{}",
        json!({
            "elf_sha256": guest.elf_sha256, "image_id": guest.image_id, "verdict": r.verdict.short(),
            "user_cycles": r.cycles, "segments": r.segments,
        })
    );
    Ok(())
}
