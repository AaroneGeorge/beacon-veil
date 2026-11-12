//! Handlers of program `bridge` (PLAN.md 2.4, owner W5). The guest binary `src/bin/bridge.rs` only calls [`handle`].
//! Inbound `Claim` against a vault root attested through RFP-021, a
//! nullifier per deposit and a wrapped-balance credit; outbound `Burn` into a burn accumulator. See
//! docs/bridge/INBOUND.md and OUTBOUND.md, and `bridge_program::lez` for accounts and messages.
//!
//! Message dispatch: a message whose sender belongs to another program is a statement delivery (`borsh(Statement)`,
//! from `attest`); everything else is a `BridgeMessage`.

use crate::{
    hash::{burn_leaf, commitment, deposit_nullifier, dest_binding, leaf},
    lez::{
        Accumulator, BridgeConfig, BridgeError, BridgeMessage, BurnEvent, ClaimArgs, Holding, accumulator_seed,
        burn_event_selector, instance_id, leaf_record_seed, nullifier_seed, pda, root_seed, statement_proves_root,
    },
    tree::root_from_path,
};
use attest_core::{AccountId, AttestMessage, EsaError, Predicate, PredicateValue, Statement};
use lee_core::{
    account::Actor,
    program::{ProgramEvent, ReceiveInput, Response, SyncMessage},
};

/// The program: one delivery in, one response out. Panics (`ESA:Exx`, `WB:Bxx`) reject the transaction.
pub fn handle(input: &ReceiveInput) -> Response {
    let own = input.receiver.program_account_id;
    if let Some(from) = input.from
        && from.program_account_id != own
    {
        let stmt: Statement = borsh::from_slice(&input.message).unwrap_or_else(|_| EsaError::E01Malformed.abort());
        return receive_statement(input, &from.program_account_id, &stmt);
    }
    let msg: BridgeMessage = borsh::from_slice(&input.message).unwrap_or_else(|_| EsaError::E01Malformed.abort());
    match msg {
        BridgeMessage::Claim(args) => claim(input, &args),
        BridgeMessage::CheckRoot {
            attest_program,
            attest_instance,
            chain_id,
            vault,
            root,
        } => {
            own_program_only(input);
            let stmt_id = attest_core::statement_id_state(chain_id, &vault, &crate::DEPOSIT_ROOT_SLOT, &root);
            expect_receiver(
                input,
                &pda(&own, &root_seed(&attest_program, &attest_instance, &stmt_id)),
            );
            let Ok(stmt) = borsh::from_slice::<Statement>(&input.pre_state) else {
                BridgeError::RootNotAttested.abort()
            };
            if !statement_proves_root(&stmt, &attest_instance, chain_id, &vault, &root) {
                BridgeError::RootNotAttested.abort()
            }
            Response::keep_state()
        }
        BridgeMessage::Nullify { instance, nullifier } => {
            own_program_only(input);
            expect_receiver(input, &pda(&own, &nullifier_seed(&instance, &nullifier)));
            if !input.pre_state.is_empty() {
                EsaError::E11AlreadyConsumed.abort()
            }
            Response::set_state(vec![1_u8])
        }
        BridgeMessage::Credit {
            instance,
            token,
            amount,
        } => {
            own_program_only(input);
            Response::set_state(credit(&input.pre_state, instance, token, amount))
        }
        BridgeMessage::Burn {
            config,
            token,
            denom,
            note,
            memo,
        } => {
            if !input.is_authorized {
                BridgeError::NotAuthorized.abort()
            }
            let entry = config
                .token(&token)
                .unwrap_or_else(|| BridgeError::UnsupportedToken.abort());
            if !entry.denoms.contains(&denom) {
                BridgeError::InvalidAmount.abort()
            }
            let instance = instance_id(&own, &config);
            let debit = denom
                .checked_add(entry.burn_fee)
                .unwrap_or_else(|| BridgeError::InvalidAmount.abort());
            let mut holding = Holding::decode(&input.pre_state).unwrap_or_else(|| EsaError::E01Malformed.abort());
            let bal = holding.balance(&instance, &token);
            let Some(rest) = bal.checked_sub(debit) else {
                BridgeError::InsufficientBalance.abort()
            };
            holding.set(instance, token, rest);
            let seed = accumulator_seed(&instance);
            let mut resp = Response::set_state(borsh::to_vec(&holding).expect("borsh")).send(
                SyncMessage::new(
                    Actor::new(pda(&own, &seed), own),
                    &BridgeMessage::Append {
                        instance,
                        token,
                        denom,
                        note,
                        memo,
                    },
                )
                .with_pda_seeds([seed]),
            );
            if entry.burn_fee > 0
                && let Some(treasury) = config.treasury
            {
                resp = resp.send(SyncMessage::new(
                    Actor::new(treasury, own),
                    &BridgeMessage::Credit {
                        instance,
                        token,
                        amount: entry.burn_fee,
                    },
                ));
            }
            resp
        }
        BridgeMessage::Append {
            instance,
            token,
            denom,
            note,
            memo,
        } => {
            own_program_only(input);
            expect_receiver(input, &pda(&own, &accumulator_seed(&instance)));
            let mut acc: Accumulator = if input.pre_state.is_empty() {
                Accumulator::default()
            } else {
                borsh::from_slice(&input.pre_state).unwrap_or_else(|_| EsaError::E01Malformed.abort())
            };
            let index = acc.tree.insert(burn_leaf(&token, denom, &note));
            acc.root = acc.tree.root();
            let event = BurnEvent {
                instance,
                index,
                token,
                denom,
                note,
                memo,
                root: acc.root,
            };
            let lseed = leaf_record_seed(&instance, index);
            Response::set_state(borsh::to_vec(&acc).expect("borsh"))
                .event(ProgramEvent {
                    selector: burn_event_selector(),
                    data: borsh::to_vec(&event).expect("borsh"),
                })
                .send(
                    SyncMessage::new(Actor::new(pda(&own, &lseed), own), &BridgeMessage::RecordLeaf(event))
                        .with_pda_seeds([lseed]),
                )
        }
        BridgeMessage::RecordLeaf(event) => {
            own_program_only(input);
            expect_receiver(input, &pda(&own, &leaf_record_seed(&event.instance, event.index)));
            if !input.pre_state.is_empty() {
                BridgeError::WrongAccount.abort()
            }
            Response::set_state(borsh::to_vec(&event).expect("borsh"))
        }
    }
}

