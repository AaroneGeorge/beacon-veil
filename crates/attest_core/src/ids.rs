//! Account and statement ids (docs/DECISIONS.md D-11).

use lee_core::{account::AccountId, program::PdaSeed};
use sha2::{Digest, Sha256};

use crate::{Bytes32, InstanceConfig, domain};

/// `sha256("esa/instance/v1" || borsh(config))`
#[must_use]
pub fn instance_seed(config: &InstanceConfig) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain::INSTANCE);
    h.update(borsh::to_vec(config).expect("borsh encoding of InstanceConfig"));
    h.finalize().into()
}

/// `instance_id = PDA(attest, instance_seed(config))`
#[must_use]
pub fn instance_id(attest_program: &AccountId, config: &InstanceConfig) -> AccountId {
    AccountId::for_public_pda(attest_program, &PdaSeed::new(instance_seed(config)))
}

/// `sha256("esa/record/v1" || instance_id || slot)`, slot little-endian u64.
#[must_use]
pub fn record_seed(instance: &AccountId, slot: u64) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain::RECORD);
    h.update(instance.value());
    h.update(slot.to_le_bytes());
    h.finalize().into()
}

/// `record_id = PDA(attest, record_seed(instance, slot))`
#[must_use]
pub fn record_id(attest_program: &AccountId, instance: &AccountId, slot: u64) -> AccountId {
    AccountId::for_public_pda(attest_program, &PdaSeed::new(record_seed(instance, slot)))
}

/// Log statement id: `sha256("esa/stmt/v1" || chain_id || exec_hash || tx_index || log_index || keccak(log))`.
/// Integers little-endian; `log_keccak` is computed by the caller.
#[must_use]
pub fn statement_id_log(
    chain_id: u64,
    exec_hash: &Bytes32,
    tx_index: u64,
    log_index: u64,
    log_keccak: &Bytes32,
) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain::STATEMENT);
    h.update(chain_id.to_le_bytes());
    h.update(exec_hash);
    h.update(tx_index.to_le_bytes());
    h.update(log_index.to_le_bytes());
    h.update(log_keccak);
    h.finalize().into()
}

/// Account or storage statement id: `sha256("esa/stmt/v1" || chain_id || address || slot_key || value)`.
/// Same state gives the same id (F6). For an account claim `slot_key` is empty.
#[must_use]
pub fn statement_id_state(chain_id: u64, address: &[u8; 20], slot_key: &[u8], value: &[u8]) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain::STATEMENT);
    h.update(chain_id.to_le_bytes());
    h.update(address);
    h.update(slot_key);
    h.update(value);
    h.finalize().into()
}
