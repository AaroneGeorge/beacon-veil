//! Guest entry of `attest_v03` (logic in `src/lib.rs`). Every refusal, a malformed instruction included, exits with
//! the `EsaError` number, never a panic: LEZ charges a non-zero exit by its cycles, a panic by the whole gas limit.

use attest_core::EsaError;
use attest_v03::{Effect, Instruction, apply, plan};
use lee_core::program::{ApplyInput, CallKind, PlanInput, apply_keep, apply_write, read_input_frame};

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
            let instruction: Instruction = decode(&input.instruction_data);
            plan(&input, instruction).unwrap_or_else(|e| refuse(e)).write()
        }
        CallKind::Apply => {
            let input: ApplyInput = decode(&payload);
            let effect: Effect = decode(&input.effect_data);
            match apply(
                &input.self_account_id,
                &input.selector.account_id,
                effect,
                &input.pre_data,
            ) {
                Ok(Some(data)) => match data.try_into() {
                    Ok(data) => apply_write(input, data),
                    Err(_) => refuse(EsaError::E01Malformed),
                },
                Ok(None) => apply_keep(input),
                Err(e) => refuse(e),
            }
        }
    }
}