/// A statement from another program lands only on the receipt PDA its own sender, instance and id derive, so a
/// fake program or a fake light-client instance can only write receipts that no bridge config points at.
fn receive_statement(input: &ReceiveInput, sender_program: &AccountId, stmt: &Statement) -> Response {
    let own = input.receiver.program_account_id;
    expect_receiver(input, &pda(&own, &root_seed(sender_program, &stmt.instance, &stmt.id)));
    if input.pre_state.is_empty() {
        Response::set_state(borsh::to_vec(stmt).expect("borsh"))
    } else {
        // Same id means same chain, vault, slot and value: the first receipt is as good as any later one.
        Response::keep_state()
    }
}

fn claim(input: &ReceiveInput, a: &ClaimArgs) -> Response {
    let own = input.receiver.program_account_id;
    let config: &BridgeConfig = &a.config;
    let entry = config
        .token(&a.token)
        .unwrap_or_else(|| BridgeError::UnsupportedToken.abort());
    if !entry.denoms.contains(&a.denom) {
        BridgeError::InvalidAmount.abort()
    }
    let instance = instance_id(&own, config);

    // The opening: the destination is bound in the commitment, so nobody can redirect the claim (Security #3).
    let binding = dest_binding(instance.value(), a.dest.value());
    let c = commitment(&a.secret, &binding, &a.token, a.denom);
    if root_from_path(&leaf(&a.token, a.denom, &c), a.leaf_index, &a.path) != a.root {
        EsaError::E07InclusionInvalid.abort()
    }
    let nullifier = deposit_nullifier(&a.secret);

    let stmt_id = config.root_statement_id(&a.root);
    let rseed = root_seed(&config.attest_program, &config.attest_instance, &stmt_id);
    let receipt = Actor::new(pda(&own, &rseed), own);
    let mut resp = Response::keep_state();

    // 1. RFP-021: attest "vault.depositRoot == root" at a finalised header; delivered to the receipt.
    if let Some(call) = &a.attest {
        resp = resp.send(SyncMessage::new(
            Actor::new(call.on, config.attest_program),
            &AttestMessage::Attest {
                instance: config.attest_instance,
                record: call.record.clone(),
                claim: call.claim.clone(),
                predicate: Predicate::Eq(PredicateValue::Bytes32(a.root)),
                deliver_to: receipt,
            },
        ));
    }
    // 2. The receipt must hold that statement (from this or an earlier transaction).
    resp = resp.send(
        SyncMessage::new(
            receipt,
            &BridgeMessage::CheckRoot {
                attest_program: config.attest_program,
                attest_instance: config.attest_instance,
                chain_id: config.chain_id,
                vault: config.vault,
                root: a.root,
            },
        )
        .with_pda_seeds([rseed]),
    );
    // 3. One claim per deposit (R3).
    let nseed = nullifier_seed(&instance, &nullifier);
    resp = resp.send(
        SyncMessage::new(
            Actor::new(pda(&own, &nseed), own),
            &BridgeMessage::Nullify { instance, nullifier },
        )
        .with_pda_seeds([nseed]),
    );
    // 4. Credit the destination, net of the protocol fee; the fee to the treasury.
    let fee = entry.mint_fee.min(a.denom);
    let net = a.denom - fee;
    if a.dest == input.receiver.account_id {
        resp.post_state = Some(credit(&input.pre_state, instance, a.token, net).into());
    } else {
        resp = resp.send(SyncMessage::new(
            Actor::new(a.dest, own),
            &BridgeMessage::Credit {
                instance,
                token: a.token,
                amount: net,
            },
        ));
    }
    if fee > 0
        && let Some(treasury) = config.treasury
    {
        resp = resp.send(SyncMessage::new(
            Actor::new(treasury, own),
            &BridgeMessage::Credit {
                instance,
                token: a.token,
                amount: fee,
            },
        ));
    }
    resp
}

fn credit(pre_state: &[u8], instance: AccountId, token: crate::Addr, amount: u128) -> Vec<u8> {
    let mut holding = Holding::decode(pre_state).unwrap_or_else(|| EsaError::E01Malformed.abort());
    let bal = holding
        .balance(&instance, &token)
        .checked_add(amount)
        .unwrap_or_else(|| BridgeError::InvalidAmount.abort());
    holding.set(instance, token, bal);
    borsh::to_vec(&holding).expect("borsh")
}

fn own_program_only(input: &ReceiveInput) {
    if !input.from_own_program() {
        BridgeError::NotAuthorized.abort()
    }
}

fn expect_receiver(input: &ReceiveInput, expected: &AccountId) {
    if input.receiver.account_id != *expected {
        BridgeError::WrongAccount.abort()
    }
}
