//! Builds guest `burn_chain` with risc0-build 3.0.7 (the version risc0 v3.1.0 ships) and the rzup toolchain of
//! `$RISC0_HOME` (the isolated 3.1.0 home, see guests/burn_proof/r31/README.md).
fn main() {
    risc0_build::embed_methods();
}
