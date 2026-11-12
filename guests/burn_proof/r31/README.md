# burn_proof on RISC Zero 3.1.0 (separate workspace)

Why: RISC Zero advisory GHSA-g6qg-9j8p-7j87 (2026-11-11) affects risc0-zkvm below 3.1.0 and Groth16 verifier 3.0.0.
The repo root pins 3.0.5 because LEZ v0.4.0-rc3 does. `burn_proof` needs only sha2 and borsh, so it is built and proven
here, on 3.1.0, in its own workspace and target dir, with its own rzup home. The vault v3 holds verifier 3.1.0 behind
its emergency stop and the image id of THIS guest.

- `methods/guest`: the guest. It compiles the same `programs/bridge/src/{hash,tree,journal}.rs` files as the 3.0.5
  build (`#[path]`), so the statement is the same source.
- `methods`: risc0-build 3.1.0, embeds the ELF and image id.
- `host`: `burn_prover_r31 image-id | prove --witness FILE --out DIR --mode execute|groth16`.

Environment (bridge_cli prove --mode witness writes the witness file, mode 0600):
```
export RISC0_HOME=$B/run/w5/risc0-31 CARGO_TARGET_DIR=$B/target-w5-r31
export RISC0_SERVER_PATH=$RISC0_HOME/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm
cargo run --release --manifest-path guests/burn_proof/r31/Cargo.toml -p burn_prover_r31 -- image-id
```
