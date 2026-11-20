//! Builds the measurement guest with risc0-build 3.0.7 and the rzup toolchain of `$RISC0_HOME`.
fn main() {
    risc0_build::embed_methods();
}
