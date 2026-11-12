//! `attest_follower_module`: a Logos core module that supervises `attest-followerd` (beacon-veil, PLAN.md 2.6).
//!
//! The follower is a separate process: it links the LEZ wallet and RISC Zero host crates, which do not belong inside
//! a Qt plugin, and it must keep running (and be killable and restartable) on its own. The module starts and stops
//! it, reports its status, and turns its JSON log lines into Logos events. The follower takes its beacon data from
//! `beacon_api_module` (declared as this module's dependency) through `logosctl call`.
//!
//! The supervisor (`supervisor`) is plain Rust, tested with `cargo test --no-default-features`; the Logos glue is
//! behind the default `logos` feature.

pub mod supervisor;

#[cfg(feature = "logos")]
mod glue;
