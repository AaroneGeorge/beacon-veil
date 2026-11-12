//! Guest `burn_proof` on RISC Zero 3.1.0. Same statement and source as `guests/burn_proof` (3.0.5): the modules below
//! are the files of `programs/bridge/src`, included by path. See ../../../README.md.

#![no_main]
#![allow(dead_code)]

pub type B32 = [u8; 32];
pub type Addr = [u8; 20];

#[path = "../../../../../../programs/bridge/src/hash.rs"]
pub mod hash;
#[path = "../../../../../../programs/bridge/src/journal.rs"]
pub mod journal;
#[path = "../../../../../../programs/bridge/src/tree.rs"]
pub mod tree;

use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let input: Vec<u8> = env::read();
    let witness: journal::BurnWitness = borsh::from_slice(&input).expect("burn witness must decode");
    env::commit_slice(&journal::prove_burn(&witness).encode());
}
