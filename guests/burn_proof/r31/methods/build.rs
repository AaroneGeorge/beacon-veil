//! Builds guest `burn_proof` with risc0-build 3.0.7 (the version risc0 v3.1.0 ships).
//! - Default: the rzup toolchain of `$RISC0_HOME` (the isolated 3.1.0 home, see ../README.md).
//! - `BV_GUEST_DOCKER=1`: RISC Zero's guest builder image (tag `BV_R31_DOCKER_TAG`, default r0.1.97.0, the default of
//!   risc0-build 3.0.7), with the repository root as docker context, because the guest includes
//!   `programs/bridge/src/*.rs`. Reproducible image id; the one vault v3 uses.

use std::{collections::HashMap, path::Path};

fn main() {
    println!("cargo:rerun-if-env-changed=BV_GUEST_DOCKER");
    println!("cargo:rerun-if-env-changed=BV_R31_DOCKER_TAG");
    let mut opts = HashMap::new();
    if std::env::var("BV_GUEST_DOCKER").is_ok_and(|v| v == "1") {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().expect("repo root");
        let tag = std::env::var("BV_R31_DOCKER_TAG").unwrap_or_else(|_| "r0.1.97.0".into());
        let docker = risc0_build::DockerOptionsBuilder::default()
            .root_dir(root)
            .docker_container_tag(tag)
            .build()
            .expect("docker options");
        let guest = risc0_build::GuestOptionsBuilder::default().use_docker(docker).build().expect("guest options");
        opts.insert("burn_proof_r31", guest);
    }
    risc0_build::embed_methods_with_options(opts);
}
