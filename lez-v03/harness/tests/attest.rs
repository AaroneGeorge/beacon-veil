//! `Attest` of `attest_fact_v03` through LEZ v0.3.2, on the Sepolia claim recorded for the public testnet
//! (`evidence/public-testnet/attest/data`: the vault's root slot in the execution block of beacon slot 11,649,024)
//! against that slot's header record as `attest_v03` holds it on the testnet (`run.json` `records`).
//!
//! `attest_v03` is deployed here at its testnet id (`ATTEST_V03`, which `attest_fact_v03` trusts). The record shard is
//! seeded with the testnet bytes. Checked: `Attest` applies under the step limit, writes the statement the native
//! `verify_claim` computes, leaves the record unchanged; refusals by exit code, each changing nothing.

use std::path::PathBuf;

use attest_core::{EsaError, HeaderRecord, Predicate, PredicateValue, Statement};
use attest_fact_v03::{ATTEST_V03, Instruction};
use attest_inclusion::ClaimProof;
use lee::V03State;
use lee_core::account::{Account, AccountId, ProgramShardSelector};
use v03_harness::{
    attest::{self, FactData},
    guests,
    steps::{Ids, Tx},
    world::{self, STEP_LIMIT, World, shard_data},
};

const FACT: AccountId = AccountId::new([0xFA; 32]);

fn evidence() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evidence/public-testnet")
}

/// The testnet instance and the header record of `slot`, from `run.json`.
fn testnet_record(slot: u64) -> ([u8; 32], Vec<u8>) {
    let r: serde_json::Value = serde_json::from_slice(&std::fs::read(evidence().join("run.json")).unwrap()).unwrap();
    let instance: AccountId = r["instance"].as_str().unwrap().parse().unwrap();
    let rec = r["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["slot"].as_u64() == Some(slot))
        .unwrap();
    (
        *instance.value(),
        hex::decode(rec["bytes_hex"].as_str().unwrap()).unwrap(),
    )
}

fn world_with(record: Option<(AccountId, Vec<u8>)>) -> World {
    let attest = AccountId::new(ATTEST_V03);
    let accounts = record.map(|(id, bytes)| (id, Account::default().with_shard(attest, shard_data(bytes).unwrap())));
    let state = V03State::new().with_public_accounts(accounts).with_named_programs([
        (attest, world::program(&guests::bin("attest_v03").unwrap()).unwrap()),
        (FACT, world::program(&guests::bin("attest_fact_v03").unwrap()).unwrap()),
    ]);
    World {
        state,
        ids: Ids {
            attest,
            v3: AccountId::new([0x33; 32]),
        },
        block_id: 1,
    }
}

fn snapshot(w: &World, tx: &Tx) -> Vec<Vec<u8>> {
    tx.selectors
        .iter()
        .map(|s| w.shard(s.account_id, s.program_account_id))
        .collect()
}

fn refused(w: &mut World, tx: &Tx, code: EsaError, what: &str) {
    let before = snapshot(w, tx);
    let o = w.exec(tx, 0, STEP_LIMIT);
    assert!(o.result.is_err(), "{what}: applied");
    assert_eq!(o.exit_code, Some(u32::from(code as u8)), "{what}: {:?}", o.result);
    assert!(o.cycles < STEP_LIMIT, "{what}: charged {} cycles", o.cycles);
    assert_eq!(snapshot(w, tx), before, "{what}: a shard changed");
    println!("{what}: refused {} with {} cycles", code.code(), o.cycles);
}

fn with_ix(tx: &Tx, f: impl FnOnce(&mut Instruction)) -> Tx {
    let mut ix: Instruction = borsh::from_slice(&tx.instruction).unwrap();
    f(&mut ix);
    Tx {
        instruction: borsh::to_vec(&ix).unwrap(),
        ..tx.clone()
    }
}

#[test]
fn attest_on_the_testnet_record() {
    let fact: FactData = attest::load(&evidence().join("attest/data")).unwrap();
    let (instance, rec_bytes) = testnet_record(fact.beacon_slot);
    let record: HeaderRecord = borsh::from_slice(&rec_bytes).unwrap();
    let (tx, st, fact_acc) = attest::tx(FACT, &instance, &record, &fact).unwrap();
    let rec_acc = tx.selectors[0].account_id;
    assert_eq!(st.value, fact.value.to_vec());
    assert_eq!(st.slot, fact.beacon_slot);
    assert_eq!(st.exec_number, fact.block_number);

    // No record at the address: E02.
    let mut w = world_with(None);
    refused(&mut w, &tx, EsaError::E02NotFinalised, "Attest with no record");

    let mut w = world_with(Some((rec_acc, rec_bytes.clone())));
    // The instruction's record differs from the stored one (it names the block number, which the stored record
    // leaves zero; the proof still verifies under it and the statement is the same): E15.
    let other = with_ix(&tx, |Instruction::Attest { record, .. }| {
        record.exec_number = fact.block_number
    });
    refused(
        &mut w,
        &other,
        EsaError::E15HeaderMismatch,
        "Attest with a record that is not the stored one",
    );
    // A predicate that does not hold: E08.
    let mut wrong = fact.value;
    wrong[31] ^= 1;
    let p = with_ix(&tx, |Instruction::Attest { predicate, .. }| {
        *predicate = Predicate::Eq(PredicateValue::Bytes32(wrong));
    });
    refused(&mut w, &p, EsaError::E08PredicateFalse, "Attest with a false predicate");
    // One byte of a storage proof node changed: E07.
    let mut bad = fact.clone();
    if let ClaimProof::Storage { storage_proof, .. } = &mut bad.claim.proof {
        let n = storage_proof.last_mut().unwrap();
        let i = n.len() - 1;
        n[i] ^= 1;
    }
    let b = with_ix(&tx, |Instruction::Attest { claim, .. }| *claim = bad.claim.to_bytes());
    refused(
        &mut w,
        &b,
        EsaError::E07InclusionInvalid,
        "Attest with a changed proof node",
    );
    // The record handle of another slot: E09.
    let mut moved = tx.clone();
    moved.selectors[0] = ProgramShardSelector::new(
        attest_fact_v03::record_of(&instance, record.slot + 32),
        AccountId::new(ATTEST_V03),
    );
    refused(
        &mut w,
        &moved,
        EsaError::E09ChainMismatch,
        "Attest naming another record",
    );

    let o = w.exec(&tx, 0, STEP_LIMIT);
    assert_eq!(o.result, Ok(()), "Attest");
    assert!(o.cycles <= STEP_LIMIT, "Attest: {} cycles", o.cycles);
    println!("Attest: {} cycles ({} bytes)", o.cycles, tx.instruction.len());
    let written: Statement = borsh::from_slice(&w.shard(fact_acc, FACT)).unwrap();
    assert_eq!(written, st);
    assert_eq!(
        w.shard(rec_acc, AccountId::new(ATTEST_V03)),
        rec_bytes,
        "the record changed"
    );

    // The same fact again: E14.
    refused(&mut w, &tx, EsaError::E14AlreadyApplied, "Attest again");
}
