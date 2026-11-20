//! The statement of guest `burn_chain` on the synthetic fixture chain (fixtures/lez-chain/synthetic, pinned at real
//! seq-dev block 2585): the private and the public synthetic burn prove; each wrong witness is refused.

use std::path::PathBuf;

use lez_chain::{
    ChainError, block_hash,
    bridge_hash::{burn_note, burn_nullifier},
    statement::{CHAIN_JOURNAL_LEN, ChainJournal, ChainWitness, StatementError, prove_burn_chain},
};
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lez-chain")
}

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Witness of the synthetic burn `n` (0 private in block 2586, 1 public in block 2590), tip 2591.
fn witness(n: usize) -> ChainWitness {
    let m: Value = serde_json::from_slice(&std::fs::read(dir().join("manifest.json")).unwrap()).unwrap();
    let burn = &m["synthetic"]["burns"][n];
    let w = &burn["witness"];
    let s = |v: &Value| v.as_str().unwrap().to_owned();
    let read = |set: &str, i: u64| std::fs::read(dir().join(format!("{set}/{i}.borsh"))).unwrap();
    ChainWitness {
        burn_secret: hx(&s(&w["burn_secret"])).try_into().unwrap(),
        recipient: hx(&s(&w["recipient"])).try_into().unwrap(),
        fee: s(&w["fee"]).parse().unwrap(),
        token: hx(&s(&w["token"])).try_into().unwrap(),
        denom: s(&w["denom"]).parse().unwrap(),
        chain_id: 11_155_111,
        vault: hx("380be74d9f3dfebaafc5f911cd6a45bfbee3ad27").try_into().unwrap(),
        lez_program: hx(&s(&m["bridge_program_hex"])).try_into().unwrap(),
        lez_instance: hx(&s(&m["instance_hex"])).try_into().unwrap(),
        pinned_id: 2585,
        pinned_hash: block_hash(&read("seq-dev", 2585)).unwrap(),
        blocks: (2586..=2591).map(|i| read("synthetic", i)).collect(),
        burn_block_id: burn["block"].as_u64().unwrap(),
        burn_tx: u32::try_from(burn["tx"].as_u64().unwrap()).unwrap(),
    }
}

#[test]
fn synthetic_private_and_public_burns_prove() {
    for n in 0..2 {
        let w = witness(n);
        let j = prove_burn_chain(&w).unwrap();
        assert_eq!((j.pinned_id, j.tip_id, j.burn_block_id), (2585, 2591, w.burn_block_id));
        assert_eq!(j.tip_hash, block_hash(w.blocks.last().unwrap()).unwrap());
        assert_eq!(j.nullifier, burn_nullifier(&w.burn_secret));
        let bytes = j.encode();
        assert_eq!(bytes.len(), CHAIN_JOURNAL_LEN);
        assert_eq!(bytes[31], 2, "version word");
        assert_eq!(ChainJournal::decode(&bytes), Some(j));
        // A shorter chain to the burn block itself is also a valid statement, with that block as tip.
        let mut short = w.clone();
        short.blocks.truncate(usize::try_from(w.burn_block_id - 2585).unwrap());
        assert_eq!(prove_burn_chain(&short).unwrap().tip_id, w.burn_block_id);
    }
}

#[test]
fn wrong_witnesses_are_refused() {
    let not_a_burn = Err(StatementError::Chain(ChainError::NotABurn));
    for n in 0..2 {
        let base = witness(n);
        let check = |f: &dyn Fn(&mut ChainWitness), want: Result<(), StatementError>| {
            let mut w = base.clone();
            f(&mut w);
            assert_eq!(prove_burn_chain(&w).map(|_| ()), want);
        };
        check(&|w| w.burn_secret[0] ^= 1, not_a_burn);
        check(&|w| w.recipient[0] ^= 1, not_a_burn);
        check(&|w| w.fee += 1, not_a_burn);
        check(&|w| w.lez_instance[0] ^= 1, not_a_burn);
        check(&|w| w.lez_program[0] ^= 1, not_a_burn);
        check(&|w| w.token[19] ^= 1, Err(StatementError::TokenOrDenom));
        check(&|w| w.denom += 1, Err(StatementError::TokenOrDenom));
        check(&|w| w.fee = w.denom, Err(StatementError::FeeNotBelowDenom));
        check(
            &|w| w.burn_tx += 1,
            Err(StatementError::Chain(if n == 0 {
                ChainError::NotABurn
            } else {
                ChainError::NoSuchTransaction
            })),
        );
        check(&|w| w.burn_block_id = 2592, Err(StatementError::BurnBlockOutOfRange));
        check(&|w| w.burn_block_id = 2585, Err(StatementError::BurnBlockOutOfRange));
        // The tip before the burn block.
        check(
            &|w| w.blocks.truncate(usize::try_from(w.burn_block_id - 2586).unwrap()),
            Err(StatementError::BurnBlockOutOfRange),
        );
        check(
            &|w| w.pinned_hash[0] ^= 1,
            Err(StatementError::Chain(ChainError::BrokenLink { block_id: 2586 })),
        );
        check(
            &|w| w.pinned_id += 1,
            Err(StatementError::Chain(ChainError::IdGap {
                expected: 2587,
                got: 2586,
            })),
        );
        check(
            &|w| {
                w.blocks.remove(2);
            },
            Err(StatementError::Chain(ChainError::IdGap {
                expected: 2588,
                got: 2589,
            })),
        );
        // The real burn 0 note in the synthetic block (replaced there).
        check(&|w| w.burn_block_id = 2588, not_a_burn);
    }
    // The note binds recipient and fee.
    let w = witness(0);
    assert_ne!(
        burn_note(&w.burn_secret, &w.recipient, w.fee),
        burn_note(&w.burn_secret, &w.recipient, w.fee + 1)
    );
}
