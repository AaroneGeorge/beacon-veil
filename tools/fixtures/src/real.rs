//! Loads the fetched fixtures of one chain (`fixtures/<chain>/`, written by `tools/fixtures/fetch.py`) and builds the
//! instance config of a checkpoint at the fetched bootstrap.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use attest_core::{AccountId, InstanceConfig};
use serde_json::Value;

/// One chain's fixtures.
#[derive(Debug, Clone)]
pub struct RealChain {
    pub chain: String,
    pub dir: PathBuf,
    pub config: InstanceConfig,
    /// SSZ `LightClientBootstrap` at `config.checkpoint_slot`.
    pub bootstrap_ssz: Vec<u8>,
    /// SSZ `LightClientUpdate`s of adjacent periods, from the first range file (`updates_<p0>_<n>.ssz`).
    pub updates: Vec<Vec<u8>>,
    pub first_period: u64,
    /// The newest SSZ `LightClientFinalityUpdate` with a JSON twin.
    pub finality_ssz: Vec<u8>,
    pub finality_attested_slot: u64,
}

fn hex32(v: &Value) -> Result<[u8; 32]> {
    let s = v.as_str().context("expected a hex string")?;
    let b = hex::decode(s.trim_start_matches("0x"))?;
    b.try_into().map_err(|_| anyhow::anyhow!("expected 32 bytes"))
}

fn hex4(v: &Value) -> Result<[u8; 4]> {
    let s = v.as_str().context("expected a hex string")?;
    let b = hex::decode(s.trim_start_matches("0x"))?;
    b.try_into().map_err(|_| anyhow::anyhow!("expected 4 bytes"))
}

fn num(v: &Value) -> Result<u64> {
    match v {
        Value::String(s) => Ok(s.parse()?),
        Value::Number(n) => n.as_u64().context("u64"),
        _ => bail!("expected a number"),
    }
}

fn json(p: &Path) -> Result<Value> {
    serde_json::from_slice(&std::fs::read(p).with_context(|| p.display().to_string())?)
        .with_context(|| p.display().to_string())
}

/// `(activation_epoch, version)` for genesis..gloas (at most 8 entries, as `attest_core::lc::forks` expects), from
/// `/eth/v1/config/fork_schedule`, whose entries are in fork order.
pub fn forks_from_schedule(schedule: &Value) -> Result<Vec<(u64, [u8; 4])>> {
    let data = schedule["data"].as_array().context("fork_schedule.data")?;
    data.iter()
        .take(8)
        .map(|f| Ok((num(&f["epoch"])?, hex4(&f["current_version"])?)))
        .collect()
}

/// Loads `fixtures/<chain>`. `verifiers` go into the config (1..=2 ids); `min_participants` as given.
pub fn load(chain: &str, verifiers: Vec<AccountId>, min_participants: u16) -> Result<RealChain> {
    load_from(&crate::fixtures_dir().join(chain), verifiers, min_participants)
}

pub fn load_from(dir: &Path, verifiers: Vec<AccountId>, min_participants: u16) -> Result<RealChain> {
    let manifest = json(&dir.join("manifest.json"))?;
    let files = manifest["files"].as_object().context("manifest.files")?;
    let chain_id = num(&manifest["chain_id"])?;
    let genesis = json(&dir.join("genesis.json"))?;
    let schedule = json(&dir.join("fork_schedule.json"))?;

    let (bname, bentry) = files
        .iter()
        .filter(|(k, _)| k.starts_with("bootstrap_") && k.ends_with(".ssz"))
        .min_by_key(|(k, _)| (*k).clone())
        .context("no bootstrap_*.ssz in manifest")?;
    let bjson = files
        .get(&bname.replace(".ssz", ".json"))
        .context("bootstrap json entry")?;
    let checkpoint_root = hex32(&bjson["block_root"])?;
    let checkpoint_slot = num(&bjson["slot"])?;
    let _ = bentry;

    let (rname, rentry) = files
        .iter()
        .filter(|(k, _)| k.starts_with("updates_") && k.ends_with(".ssz"))
        .min_by_key(|(k, _)| (*k).clone())
        .context("no updates_*.ssz in manifest")?;
    let first_period = num(&rentry["start_period"])?;
    let updates = crate::ssz::split_range(&std::fs::read(dir.join(rname))?)?
        .into_iter()
        .map(|(_, p)| p)
        .collect();

    let (fname, fentry) = files
        .iter()
        .filter(|(k, _)| k.starts_with("finality_update_") && k.ends_with(".ssz"))
        .max_by_key(|(_, v)| v["attested_slot"].as_u64().unwrap_or(0))
        .context("no finality_update_*.ssz in manifest")?;

    let config = InstanceConfig {
        chain_id,
        genesis_validators_root: hex32(&genesis["data"]["genesis_validators_root"])?,
        genesis_time: num(&genesis["data"]["genesis_time"])?,
        forks: forks_from_schedule(&schedule)?,
        checkpoint_root,
        checkpoint_slot,
        verifiers,
        min_participants,
    };
    Ok(RealChain {
        chain: manifest["chain"].as_str().unwrap_or_default().to_string(),
        dir: dir.to_path_buf(),
        config,
        bootstrap_ssz: std::fs::read(dir.join(bname))?,
        updates,
        first_period,
        finality_ssz: std::fs::read(dir.join(fname))?,
        finality_attested_slot: num(&fentry["attested_slot"])?,
    })
}

#[cfg(test)]
mod tests {
    use attest_core::lc::{init_store, verify_and_apply};

    use super::*;
    use crate::ssz::{BootstrapParts, UpdateParts};

    /// G1 real-proof input (2026-11-12): a checkpoint at update[3]'s finalized header (slot 15631808,
    /// period 1908), then `finality_update_15637644` as the FIRST update.
    #[test]
    fn mainnet_bootstrap_1908_then_finality_update() {
        let dir = crate::fixtures_dir().join("mainnet");
        let mut c = load_from(&dir, vec![AccountId::new([0x11; 32])], 0).expect("mainnet fixtures");
        let manifest = json(&dir.join("manifest.json")).expect("manifest");
        let e = &manifest["files"]["bootstrap_15631808.json"];
        c.config.checkpoint_root = hex32(&e["block_root"]).expect("root");
        c.config.checkpoint_slot = num(&e["slot"]).expect("slot");
        let bootstrap = std::fs::read(dir.join("bootstrap_15631808.ssz")).expect("bootstrap ssz");

        let b = BootstrapParts::from_ssz(&bootstrap).expect("decode bootstrap");
        let u3 = UpdateParts::from_ssz(&c.updates[3]).expect("update[3]");
        assert_eq!(
            b.header.root(),
            u3.finalized.root(),
            "bootstrap header = update[3] finalized header"
        );
        assert_eq!(b.header.root().0, c.config.checkpoint_root);

        let (w, head) = init_store(&c.config, &bootstrap).expect("init_store");
        assert_eq!(head.slot, 15_631_808);
        let fu = std::fs::read(dir.join("finality_update_15637644.ssz")).expect("finality ssz");
        let out = verify_and_apply(&c.config, &w, &fu).expect("finality update applies first");
        assert!(out.new_slot, "finalized slot advanced");
        eprintln!(
            "bootstrap 15631808 -> finality_update_15637644: head.slot {} period {} participation {} exec_number {} pre {} post {}",
            out.head.slot,
            out.head.period,
            out.participation,
            out.head.exec_number,
            hex::encode(out.pre_store),
            hex::encode(out.post_store)
        );
    }
}
