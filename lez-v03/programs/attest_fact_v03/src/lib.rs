//! Program `attest_fact_v03`: the `Attest` of beacon-veil as an LEZ v0.3 program (`plan` + `apply`), against the
//! header records of the deployed `attest_v03`.
//!
//! `attest_v03` is deployed immutable with `Init` and `Apply` only, so `Attest` is a second program. It reads the
//! header record where `attest_v03` keeps it, through LEZ 0.3's inspection of a foreign shard (`Plan::inspect`: this
//! program's `apply` runs on `attest_v03`'s shard and may not write it). The `attest_v03` it trusts is fixed in the
//! code ([`ATTEST_V03`]), so no instruction can point it at another program's records.
//!
//! Accounts:
//! - record `record_id(ATTEST_V03, instance, slot)`, `attest_v03`'s shard: inspected, never written;
//! - fact `fact_id(self, instance, statement.id)`, this program's shard: `borsh(Statement)`, written once.
//!
//! Instruction `Attest { instance, record, claim, predicate }`, accounts `[record, fact]`, top-level, anyone:
//! - `plan` runs `attest_inclusion::verify_claim` on the instruction (inclusion under the record's execution roots,
//!   the execution header against `record.exec_hash`, then the predicate) and gets the `Statement`; it checks both
//!   account handles and declares two effects;
//! - `apply` on the record shard: `rules::check_header` (E02 if no record, E15 unless `sha256(shard) ==
//!   sha256(borsh(record))`), keeps the shard;
//! - `apply` on the fact shard: writes the `Statement` (E14 if one is there).
//!
//! A refusal ends the program with exit code = the `EsaError` number; it reverts the whole transaction.

use attest_core::{Bytes32, EsaError, HeaderRecord, Predicate, Statement, record_id, rules};
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::AccountId,
    program::{PdaSeed, Plan, PlanInput},
};
use sha2::{Digest, Sha256};

/// The `attest_v03` program on LEZ testnet 0.3 (program-loader header `8SXSF3i2Ht8acLuzDYYKitVtDfdkKmg4iAkkhFV1GYM3`,
/// immutable, image id `6daec75d…67aae0`): the only program whose header records this program reads.
pub const ATTEST_V03: [u8; 32] = [
    0x6e, 0x8b, 0xbd, 0xee, 0x04, 0x01, 0x24, 0x49, 0x48, 0x9f, 0xab, 0x98, 0xf1, 0xde, 0x14, 0x40, 0x1d, 0xed, 0x8a,
    0xec, 0x60, 0x6d, 0xc5, 0xbf, 0xba, 0x5e, 0xe8, 0xb7, 0xf9, 0xcc, 0x22, 0x3e,
];

/// Domain of the fact address seed.
pub const FACT_DOMAIN: &[u8] = b"esa/fact/v03";

/// `sha256("esa/fact/v03" || instance || statement_id)`
#[must_use]
pub fn fact_seed(instance: &Bytes32, statement_id: &Bytes32) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(FACT_DOMAIN);
    h.update(instance);
    h.update(statement_id);
    h.finalize().into()
}

/// `fact_id = PDA(attest_fact_v03, fact_seed(instance, statement_id))`
#[must_use]
pub fn fact_id(program: &AccountId, instance: &Bytes32, statement_id: &Bytes32) -> AccountId {
    AccountId::for_public_pda(program, &PdaSeed::new(fact_seed(instance, statement_id)))
}

/// `record_id(ATTEST_V03, instance, slot)` as an LEZ v0.3 account id.
#[must_use]
pub fn record_of(instance: &Bytes32, slot: u64) -> AccountId {
    let r = record_id(
        &attest_core::AccountId::new(ATTEST_V03),
        &attest_core::AccountId::new(*instance),
        slot,
    );
    AccountId::new(*r.value())
}

/// Instruction of `attest_fact_v03`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    Attest {
        /// The `attest_v03` instance whose record proves the fact.
        instance: Bytes32,
        /// The header record, as `attest_v03` stores it at `record_id(ATTEST_V03, instance, record.slot)`.
        record: HeaderRecord,
        /// `borsh(attest_inclusion::Claim)`.
        claim: Vec<u8>,
        predicate: Predicate,
    },
}

/// Effect `plan` declares, applied to one shard.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    /// On `attest_v03`'s record shard (inspected): the record there must hash to `record_hash`.
    CheckHeader { record_hash: Bytes32 },
    /// On this program's fact shard: write the statement once.
    Write { statement: Statement },
}

/// The statement `Attest` with these fields proves, as `plan` computes it (also used by clients and tests).
pub fn statement(
    instance: &Bytes32,
    record: &HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
) -> Result<Statement, EsaError> {
    attest_inclusion::verify_claim(attest_core::AccountId::new(*instance), record, claim, predicate)
}

/// `plan`: verifies the claim, checks the handles, declares the effects. No state is read.
pub fn plan(input: &PlanInput, instruction: Instruction) -> Result<Plan, EsaError> {
    let Instruction::Attest {
        instance,
        record,
        claim,
        predicate,
    } = instruction;
    let [record_acc, fact_acc] = input.accounts.as_slice() else {
        return Err(EsaError::E01Malformed);
    };
    let st = statement(&instance, &record, &claim, &predicate)?;
    let attest = AccountId::new(ATTEST_V03);
    if record_acc.program_account_id != attest
        || record_acc.account_id != record_of(&instance, record.slot)
        || fact_acc.program_account_id != input.self_account_id
        || fact_acc.account_id != fact_id(&input.self_account_id, &instance, &st.id)
    {
        return Err(EsaError::E09ChainMismatch);
    }
    let mut plan = Plan::new(input);
    plan.inspect(
        record_acc,
        attest,
        &Effect::CheckHeader {
            record_hash: record.hash(),
        },
    );
    plan.effect(fact_acc, &Effect::Write { statement: st });
    Ok(plan)
}

/// `apply`: one effect on one shard. `Ok(None)` keeps the shard, `Ok(Some(bytes))` writes it.
pub fn apply(
    self_program: &AccountId,
    shard_program: &AccountId,
    effect: Effect,
    pre: &[u8],
) -> Result<Option<Vec<u8>>, EsaError> {
    match effect {
        Effect::CheckHeader { record_hash } => {
            if *shard_program != AccountId::new(ATTEST_V03) {
                return Err(EsaError::E09ChainMismatch);
            }
            rules::check_header(pre, &record_hash)?;
            Ok(None)
        }
        Effect::Write { statement } => {
            if shard_program != self_program {
                return Err(EsaError::E09ChainMismatch);
            }
            if !pre.is_empty() {
                return Err(EsaError::E14AlreadyApplied);
            }
            Ok(Some(borsh::to_vec(&statement).unwrap_or_default()))
        }
    }
}
