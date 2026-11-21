//! The committed fixtures (`fixtures/` of the repository). Same readers as `tests/e2e_chain` (copied, so this
//! workspace does not build the root workspace's test crates).

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, InstanceConfig, lc};

/// `$BV_REPO/fixtures`, else `../../fixtures` from this crate.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    std::env::var("BV_REPO").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        |r| PathBuf::from(r).join("fixtures"),
    )
}

pub fn fixture(chain: &str, name: &str) -> Result<Vec<u8>> {
    let p = fixtures_dir().join(chain).join(name);
    std::fs::read(&p).with_context(|| format!("reading fixture {}", p.display()))
}

/// Splits an SSZ `updates_by_range` body: per item a u64 little-endian length, a 4-byte fork digest, then the SSZ
/// update (either reading of the length, whichever tiles the body).
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

fn hex_n<const N: usize>(s: &str) -> Result<[u8; N]> {
    let v = hex::decode(s.trim_start_matches("0x"))?;
    v.try_into().map_err(|_| anyhow!("expected {N} bytes: {s}"))
}

fn json_u64(v: &serde_json::Value) -> Result<u64> {
    match v {
        serde_json::Value::String(s) => Ok(s.parse()?),
        serde_json::Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("not a u64: {n}")),
        _ => bail!("not a u64: {v}"),
    }
}

/// `InstanceConfig` from the Beacon API `genesis` and `config/fork_schedule` bodies and a bootstrap (its own header is
/// the checkpoint); forks: the first 8 schedule entries.
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
            hex_n::<4>(f["current_version"].as_str().unwrap_or_default())?,
        ));
    }
    let mut config = InstanceConfig {
        chain_id,
        genesis_validators_root: hex_n::<32>(g["data"]["genesis_validators_root"].as_str().unwrap_or_default())?,
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

/// One chain: config, bootstrap, `(label, update_ssz)` in submission order.
pub struct FixtureChain {
    pub network: &'static str,
    pub config: InstanceConfig,
    pub bootstrap: Vec<u8>,
    pub updates: Vec<(String, Vec<u8>)>,
}

fn chain(
    network: &'static str,
    chain_id: u64,
    bootstrap: &str,
    by_range: &str,
    finality: &str,
    verifiers: Vec<AccountId>,
) -> Result<FixtureChain> {
    let bootstrap = fixture(network, bootstrap)?;
    let config = config_from_beacon(
        chain_id,
        &fixture(network, "genesis.json")?,
        &fixture(network, "fork_schedule.json")?,
        &bootstrap,
        verifiers,
        0,
    )?;
    let mut updates: Vec<(String, Vec<u8>)> = split_updates_by_range(&fixture(network, by_range)?)?
        .into_iter()
        .enumerate()
        .map(|(i, u)| (format!("update[{i}]"), u))
        .collect();
    updates.push((finality.trim_end_matches(".ssz").into(), fixture(network, finality)?));
    Ok(FixtureChain {
        network,
        config,
        bootstrap,
        updates,
    })
}

/// Mainnet (fixtures 2026-11-12): bootstrap 15605760, `updates_by_range` 1905..1908, finality update 15637644.
pub fn mainnet(verifiers: Vec<AccountId>) -> Result<FixtureChain> {
    chain(
        "mainnet",
        1,
        "bootstrap_15605760.ssz",
        "updates_1905_4.ssz",
        "finality_update_15637644.ssz",
        verifiers,
    )
}

/// Sepolia (fixtures 2026-11-12, Gloas): bootstrap 11534336, `updates_by_range` 1408..1411, finality update 11561824.
pub fn sepolia(verifiers: Vec<AccountId>) -> Result<FixtureChain> {
    chain(
        "sepolia",
        11_155_111,
        "bootstrap_11534336.ssz",
        "updates_1408_4.ssz",
        "finality_update_11561824.ssz",
        verifiers,
    )
}

/// The fresh public data of the rc3 on-chain runs (`evidence/public-path/onchain/<network>/run.json`, fetched 18-19
/// Nov 2026): the bootstrap at the first slot of a period (`fixtures/pub/<network>`), the best updates of that
/// period and the next (`fixtures/diff/<network>/updates_*`), then the newest finality update that applies after
/// them. Same selection as the root workspace's `tools/cost` `v3_onchain`.
pub fn pub_chain(network: &'static str, verifiers: Vec<AccountId>) -> Result<FixtureChain> {
    let (chain_id, bootstrap, range, first, period) = match network {
        "mainnet" => (1u64, "bootstrap_15679488.ssz", "updates_1904_12.ssz", 1904u64, 1914u64),
        "sepolia" => (11_155_111, "bootstrap_11599872.ssz", "updates_1406_12.ssz", 1406, 1416),
        "hoodi" => (560_048, "bootstrap_4399104.ssz", "updates_530_9.ssz", 530, 537),
        _ => bail!("network: mainnet | sepolia | hoodi"),
    };
    let pub_dir = format!("pub/{network}");
    let bootstrap = fixture(&pub_dir, bootstrap)?;
    let config = config_from_beacon(
        chain_id,
        &fixture(&pub_dir, "genesis.json")?,
        &fixture(&pub_dir, "fork_schedule.json")?,
        &bootstrap,
        verifiers,
        0,
    )?;
    let diff = format!("diff/{network}");
    let ups = split_updates_by_range(&fixture(&diff, range)?)?;
    let at = |p: u64| -> Result<Vec<u8>> {
        ups.get(usize::try_from(p - first)?)
            .cloned()
            .ok_or_else(|| anyhow!("no update for period {p}"))
    };
    let mut updates = vec![
        (format!("update_{period}"), at(period)?),
        (format!("update_{}", period + 1), at(period + 1)?),
    ];
    let (mut w, _) = lc::init_store(&config, &bootstrap).map_err(|e| anyhow!("init: {e}"))?;
    for (_, u) in &updates {
        w = lc::verify_and_apply(&config, &w, u)
            .map_err(|e| anyhow!("{network}: {e}"))?
            .new_witness();
    }
    let mut names: Vec<String> = std::fs::read_dir(fixtures_dir().join(&diff))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("finality_update_") && n.ends_with(".ssz"))
        .collect();
    names.sort();
    let mut fin = None;
    for n in names.iter().rev() {
        let u = fixture(&diff, n)?;
        if lc::verify_and_apply(&config, &w, &u).is_ok() {
            fin = Some((n.trim_end_matches(".ssz").to_string(), u));
            break;
        }
    }
    updates.push(fin.ok_or_else(|| anyhow!("{network}: no finality update applies after the period updates"))?);
    Ok(FixtureChain {
        network,
        config,
        bootstrap,
        updates,
    })
}
