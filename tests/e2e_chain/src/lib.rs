//! Test crate `e2e_chain` (PLAN.md section 4, owner W1). Integration tests live in `tests/`; this lib holds the
//! fixture helpers they share with the `v1_exec` harness (`src/bin/v1_exec.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, InstanceConfig, lc};

/// `$BV_REPO/fixtures`, else the repo's `fixtures/` relative to this crate.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    std::env::var("BV_REPO").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        |r| PathBuf::from(r).join("fixtures"),
    )
}

/// Reads `fixtures/<chain>/<name>`.
pub fn fixture(chain: &str, name: &str) -> Result<Vec<u8>> {
    let p = fixtures_dir().join(chain).join(name);
    std::fs::read(&p).with_context(|| format!("reading fixture {}", p.display()))
}

/// Splits an SSZ `updates_by_range` body: per item a u64 little-endian length, a 4-byte fork digest, then the SSZ
/// update. Accepts both readings of the length (with or without the 4 context bytes) by checking which one tiles the
/// body exactly.
pub fn split_updates_by_range(body: &[u8]) -> Result<Vec<Vec<u8>>> {
    for len_includes_digest in [false, true] {
        let mut out = Vec::new();
        let mut rest = body;
        let ok = loop {
            if rest.is_empty() {
                break true;
            }
            if rest.len() < 12 {
                break false;
            }
            let len = usize::try_from(u64::from_le_bytes(rest[..8].try_into().expect("8 bytes")))?;
            let data_len = if len_includes_digest {
                len.checked_sub(4)
            } else {
                Some(len)
            };
            let Some(data_len) = data_len else { break false };
            let Some(item) = rest.get(12..12 + data_len) else {
                break false;
            };
            out.push(item.to_vec());
            rest = &rest[12 + data_len..];
        };
        if ok && !out.is_empty() {
            return Ok(out);
        }
    }
    bail!("updates_by_range body does not tile into length-prefixed items")
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim_start_matches("0x"))?;
    v.try_into().map_err(|_| anyhow!("expected 32 bytes: {s}"))
}

fn hex4(s: &str) -> Result<[u8; 4]> {
    let v = hex::decode(s.trim_start_matches("0x"))?;
    v.try_into().map_err(|_| anyhow!("expected 4 bytes: {s}"))
}

fn json_u64(v: &serde_json::Value) -> Result<u64> {
    match v {
        serde_json::Value::String(s) => Ok(s.parse()?),
        serde_json::Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("not a u64: {n}")),
        _ => bail!("not a u64: {v}"),
    }
}

/// Builds an `InstanceConfig` from the Beacon API `genesis` and `config/fork_schedule` bodies and a bootstrap: the
/// checkpoint is the bootstrap's own header (its root and slot). Forks: the first 8 schedule entries (genesis ..
/// gloas), as `attest_core::lc::FORK_ORDER`.
pub fn config_from_beacon(
    chain_id: u64,
    genesis_json: &[u8],
    fork_schedule_json: &[u8],
    bootstrap_ssz: &[u8],
    verifiers: Vec<AccountId>,
    min_participants: u16,
) -> Result<InstanceConfig> {
    let g: serde_json::Value = serde_json::from_slice(genesis_json)?;
    let fs: serde_json::Value = serde_json::from_slice(fork_schedule_json)?;
    let mut forks = Vec::new();
    for f in fs["data"]
        .as_array()
        .ok_or_else(|| anyhow!("fork_schedule.data"))?
        .iter()
        .take(8)
    {
        forks.push((
            json_u64(&f["epoch"])?,
            hex4(f["current_version"].as_str().unwrap_or_default())?,
        ));
    }
    let mut config = InstanceConfig {
        chain_id,
        genesis_validators_root: hex32(g["data"]["genesis_validators_root"].as_str().unwrap_or_default())?,
        genesis_time: json_u64(&g["data"]["genesis_time"])?,
        forks,
        checkpoint_root: [0; 32],
        checkpoint_slot: 0,
        verifiers,
        min_participants,
    };
    let helios_forks = lc::forks(&config).map_err(|e| anyhow!("forks: {e}"))?;
    let b = lc::decode_bootstrap(bootstrap_ssz, &helios_forks).map_err(|e| anyhow!("bootstrap: {e}"))?;
    let header = lc::BeaconHeader::from_helios(b.header().beacon());
    config.checkpoint_root = header.root();
    config.checkpoint_slot = header.slot;
    Ok(config)
}

/// A real chain from the fixtures: config, bootstrap and the ordered updates of one test chain.
pub struct FixtureChain {
    pub config: InstanceConfig,
    pub bootstrap: Vec<u8>,
    /// `(label, update_ssz)` in submission order.
    pub updates: Vec<(String, Vec<u8>)>,
}

/// Mainnet chain (W2 fixtures 2026-11-12): bootstrap 15605760 (period 1905), `updates_by_range` 1905..1908
/// (update[0] adds the next committee, update[1..3] hand off to 1906, 1907, 1908), then finality update 15637644.
pub fn mainnet_chain(verifiers: Vec<AccountId>) -> Result<FixtureChain> {
    let bootstrap = fixture("mainnet", "bootstrap_15605760.ssz")?;
    let config = config_from_beacon(
        1,
        &fixture("mainnet", "genesis.json")?,
        &fixture("mainnet", "fork_schedule.json")?,
        &bootstrap,
        verifiers,
        0,
    )?;
    let mut updates: Vec<(String, Vec<u8>)> = split_updates_by_range(&fixture("mainnet", "updates_1905_4.ssz")?)?
        .into_iter()
        .enumerate()
        .map(|(i, u)| (format!("update[{i}]"), u))
        .collect();
    updates.push((
        "finality_update_15637644".into(),
        fixture("mainnet", "finality_update_15637644.ssz")?,
    ));
    Ok(FixtureChain {
        config,
        bootstrap,
        updates,
    })
}
