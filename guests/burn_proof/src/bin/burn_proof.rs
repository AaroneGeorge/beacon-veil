//! Guest `burn_proof` (PLAN.md 2.4 outbound step 1, owner W5).
//!
//! Private input: `borsh(BurnWitness)` as one `Vec<u8>`. Proves that a burn note opening to (secret, recipient, fee)
//! sits, as leaf (token, denom, note), at some index of a burn-accumulator root, and derives the release nullifier
//! from the secret. Public output: the 352-byte ABI journal `Vault.release` decodes. The root's finality and its
//! instance are the anchor's job (`ILezAnchor`), not this guest's.

#![cfg_attr(target_os = "zkvm", no_main)]

#[cfg(target_os = "zkvm")]
risc0_zkvm::guest::entry!(main);

#[cfg(target_os = "zkvm")]
fn main() {
    use bridge_program::journal::{BurnWitness, prove_burn};
    use risc0_zkvm::guest::env;

    let input: Vec<u8> = env::read();
    let witness: BurnWitness = borsh::from_slice(&input).expect("burn witness must decode");
    env::commit_slice(&prove_burn(&witness).encode());
}

#[cfg(not(target_os = "zkvm"))]
fn main() {
    // Native build: the statement is `bridge_program::journal::prove_burn`, tested there.
    let _ = (bridge_program::journal::JOURNAL_LEN, borsh::to_vec(&0_u8));
}
