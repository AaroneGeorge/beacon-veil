//! Guest `bls_bench` (measurement only, never deployed). Input: `borsh(Job)`, the raw stdin bytes.
//! Journal: `borsh(JobOutput)`.

#![cfg_attr(target_os = "zkvm", no_main)]

#[cfg(target_os = "zkvm")]
risc0_zkvm::guest::entry!(main);

#[cfg(target_os = "zkvm")]
fn main() {
    use std::io::Read;

    use risc0_zkvm::guest::env;

    let mut input = Vec::new();
    env::stdin().read_to_end(&mut input).expect("read input");
    let job: bls_bench_guest::Job = borsh::from_slice(&input).expect("job must decode");
    let out = bls_bench_guest::run(&job);
    env::commit_slice(&borsh::to_vec(&out).expect("encode output"));
}

#[cfg(not(target_os = "zkvm"))]
fn main() {
    // Native build: the logic is `bls_bench_guest::run`, compared with V1 by `tools/cost bench-bls`.
    let _ = bls_bench_guest::PHASES;
}
