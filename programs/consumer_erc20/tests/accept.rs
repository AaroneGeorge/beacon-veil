//! Native tests of the consumer rule on real attested statements (mainnet USDT `Transfer`, block 26397891), built
//! with `attest_inclusion::verify_claim` exactly as the `attest` program builds them.

use attest_core::{AccountId, EsaError, HeaderRecord, Predicate, Statement};
use consumer_erc20_program::{
    ConsumerConfig, ConsumerState, accept, consumer_instance_id, nullifier_id, nullifier_seed,
};
use serde_json::Value;

const ATTEST: AccountId = AccountId::new([0xA7; 32]);
const INSTANCE: AccountId = AccountId::new([0x11; 32]);

fn load(name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/attest_inclusion/tests/data")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}
fn hx(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap().trim_start_matches("0x")).unwrap()
}
fn w(v: &Value) -> [u8; 32] {
    let b = hx(v);
    let mut o = [0u8; 32];
    o[32 - b.len()..].copy_from_slice(&b);
    o
}
fn statement(name: &str, instance: AccountId) -> (Value, Statement) {
    let f = load(name);
    let r = HeaderRecord {
        chain_id: f["chain_id"].as_u64().unwrap(),
        slot: 15_637_568,
        beacon_root: [1; 32],
        exec_number: f["block_number"].as_u64().unwrap(),
        exec_hash: w(&f["block_hash"]),
        exec_state_root: w(&f["state_root"]),
        exec_receipts_root: w(&f["receipts_root"]),
    };
    let st = attest_inclusion::verify_claim(instance, &r, &hx(&f["claim"]), &Predicate::All(vec![])).unwrap();
    (f, st)
}
fn config_for(f: &Value) -> ConsumerConfig {
    let e = &f["expected"];
    ConsumerConfig {
        attest_program: ATTEST,
        instance: INSTANCE,
        chain_id: 1,
        token: hx(&e["address"]).try_into().unwrap(),
        recipient: w(&e["topics"][2])[12..].try_into().unwrap(),
        salt: 0,
    }
}

#[test]
fn accepts_the_configured_transfer_and_credits_it() {
    let (f, st) = statement("mainnet_log_usdt_tx1_log2.json", INSTANCE);
    let state = ConsumerState::new(config_for(&f));
    let credit = accept(&state, &ATTEST, &st).unwrap();
    assert_eq!(credit.statement_id, st.id);
    assert_eq!(credit.amount, w(&f["expected"]["data"]));
    assert_eq!(credit.from.to_vec(), w(&f["expected"]["topics"][1])[12..].to_vec());
    let next = state.credited(credit);
    assert_eq!((next.count, next.total), (1, w(&f["expected"]["data"])));
}

#[test]
fn refusals_carry_the_right_codes() {
    let (f, st) = statement("mainnet_log_usdt_tx1_log2.json", INSTANCE);
    let state = ConsumerState::new(config_for(&f));
    // Sender is not the configured attest program.
    assert_eq!(
        accept(&state, &AccountId::new([0xEE; 32]), &st),
        Err(EsaError::E12UnknownVerifier)
    );
    // Statement for another light-client instance, or another chain.
    let (_, other) = statement("mainnet_log_usdt_tx1_log2.json", AccountId::new([0x12; 32]));
    assert_eq!(accept(&state, &ATTEST, &other), Err(EsaError::E09ChainMismatch));
    let mut cfg = config_for(&f);
    cfg.chain_id = 11_155_111;
    assert_eq!(
        accept(&ConsumerState::new(cfg), &ATTEST, &st),
        Err(EsaError::E09ChainMismatch)
    );
    // A storage statement is not a transfer.
    let (_, storage) = statement("mainnet_storage_usdt_slot1.json", INSTANCE);
    assert_eq!(accept(&state, &ATTEST, &storage), Err(EsaError::E10PredicateNotCovered));
    // Another recipient (tx 1 log 4 goes elsewhere), another token (USDC log).
    let (_, elsewhere) = statement("mainnet_log_usdt_tx1_log4.json", INSTANCE);
    assert_eq!(accept(&state, &ATTEST, &elsewhere), Err(EsaError::E08PredicateFalse));
    let (_, usdc) = statement("mainnet_log_usdc_tx3_log0.json", INSTANCE);
    assert_eq!(accept(&state, &ATTEST, &usdc), Err(EsaError::E08PredicateFalse));
    // A tampered value (not what attest produced) fails to decode.
    let mut bad = st.clone();
    bad.value.truncate(10);
    assert_eq!(accept(&state, &ATTEST, &bad), Err(EsaError::E01Malformed));
}

#[test]
fn nullifiers_are_per_consumer_instance() {
    let (f, st) = statement("mainnet_log_usdt_tx1_log2.json", INSTANCE);
    let prog = AccountId::new([0xC2; 32]);
    let a = consumer_instance_id(&prog, &config_for(&f));
    let mut c2 = config_for(&f);
    c2.salt = 1;
    let b = consumer_instance_id(&prog, &c2);
    assert_ne!(a, b);
    // Same statement, two consumer instances: two different nullifier accounts (no cross-instance griefing).
    assert_ne!(nullifier_id(&prog, &a, &st.id), nullifier_id(&prog, &b, &st.id));
    assert_ne!(nullifier_seed(&a, &st.id), st.id);
}
