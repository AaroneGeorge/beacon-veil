//! Guest `burn_proof` (PLAN.md 2.4 outbound step 1, owner W5). Freeze-1 stub: commits an empty journal.

#![cfg_attr(target_os = "zkvm", no_main)]

#[cfg(target_os = "zkvm")]
risc0_zkvm::guest::entry!(main);

fn main() {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::commit_slice(&[0_u8; 0]);
}
