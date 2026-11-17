//! Follower: `attest-followerd` follows a beacon chain and keeps an `attest` instance on LEZ
//! current, submitting light-client updates through `attest_host`.
//!
//! - [`source`]: the [`BeaconSource`] trait, with [`LogosModuleSource`] (Logos `beacon_api_module` through
//!   `logosctl`) and [`DirectHttpSource`] (the same Beacon API routes over HTTP).
//! - [`ssz`]: Beacon API JSON to canonical SSZ (electra, fulu, gloas).
//! - [`chain`]: [`Chain`] with [`HostChain`] (attest_host) and [`MockChain`] (tests only).
//! - [`follower`]: the loop and its policy ([`follower::decide`]).
//! - [`log`]: the JSON log line G4 reads.

#![forbid(unsafe_code)]

pub mod chain;
pub mod config;
pub mod follower;
pub mod log;
pub mod source;
pub mod ssz;

pub use chain::{Chain, HostChain, MockChain};
pub use log::LogLine;
pub use source::{AnySource, BeaconSource, BeaconSourceKind, DirectHttpSource, LcObject, LogosModuleSource};
