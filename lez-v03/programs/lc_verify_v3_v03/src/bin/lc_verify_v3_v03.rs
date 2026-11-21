//! Guest entry of `lc_verify_v3_v03` (logic in `src/lib.rs`). Every refusal, a malformed instruction included, exits
//! with the `EsaError` number, never a panic: LEZ charges a non-zero exit by its cycles, a panic by the whole limit.

use attest_core::EsaError;
use lc_verify_v3_v03::{apply, plan};
use lee_core::program::{ApplyInput, CallKind, PlanInput, apply_write, read_input_frame};

fn refuse(e: EsaError) -> ! {
    #[cfg(target_os = "zkvm")]
    {
        risc0_zkvm::guest::env::exit(e as u8)
    }
    #[cfg(not(target_os = "zkvm"))]
    {
        e.abort()
    }
}

fn decode<T: borsh::BorshDeserialize>(bytes: &[u8]) -> T {
    borsh::from_slice(bytes).unwrap_or_else(|_| refuse(EsaError::E01Malformed))
}

fn main() {
    let kind: CallKind = decode(&read_input_frame());
    let payload = read_input_frame();
    match kind {
        CallKind::Plan => {
            let input: PlanInput = decode(&payload);
            plan(&input).unwrap_or_else(|e| refuse(e)).write()
        }
        CallKind::Apply => {
            let input: ApplyInput = decode(&payload);
            let state = apply(
                &input.self_account_id,
                &input.selector.account_id,
                &input.effect_data,
                &input.pre_data,
            )
            .unwrap_or_else(|e| refuse(e));
            match state.try_into() {
                Ok(data) => apply_write(input, data),
                Err(_) => refuse(EsaError::E01Malformed),
            }
        }
    }
}
