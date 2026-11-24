//! Program `consumer_erc20` (D-14 consumer rule, 021 U6 first case). See the library docs for the
//! pattern and the error table.
//!
//! Dispatch by sender (the runtime's word, not the message's):
//! - no sender (top level): `ConsumerMessage::Init`;
//! - this program: `ConsumerMessage::MarkConsumed` (nullifier);
//! - any other program: a bare borsh `Statement`, accepted only from `config.attest_program`.

use attest_core::{EsaError, Statement};
use consumer_erc20_program::{
    ConsumerMessage, ConsumerState, accept, consumer_instance_id, credited_selector, nullifier_id, nullifier_seed,
};
use lee_core::{
    account::Actor,
    program::{PdaSeed, ProgramEvent, ReceiveInput, Response, SyncMessage},
};

lee_core::actor_entrypoint!(raw handle_message);

fn handle_message(input: &ReceiveInput) -> Response {
    let program = input.receiver.program_account_id;
    match input.from {
        Some(from) if from.program_account_id != program => {
            let st: Statement = borsh::from_slice(&input.message).unwrap_or_else(|_| EsaError::E01Malformed.abort());
            on_statement(input, from.program_account_id, &st)
        }
        _ => {
            let msg: ConsumerMessage =
                borsh::from_slice(&input.message).unwrap_or_else(|_| EsaError::E01Malformed.abort());
            match msg {
                ConsumerMessage::Init { config } => {
                    if input.from.is_some()
                        || input.receiver.account_id != consumer_instance_id(&program, &config)
                        || !input.pre_state.is_empty()
                    {
                        EsaError::E01Malformed.abort();
                    }
                    Response::set_state(borsh::to_vec(&ConsumerState::new(config)).expect("borsh"))
                }
                ConsumerMessage::MarkConsumed {
                    consumer_instance,
                    statement_id,
                } => {
                    if !input.from_own_program()
                        || !input.is_authorized
                        || input.receiver.account_id != nullifier_id(&program, &consumer_instance, &statement_id)
                    {
                        EsaError::E01Malformed.abort();
                    }
                    if !input.pre_state.is_empty() {
                        EsaError::E11AlreadyConsumed.abort();
                    }
                    Response::set_state(consumer_instance.value().to_vec())
                }
            }
        }
    }
}

fn on_statement(input: &ReceiveInput, sender_program: attest_core::AccountId, st: &Statement) -> Response {
    let program = input.receiver.program_account_id;
    let me = input.receiver.account_id;
    let state: ConsumerState = borsh::from_slice(&input.pre_state).unwrap_or_else(|_| EsaError::E01Malformed.abort());
    let credit = accept(&state, &sender_program, st).unwrap_or_else(|e| e.abort());
    let event = ProgramEvent {
        selector: credited_selector(),
        data: borsh::to_vec(&credit).expect("borsh"),
    };
    let next = state.credited(credit);
    Response::set_state(borsh::to_vec(&next).expect("borsh"))
        .event(event)
        .send(
            SyncMessage::new(
                Actor::new(nullifier_id(&program, &me, &st.id), program),
                &ConsumerMessage::MarkConsumed {
                    consumer_instance: me,
                    statement_id: st.id,
                },
            )
            .with_pda_seeds([PdaSeed::new(nullifier_seed(&me, &st.id))]),
        )
}
