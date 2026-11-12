//! DEV ONLY, never deployed outside a local sequencer. A stand-in "attest" for local end-to-end tests of the bridge
//! against an anvil vault, which no light client follows. It forwards `(deliver_to, Statement)` unchecked.
//!
//! This is safe to exist: the bridge keys root receipts by the sender's program id, so this program can only write
//! receipts that a bridge config naming THIS program as `attest_program` reads. A real config names the real
//! `attest` program. See docs/bridge/INBOUND.md, "Step 1".

use attest_core::{Actor, Statement};
use lee_core::program::{ReceiveInput, Response};

lee_core::actor_entrypoint!(handle);

fn handle(_input: &ReceiveInput, msg: (Actor, Statement)) -> Response {
    let (deliver_to, statement) = msg;
    Response::keep_state().send_sync(deliver_to, &statement)
}
