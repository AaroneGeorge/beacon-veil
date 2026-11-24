//! `attest_inclusion` on real mainnet (fulu) and Sepolia (gloas) proofs, plus tampered-node cases.
//!
//! Fixtures: `tests/data/*.json`, written by `tools/proofgen/fixtures.sh` from public RPC at the execution blocks of
//! the beacon fixtures (mainnet 26397891, Sepolia 12117875). Each fixture carries the block roots, the RLP execution
//! header, the borsh claim, and the value the RPC reported.

use std::path::Path;

use attest_core::{AccountId, ClaimKind, EsaError, HeaderRecord, Predicate, PredicateValue};
use attest_inclusion::{
    AccountValue, Claim, ClaimProof, LogValue, keccak, verify_account, verify_claim, verify_claim_value, verify_log,
    verify_storage,
};
use serde_json::Value;

fn load(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn hx(v: &Value) -> Vec<u8> {
    let s = v.as_str().unwrap();
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(if s.len() % 2 == 1 {
        format!("0{s}")
    } else {
        s.to_string()
    })
    .unwrap()
}

fn h32(v: &Value) -> [u8; 32] {
    let b = hx(v);
    let mut w = [0u8; 32];
    w[32 - b.len()..].copy_from_slice(&b);
    w
}

/// Record as the light client would hold it for a pre-gloas (fulu) block: every field set.
fn record(f: &Value) -> HeaderRecord {
    HeaderRecord {
        chain_id: f["chain_id"].as_u64().unwrap(),
        slot: 1,
        beacon_root: [7; 32],
        exec_number: f["block_number"].as_u64().unwrap(),
        exec_hash: h32(&f["block_hash"]),
        exec_state_root: h32(&f["state_root"]),
        exec_receipts_root: h32(&f["receipts_root"]),
    }
}

/// Record as a gloas light-client header gives it: only the execution block hash.
fn gloas_record(f: &Value) -> HeaderRecord {
    HeaderRecord {
        exec_number: 0,
        exec_state_root: [0; 32],
        exec_receipts_root: [0; 32],
        ..record(f)
    }
}

fn claim(f: &Value) -> Claim {
    Claim::from_bytes(&hx(&f["claim"])).unwrap()
}

fn with_header(f: &Value) -> Vec<u8> {
    hx(&f["claim_with_header"])
}

const INSTANCE: AccountId = AccountId::new([9; 32]);
fn yes() -> Predicate {
    Predicate::All(vec![])
}

fn proof_mut(c: &mut Claim) -> &mut Vec<Vec<u8>> {
    match &mut c.proof {
        ClaimProof::Account { account_proof, .. } => account_proof,
        ClaimProof::Storage { storage_proof, .. } => storage_proof,
        ClaimProof::Log { receipt_proof, .. } => receipt_proof,
    }
}

fn run(f: &Value, c: &Claim, p: &Predicate) -> Result<attest_core::Statement, EsaError> {
    verify_claim(INSTANCE, &record(f), &c.to_bytes(), p)
}

// ---------- real proofs: values match what the RPC reported ----------

#[test]
fn mainnet_storage_slots_match_rpc() {
    for name in [
        "mainnet_storage_usdt_slot0.json",
        "mainnet_storage_usdt_slot1.json",
        "mainnet_storage_usdt_zero.json",
    ] {
        let f = load(name);
        let st = run(&f, &claim(&f), &yes()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(st.kind, ClaimKind::Storage);
        assert_eq!(st.value, h32(&f["expected"]).to_vec(), "{name}");
        assert_eq!(st.exec_number, 26_397_891);
    }
}

#[test]
fn mainnet_zero_slot_is_an_exclusion_proof() {
    let f = load("mainnet_storage_usdt_zero.json");
    let st = run(&f, &claim(&f), &yes()).unwrap();
    assert_eq!(st.value, vec![0u8; 32]);
}

#[test]
fn mainnet_accounts_match_rpc() {
    let f = load("mainnet_account_usdc.json");
    let st = run(&f, &claim(&f), &yes()).unwrap();
    let a = AccountValue::decode(&st.value).unwrap();
    let e = &f["expected"];
    assert_eq!(
        a.nonce,
        u64::from_str_radix(e["nonce"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    );
    assert_eq!(a.balance, h32(&e["balance"]));
    assert_eq!(a.storage_root, h32(&e["storage_hash"]));
    assert_eq!(a.code_hash, h32(&e["code_hash"]));

    let f = load("mainnet_account_absent.json");
    let st = run(&f, &claim(&f), &yes()).unwrap();
    assert_eq!(AccountValue::decode(&st.value).unwrap(), AccountValue::default());
}

fn check_log(name: &str) -> attest_core::Statement {
    let f = load(name);
    let st = run(&f, &claim(&f), &yes()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let log = LogValue::decode(&st.value).unwrap();
    let e = &f["expected"];
    assert_eq!(log.address.to_vec(), hx(&e["address"]), "{name}");
    let topics: Vec<[u8; 32]> = e["topics"].as_array().unwrap().iter().map(h32).collect();
    assert_eq!(log.topics, topics, "{name}");
    assert_eq!(log.data, hx(&e["data"]), "{name}");
    assert_eq!(log.encode(), st.value, "rlp(log) round trip");
    st
}

#[test]
fn mainnet_logs_match_rpc() {
    check_log("mainnet_log_usdt_tx1_log2.json");
    check_log("mainnet_log_usdc_tx3_log0.json");
    // tx index above 127 (two-byte rlp key), receipt with 255 logs.
    check_log("mainnet_log_tx374_log254.json");
}

#[test]
fn sepolia_proofs_match_rpc() {
    for name in ["sepolia_storage_usdc_slot0.json", "sepolia_storage_usdc_zero.json"] {
        let f = load(name);
        let st = run(&f, &claim(&f), &yes()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(st.value, h32(&f["expected"]).to_vec());
    }
    let f = load("sepolia_account_absent.json");
    run(&f, &claim(&f), &yes()).unwrap();
    check_log("sepolia_log_usdc_tx48_log2.json");
    check_log("sepolia_log_usdc_tx167_log2.json");
}

// ---------- gloas: record without roots, RLP execution header ----------

#[test]
fn gloas_record_needs_and_checks_the_exec_header() {
    for name in [
        "sepolia_storage_usdc_slot0.json",
        "sepolia_log_usdc_tx48_log2.json",
        "mainnet_log_usdt_tx1_log2.json",
    ] {
        let f = load(name);
        let g = gloas_record(&f);
        // With the header: same statement as the full record.
        let a = verify_claim(INSTANCE, &g, &with_header(&f), &yes()).unwrap();
        let b = run(&f, &claim(&f), &yes()).unwrap();
        assert_eq!(a, b, "{name}");
        // Without the header: malformed.
        assert_eq!(
            verify_claim(INSTANCE, &g, &claim(&f).to_bytes(), &yes()),
            Err(EsaError::E01Malformed)
        );
        // Header of another block: E15.
        let other = load(if name.starts_with("sepolia") {
            "mainnet_log_usdt_tx1_log2.json"
        } else {
            "sepolia_log_usdc_tx48_log2.json"
        });
        let mut c = claim(&f);
        c.exec_header = Some(hx(&other["exec_header_rlp"]));
        assert_eq!(
            verify_claim(INSTANCE, &g, &c.to_bytes(), &yes()),
            Err(EsaError::E15HeaderMismatch)
        );
        // One flipped byte in the header: E15.
        let mut h = hx(&f["exec_header_rlp"]);
        let n = h.len() / 2;
        h[n] ^= 1;
        c.exec_header = Some(h);
        assert_eq!(
            verify_claim(INSTANCE, &g, &c.to_bytes(), &yes()),
            Err(EsaError::E15HeaderMismatch)
        );
    }
}

#[test]
fn header_must_agree_with_a_full_record() {
    let f = load("mainnet_storage_usdt_slot0.json");
    let mut r = record(&f);
    // Correct header on a full record: fine.
    verify_claim(INSTANCE, &r, &with_header(&f), &yes()).unwrap();
    // Record root differs from the header's: E15.
    r.exec_state_root[0] ^= 1;
    assert_eq!(
        verify_claim(INSTANCE, &r, &with_header(&f), &yes()),
        Err(EsaError::E15HeaderMismatch)
    );
    let mut r = record(&f);
    r.exec_number += 1;
    assert_eq!(
        verify_claim(INSTANCE, &r, &with_header(&f), &yes()),
        Err(EsaError::E15HeaderMismatch)
    );
}

#[test]
fn w2_sepolia_exec_headers_hash_and_decode() {
    // The header files (fixtures/sepolia/exec_headers) are accepted by exec_roots for their own hash.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sepolia/exec_headers");
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("rlp") {
            continue;
        }
        let rlp = std::fs::read(&p).unwrap();
        let rec = HeaderRecord {
            chain_id: 11_155_111,
            exec_hash: keccak(&rlp),
            ..HeaderRecord::default()
        };
        let roots = attest_inclusion::exec_roots(&rec, Some(&rlp)).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert!(roots.number > 12_000_000);
        n += 1;
    }
    assert!(n > 0);
}

// ---------- tampered proofs ----------

#[test]
fn tampered_nodes_are_rejected() {
    for name in [
        "mainnet_storage_usdt_slot0.json",
        "mainnet_account_usdc.json",
        "mainnet_log_usdt_tx1_log2.json",
        "mainnet_log_tx374_log254.json",
        "sepolia_storage_usdc_slot0.json",
        "sepolia_log_usdc_tx48_log2.json",
    ] {
        let f = load(name);
        let good = claim(&f);
        let len = proof_mut(&mut good.clone()).len();
        for node in 0..len {
            for at in [0usize, 1, 40, usize::MAX] {
                let mut c = good.clone();
                let n = &mut proof_mut(&mut c)[node];
                let i = if at == usize::MAX {
                    n.len() - 1
                } else {
                    at.min(n.len() - 1)
                };
                n[i] ^= 0x01;
                assert_eq!(
                    run(&f, &c, &yes()),
                    Err(EsaError::E07InclusionInvalid),
                    "{name}: node {node} byte {i}"
                );
            }
        }
        // Truncated proof.
        let mut c = good.clone();
        proof_mut(&mut c).pop();
        assert_eq!(
            run(&f, &c, &yes()),
            Err(EsaError::E07InclusionInvalid),
            "{name}: truncated"
        );
        // Missing root node.
        let mut c = good.clone();
        proof_mut(&mut c).remove(0);
        assert_eq!(
            run(&f, &c, &yes()),
            Err(EsaError::E07InclusionInvalid),
            "{name}: no root"
        );
        // Empty proof.
        let mut c = good.clone();
        proof_mut(&mut c).clear();
        assert_eq!(run(&f, &c, &yes()), Err(EsaError::E07InclusionInvalid), "{name}: empty");
        // Wrong root in the record.
        let mut r = record(&f);
        r.exec_state_root[5] ^= 1;
        r.exec_receipts_root[5] ^= 1;
        assert_eq!(
            verify_claim(INSTANCE, &r, &good.to_bytes(), &yes()),
            Err(EsaError::E07InclusionInvalid)
        );
    }
}

#[test]
fn proof_for_another_key_is_rejected() {
    // Slot 0's proof presented for slot 1, and a receipt proof presented for another tx index.
    let f = load("mainnet_storage_usdt_slot0.json");
    let mut c = claim(&f);
    if let ClaimProof::Storage { slot_key, .. } = &mut c.proof {
        slot_key[31] = 1;
    }
    assert_eq!(run(&f, &c, &yes()), Err(EsaError::E07InclusionInvalid));

    let f = load("mainnet_log_usdt_tx1_log2.json");
    for wrong in [0u64, 2, 129] {
        let mut c = claim(&f);
        if let ClaimProof::Log { tx_index, .. } = &mut c.proof {
            *tx_index = wrong;
        }
        assert_eq!(run(&f, &c, &yes()), Err(EsaError::E07InclusionInvalid), "tx {wrong}");
    }
    // Log index past the receipt's logs.
    let mut c = claim(&f);
    if let ClaimProof::Log { log_index, .. } = &mut c.proof {
        *log_index = 10_000;
    }
    assert_eq!(run(&f, &c, &yes()), Err(EsaError::E07InclusionInvalid));
}

#[test]
fn account_proof_swapped_between_addresses_is_rejected() {
    let usdc = load("mainnet_account_usdc.json");
    let absent = load("mainnet_account_absent.json");
    let mut c = claim(&usdc);
    let other = claim(&absent);
    if let (ClaimProof::Account { account_proof, .. }, ClaimProof::Account { account_proof: p2, .. }) =
        (&mut c.proof, &other.proof)
    {
        *account_proof = p2.clone();
    }
    assert_eq!(run(&usdc, &c, &yes()), Err(EsaError::E07InclusionInvalid));
}

#[test]
fn malformed_claim_bytes() {
    let f = load("mainnet_storage_usdt_slot0.json");
    let mut b = claim(&f).to_bytes();
    b.push(0);
    assert_eq!(
        verify_claim(INSTANCE, &record(&f), &b, &yes()),
        Err(EsaError::E01Malformed)
    );
    assert_eq!(
        verify_claim(INSTANCE, &record(&f), &[], &yes()),
        Err(EsaError::E01Malformed)
    );
    assert_eq!(
        verify_claim(INSTANCE, &record(&f), &[3, 0, 0], &yes()),
        Err(EsaError::E01Malformed)
    );
}

#[test]
fn low_level_functions_agree() {
    let f = load("mainnet_storage_usdt_slot0.json");
    let r = record(&f);
    if let ClaimProof::Storage {
        address,
        slot_key,
        account_proof,
        storage_proof,
    } = claim(&f).proof
    {
        let a = verify_account(&r.exec_state_root, &address, &account_proof).unwrap();
        let w = verify_storage(&a.storage_root, &slot_key, &storage_proof).unwrap();
        assert_eq!(w, h32(&f["expected"]));
    } else {
        panic!("storage fixture");
    }
    let f = load("mainnet_log_usdc_tx3_log0.json");
    if let ClaimProof::Log {
        tx_index,
        log_index,
        receipt_proof,
    } = claim(&f).proof
    {
        let log = verify_log(&record(&f).exec_receipts_root, tx_index, &receipt_proof, log_index).unwrap();
        assert_eq!(log.address.to_vec(), hx(&f["expected"]["address"]));
    }
}

// ---------- statement layout and ids ----------

#[test]
fn statement_fields_and_ids() {
    let f = load("mainnet_log_usdt_tx1_log2.json");
    let st = run(&f, &claim(&f), &yes()).unwrap();
    assert_eq!(st.instance, INSTANCE);
    assert_eq!(st.chain_id, 1);
    assert_eq!(st.predicate_hash, yes().hash());
    assert_eq!(&st.subject[..20], hx(&f["expected"]["address"]).as_slice());
    assert_eq!(st.subject[20..28], 1u64.to_le_bytes());
    assert_eq!(st.subject[28..36], 2u64.to_le_bytes());
    assert_eq!(
        st.id,
        attest_core::statement_id_log(1, &h32(&f["block_hash"]), 1, 2, &keccak(&st.value))
    );
    assert_eq!(st.id, h32(&f["statement_id"]));

    // Same storage fact at another height (another record) has the same id (F6).
    let f = load("mainnet_storage_usdt_slot0.json");
    let c = claim(&f);
    let v1 = verify_claim_value(&record(&f), &c).unwrap();
    let mut r2 = record(&f);
    r2.exec_number += 100;
    r2.exec_hash = [1; 32];
    let v2 = verify_claim_value(&r2, &c).unwrap();
    assert_eq!(v1.id, v2.id);
    assert_eq!(v1.subject.len(), 52);
}

// ---------- predicates on real values ----------

fn u(n: u128) -> PredicateValue {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&n.to_be_bytes());
    PredicateValue::U256(w)
}

#[test]
fn predicates_on_real_log() {
    let f = load("mainnet_log_usdt_tx1_log2.json");
    let c = claim(&f);
    let e = &f["expected"];
    let topics: Vec<[u8; 32]> = e["topics"].as_array().unwrap().iter().map(h32).collect();
    let amount = h32(&e["data"]);
    let p = Predicate::All(vec![
        Predicate::LogTopicEq { i: 0, v: topics[0] },
        Predicate::LogTopicEq { i: 2, v: topics[2] },
        Predicate::LogDataWordEq { offset: 0, v: amount },
    ]);
    run(&f, &c, &p).unwrap();
    // False.
    let mut wrong = topics[2];
    wrong[31] ^= 1;
    assert_eq!(
        run(&f, &c, &Predicate::LogTopicEq { i: 2, v: wrong }),
        Err(EsaError::E08PredicateFalse)
    );
    // Not covered: topic the log does not carry, data word out of range, word predicate on a log.
    assert_eq!(
        run(&f, &c, &Predicate::LogTopicEq { i: 3, v: wrong }),
        Err(EsaError::E10PredicateNotCovered)
    );
    assert_eq!(
        run(&f, &c, &Predicate::LogDataWordEq { offset: 1, v: amount }),
        Err(EsaError::E10PredicateNotCovered)
    );
    assert_eq!(run(&f, &c, &Predicate::Gt(u(0))), Err(EsaError::E10PredicateNotCovered));
}

#[test]
fn predicates_on_real_storage_and_account() {
    let f = load("mainnet_storage_usdt_slot1.json");
    let c = claim(&f);
    let w = h32(&f["expected"]);
    run(&f, &c, &Predicate::Eq(PredicateValue::U256(w))).unwrap();
    run(&f, &c, &Predicate::Eq(PredicateValue::Bytes32(w))).unwrap();
    run(&f, &c, &Predicate::Ge(PredicateValue::U256(w))).unwrap();
    run(&f, &c, &Predicate::Gt(u(1))).unwrap();
    assert_eq!(
        run(&f, &c, &Predicate::Lt(PredicateValue::U256(w))),
        Err(EsaError::E08PredicateFalse)
    );
    assert_eq!(
        run(&f, &c, &Predicate::Ne(PredicateValue::U256(w))),
        Err(EsaError::E08PredicateFalse)
    );
    assert_eq!(
        run(&f, &c, &Predicate::Lt(PredicateValue::Bytes32(w))),
        Err(EsaError::E10PredicateNotCovered)
    );
    assert_eq!(
        run(&f, &c, &Predicate::LogTopicEq { i: 0, v: w }),
        Err(EsaError::E10PredicateNotCovered)
    );

    // Account: word predicates compare the balance.
    let f = load("mainnet_account_absent.json");
    let c = claim(&f);
    run(&f, &c, &Predicate::Eq(u(0))).unwrap();
    assert_eq!(run(&f, &c, &Predicate::Gt(u(0))), Err(EsaError::E08PredicateFalse));
}

#[test]
fn predicate_shape_limits() {
    let f = load("mainnet_storage_usdt_slot1.json");
    let c = claim(&f);
    let mut p = Predicate::Gt(u(0));
    for _ in 0..8 {
        p = Predicate::All(vec![p]);
    }
    run(&f, &c, &p).unwrap();
    let deep = Predicate::All(vec![p]);
    assert_eq!(run(&f, &c, &deep), Err(EsaError::E01Malformed));
    let wide = Predicate::All(vec![Predicate::Gt(u(0)); 65]);
    assert_eq!(run(&f, &c, &wide), Err(EsaError::E01Malformed));
    // Shape is checked before evaluation: a false first term does not hide an oversized tree.
    let mut terms = vec![Predicate::Lt(u(0))];
    terms.extend(vec![Predicate::Gt(u(0)); 64]);
    assert_eq!(run(&f, &c, &Predicate::All(terms)), Err(EsaError::E01Malformed));
}
