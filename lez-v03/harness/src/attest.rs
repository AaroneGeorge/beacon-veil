//! The `Attest` transaction of `attest_fact_v03` from a claim data folder (`tools/fixtures/fetch_claim.py`), with
//! the statement computed natively by the same function the program's `plan` runs, so a client never sends a claim
//! the program would refuse.

use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use attest_core::{Bytes32, HeaderRecord, Predicate, PredicateValue, Statement};
use attest_fact_v03::{Instruction, fact_id, record_of};
use attest_inclusion::{Claim, ClaimProof};
use lee_core::account::{AccountId, ProgramShardSelector};
use serde_json::Value;

use crate::steps::Tx;

/// One storage claim: `slot_key` of `address` equals `value` in execution block `block_hash` (`block_number`), the
/// execution block of the beacon block at `beacon_slot`.
#[derive(Debug, Clone)]
pub struct FactData {
    pub beacon_slot: u64,
    pub block_hash: Bytes32,
    pub block_number: u64,
    pub address: [u8; 20],
    pub slot_key: Bytes32,
    pub value: Bytes32,
    pub claim: Claim,
}

fn hex_n<const N: usize>(v: &Value) -> Result<[u8; N]> {
    let s = v.as_str().context("not a string")?;
    let b = hex::decode(s.trim_start_matches("0x"))?;
    b.try_into().map_err(|_| anyhow!("expected {N} bytes: {s}"))
}

fn nodes(v: &Value) -> Result<Vec<Vec<u8>>> {
    v.as_array()
        .context("proof is not a list")?
        .iter()
        .map(|n| Ok(hex::decode(n.as_str().context("node")?.trim_start_matches("0x"))?))
        .collect()
}

/// Reads `claim.json`, `header.rlp` and `proof.json` of a claim data folder.
pub fn load(dir: &Path) -> Result<FactData> {
    let read = |f: &str| std::fs::read(dir.join(f)).with_context(|| format!("{}/{f}", dir.display()));
    let c: Value = serde_json::from_slice(&read("claim.json")?)?;
    let p: Value = serde_json::from_slice(&read("proof.json")?)?;
    let p = &p["result"];
    let sp = &p["storageProof"][0];
    let address: [u8; 20] = hex_n(&c["address"])?;
    let slot_key: Bytes32 = hex_n(&c["slot_key"])?;
    ensure!(
        hex_n::<20>(&p["address"])? == address && hex_n::<32>(&sp["key"])? == slot_key,
        "proof.json is not for the claim's address and slot"
    );
    Ok(FactData {
        beacon_slot: c["beacon_slot"].as_u64().context("beacon_slot")?,
        block_hash: hex_n(&c["block_hash"])?,
        block_number: c["block_number"].as_u64().context("block_number")?,
        address,
        slot_key,
        value: hex_n(&c["value"])?,
        claim: Claim {
            exec_header: Some(read("header.rlp")?),
            proof: ClaimProof::Storage {
                address,
                slot_key,
                account_proof: nodes(&p["accountProof"])?,
                storage_proof: nodes(&sp["proof"])?,
            },
        },
    })
}

impl FactData {
    /// The predicate attested: the slot equals the recorded value.
    #[must_use]
    pub const fn predicate(&self) -> Predicate {
        Predicate::Eq(PredicateValue::Bytes32(self.value))
    }
}

/// The `Attest` transaction of `fact` under `record` of `instance`, the statement it writes and the fact account.
pub fn tx(
    fact_program: AccountId,
    instance: &Bytes32,
    record: &HeaderRecord,
    fact: &FactData,
) -> Result<(Tx, Statement, AccountId)> {
    ensure!(
        record.slot == fact.beacon_slot && record.exec_hash == fact.block_hash,
        "the record (slot {}) is not the claim's block (slot {})",
        record.slot,
        fact.beacon_slot
    );
    let claim = fact.claim.to_bytes();
    let predicate = fact.predicate();
    let st = attest_fact_v03::statement(instance, record, &claim, &predicate)
        .map_err(|e| anyhow!("Attest refused natively: {e}"))?;
    let fact_acc = fact_id(&fact_program, instance, &st.id);
    let tx = Tx {
        name: "Attest".into(),
        program: fact_program,
        selectors: vec![
            ProgramShardSelector::new(
                record_of(instance, record.slot),
                AccountId::new(attest_fact_v03::ATTEST_V03),
            ),
            ProgramShardSelector::new(fact_acc, fact_program),
        ],
        instruction: borsh::to_vec(&Instruction::Attest {
            instance: *instance,
            record: record.clone(),
            claim,
            predicate,
        })?,
        window_ms: None,
    };
    Ok((tx, st, fact_acc))
}
