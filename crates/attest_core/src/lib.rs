//! Interface contract of beacon-veil, fixed on 2026-11-12 (docs/DECISIONS.md D-11).
//!
//! Shared by the guests (`programs/*`) and the host crates. Every type is borsh-encoded. Changing a type or a domain
//! string after 2026-11-12 needs a new entry in docs/DECISIONS.md, because domain strings define account addresses.
//!
//! Shapes, domain strings and error codes are fixed here; the programs hold the logic.

#![forbid(unsafe_code)]

pub mod error;
pub mod ids;
pub mod messages;
pub mod predicate;
pub mod rules;
pub mod types;

#[cfg(feature = "lc")]
pub mod lc;
#[cfg(all(feature = "cycle-trace", target_os = "zkvm"))]
pub mod lc_trace;
#[cfg(feature = "v2")]
pub mod lc_v2;

pub use error::EsaError;
pub use ids::{instance_id, instance_seed, record_id, record_seed, statement_id_log, statement_id_state};
pub use lee_core::account::{AccountId, Actor};
pub use messages::{AttestMessage, LcVerifyMessage};
pub use predicate::{Predicate, PredicateValue};
pub use types::{ClaimKind, Head, HeaderRecord, InstanceConfig, InstanceState, Statement, StoreCommitmentInput};

/// A 32-byte hash or root.
pub type Bytes32 = [u8; 32];

/// Domain strings (D-11). Never change them: they define account addresses.
pub mod domain {
    /// `instance_id = PDA(attest, sha256(INSTANCE || borsh(config)))`
    pub const INSTANCE: &[u8] = b"esa/instance/v1";
    /// `record_id = PDA(attest, sha256(RECORD || instance_id || slot))`
    pub const RECORD: &[u8] = b"esa/record/v1";
    /// Statement ids.
    pub const STATEMENT: &[u8] = b"esa/stmt/v1";
}

// V3 verifier (public path, staged). Appended last so that no line above moves.
#[cfg(feature = "v3")]
pub mod lc_v3;
#[cfg(feature = "v3")]
pub use messages::LcVerifyV3Message;
