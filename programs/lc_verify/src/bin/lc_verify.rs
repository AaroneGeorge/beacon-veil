//! Program `lc_verify` (PLAN.md 2.3, owner W1). Freeze-1 stub: decodes the message and rejects it.

use attest_core::{EsaError, LcVerifyMessage};
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: LcVerifyMessage) -> Response {
    let _ = input;
    match message {
        LcVerifyMessage::Verify { .. } => EsaError::E01Malformed.abort(),
    }
}
