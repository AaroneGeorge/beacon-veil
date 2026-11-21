//! The transactions of `Init` and of one update (`Begin`, `Miller`, `Finish`), with the claims computed natively by
//! the same core functions the programs run (`rules::init`, `lc_v3::handle`, `rules::apply`), so a client never
//! sends a claim the program would refuse.

use anyhow::{Result, anyhow};
use attest_core::{
    HeaderRecord, InstanceConfig, LcVerifyV3Message, instance_id,
    lc::{self, StoreWitness, UpdateOutcome},
    lc_v3::{self, StoreWitnessV3},
    record_id, rules,
};
use attest_v03::{ApplyClaim, core_id, v03_id};
use lc_verify_v3_v03::{Claims, FinishClaim, Instruction};
use lee_core::account::{AccountId, ProgramShardSelector};

/// Account ids of the two deployed programs (their program-loader headers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ids {
    pub attest: AccountId,
    pub v3: AccountId,
}

/// One transaction to send: program, shard selectors, borsh instruction bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tx {
    pub name: String,
    pub program: AccountId,
    pub selectors: Vec<ProgramShardSelector>,
    pub instruction: Vec<u8>,
    /// The timestamp window the step sets, if any (milliseconds).
    pub window_ms: Option<(u64, u64)>,
}

/// The instance id of `config` under `ids.attest`.
#[must_use]
pub fn instance(ids: &Ids, config: &InstanceConfig) -> AccountId {
    v03_id(&instance_id(&core_id(&ids.attest), config))
}

/// The record id of `slot` of `instance`.
#[must_use]
pub fn record(ids: &Ids, instance: &AccountId, slot: u64) -> AccountId {
    v03_id(&record_id(&core_id(&ids.attest), &core_id(instance), slot))
}

/// `Init` of an instance from `bootstrap`; also returns the instance state it creates and the checkpoint record.
pub fn init(ids: &Ids, config: &InstanceConfig, bootstrap: &[u8]) -> Result<(Tx, Vec<u8>, HeaderRecord)> {
    let inst = instance(ids, config);
    let (state, rec) = rules::init(&core_id(&ids.attest), &core_id(&inst), &[], config, bootstrap)
        .map_err(|e| anyhow!("Init refused natively: {e}"))?;
    let tx = Tx {
        name: "Init".into(),
        program: ids.attest,
        selectors: vec![
            ProgramShardSelector::new(inst, ids.attest),
            ProgramShardSelector::new(record(ids, &inst, rec.slot), ids.attest),
        ],
        instruction: borsh::to_vec(&attest_v03::Instruction::Init {
            config: config.clone(),
            bootstrap: bootstrap.to_vec(),
            record: rec.clone(),
        })?,
        window_ms: None,
    };
    Ok((tx, borsh::to_vec(&state)?, rec))
}

/// The three transactions of one update, and what they lead to.
pub struct Update {
    pub job: AccountId,
    pub txs: Vec<Tx>,
    /// The native (Helios) outcome: the witness after the update, and the `Apply` fields.
    pub native: UpdateOutcome,
    /// The instance state after `Finish`, as `rules::apply` computes it.
    pub instance_state: Vec<u8>,
    /// The job state after each step.
    pub job_states: Vec<Vec<u8>>,
}

/// The messages of one update on the three-step path.
pub fn messages(
    ids: &Ids,
    config: &InstanceConfig,
    w: &StoreWitness,
    update_ssz: &[u8],
) -> Result<(UpdateOutcome, AccountId, Vec<(&'static str, LcVerifyV3Message)>)> {
    let native = lc::verify_and_apply(config, w, update_ssz).map_err(|e| anyhow!("native verify refused: {e}"))?;
    let inst = core_id(&instance(ids, config));
    let w3 = StoreWitnessV3::from_v1(w, native.signature_slot);
    let absent_keys = lc_v3::absent_keys(&w3.committee_ssz, update_ssz);
    let job = lc_v3::job_id(
        &core_id(&ids.v3),
        &lc_v3::job_seed(&inst, &native.pre_store, update_ssz),
    );
    let msgs = vec![
        (
            "Begin",
            LcVerifyV3Message::Begin {
                attest_program: core_id(&ids.attest),
                instance: inst,
                config: config.clone(),
                witness: w3,
                update_ssz: update_ssz.to_vec(),
                absent_keys,
            },
        ),
        ("Miller", LcVerifyV3Message::Miller),
        ("Finish", LcVerifyV3Message::Finish),
    ];
    Ok((native, v03_id(&job), msgs))
}

/// Builds the three transactions of `update_ssz` against the instance state `instance_state` (claims computed by
/// running the steps natively).
pub fn update(
    ids: &Ids,
    config: &InstanceConfig,
    w: &StoreWitness,
    instance_state: &[u8],
    update_ssz: &[u8],
) -> Result<Update> {
    let (native, job, msgs) = messages(ids, config, w, update_ssz)?;
    let inst = instance(ids, config);
    let mut job_state: Vec<u8> = Vec::new();
    let mut txs = Vec::new();
    let mut job_states = Vec::new();
    let mut new_instance = instance_state.to_vec();
    for (name, msg) in msgs {
        let step = lc_v3::handle(&core_id(&ids.v3), &core_id(&job), &job_state, msg.clone())
            .map_err(|e| anyhow!("{name} refused natively: {e}"))?;
        let mut selectors = vec![ProgramShardSelector::new(job, ids.v3)];
        let finish = match &step.apply {
            None => None,
            Some((instance_core, attest_core_id, out)) => {
                let applied = rules::apply(
                    instance_state,
                    Some(core_id(&ids.v3)),
                    out.pre_store,
                    out.post_store,
                    out.head.clone(),
                    out.record.clone(),
                )
                .map_err(|e| anyhow!("Apply refused natively: {e}"))?;
                new_instance = borsh::to_vec(&applied.new_state)?;
                selectors.push(ProgramShardSelector::new(inst, ids.attest));
                selectors.push(ProgramShardSelector::new(
                    record(ids, &inst, out.record.slot),
                    ids.attest,
                ));
                Some(FinishClaim {
                    attest_program: *attest_core_id.value(),
                    instance: *instance_core.value(),
                    apply: ApplyClaim {
                        pre_store: out.pre_store,
                        post_store: out.post_store,
                        head: out.head.clone(),
                        record: out.record.clone(),
                        writes_record: applied.write_record.is_some(),
                    },
                })
            }
        };
        let claims = Claims {
            window_ms: step.window_ms,
            finish,
        };
        txs.push(Tx {
            name: name.into(),
            program: ids.v3,
            selectors,
            instruction: borsh::to_vec(&Instruction::new(claims, &msg))?,
            window_ms: step.window_ms,
        });
        job_state = step.state.clone().unwrap_or_default();
        job_states.push(job_state.clone());
    }
    Ok(Update {
        job,
        txs,
        native,
        instance_state: new_instance,
        job_states,
    })
}
