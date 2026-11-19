//! Program `lc_verify_v3`: the public-path light client. Thin wrapper around `attest_core::lc_v3::handle`.
//!
//! Public path: `Begin`, `Miller`, `Finish` (or `BeginPrepare`, `BeginKeys`, `Miller`, `Finish`) as separate public
//! transactions to the job account `PDA(lc_verify_v3, job_seed(instance, pre_store, update_ssz))`; `Finish` sends
//! sync `Apply` to the instance, where `attest` checks this program's id against `config.verifiers` and the
//! pre-store commitment against its head. Private path: `VerifyOnce` in a private actor, then the same `Apply`.
//!
//! A refusal ends the program with exit code = the `EsaError` number (1..=15) and no journal: LEZ charges a
//! non-zero exit by the cycles used, where a panic is charged the whole declared gas.

use attest_core::{AttestMessage, LcVerifyV3Message, lc_v3};
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};

lee_core::actor_entrypoint!(raw handle_input);

fn refuse(e: attest_core::EsaError) -> ! {
    #[cfg(target_os = "zkvm")]
    {
        risc0_zkvm::guest::env::exit(e as u8)
    }
    #[cfg(not(target_os = "zkvm"))]
    {
        e.abort()
    }
}

fn handle_input(input: &ReceiveInput) -> Response {
    lc_v3::mark("start");
    let Ok(message) = borsh::from_slice::<LcVerifyV3Message>(&input.message) else {
        refuse(attest_core::EsaError::E01Malformed)
    };
    let effect = lc_v3::handle(
        &input.receiver.program_account_id,
        &input.receiver.account_id,
        &input.pre_state,
        message,
    )
    .unwrap_or_else(|e| refuse(e));
    let mut response = match effect.state {
        Some(state) => Response::set_state(state),
        None => Response::keep_state(),
    };
    if let Some((from, to)) = effect.window_ms {
        response = response
            .try_timestamp_window(from..to)
            .unwrap_or_else(|_| refuse(attest_core::EsaError::E06StaleCheckpoint));
    }
    if let Some((instance, attest_program, out)) = effect.apply {
        response = response.send_sync(
            Actor::new(instance, attest_program),
            &AttestMessage::Apply {
                pre_store: out.pre_store,
                post_store: out.post_store,
                head: out.head,
                record: out.record,
            },
        );
    }
    lc_v3::mark("end");
    response
}
