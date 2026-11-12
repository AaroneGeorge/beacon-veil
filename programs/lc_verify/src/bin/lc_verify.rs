//! Program `lc_verify` (PLAN.md 2.2 D1/D2, owner W1): Helios `verify_*` + `apply_*` (unmodified, V1), then a sync
//! `Apply` to the instance. Holds no ids of its own and keeps no state.
//!
//! Path A: runs in a private actor; the privacy circuit composes its receipt, and the public `attest.Apply` checks
//! the sender's program id and the pre-store commitment. Path B0: the same handler run by the public executor.

use attest_core::{AttestMessage, EsaError, LcVerifyMessage, instance_id, lc};
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(_input: &ReceiveInput, message: LcVerifyMessage) -> Response {
    let LcVerifyMessage::Verify {
        attest_program,
        instance,
        config,
        store_witness,
        update_ssz,
    } = message;
    // The config must be the instance's own: the instance address is the PDA of its config hash.
    if instance_id(&attest_program, &config) != instance {
        EsaError::E09ChainMismatch.abort();
    }
    let witness: lc::StoreWitness =
        borsh::from_slice(&store_witness).unwrap_or_else(|_| EsaError::E01Malformed.abort());
    #[cfg(not(feature = "v2"))]
    let out = lc::verify_and_apply(&config, &witness, &update_ssz).unwrap_or_else(|e| e.abort());
    #[cfg(feature = "v2")]
    let out = lc::verify_and_apply_v2(&config, &witness, &update_ssz).unwrap_or_else(|e| e.abort());
    // Chain time must have reached the signature slot (Helios's `expected_current_slot >= signature_slot`).
    let not_before_ms = config
        .genesis_time
        .saturating_add(lc::SECONDS_PER_SLOT.saturating_mul(out.signature_slot))
        .saturating_mul(1000);
    Response::keep_state().timestamp_window(not_before_ms..).send_sync(
        Actor::new(instance, attest_program),
        &AttestMessage::Apply {
            pre_store: out.pre_store,
            post_store: out.post_store,
            head: out.head,
            record: out.record,
        },
    )
}
