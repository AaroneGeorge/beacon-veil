//! Program `lc_verify_v3_v03`: the public-path light client (`lc_verify_v3`) as an LEZ v0.3 program
//! (`plan` + `apply`). Every check is `attest_core::lc_v3::handle`, unchanged, run in `apply` on the job shard.
//!
//! One update is three public transactions on the job account `job_id(self, job_seed(instance, pre_store, update))`:
//! `Begin`, `Miller`, `Finish`. The instruction is `Instruction { claims, message }`, where `message` is the
//! unchanged borsh `LcVerifyV3Message`.
//!
//! Why claims: on LEZ v0.3, `plan` sees no state and `apply` cannot call another program (`ChainedCallsFromApply`),
//! and only `plan` sets the timestamp window. So the step that reads the job (`apply`) cannot be the one that sets
//! the window or calls `attest_v03.Apply`. The instruction states both (the claims); `plan` sets the window and
//! declares the call from them; `apply` runs `lc_v3::handle` and refuses unless the window and the `Apply` fields it
//! computed equal the claims. A refusal reverts the whole transaction, the chained `Apply` included, so what reaches
//! the instance is what the verifier computed. This is LEZ's own "pinning" pattern (instruction states a value, the
//! `apply` that can read it checks it).
//!
//! - `plan`: accounts `[job]`, or for a `Finish` claim `[job, instance, record]` (the instance and record shards of
//!   `attest_v03`); the effect on the job is the instruction bytes as they are (no decode of the message); window
//!   `[claims.window_ms)`; for `Finish`, a chained call `attest_v03.Apply(claim)` on `[instance, record]`.
//! - `apply`: decodes the message, `lc_v3::handle(self, job, pre_data, message)`; window differs from the claim:
//!   E06; `Apply` fields differ (or are claimed where none are due): E15; writes the job.
//!
//! A refusal ends the program with exit code = the `EsaError` number (1..=15), never a panic.

use attest_core::{Bytes32, EsaError, LcVerifyV3Message, lc_v3};
use attest_v03::{ApplyClaim, core_id};
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    program::{AccountMeta, ChainedCall, Plan, PlanInput, TimestampValidityWindow},
};

/// The `Finish` claim: the call `attest_v03.Apply` that `Finish` makes.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FinishClaim {
    pub attest_program: Bytes32,
    pub instance: Bytes32,
    pub apply: ApplyClaim,
}

/// What the instruction states that `plan` cannot read.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Claims {
    /// The timestamp window `[from, to)` in milliseconds that the step sets (`Begin`, `Finish`), or `None`.
    pub window_ms: Option<(u64, u64)>,
    /// `Finish` only.
    pub finish: Option<FinishClaim>,
}

/// Instruction of `lc_verify_v3_v03`; also the effect on the job shard (the same bytes).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Instruction {
    pub claims: Claims,
    /// `borsh(LcVerifyV3Message)`.
    pub message: Vec<u8>,
}

impl Instruction {
    #[must_use]
    pub fn new(claims: Claims, message: &LcVerifyV3Message) -> Self {
        Self {
            claims,
            message: borsh::to_vec(message).unwrap_or_default(),
        }
    }
}

/// Bytes already borsh-encoded, written as they are.
struct Raw<'a>(&'a [u8]);

impl BorshSerialize for Raw<'_> {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(self.0)
    }
}

/// Reads `claims` and the message bytes of an encoded [`Instruction`] without copying the message.
pub fn split(bytes: &[u8]) -> Result<(Claims, &[u8]), EsaError> {
    let mut rest = bytes;
    let claims = Claims::deserialize(&mut rest).map_err(|_| EsaError::E01Malformed)?;
    let len = u32::deserialize(&mut rest).map_err(|_| EsaError::E01Malformed)?;
    if usize::try_from(len).ok() != Some(rest.len()) {
        return Err(EsaError::E01Malformed);
    }
    Ok((claims, rest))
}

fn shard(a: &AccountMeta, program: &AccountId) -> Result<(), EsaError> {
    if a.program_account_id == *program {
        Ok(())
    } else {
        Err(EsaError::E09ChainMismatch)
    }
}

/// `plan`: `instruction_data` is the encoded [`Instruction`]. No state is read and the message is not decoded.
pub fn plan(input: &PlanInput) -> Result<Plan, EsaError> {
    let (claims, _) = split(&input.instruction_data)?;
    let me = input.self_account_id;
    let mut plan = Plan::new(input);
    let job = match (input.accounts.as_slice(), &claims.finish) {
        ([job], None) => job,
        ([job, instance, record], Some(f)) => {
            let attest = AccountId::new(f.attest_program);
            shard(instance, &attest)?;
            shard(record, &attest)?;
            if instance.account_id != AccountId::new(f.instance) {
                return Err(EsaError::E09ChainMismatch);
            }
            plan.call(ChainedCall::new(
                attest,
                vec![
                    ProgramShardSelector::new(instance.account_id, attest),
                    ProgramShardSelector::new(record.account_id, attest),
                ],
                &attest_v03::Instruction::Apply(f.apply.clone()),
            ));
            job
        }
        _ => return Err(EsaError::E01Malformed),
    };
    shard(job, &me)?;
    plan.effect(job, &Raw(&input.instruction_data));
    if let Some((from, to)) = claims.window_ms {
        let w = TimestampValidityWindow::try_from(from..to).map_err(|_| EsaError::E06StaleCheckpoint)?;
        plan.timestamp_window(w);
    }
    Ok(plan)
}

/// `apply` on the job shard: the step itself. Returns the new job state.
pub fn apply(self_program: &AccountId, job: &AccountId, effect: &[u8], pre: &[u8]) -> Result<Vec<u8>, EsaError> {
    let (claims, message) = split(effect)?;
    let message = LcVerifyV3Message::try_from_slice(message).map_err(|_| EsaError::E01Malformed)?;
    if matches!(message, LcVerifyV3Message::VerifyOnce { .. }) {
        // The private one-transaction path is not part of this port.
        return Err(EsaError::E01Malformed);
    }
    let step = lc_v3::handle(&core_id(self_program), &core_id(job), pre, message)?;
    if step.window_ms != claims.window_ms {
        return Err(EsaError::E06StaleCheckpoint);
    }
    let matches = match (&step.apply, &claims.finish) {
        (None, None) => true,
        (Some((instance, attest, out)), Some(f)) => {
            *instance.value() == f.instance
                && *attest.value() == f.attest_program
                && out.pre_store == f.apply.pre_store
                && out.post_store == f.apply.post_store
                && out.head == f.apply.head
                && out.record == f.apply.record
        }
        _ => false,
    };
    if !matches {
        return Err(EsaError::E15HeaderMismatch);
    }
    step.state.ok_or(EsaError::E01Malformed)
}
