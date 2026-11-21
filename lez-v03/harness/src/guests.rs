//! The built guests: `$V03_GUESTS/{attest_v03,lc_verify_v3_v03}.bin` (from `scripts/build_guests.sh`; default
//! `../target-guests/release` relative to this crate's workspace).

use std::path::PathBuf;

use anyhow::{Context, Result};

#[must_use]
pub fn dir() -> PathBuf {
    std::env::var("V03_GUESTS").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target-guests/local"),
        PathBuf::from,
    )
}

pub fn bin(name: &str) -> Result<Vec<u8>> {
    let p = dir().join(format!("{name}.bin"));
    std::fs::read(&p).with_context(|| {
        format!(
            "reading {} (build it with lez-v03/scripts/build_guests.sh)",
            p.display()
        )
    })
}
