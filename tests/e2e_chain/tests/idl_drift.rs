//! Drift test for the hand-kept SPEL-shaped IDLs `idl/attest.json` and `idl/lc_verify.json` (021 U4).
//!
//! The Rust side is listed by exhaustive destructuring (no `..`): adding, removing or renaming a message variant or
//! field breaks compilation here, and a changed order or name in the IDL fails the test. Error codes are compared
//! with `EsaError::ALL`.

use attest_core::{AttestMessage, EsaError, LcVerifyMessage};

fn idl(name: &str) -> serde_json::Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../idl")
        .join(name);
    serde_json::from_slice(&std::fs::read(&p).expect("idl file")).expect("idl json")
}

fn snake(s: &str) -> String {
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                o.push('_');
            }
            o.extend(c.to_lowercase());
        } else {
            o.push(c);
        }
    }
    o
}

/// (variant name, field names) in declaration order, from an exhaustive match.
fn attest_variants() -> Vec<(&'static str, Vec<&'static str>)> {
    // One value per variant, in borsh discriminant order; the match lists every field.
    let names = |m: &AttestMessage| -> (&'static str, Vec<&'static str>) {
        match m {
            AttestMessage::Init {
                config: _,
                bootstrap: _,
            } => ("Init", vec!["config", "bootstrap"]),
            AttestMessage::Apply {
                pre_store: _,
                post_store: _,
                head: _,
                record: _,
            } => ("Apply", vec!["pre_store", "post_store", "head", "record"]),
            AttestMessage::WriteRecord { record: _ } => ("WriteRecord", vec!["record"]),
            AttestMessage::CheckHeader { record_hash: _ } => ("CheckHeader", vec!["record_hash"]),
            AttestMessage::Attest {
                instance: _,
                record: _,
                claim: _,
                predicate: _,
                deliver_to: _,
            } => ("Attest", vec!["instance", "record", "claim", "predicate", "deliver_to"]),
        }
    };
    let id = attest_core::AccountId::new([0; 32]);
    let samples = [
        AttestMessage::Init {
            config: attest_core::InstanceConfig {
                chain_id: 0,
                genesis_validators_root: [0; 32],
                genesis_time: 0,
                forks: vec![],
                checkpoint_root: [0; 32],
                checkpoint_slot: 0,
                verifiers: vec![],
                min_participants: 0,
            },
            bootstrap: vec![],
        },
        AttestMessage::Apply {
            pre_store: [0; 32],
            post_store: [0; 32],
            head: Default::default(),
            record: Default::default(),
        },
        AttestMessage::WriteRecord {
            record: Default::default(),
        },
        AttestMessage::CheckHeader { record_hash: [0; 32] },
        AttestMessage::Attest {
            instance: id,
            record: Default::default(),
            claim: vec![],
            predicate: attest_core::Predicate::All(vec![]),
            deliver_to: attest_core::Actor::new(id, id),
        },
    ];
    samples
        .iter()
        .enumerate()
        .map(|(i, m)| {
            assert_eq!(borsh::to_vec(m).unwrap()[0] as usize, i, "borsh discriminant order");
            names(m)
        })
        .collect()
}

