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
