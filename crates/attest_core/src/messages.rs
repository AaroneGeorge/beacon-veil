//! Program messages (PLAN.md 2.3). Borsh-encoded `MessageData` of the `attest` and `lc_verify` programs.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

use crate::{Bytes32, Head, HeaderRecord, InstanceConfig, Predicate};

/// Messages of program `attest`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum AttestMessage {
    /// Top-level, public. Receiver must equal `instance_id(config)`; checks the committee branch against
    /// `checkpoint_root`. `bootstrap` is the SSZ-encoded `LightClientBootstrap`.
    Init { config: InstanceConfig, bootstrap: Vec<u8> },
    /// Only if `from.program_account_id` is in `config.verifiers`; `pre_store` must equal state (E13/E14);
    /// `head.slot` must increase; then sync `WriteRecord` to `record_id`.
    Apply {
        pre_store: Bytes32,
        post_store: Bytes32,
        head: Head,
        record: HeaderRecord,
    },
    /// Only `from_own_program()`; state must be empty.
    WriteRecord { record: HeaderRecord },
    /// Anyone; panics E15/E02 unless `sha256(state) == record_hash`.
    CheckHeader { record_hash: Bytes32 },
    /// On any account, public or private: verify inclusion under the record roots, evaluate the predicate, sync
    /// `CheckHeader` to `record_id`, sync the `Statement` to `deliver_to`. `claim` is the encoded inclusion claim
    /// (layout owned by W3 in `attest_inclusion`).
    Attest {
        record: HeaderRecord,
        claim: Vec<u8>,
        predicate: Predicate,
        deliver_to: AccountId,
    },
}

/// Message of program `lc_verify`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LcVerifyMessage {
    /// Runs Helios verify + apply, computes pre and post store commitments, sets
    /// `Response::timestamp_window(start = genesis_time + 12 * signature_slot)` and sends sync `Apply` to
    /// `Actor(instance, attest_program)`. Holds no ids of its own.
    Verify {
        attest_program: AccountId,
        instance: AccountId,
        config: InstanceConfig,
        /// Encoded light-client store witness (layout owned by W1).
        store_witness: Vec<u8>,
        /// SSZ-encoded `LightClientUpdate` or `LightClientFinalityUpdate`.
        update_ssz: Vec<u8>,
    },
}
