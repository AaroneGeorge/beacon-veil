//! Program `consumer_erc20` (PLAN.md 2.3 consumer rule, owner W3). Freeze-1 stub: receives a `Statement` and rejects it.

use attest_core::{EsaError, Statement};
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, statement: Statement) -> Response {
    let _ = (input, statement);
    EsaError::E12UnknownVerifier.abort()
}
