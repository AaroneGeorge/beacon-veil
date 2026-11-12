//! Inclusion proofs and predicates (PLAN.md 2.3, owner W3). Pure functions over `alloy-trie`, no I/O; runs natively
//! and in the guest.
//!
//! Status: freeze-1 stubs. Signatures are the contract; bodies return `E07_INCLUSION_INVALID` / `E10` until W3 lands.

#![forbid(unsafe_code)]

use attest_core::{Bytes32, EsaError, Predicate};
pub use attest_core::{statement_id_log, statement_id_state};

/// Ethereum account fields proven under a state root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountValue {
    pub nonce: u64,
    /// Big-endian u256.
    pub balance: Bytes32,
    pub storage_root: Bytes32,
    pub code_hash: Bytes32,
}

/// A receipt log proven under a receipts root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogValue {
    pub address: [u8; 20],
    pub topics: Vec<Bytes32>,
    pub data: Vec<u8>,
}

/// Verifies `address`'s account under `state_root` (MPT proof as in `eth_getProof`, RLP nodes).
pub fn verify_account(state_root: &Bytes32, address: &[u8; 20], proof: &[Vec<u8>]) -> Result<AccountValue, EsaError> {
    let _ = (state_root, address, proof);
    Err(EsaError::E07InclusionInvalid)
}

/// Verifies a storage slot under an account's `storage_root`. Returns the 32-byte big-endian value.
pub fn verify_storage(storage_root: &Bytes32, slot_key: &Bytes32, proof: &[Vec<u8>]) -> Result<Bytes32, EsaError> {
    let _ = (storage_root, slot_key, proof);
    Err(EsaError::E07InclusionInvalid)
}

/// Verifies log `log_index` of receipt `tx_index` under `receipts_root`.
pub fn verify_log(
    receipts_root: &Bytes32,
    tx_index: u64,
    proof: &[Vec<u8>],
    log_index: u64,
) -> Result<LogValue, EsaError> {
    let _ = (receipts_root, tx_index, proof, log_index);
    Err(EsaError::E07InclusionInvalid)
}

/// Value a predicate is evaluated against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimValue {
    Word(Bytes32),
    Log(LogValue),
}

/// Evaluates `predicate` on `value`: `Ok(())`, `E08_PREDICATE_FALSE` or `E10_PREDICATE_NOT_COVERED`.
pub fn eval(predicate: &Predicate, value: &ClaimValue) -> Result<(), EsaError> {
    let _ = (predicate, value);
    Err(EsaError::E10PredicateNotCovered)
}
