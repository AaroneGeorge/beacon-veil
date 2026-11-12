//! Program `attest` (PLAN.md 2.3, owner W1). Freeze-1 stub: decodes the message and rejects every variant.

use attest_core::{AttestMessage, EsaError};
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: AttestMessage) -> Response {
    let _ = input;
    match message {
        AttestMessage::Init { .. }
        | AttestMessage::Apply { .. }
        | AttestMessage::WriteRecord { .. }
        | AttestMessage::CheckHeader { .. }
        | AttestMessage::Attest { .. } => EsaError::E01Malformed.abort(),
    }
}
