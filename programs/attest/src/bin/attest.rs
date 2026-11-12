//! Program `attest` (PLAN.md 2.2 D1, 2.3; owner W1): light-client instances, `Apply` from an accepted verifier,
//! write-once header records, `CheckHeader`, and `Attest` (inclusion + predicate, then delivery to a consumer).
//!
//! Accounts (all under this program):
//! - instance `instance_id(attest, config)`: state `borsh(InstanceState { config, head })`;
//! - record `record_id(attest, instance, slot)`: state `borsh(HeaderRecord)`, written once.
//!
//! Thin wrapper: every check is in `attest_core::rules` (pure, also run by host tests). Every refusal panics with
//! `ESA:Exx`, which rejects the whole transaction.

use attest_core::{AttestMessage, EsaError, HeaderRecord, record_id, record_seed, rules};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};

lee_core::actor_entrypoint!(handle_message);

fn or_abort<T>(r: Result<T, EsaError>) -> T {
    r.unwrap_or_else(|e| e.abort())
}

fn write_record_message(program: AccountId, instance: AccountId, record: HeaderRecord) -> SyncMessage {
    let seed = PdaSeed::new(record_seed(&instance, record.slot));
    SyncMessage::new(
        Actor::new(record_id(&program, &instance, record.slot), program),
        &AttestMessage::WriteRecord { record },
    )
    .with_pda_seeds([seed])
}

fn handle_message(input: &ReceiveInput, message: AttestMessage) -> Response {
    let program = input.receiver.program_account_id;
    let me = input.receiver.account_id;
    match message {
        AttestMessage::Init { config, bootstrap } => {
            let (state, record) = or_abort(rules::init(&program, &me, &input.pre_state, &config, &bootstrap));
            Response::set_state(borsh::to_vec(&state).expect("borsh")).send(write_record_message(program, me, record))
        }
        AttestMessage::Apply {
            pre_store,
            post_store,
            head,
            record,
        } => {
            let sender = input.from.map(|a| a.program_account_id);
            let effect = or_abort(rules::apply(
                &input.pre_state,
                sender,
                pre_store,
                post_store,
                head,
                record,
            ));
            let response = Response::set_state(borsh::to_vec(&effect.new_state).expect("borsh"));
            match effect.write_record {
                Some(record) => response.send(write_record_message(program, me, record)),
                None => response,
            }
        }
        AttestMessage::WriteRecord { record } => {
            let from = input.from.filter(|_| input.from_own_program()).map(|a| a.account_id);
            Response::set_state(or_abort(rules::write_record(
                &program,
                &me,
                &input.pre_state,
                from,
                &record,
            )))
        }
        AttestMessage::CheckHeader { record_hash } => {
            or_abort(rules::check_header(&input.pre_state, &record_hash));
            Response::keep_state()
        }
        AttestMessage::Attest {
            instance,
            record,
            claim,
            predicate,
            deliver_to,
        } => {
            let statement = or_abort(attest_inclusion::verify_claim(instance, &record, &claim, &predicate));
            let record_actor = Actor::new(record_id(&program, &instance, record.slot), program);
            Response::keep_state()
                .send_sync(
                    record_actor,
                    &AttestMessage::CheckHeader {
                        record_hash: record.hash(),
                    },
                )
                .send_sync(deliver_to, &statement)
        }
    }
}
