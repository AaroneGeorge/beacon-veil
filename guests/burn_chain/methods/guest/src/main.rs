//! Guest `burn_chain`: proves `lez_chain::statement::prove_burn_chain` for a borsh `ChainWitness` read from stdin and
//! commits the 480-byte version-2 journal. See crates/lez_chain/src/statement.rs and ../../README.md.

#![no_main]

use std::io::Read as _;

use lez_chain::statement::{ChainWitness, prove_burn_chain};
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let mut input = Vec::new();
    env::stdin().read_to_end(&mut input).expect("witness");
    let witness: ChainWitness = borsh::from_slice(&input).expect("witness decodes");
    match prove_burn_chain(&witness) {
        Ok(journal) => env::commit_slice(&journal.encode()),
        Err(e) => panic!("burn_chain refused: {e:?}"),
    }
}
