//! Usage: lez_chain_synth SEQ_DEV_BLOCK_DIR FIXTURE_DIR
//!
//! Writes the committed fixture range `fixtures/lez-chain/` with LEZ's own types:
//! - `seq-dev/2581..2591.borsh`: real seq-dev blocks (dev mode, 2026-11-12) holding burn 0 (block 2586) and burn 1
//!   (block 2588), copied byte for byte;
//! - `synthetic/2586..2591.borsh`: a SYNTHETIC chain forked after real block 2585. Block 2586 is the real burn 0
//!   block with the `Append` note and memo of its boundary replaced by ones from a known test secret (its proof no
//!   longer matches; no node would accept it); block 2590 gets one extra public transaction, a `Burn` root call to the
//!   bridge with the real seq-dev bridge config, signed by a test key. Every block is re-hashed and re-linked.
//! - `manifest.json`: hashes, burns, and the witnesses of the two synthetic burns (test secrets only).
//!
//! Deterministic except the test key's signature (LEZ signs with fresh randomness), which changes blocks 2590 and
//! 2591 and their hashes on every run; the committed bytes are the fixture.

use std::{collections::BTreeMap, fs, path::Path};

use bridge_program::{
    hash::{burn_note, memo_seal, sha},
    lez::{BridgeConfig, BridgeMessage, instance_id},
};
use common::{
    block::{Block, HashableBlockData},
    transaction::LeeTransaction,
};
use lee::{
    FeeDeclaration, PrivateKey, PublicKey,
    public_transaction::{Message, PublicTransaction, WitnessSet},
};
use lee_core::{
    account::{AccountId, Actor, Nonce},
    execution_state::BoundaryStep,
};
use serde_json::json;

const BRIDGE: &str = "7ahb3h5n3MTeq2JtYhLbQL75gRpL7xNAmV54SZjvJQ6y";
/// `evidence/bridge/raw-v3/config.json` `config_borsh`: the seq-dev bridge config of instance `BTpCBokr...`.
const CONFIG_BORSH: &str = "a736aa0000000000380be74d9f3dfebaafc5f911cd6a45bfbee3ad27b32000ac5378e05f6892e516143997d5280f4c89481af2d4b587e95757e8201e25daad8ccf02189ff9e6509bcf2efe50c57a97dc0400c9f395fe0e15b2b33296020000000000000000000000000000000000000000000000020000000080c6a47e8d030000000000000000000000c16ff286230000000000000000000000000000000000000000000000000000000000000000000000000000000000001b4d55b24a5ebf4e4e0d9cbf964e6170169e2b0200000000e1f50500000000000000000000000000ca9a3b000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
const REAL: std::ops::RangeInclusive<u64> = 2581..=2591;
const FORK_FROM: u64 = 2586;
const PUBLIC_BURN_BLOCK: u64 = 2590;

