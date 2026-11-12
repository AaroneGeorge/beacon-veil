//! Test crate `e2e_consumer` (PLAN.md section 4, owner W3).
//!
//! - `tests/e2e_consumer.rs`: G3 on a running `seq-dev`: one attested ERC-20 `Transfer` consumed in the consumer's
//!   own transaction, the identical transaction refused with `E11_ALREADY_CONSUMED`; and the private placement.
//! - `src/bin/attest_exec.rs`: cycles of `Attest` (log, storage) and of the consumer in the RISC Zero executor.
//!
//! This lib holds the fixture helpers both use. Fixtures are `crates/attest_inclusion/tests/data/*.json`
//! (`tools/proofgen/fixtures.sh`); the mainnet ones sit at execution block 26397891, which is the finalized header of
//! W2's `finality_update_15637644` (beacon slot 15637568), the last step of `e2e_chain::mainnet_chain`.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use attest_core::{AccountId, HeaderRecord};
use consumer_erc20_program::ConsumerConfig;
use serde_json::Value;

/// Beacon slot of the record that holds mainnet execution block 26397891.
pub const MAINNET_RECORD_SLOT: u64 = 15_637_568;

#[must_use]
pub fn data_dir() -> PathBuf {
    std::env::var("BV_REPO").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/attest_inclusion/tests/data"),
        |r| PathBuf::from(r).join("crates/attest_inclusion/tests/data"),
    )
}

/// A proofgen fixture.
pub struct Fixture(pub Value);

impl Fixture {
    pub fn load(name: &str) -> Result<Self> {
        let p = data_dir().join(name);
        let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Self(serde_json::from_str(&text)?))
    }

    pub fn bytes(&self, key: &str) -> Result<Vec<u8>> {
        let s = self.0[key].as_str().ok_or_else(|| anyhow!("fixture field {key}"))?;
        Ok(hex::decode(s.trim_start_matches("0x"))?)
    }

    pub fn word(&self, v: &Value) -> Result<[u8; 32]> {
        let s = v.as_str().ok_or_else(|| anyhow!("hex"))?.trim_start_matches("0x");
        let b = hex::decode(if s.len() % 2 == 1 {
            format!("0{s}")
        } else {
            s.to_string()
        })?;
        anyhow::ensure!(b.len() <= 32, "word above 32 bytes");
        let mut w = [0u8; 32];
        w[32 - b.len()..].copy_from_slice(&b);
        Ok(w)
    }

    /// The record a fulu light client holds for this block (every field set), at `slot`.
    pub fn record(&self, slot: u64, beacon_root: [u8; 32]) -> Result<HeaderRecord> {
        Ok(HeaderRecord {
            chain_id: self.0["chain_id"].as_u64().context("chain_id")?,
            slot,
            beacon_root,
            exec_number: self.0["block_number"].as_u64().context("block_number")?,
            exec_hash: self.word(&self.0["block_hash"])?,
            exec_state_root: self.word(&self.0["state_root"])?,
            exec_receipts_root: self.word(&self.0["receipts_root"])?,
        })
    }

    /// Consumer config accepting this fixture's `Transfer` log (token = emitter, recipient = indexed `to`).
    pub fn consumer_config(&self, attest_program: AccountId, instance: AccountId, salt: u64) -> Result<ConsumerConfig> {
        let e = &self.0["expected"];
        let token: [u8; 20] = hex::decode(e["address"].as_str().context("address")?.trim_start_matches("0x"))?
            .try_into()
            .map_err(|_| anyhow!("address"))?;
        let to = self.word(&e["topics"][2])?;
        Ok(ConsumerConfig {
            attest_program,
            instance,
            chain_id: self.0["chain_id"].as_u64().context("chain_id")?,
            token,
            recipient: to[12..].try_into().expect("20 bytes"),
            salt,
        })
    }

    /// Transfer amount (data word 0).
    pub fn amount(&self) -> Result<[u8; 32]> {
        self.word(&self.0["expected"]["data"])
    }
}
