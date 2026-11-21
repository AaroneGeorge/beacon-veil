//! Program `attest_v03`: the `attest` program of beacon-veil as an LEZ v0.3 program (`plan` + `apply`).
//!
//! Accounts (shards of this program):
//! - instance `instance_id(attest_v03, config)`: `borsh(InstanceState { config, head })`;
//! - record `record_id(attest_v03, instance, slot)`: `borsh(HeaderRecord)`, written once.
//!
//! LEZ v0.3 splits a program into `plan` (sees the instruction and the account handles, no state) and `apply` (sees
//! one shard and one effect, cannot call other programs). Every check stays in `attest_core::rules`, run in `apply`
//! against the shard's state; `plan` only checks the handles and declares the effects. Where `plan` must name a value
//! it cannot read (the record `Init` writes, whether `Apply` writes a record), the instruction claims it and `apply`
//! refuses unless the claim equals what the rule computed.
//!
//! Instructions:
//! - `Init { config, bootstrap, record }`, accounts `[instance, record]`: `rules::init` on the instance shard (it must
//!   be empty and at `instance_id(self, config)`), which must give exactly `record`; the record shard is written once.
//! - `Apply(ApplyClaim)`, accounts `[instance]` or `[instance, record]`, only as a chained call from a program in
//!   `config.verifiers` (the caller id goes into the instance effect): `rules::apply` (E12, E13, E14, E15, E09 as
//!   on rc3); `writes_record` must equal whether the rule writes a record, and if it does, the record shard is
//!   written once.
//!
//! A refusal ends the program with exit code = the `EsaError` number (1..=15): LEZ charges a non-zero exit by the
//! cycles used and reverts the whole transaction.

use attest_core::{Bytes32, EsaError, Head, HeaderRecord, InstanceConfig, instance_id, record_id, rules};
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::AccountId,
    program::{AccountMeta, Plan, PlanInput},
};

/// An LEZ v0.3 account id as the verifier core's (rc3) id: the same 32 bytes.
#[must_use]
pub fn core_id(id: &AccountId) -> attest_core::AccountId {
    attest_core::AccountId::new(*id.value())
}

/// The verifier core's (rc3) id as an LEZ v0.3 account id.
#[must_use]
pub fn v03_id(id: &attest_core::AccountId) -> AccountId {
    AccountId::new(*id.value())
}

/// The fields of `Apply`, as `lc_verify_v3_v03`'s `Finish` claims them.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ApplyClaim {
    pub pre_store: Bytes32,
    pub post_store: Bytes32,
    pub head: Head,
    pub record: HeaderRecord,
    /// The finalized slot moves, so a record is written at `record_id(self, instance, record.slot)`.
    pub writes_record: bool,
}

/// Instruction of `attest_v03`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    Init {
        config: InstanceConfig,
        bootstrap: Vec<u8>,
        /// The checkpoint's header record, as `rules::init` computes it.
        record: HeaderRecord,
    },
    Apply(ApplyClaim),
}

/// Effect `plan` declares, applied to one shard.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    /// On the instance shard.
    Init {
        config: InstanceConfig,
        bootstrap: Vec<u8>,
        record: HeaderRecord,
    },
    /// On the instance shard. `caller`: the calling program (`None` for a top-level transaction).
    Apply { caller: Option<Bytes32>, claim: ApplyClaim },
    /// On a record shard, for `instance`.
    Record { instance: Bytes32, record: HeaderRecord },
}

fn own<'a>(input: &PlanInput, a: &'a AccountMeta) -> Result<&'a AccountMeta, EsaError> {
    if a.program_account_id == input.self_account_id {
        Ok(a)
    } else {
        Err(EsaError::E09ChainMismatch)
    }
}

/// `plan`: checks the handles, declares the effects. No state is read.
pub fn plan(input: &PlanInput, instruction: Instruction) -> Result<Plan, EsaError> {
    let me = core_id(&input.self_account_id);
    let mut plan = Plan::new(input);
    match instruction {
        Instruction::Init {
            config,
            bootstrap,
            record,
        } => {
            let [instance, record_acc] = input.accounts.as_slice() else {
                return Err(EsaError::E01Malformed);
            };
            let (instance, record_acc) = (own(input, instance)?, own(input, record_acc)?);
            let inst = instance_id(&me, &config);
            if core_id(&instance.account_id) != inst
                || core_id(&record_acc.account_id) != record_id(&me, &inst, record.slot)
            {
                return Err(EsaError::E09ChainMismatch);
            }
            plan.effect(
                record_acc,
                &Effect::Record {
                    instance: *inst.value(),
                    record: record.clone(),
                },
            );
            plan.effect(
                instance,
                &Effect::Init {
                    config,
                    bootstrap,
                    record,
                },
            );
        }
        Instruction::Apply(claim) => {
            let (instance, record_acc) = match input.accounts.as_slice() {
                [i] => (own(input, i)?, None),
                [i, r] => (own(input, i)?, Some(own(input, r)?)),
                _ => return Err(EsaError::E01Malformed),
            };
            let inst = core_id(&instance.account_id);
            plan.effect(
                instance,
                &Effect::Apply {
                    caller: input.caller_account_id.map(|c| *c.value()),
                    claim: claim.clone(),
                },
            );
            if claim.writes_record {
                let record_acc = record_acc.ok_or(EsaError::E01Malformed)?;
                if core_id(&record_acc.account_id) != record_id(&me, &inst, claim.record.slot) {
                    return Err(EsaError::E09ChainMismatch);
                }
                plan.effect(
                    record_acc,
                    &Effect::Record {
                        instance: *inst.value(),
                        record: claim.record,
                    },
                );
            }
        }
    }
    Ok(plan)
}

/// `apply`: one effect on one shard of this program. `Ok(None)` keeps the shard, `Ok(Some(bytes))` writes it.
pub fn apply(
    self_program: &AccountId,
    account: &AccountId,
    effect: Effect,
    pre: &[u8],
) -> Result<Option<Vec<u8>>, EsaError> {
    let me = core_id(self_program);
    let acc = core_id(account);
    match effect {
        Effect::Init {
            config,
            bootstrap,
            record,
        } => {
            let (state, computed) = rules::init(&me, &acc, pre, &config, &bootstrap)?;
            if computed != record {
                return Err(EsaError::E15HeaderMismatch);
            }
            Ok(Some(encode(&state)))
        }
        Effect::Apply { caller, claim } => {
            let out = rules::apply(
                pre,
                caller.map(attest_core::AccountId::new),
                claim.pre_store,
                claim.post_store,
                claim.head,
                claim.record,
            )?;
            if out.write_record.is_some() != claim.writes_record {
                return Err(EsaError::E15HeaderMismatch);
            }
            Ok(Some(encode(&out.new_state)))
        }
        Effect::Record { instance, record } => Ok(Some(rules::write_record(
            &me,
            &acc,
            pre,
            Some(attest_core::AccountId::new(instance)),
            &record,
        )?)),
    }
}

fn encode<T: BorshSerialize>(v: &T) -> Vec<u8> {
    borsh::to_vec(v).unwrap_or_default()
}