fn h(b: &[u8]) -> String {
    hex::encode(b)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() == 3, "usage: lez_chain_synth SEQ_DEV_BLOCK_DIR FIXTURE_DIR");
    let (src, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let bridge: AccountId = BRIDGE.parse().expect("bridge id");
    let config: BridgeConfig = borsh::from_slice(&hex::decode(CONFIG_BORSH).unwrap()).expect("config");
    let instance = instance_id(&bridge, &config);
    let read = |i: u64| fs::read(src.join(format!("{i}.borsh"))).expect("block");

    // Real range, copied.
    fs::create_dir_all(out.join("seq-dev")).unwrap();
    fs::create_dir_all(out.join("synthetic")).unwrap();
    let mut real = Vec::new();
    for i in REAL {
        let raw = read(i);
        let b: Block = borsh::from_slice(&raw).unwrap();
        assert_eq!(b.recompute_hash(), b.header.hash);
        fs::write(out.join(format!("seq-dev/{i}.borsh")), &raw).unwrap();
        real.push(json!({"id": i, "hash": h(&b.header.hash.0), "bytes": raw.len(), "transactions": b.body.transactions.len()}));
    }

    // Synthetic private burn: real burn 0, Append note and memo from a test secret.
    let priv_secret = sha(&[b"fixture/lez-chain/private-burn"]);
    let priv_recipient = [0x52_u8; 20];
    let priv_fee = 10_000_000_000_000_u128;
    // Synthetic public burn: test key, real config, ERC-20 test token of the config.
    let pub_secret = sha(&[b"fixture/lez-chain/public-burn"]);
    let pub_recipient = [0x77_u8; 20];
    let pub_fee = 0_u128;
    let pub_token = config.tokens[1].token;
    let pub_denom = config.tokens[1].denoms[0];

    let mut prev = {
        let b: Block = borsh::from_slice(&read(FORK_FROM - 1)).unwrap();
        b.header.hash
    };
    let mut synth = Vec::new();
    let mut burns = Vec::new();
    for i in FORK_FROM..=*REAL.end() {
        let mut d = HashableBlockData::from(borsh::from_slice::<Block>(&read(i)).unwrap());
        d.prev_block_hash = prev;
        if i == FORK_FROM {
            let LeeTransaction::PrivacyPreserving(pp) = &mut d.transactions[0] else {
                panic!("burn 0 is private")
            };
            let BoundaryStep::PrivateToPublic(del) = &mut pp.message.execution.boundary[0] else {
                panic!("step 0")
            };
            let Ok(BridgeMessage::Append {
                instance: ai,
                token,
                denom,
                ..
            }) = borsh::from_slice(&del.envelope.message)
            else {
                panic!("Append")
            };
            assert_eq!(ai, instance);
            let note = burn_note(&priv_secret, &priv_recipient, priv_fee);
            del.envelope.message = borsh::to_vec(&BridgeMessage::Append {
                instance: ai,
                token,
                denom,
                note,
                memo: memo_seal(&priv_secret, &priv_recipient, priv_fee),
            })
            .unwrap();
            burns.push(json!({"name": "synthetic private burn", "route": "private", "block": i, "tx": 0,
                "witness": {"burn_secret": h(&priv_secret), "recipient": h(&priv_recipient), "fee": priv_fee.to_string(),
                    "token": h(&token), "denom": denom.to_string()}, "note": h(&note)}));
        }
        if i == PUBLIC_BURN_BLOCK {
            let key = PrivateKey::try_new(sha(&[b"fixture/lez-chain/public-holder-key"])).expect("key");
            let holder = AccountId::from(&PublicKey::new_from_private_key(&key));
            let actor = Actor::new(holder, bridge);
            let note = burn_note(&pub_secret, &pub_recipient, pub_fee);
            let msg = borsh::to_vec(&BridgeMessage::Burn {
                config: config.clone(),
                token: pub_token,
                denom: pub_denom,
                note,
                memo: memo_seal(&pub_secret, &pub_recipient, pub_fee),
            })
            .unwrap();
            let fee = FeeDeclaration {
                payer: holder,
                gas_limit: 1_000_000,
                tip: 0,
                max_fee: 1_000_000,
            };
            let m = Message::new(
                actor,
                msg,
                [actor],
                BTreeMap::from([(holder, Nonce(0))]),
                Some(fee),
                vec![],
            );
            let ws = WitnessSet::for_message(&m, &[&key]);
            d.transactions
                .push(LeeTransaction::Public(PublicTransaction::new(m, ws)));
            burns.push(
                json!({"name": "synthetic public burn", "route": "public", "block": i, "tx": d.transactions.len() - 1,
                "witness": {"burn_secret": h(&pub_secret), "recipient": h(&pub_recipient), "fee": pub_fee.to_string(),
                    "token": h(&pub_token), "denom": pub_denom.to_string()}, "note": h(&note)}),
            );
        }
        let block = d.into_pending_block();
        prev = block.header.hash;
        let raw = borsh::to_vec(&block).unwrap();
        fs::write(out.join(format!("synthetic/{i}.borsh")), &raw).unwrap();
        synth.push(json!({"id": i, "hash": h(&block.header.hash.0), "bytes": raw.len(), "transactions": block.body.transactions.len()}));
    }

    let manifest = json!({
        "about": "LEZ v0.4.0-rc3 block fixtures for crates/lez_chain. Written by tools/lez_chain_probe/src/bin/synth.rs.",
        "bridge_program": BRIDGE, "bridge_program_hex": h(bridge.value()),
        "instance": instance.to_string(), "instance_hex": h(instance.value()),
        "config_borsh": CONFIG_BORSH,
        "seq_dev": {
            "about": "REAL blocks of seq-dev (dev mode, RISC0_DEV_MODE set, 2026-11-12), read from a copy of its store. \
                      Burn 0 and burn 1 are the burns released on vault v3 (evidence/bridge/groth16-release-v3). \
                      Their burn secrets are not here.",
            "blocks": real,
            "burns": [
                {"name": "burn 0", "route": "private", "block": 2586, "tx": 0,
                 "tx_hash": "fa5e518963c6df641f6e42432f41fc0a20fbcea499f2c68c10619836006b3426",
                 "note": "a78d5c17f272b7644439d956aed272ab8058acc0c354a7f9cb7c143c6ac78d85",
                 "token": h(&[0; 20]), "denom": "1000000000000000"},
                {"name": "burn 1", "route": "private", "block": 2588, "tx": 0,
                 "tx_hash": "d5a06127811fe2f478b1a560c53c7a2d03ecd8948ff08b2e016d3774b71e59c8",
                 "note": "34a49463d904527cf9ddf685ee6e94efa97b8596dbeffef8f83f3b0239aac090",
                 "token": "001b4d55b24a5ebf4e4e0d9cbf964e6170169e2b", "denom": "100000000"}
            ]
        },
        "synthetic": {
            "about": "SYNTHETIC chain forked after real seq-dev block 2585 (pinned hash = seq-dev block 2585). Block 2586 \
                      is real burn 0 with its Append note and memo replaced (its proof no longer matches); block 2590 \
                      has one added public Burn transaction signed by a test key. Re-hashed and re-linked with LEZ's \
                      own types. Test secrets only; no node would accept these blocks.",
            "pinned": {"id": FORK_FROM - 1, "from": "seq-dev"},
            "blocks": synth,
            "burns": burns
        }
    });
    fs::write(
        out.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    )
    .unwrap();
    println!("{}", out.display());
}
