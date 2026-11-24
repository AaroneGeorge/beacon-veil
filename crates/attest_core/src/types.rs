//! State and record types (docs/DECISIONS.md D-11).

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

use crate::Bytes32;

/// Immutable configuration of one light-client instance. Its hash is the instance PDA seed, so it cannot change
/// (F9, F13, VS3). A new Ethereum fork means a new verifier and a new instance.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InstanceConfig {
    pub chain_id: u64,
    pub genesis_validators_root: Bytes32,
    pub genesis_time: u64,
    /// `(activation_epoch, fork_version)` in activation order.
    pub forks: Vec<(u64, [u8; 4])>,
    pub checkpoint_root: Bytes32,
    pub checkpoint_slot: u64,
    /// Accepted `lc_verify` program account ids, 1..=2.
    pub verifiers: Vec<AccountId>,
    pub min_participants: u16,
}

/// Head of an instance: the latest applied finalised header.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Head {
    pub slot: u64,
    pub beacon_root: Bytes32,
    pub exec_number: u64,
    pub exec_hash: Bytes32,
    pub exec_state_root: Bytes32,
    pub exec_receipts_root: Bytes32,
    pub period: u64,
    /// `sha256(finalized_header_root || current_committee_root || next_committee_root_or_zero || participation fields)`
    pub store_commitment: Bytes32,
}

/// Inputs of `store_commitment`. The exact participation fields are fixed in `attest_core::lc`.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StoreCommitmentInput {
    pub finalized_header_root: Bytes32,
    pub current_committee_root: Bytes32,
    /// Zero when the next committee is not known.
    pub next_committee_root: Bytes32,
    pub previous_max_active_participants: u64,
    pub current_max_active_participants: u64,
}

impl StoreCommitmentInput {
    /// `sha256(finalized_header_root || current_committee_root || next_committee_root_or_zero || participation)`,
    /// participation as two little-endian u64.
    #[must_use]
    pub fn commitment(&self) -> Bytes32 {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.finalized_header_root);
        h.update(self.current_committee_root);
        h.update(self.next_committee_root);
        h.update(self.previous_max_active_participants.to_le_bytes());
        h.update(self.current_max_active_participants.to_le_bytes());
        h.finalize().into()
    }
}

/// Write-once record of one finalised header, stored at `record_id(instance, slot)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HeaderRecord {
    pub chain_id: u64,
    pub slot: u64,
    pub beacon_root: Bytes32,
    pub exec_number: u64,
    pub exec_hash: Bytes32,
    pub exec_state_root: Bytes32,
    pub exec_receipts_root: Bytes32,
}

impl HeaderRecord {
    /// `sha256(borsh(record))`: what `CheckHeader{record_hash}` compares against.
    #[must_use]
    pub fn hash(&self) -> Bytes32 {
        use sha2::{Digest, Sha256};
        Sha256::digest(borsh::to_vec(self).expect("borsh encoding of HeaderRecord")).into()
    }
}

/// What a statement is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ClaimKind {
    Account,
    Storage,
    Log,
}

/// An attested fact, delivered by `Attest` to a consumer (`deliver_to`).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Statement {
    pub instance: AccountId,
    pub chain_id: u64,
    pub slot: u64,
    pub exec_number: u64,
    pub kind: ClaimKind,
    /// Account: address. Storage: address || slot key. Log: emitter || tx index || log index (layout fixed in `attest_inclusion`).
    pub subject: Vec<u8>,
    /// Account or storage value, or the full log encoding.
    pub value: Vec<u8>,
    /// `sha256(borsh(predicate))`
    pub predicate_hash: Bytes32,
    /// Statement id, see [`crate::ids`]. Consumers key their nullifier on it (F6).
    pub id: Bytes32,
}

/// Actor state of an instance under the `attest` program (account `instance_id(config)`), borsh-encoded. The config
/// is stored so that `Apply` can check the sender against `config.verifiers`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InstanceState {
    pub config: InstanceConfig,
    pub head: Head,
}
