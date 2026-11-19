//! Program messages (PLAN.md 2.3). Borsh-encoded `MessageData` of the `attest` and `lc_verify` programs.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::{AccountId, Actor};

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
    /// On any account, public or private: verify inclusion under the record roots, evaluate the predicate
    /// (`attest_inclusion::verify_claim`), sync `CheckHeader` to `record_id(attest, instance, record.slot)`, then sync
    /// the `Statement` (borsh, as the whole message) to the consumer actor `deliver_to`. `claim` is the encoded
    /// inclusion claim (layout owned by W3 in `attest_inclusion`).
    Attest {
        instance: AccountId,
        record: HeaderRecord,
        claim: Vec<u8>,
        predicate: Predicate,
        deliver_to: Actor,
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

/// Messages of program `lc_verify_v3` (feature `v3`): one update as up to three public transactions on a job
/// account `PDA(lc_verify_v3, lc_v3::job_seed(instance, pre_store, update_ssz))`, the receiver of every step. Anyone
/// may send any step. Refusals end the program with exit code = the `EsaError` number (cycle-counted refusal);
/// `attest.Apply` refusals inside `Finish` still panic with `ESA:Exx`.
#[cfg(feature = "v3")]
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LcVerifyV3Message {
    /// Checks the update, computes the `Apply` fields, aggregates the key and decodes the signature; writes the job
    /// at stage `BEGUN`. Timestamp window: from `genesis_time + 12 * signature_slot` to the weak-subjectivity bound.
    Begin {
        attest_program: AccountId,
        instance: AccountId,
        config: InstanceConfig,
        witness: crate::lc_v3::StoreWitnessV3,
        update_ssz: Vec<u8>,
        /// Uncompressed G1 keys of the cleared participation bits, in bit order (`lc_v3::absent_keys`).
        absent_keys: Vec<[u8; 96]>,
    },
    /// Hash to G2 and the Miller loop; job stage `BEGUN` -> `MILLER`.
    Miller,
    /// Final exponentiation; on success sends sync `Apply` to the instance; job stage `MILLER` -> `DONE`.
    Finish,
    /// All steps in one execution (private path). Same checks and outcome as `Begin`, `Miller`, `Finish`.
    VerifyOnce {
        attest_program: AccountId,
        instance: AccountId,
        config: InstanceConfig,
        witness: crate::lc_v3::StoreWitnessV3,
        update_ssz: Vec<u8>,
        absent_keys: Vec<[u8; 96]>,
    },
    /// Four-step path, first half of `Begin`: checks and `Apply` fields, no keys; job stage `PREPARED`.
    BeginPrepare {
        attest_program: AccountId,
        instance: AccountId,
        config: InstanceConfig,
        witness: crate::lc_v3::StoreWitnessV3,
        update_ssz: Vec<u8>,
    },
    /// Four-step path, second half of `Begin`: the signing committee again (its root must equal the job's) and the
    /// absent keys; job stage `PREPARED` -> `BEGUN`.
    BeginKeys {
        committee_ssz: Vec<u8>,
        absent_keys: Vec<[u8; 96]>,
    },
}
