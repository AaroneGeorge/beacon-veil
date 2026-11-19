//! Builds the guests listed in `[package.metadata.risc0] methods`.
//!
//! - Default: local rzup toolchain (`rust` 1.91.1, `cpp`), fast, for dev work.
//! - `BV_GUEST_DOCKER=1`: pinned Docker image `RISC0_DOCKER_CONTAINER_TAG` (default r0.1.91.1, same as LEZ rc3 Justfile),
//!   reproducible image ids; used by CI and before any deployment whose ids are recorded.
//! - `RISC0_SKIP_BUILD=1`: no guest build (fmt, clippy, host-only tests).

use std::collections::HashMap;

const GUESTS: [&str; 6] = [
    "attest_program",
    "lc_verify_program",
    "consumer_erc20_program",
    "bridge_program",
    "burn_proof_guest",
    "lc_verify_v3_program",
];
const DOCKER_TAG: &str = "r0.1.91.1";

fn main() {
    println!("cargo:rerun-if-env-changed=BV_GUEST_DOCKER");
    let mut opts = HashMap::new();
    if std::env::var("BV_GUEST_DOCKER").is_ok_and(|v| v == "1") {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let docker = risc0_build::DockerOptionsBuilder::default()
            .root_dir(root.canonicalize().expect("workspace root"))
            .docker_container_tag(DOCKER_TAG)
            .build()
            .expect("docker options");
        for g in GUESTS {
            let o = risc0_build::GuestOptionsBuilder::default()
                .use_docker(docker.clone())
                .build()
                .expect("guest options");
            opts.insert(g, o);
        }
    }
    risc0_build::embed_methods_with_options(opts);
}