#[test]
fn attest_idl_matches_messages() {
    let doc = idl("attest.json");
    let ins = doc["instructions"].as_array().unwrap();
    let rust = attest_variants();
    assert_eq!(ins.len(), rust.len(), "instruction count");
    for (j, (variant, fields)) in ins.iter().zip(&rust) {
        assert_eq!(j["name"].as_str().unwrap(), snake(variant));
        let args: Vec<&str> = j["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(&args, fields, "args of {variant}");
    }
    assert_eq!(doc["instruction_type"], "attest_core::AttestMessage");
    let errors: Vec<(u64, String)> = doc["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["code"].as_u64().unwrap(), e["name"].as_str().unwrap().to_owned()))
        .collect();
    let want: Vec<(u64, String)> = EsaError::ALL.iter().map(|e| (*e as u64, e.name().to_owned())).collect();
    assert_eq!(errors, want);
}

#[test]
fn lc_verify_idl_matches_message() {
    let doc = idl("lc_verify.json");
    let ins = doc["instructions"].as_array().unwrap();
    assert_eq!(ins.len(), 1);
    let m = LcVerifyMessage::Verify {
        attest_program: attest_core::AccountId::new([0; 32]),
        instance: attest_core::AccountId::new([0; 32]),
        config: attest_core::InstanceConfig {
            chain_id: 0,
            genesis_validators_root: [0; 32],
            genesis_time: 0,
            forks: vec![],
            checkpoint_root: [0; 32],
            checkpoint_slot: 0,
            verifiers: vec![],
            min_participants: 0,
        },
        store_witness: vec![],
        update_ssz: vec![],
    };
    let LcVerifyMessage::Verify {
        attest_program: _,
        instance: _,
        config: _,
        store_witness: _,
        update_ssz: _,
    } = &m;
    let args: Vec<&str> = ins[0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(ins[0]["name"], "verify");
    assert_eq!(
        args,
        ["attest_program", "instance", "config", "store_witness", "update_ssz"]
    );
}

#[cfg(feature = "v3")]
#[test]
fn lc_verify_v3_idl_matches_messages() {
    use attest_core::{LcVerifyV3Message as M, lc_v3::StoreWitnessV3};
    let doc = idl("lc_verify_v3.json");
    assert_eq!(doc["instruction_type"], "attest_core::LcVerifyV3Message");
    let id = attest_core::AccountId::new([0; 32]);
    let config = attest_core::InstanceConfig {
        chain_id: 0,
        genesis_validators_root: [0; 32],
        genesis_time: 0,
        forks: vec![],
        checkpoint_root: [0; 32],
        checkpoint_slot: 0,
        verifiers: vec![],
        min_participants: 0,
    };
    let witness = StoreWitnessV3 {
        finalized: Default::default(),
        committee_ssz: vec![],
        signing: attest_core::lc_v3::CommitteeSlot::Current,
        other_root: [0; 32],
        previous_max_active_participants: 0,
        current_max_active_participants: 0,
    };
    let names = |m: &M| -> (&'static str, Vec<&'static str>) {
        match m {
            M::Begin {
                attest_program: _,
                instance: _,
                config: _,
                witness: _,
                update_ssz: _,
                absent_keys: _,
            } => (
                "Begin",
                vec![
                    "attest_program",
                    "instance",
                    "config",
                    "witness",
                    "update_ssz",
                    "absent_keys",
                ],
            ),
            M::Miller => ("Miller", vec![]),
            M::Finish => ("Finish", vec![]),
            M::VerifyOnce {
                attest_program: _,
                instance: _,
                config: _,
                witness: _,
                update_ssz: _,
                absent_keys: _,
            } => (
                "VerifyOnce",
                vec![
                    "attest_program",
                    "instance",
                    "config",
                    "witness",
                    "update_ssz",
                    "absent_keys",
                ],
            ),
            M::BeginPrepare {
                attest_program: _,
                instance: _,
                config: _,
                witness: _,
                update_ssz: _,
            } => (
                "BeginPrepare",
                vec!["attest_program", "instance", "config", "witness", "update_ssz"],
            ),
            M::BeginKeys {
                committee_ssz: _,
                absent_keys: _,
            } => ("BeginKeys", vec!["committee_ssz", "absent_keys"]),
        }
    };
    let begin = |once: bool| {
        let (a, i, c, w) = (id, id, config.clone(), witness.clone());
        if once {
            M::VerifyOnce {
                attest_program: a,
                instance: i,
                config: c,
                witness: w,
                update_ssz: vec![],
                absent_keys: vec![],
            }
        } else {
            M::Begin {
                attest_program: a,
                instance: i,
                config: c,
                witness: w,
                update_ssz: vec![],
                absent_keys: vec![],
            }
        }
    };
    let samples = [
        begin(false),
        M::Miller,
        M::Finish,
        begin(true),
        M::BeginPrepare {
            attest_program: id,
            instance: id,
            config: config.clone(),
            witness: witness.clone(),
            update_ssz: vec![],
        },
        M::BeginKeys {
            committee_ssz: vec![],
            absent_keys: vec![],
        },
    ];
    let ins = doc["instructions"].as_array().unwrap();
    assert_eq!(ins.len(), samples.len(), "instruction count");
    for (i, (m, j)) in samples.iter().zip(ins).enumerate() {
        assert_eq!(borsh::to_vec(m).unwrap()[0] as usize, i, "borsh discriminant order");
        let (variant, fields) = names(m);
        assert_eq!(j["name"].as_str().unwrap(), snake(variant));
        let args: Vec<&str> = j["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(args, fields, "args of {variant}");
    }
    // StoreWitnessV3 field order.
    let t = doc["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "StoreWitnessV3")
        .expect("StoreWitnessV3 type");
    let StoreWitnessV3 {
        finalized: _,
        committee_ssz: _,
        signing: _,
        other_root: _,
        previous_max_active_participants: _,
        current_max_active_participants: _,
    } = witness;
    let fields: Vec<&str> = t["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        [
            "finalized",
            "committee_ssz",
            "signing",
            "other_root",
            "previous_max_active_participants",
            "current_max_active_participants"
        ]
    );
}
