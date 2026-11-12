//! Program `bridge` (PLAN.md 2.4, owner W5). Freeze-1 stub: accepts any borsh bytes and rejects them.

use attest_core::EsaError;
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, message: Vec<u8>) -> Response {
    let _ = (input, message);
    EsaError::E01Malformed.abort()
}
