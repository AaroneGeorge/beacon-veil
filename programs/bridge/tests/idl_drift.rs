//! Drift test for the hand-kept SPEL-shaped IDL `idl/bridge.json` (022 U4). Exhaustive destructuring: a changed
//! message variant or field breaks compilation here; a changed order or name in the IDL fails the test.

use attest_core::AccountId;
use bridge_program::lez::{BridgeConfig, BridgeError, BridgeMessage, BurnEvent, ClaimArgs};

fn names(m: &BridgeMessage) -> (&'static str, Vec<&'static str>) {
    match m {
        BridgeMessage::Claim(_) => ("claim", vec!["args"]),
        BridgeMessage::CheckRoot {
            attest_program: _,
            attest_instance: _,
            chain_id: _,
            vault: _,
            root: _,
        } => (
            "check_root",
            vec!["attest_program", "attest_instance", "chain_id", "vault", "root"],
        ),
        BridgeMessage::Nullify {
            instance: _,
            nullifier: _,
        } => ("nullify", vec!["instance", "nullifier"]),
        BridgeMessage::Credit {
            instance: _,
            token: _,
            amount: _,
        } => ("credit", vec!["instance", "token", "amount"]),
        BridgeMessage::Burn {
            config: _,
            token: _,
            denom: _,
            note: _,
            memo: _,
        } => ("burn", vec!["config", "token", "denom", "note", "memo"]),
        BridgeMessage::Append {
            instance: _,
            token: _,
            denom: _,
            note: _,
            memo: _,
        } => ("append", vec!["instance", "token", "denom", "note", "memo"]),
        BridgeMessage::RecordLeaf(_) => ("record_leaf", vec!["event"]),
    }
}

#[test]
fn bridge_idl_matches_messages() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../idl/bridge.json");
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
    let id = AccountId::new([0; 32]);
    let config = BridgeConfig {
        chain_id: 0,
        vault: [0; 20],
        attest_program: id,
        attest_instance: id,
        tokens: vec![],
        treasury: None,
    };
    let event = BurnEvent {
        instance: id,
        index: 0,
        token: [0; 20],
        denom: 0,
        note: [0; 32],
        memo: [0; 36],
        root: [0; 32],
    };
    let samples = [
        BridgeMessage::Claim(Box::new(ClaimArgs {
            config: config.clone(),
            token: [0; 20],
            denom: 0,
            secret: [0; 32],
            dest: id,
            leaf_index: 0,
            path: [[0; 32]; 32],
            root: [0; 32],
            attest: None,
        })),
        BridgeMessage::CheckRoot {
            attest_program: id,
            attest_instance: id,
            chain_id: 0,
            vault: [0; 20],
            root: [0; 32],
        },
        BridgeMessage::Nullify {
            instance: id,
            nullifier: [0; 32],
        },
        BridgeMessage::Credit {
            instance: id,
            token: [0; 20],
            amount: 0,
        },
        BridgeMessage::Burn {
            config,
            token: [0; 20],
            denom: 0,
            note: [0; 32],
            memo: [0; 36],
        },
        BridgeMessage::Append {
            instance: id,
            token: [0; 20],
            denom: 0,
            note: [0; 32],
            memo: [0; 36],
        },
        BridgeMessage::RecordLeaf(event),
    ];
    let ins = doc["instructions"].as_array().unwrap();
    assert_eq!(ins.len(), samples.len());
    for (i, (m, j)) in samples.iter().zip(ins).enumerate() {
        assert_eq!(borsh::to_vec(m).unwrap()[0] as usize, i, "borsh discriminant order");
        let (name, fields) = names(m);
        assert_eq!(j["name"].as_str().unwrap(), name);
        let args: Vec<&str> = j["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(args, fields, "args of {name}");
    }
    assert_eq!(doc["instruction_type"], "bridge_program::lez::BridgeMessage");
    let errors: Vec<u64> = doc["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_u64().unwrap())
        .collect();
    let want: Vec<u64> = BridgeError::ALL.iter().map(|e| *e as u64).collect();
    assert_eq!(errors, want);
}
