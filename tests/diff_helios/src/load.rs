//! Loads the fetched fixtures of the differential test, `fixtures/diff/<network>/` (written by
//! `tools/fixtures/fetch_range.py`; every file listed in `manifest.json` with its URL, provider, time and SHA-256).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use attest_core::{AccountId, InstanceConfig};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// One fetched update.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// File it came from, with the index inside a range file (`updates_1904_12.ssz#3`).
    pub source: String,
    pub ssz: Vec<u8>,
}

/// One `updates_by_range` response: the best update of each of `count` adjacent periods.
#[derive(Debug, Clone)]
pub struct PeriodRange {
    pub start_period: u64,
    pub updates: Vec<Fetched>,
}

#[derive(Debug, Clone)]
pub struct Network {
    pub name: String,
    pub dir: PathBuf,
    pub config: InstanceConfig,
    pub ranges: Vec<PeriodRange>,
    /// Finality updates, oldest first.
    pub finality: Vec<Fetched>,
    /// Optimistic updates, oldest first.
    pub optimistic: Vec<Fetched>,
}

/// `fixtures/diff` (respects `BV_REPO` like the rest of the workspace).
#[must_use]
pub fn diff_fixtures_dir() -> PathBuf {
    fixtures::fixtures_dir().join("diff")
}

fn json(p: &Path) -> Result<Value> {
    serde_json::from_slice(&std::fs::read(p).with_context(|| p.display().to_string())?)
        .with_context(|| p.display().to_string())
}

fn num(v: &Value) -> Result<u64> {
    match v {
        Value::String(s) => Ok(s.parse()?),
        Value::Number(n) => n.as_u64().context("u64"),
        _ => bail!("expected a number"),
    }
}

fn hex32(v: &Value) -> Result<[u8; 32]> {
    let b = hex::decode(v.as_str().context("hex string")?.trim_start_matches("0x"))?;
    b.try_into().map_err(|_| anyhow::anyhow!("expected 32 bytes"))
}

/// Reads a file listed in the manifest and checks its SHA-256 against the manifest entry.
fn read_checked(dir: &Path, name: &str, entry: &Value) -> Result<Vec<u8>> {
    let bytes = std::fs::read(dir.join(name)).with_context(|| format!("{}/{name}", dir.display()))?;
    let want = entry["sha256"].as_str().context("sha256 in manifest")?;
    let got = hex::encode(Sha256::digest(&bytes));
    if got != want {
        bail!("{name}: sha256 {got} differs from the manifest ({want})");
    }
    Ok(bytes)
}

pub fn load_network(name: &str) -> Result<Network> {
    load_network_from(&diff_fixtures_dir().join(name))
}

pub fn load_network_from(dir: &Path) -> Result<Network> {
    let manifest = json(&dir.join("manifest.json"))?;
    let files = manifest["files"].as_object().context("manifest.files")?;
    let genesis = json(&dir.join("genesis.json"))?;
    let schedule = json(&dir.join("fork_schedule.json"))?;
    let config = InstanceConfig {
        chain_id: num(&manifest["chain_id"])?,
        genesis_validators_root: hex32(&genesis["data"]["genesis_validators_root"])?,
        genesis_time: num(&genesis["data"]["genesis_time"])?,
        forks: fixtures::real::forks_from_schedule(&schedule)?,
        // Not read by `Verify` (only `Init` checks the checkpoint).
        checkpoint_root: [0; 32],
        checkpoint_slot: 0,
        verifiers: vec![AccountId::new([0x11; 32])],
        min_participants: 0,
    };

    let mut ranges = Vec::new();
    let mut finality = Vec::new();
    let mut optimistic = Vec::new();
    // BTreeMap-ordered names; ranges by start period, live updates by attested slot.
    for (fname, entry) in files {
        if !fname.ends_with(".ssz") {
            continue;
        }
        if fname.starts_with("updates_") {
            let bytes = read_checked(dir, fname, entry)?;
            let items = fixtures::ssz::split_range(&bytes)?;
            let count = num(&entry["count"])?;
            if items.len() as u64 != count {
                bail!("{fname}: {} items, manifest says {count}", items.len());
            }
            ranges.push(PeriodRange {
                start_period: num(&entry["start_period"])?,
                updates: items
                    .into_iter()
                    .enumerate()
                    .map(|(i, (_, ssz))| Fetched {
                        source: format!("{fname}#{i}"),
                        ssz,
                    })
                    .collect(),
            });
        } else if fname.starts_with("finality_update_") || fname.starts_with("optimistic_update_") {
            let f = Fetched {
                source: fname.clone(),
                ssz: read_checked(dir, fname, entry)?,
            };
            let slot = num(&entry["attested_slot"])?;
            if fname.starts_with("finality_") {
                finality.push((slot, f));
            } else {
                optimistic.push((slot, f));
            }
        }
    }
    ranges.sort_by_key(|r| r.start_period);
    finality.sort_by_key(|(s, _)| *s);
    optimistic.sort_by_key(|(s, _)| *s);
    Ok(Network {
        name: manifest["chain"].as_str().unwrap_or_default().to_string(),
        dir: dir.to_path_buf(),
        config,
        ranges,
        finality: finality.into_iter().map(|(_, f)| f).collect(),
        optimistic: optimistic.into_iter().map(|(_, f)| f).collect(),
    })
}
