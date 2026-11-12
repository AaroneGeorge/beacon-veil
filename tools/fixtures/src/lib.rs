//! Fixtures (W2, PLAN.md section 4).
//!
//! - [`real`]: load the fetched Beacon API fixtures in `fixtures/<chain>/` (see `fetch.py` and `manifest.json`) and
//!   build an [`attest_core::InstanceConfig`] from them.
//! - [`ssz`]: SSZ encoders for the Electra/Fulu light-client containers (Helios 0.12.0 derives only `Decode` for
//!   them), plus the Beacon API `updates_by_range` SSZ list splitter. Checked byte for byte against real fixtures.
//! - [`fabricate`]: the fabricated chain of RFP-021 Verification Security 1: 512 fresh BLS keys per period, a
//!   self-consistent committee chain, signed headers, valid Merkle branches and a matching execution-state proof.
//!
//! Reuse: every container, tree hash and the BLS curve come from Helios 0.12.0 (`helios-consensus-core`, MIT) and
//! `bls12_381` (RISC Zero fork); execution tries come from `alloy-trie`'s `HashBuilder` (MIT/Apache-2.0). Written
//! here: key derivation, the sparse beacon-state tree, the SSZ encoders and the chain assembly.

#![forbid(unsafe_code)]

pub mod exec;
pub mod fabricate;
pub mod merkle;
pub mod real;
pub mod ssz;

/// Workspace-relative fixtures directory: `$BV_REPO/fixtures`, else `<crate>/../../fixtures`.
#[must_use]
pub fn fixtures_dir() -> std::path::PathBuf {
    if let Ok(r) = std::env::var("BV_REPO") {
        return std::path::Path::new(&r).join("fixtures");
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}
