//! Measurement guest: checks blocks P+1..T as one hash chain from the pinned (P, hash), parses the burn block in
//! full and finds the burn in it. Commits borsh `(tip_id, tip_hash, burn_block_id, marks)`, where `marks` are the
//! cycle counts after reading the input, after the chain check, after the parse and after the finder. A zero note
//! skips the finder (to measure the parse of a block that holds no burn).

#![no_main]

use std::io::Read as _;

use borsh::{BorshDeserialize, BorshSerialize};
use lez_chain::{AccountId, Tip};
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

#[derive(BorshDeserialize)]
struct Input {
    pinned_id: u64,
    pinned_hash: [u8; 32],
    blocks: Vec<Vec<u8>>,
    burn_pos: u32,
    tx_index: u32,
    bridge: [u8; 32],
    instance: [u8; 32],
    note: [u8; 32],
}

#[derive(BorshSerialize)]
struct Output {
    tip_id: u64,
    tip_hash: [u8; 32],
    burn_block_id: u64,
    marks: [u64; 4],
}

fn main() {
    let mut bytes = Vec::new();
    env::stdin().read_to_end(&mut bytes).expect("input");
    let input = Input::try_from_slice(&bytes).expect("input decodes");
    let m0 = env::cycle_count();
    let tip = lez_chain::verify_chain(
        Tip {
            block_id: input.pinned_id,
            hash: input.pinned_hash,
        },
        input.blocks.iter().map(Vec::as_slice),
    )
    .expect("hash chain");
    let m1 = env::cycle_count();
    let raw = &input.blocks[input.burn_pos as usize];
    let block = lez_chain::parse_block(raw).expect("burn block parses");
    let m2 = env::cycle_count();
    if input.note != [0; 32] {
        lez_chain::find_burn(
            &block,
            input.tx_index as usize,
            &AccountId(input.bridge),
            &AccountId(input.instance),
            &input.note,
        )
        .expect("burn");
    }
    let m3 = env::cycle_count();
    let out = Output {
        tip_id: tip.block_id,
        tip_hash: tip.hash,
        burn_block_id: block.header.block_id,
        marks: [m0, m1, m2, m3],
    };
    env::commit_slice(&borsh::to_vec(&out).expect("encode"));
}
