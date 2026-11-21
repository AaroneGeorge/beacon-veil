//! Shared by the tests and the client: the committed fixtures, the messages of one update, and an in-process LEZ
//! v0.3.2 state (`lee::V03State`) with both programs deployed, which executes public transactions exactly as the
//! sequencer's action phase does (`ValidatedStateDiff::from_public_transaction_metered`) and reports their cycles.

pub mod fixtures;
pub mod guests;
pub mod run;
pub mod steps;
pub mod world;
