//! The checks of program `attest` as pure functions. The guest (`programs/attest`) is a thin wrapper
//! over these, and host tests (the G2 suite, `attest_host` dry runs) call the same code.

use lee_core::account::AccountId;
use sha2::{Digest, Sha256};

use crate::{Bytes32, EsaError, Head, HeaderRecord, InstanceConfig, InstanceState, instance_id, record_id};

/// Decodes an instance's actor state. Empty or undecodable state is E01 (no instance at this address).
pub fn decode_instance(state: &[u8]) -> Result<InstanceState, EsaError> {
    if state.is_empty() {
        return Err(EsaError::E01Malformed);
    }
    borsh::from_slice(state).map_err(|_| EsaError::E01Malformed)
}

/// `Init` checks that do not need Helios: the receiver must be the PDA of the config, and empty.
pub fn check_init_target(
    attest_program: &AccountId,
    receiver: &AccountId,
    receiver_state: &[u8],
    config: &InstanceConfig,
) -> Result<(), EsaError> {
    if instance_id(attest_program, config) != *receiver {
        return Err(EsaError::E09ChainMismatch);
    }
    if !receiver_state.is_empty() {
        return Err(EsaError::E14AlreadyApplied);
    }
    Ok(())
}

/// Full `Init`: target checks, then Helios `verify_bootstrap` (via [`crate::lc`]). Returns the new instance state
/// and the checkpoint's header record.
#[cfg(feature = "lc")]
pub fn init(
    attest_program: &AccountId,
    receiver: &AccountId,
    receiver_state: &[u8],
    config: &InstanceConfig,
    bootstrap_ssz: &[u8],
) -> Result<(InstanceState, HeaderRecord), EsaError> {
    check_init_target(attest_program, receiver, receiver_state, config)?;
    let out = crate::lc::verify_bootstrap_ssz(config, bootstrap_ssz)?;
    Ok((
        InstanceState {
            config: config.clone(),
            head: out.head,
        },
        out.record,
    ))
}

/// What a successful `Apply` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyEffect {
    pub new_state: InstanceState,
    /// A header record is due at `record_id(instance, head.slot)` (the finalized slot moved forward).
    pub write_record: Option<HeaderRecord>,
}

/// `Apply{pre_store, post_store, head, record}` against the instance state.
///
/// - sender's program must be in `config.verifiers` (E12);
/// - `pre_store` must equal the stored commitment: if not, E14 when `post_store` is the stored one (replay), else E13;
/// - the store must change and the slot must not go back (coordinator rule 2026-11-12: `post != pre`,
///   `head.slot >= old slot`), else E14;
/// - `head` and `record` must agree with each other and with `post_store` (E15), and `record.chain_id` with the
///   config (E09).
pub fn apply(
    state: &[u8],
    sender_program: Option<AccountId>,
    pre_store: Bytes32,
    post_store: Bytes32,
    head: Head,
    record: HeaderRecord,
) -> Result<ApplyEffect, EsaError> {
    let mut st = decode_instance(state)?;
    let sender = sender_program.ok_or(EsaError::E12UnknownVerifier)?;
    if !st.config.verifiers.contains(&sender) {
        return Err(EsaError::E12UnknownVerifier);
    }
    let current = st.head.store_commitment;
    if pre_store != current {
        return Err(if post_store == current {
            EsaError::E14AlreadyApplied
        } else {
            EsaError::E13StoreMismatch
        });
    }
    if post_store == pre_store || head.slot < st.head.slot {
        return Err(EsaError::E14AlreadyApplied);
    }
    if record.chain_id != st.config.chain_id {
        return Err(EsaError::E09ChainMismatch);
    }
    if head.store_commitment != post_store
        || record.slot != head.slot
        || record.beacon_root != head.beacon_root
        || record.exec_hash != head.exec_hash
        || record.exec_number != head.exec_number
        || record.exec_state_root != head.exec_state_root
        || record.exec_receipts_root != head.exec_receipts_root
    {
        return Err(EsaError::E15HeaderMismatch);
    }
    if head.slot > st.head.slot {
        st.head = head;
        return Ok(ApplyEffect {
            new_state: st,
            write_record: Some(record),
        });
    }
    // Same slot: a committee-only update (the next committee became known). The finalized header is unchanged, so
    // only the commitment moves; the execution fields of the head stay (the store witness carries only the beacon
    // header, so `lc` reports them as zero here).
    if head.beacon_root != st.head.beacon_root {
        return Err(EsaError::E15HeaderMismatch);
    }
    st.head.store_commitment = post_store;
    Ok(ApplyEffect {
        new_state: st,
        write_record: None,
    })
}

/// `WriteRecord{record}`: only from this program (`from_own_program`), from the instance whose record address the
/// receiver is, onto an empty account (not `write_once`: it accepts an identical rewrite).
pub fn write_record(
    attest_program: &AccountId,
    receiver: &AccountId,
    receiver_state: &[u8],
    from_own_program_account: Option<AccountId>,
    record: &HeaderRecord,
) -> Result<Vec<u8>, EsaError> {
    let instance = from_own_program_account.ok_or(EsaError::E12UnknownVerifier)?;
    if record_id(attest_program, &instance, record.slot) != *receiver {
        return Err(EsaError::E01Malformed);
    }
    if !receiver_state.is_empty() {
        return Err(EsaError::E14AlreadyApplied);
    }
    Ok(borsh::to_vec(record).expect("borsh encoding of HeaderRecord"))
}

/// `CheckHeader{record_hash}`: E02 if no record exists at this address, E15 if `sha256(state) != record_hash`.
pub fn check_header(record_state: &[u8], record_hash: &Bytes32) -> Result<(), EsaError> {
    if record_state.is_empty() {
        return Err(EsaError::E02NotFinalised);
    }
    let got: Bytes32 = Sha256::digest(record_state).into();
    if got != *record_hash {
        return Err(EsaError::E15HeaderMismatch);
    }
    Ok(())
}
