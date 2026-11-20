//! Differential test: every case runs through unmodified Helios 0.12.0 (`verify_*` then `apply_*`) and
//! through each verifier column (V1 = `lc::verify_and_apply`, the reference; V2 = `lc::verify_and_apply_v2`; V3 plugs
//! in as one more [`Column`]). The pass rule: every column agrees with V1 on every case (accept or refuse, the error
//! code, and on accept the pre-store and post-store commitments, head and record).
//!
//! Cases (see [`cases`]): real best updates of adjacent periods and live finality updates of mainnet, Sepolia
//! (Gloas, with the Fulu-to-Gloas crossing) and Hoodi (Fulu), fetched once into `fixtures/diff/`; one tampered copy per
//! tamper class of every valid real update; optimistic updates; and a fabricated chain with our own keys
//! (participation sweep, previous-period signer, identity key, fabricated update against a real store).

#![forbid(unsafe_code)]

pub mod cases;
#[cfg(feature = "executor")]
pub mod exec;
pub mod layout;
pub mod load;
pub mod report;

use std::panic::{AssertUnwindSafe, catch_unwind};

use alloy_primitives::B256;
use attest_core::{
    Bytes32, EsaError, Head, HeaderRecord, InstanceConfig,
    lc::{self, StoreWitness},
};
use helios_consensus_core::{
    apply_finality_update, apply_optimistic_update, apply_update,
    errors::ConsensusError,
    types::{FinalityUpdate, OptimisticUpdate, Update},
    verify_finality_update, verify_optimistic_update, verify_update,
};
use sha2::{Digest, Sha256};
use ssz::Decode;

/// What a case is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A real update, unchanged, on a store it applies to.
    Valid,
    /// A real update with one change (bytes, config or store).
    Tampered,
    /// A real optimistic update (no finalized header).
    Optimistic,
    /// The fabricated chain (our own keys).
    Fabricated,
}

impl Kind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Tampered => "tampered",
            Self::Optimistic => "optimistic",
            Self::Fabricated => "fabricated",
        }
    }
}

/// The verdict expected for a case. V1 is asserted against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Accept,
    Refuse(EsaError),
}

impl Expect {
    #[must_use]
    pub fn short(self) -> String {
        match self {
            Self::Accept => "accept".into(),
            Self::Refuse(e) => e.code().into(),
        }
    }
}

/// One differential case.
#[derive(Debug, Clone)]
pub struct Case {
    /// Stable id, for example `mainnet/p1909/handoff/bit_cleared`.
    pub id: String,
    /// `mainnet`, `sepolia`, `hoodi` (Fulu) or `fabricated`.
    pub network: String,
    /// Fork of the update's attested slot (`fulu`, `gloas`).
    pub fork: &'static str,
    pub kind: Kind,
    pub class: &'static str,
    /// The update is signed in a later fork than the store's finalized header (Fulu store, Gloas update).
    pub crossing: bool,
    /// Fixture file (and index) the update came from.
    pub source: String,
    pub config: InstanceConfig,
    pub witness: StoreWitness,
    pub update: Vec<u8>,
    pub expect: Expect,
    /// A difference from V1 that a later column is designed to have (listed in the report, never silent).
    pub divergence: Option<&'static str>,
}

impl Case {
    /// SHA-256 over `borsh(config) || borsh(witness) || update`: everything a verifier reads.
    #[must_use]
    pub fn input_sha256(&self) -> String {
        let mut h = Sha256::new();
        h.update(borsh::to_vec(&self.config).expect("borsh config"));
        h.update(borsh::to_vec(&self.witness).expect("borsh witness"));
        h.update(&self.update);
        hex::encode(h.finalize())
    }
}

/// What a verifier column did with a case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Accept(Box<Accepted>),
    Refuse(EsaError),
    /// The verifier panicked (never expected; reported as a finding).
    Panic(String),
}

/// The outputs `attest.Apply` consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub pre_store: Bytes32,
    pub post_store: Bytes32,
    pub head: Head,
    pub record: HeaderRecord,
}

impl Verdict {
    /// `accept:<first 8 bytes of the post-store commitment>` or the error code.
    #[must_use]
    pub fn short(&self) -> String {
        match self {
            Self::Accept(a) => format!("accept:{}", hex::encode(&a.post_store[..8])),
            Self::Refuse(e) => e.code().into(),
            Self::Panic(_) => "panic".into(),
        }
    }

    /// `short()` for a refusal; for an accept, `accept:` and the SHA-256 of `borsh(pre_store, post_store, head,
    /// record)`, so two accepts compare equal exactly when every field `attest.Apply` reads is equal.
    #[must_use]
    pub fn digest(&self) -> String {
        match self {
            Self::Accept(a) => {
                let mut h = Sha256::new();
                h.update(a.pre_store);
                h.update(a.post_store);
                h.update(borsh::to_vec(&a.head).expect("borsh head"));
                h.update(borsh::to_vec(&a.record).expect("borsh record"));
                format!("accept:{}", hex::encode(h.finalize()))
            }
            other => other.short(),
        }
    }

    #[must_use]
    pub fn matches(&self, e: Expect) -> bool {
        match (self, e) {
            (Self::Accept(_), Expect::Accept) => true,
            (Self::Refuse(a), Expect::Refuse(b)) => *a == b,
            _ => false,
        }
    }

    fn from_outcome(r: Result<lc::UpdateOutcome, EsaError>) -> Self {
        match r {
            Ok(o) => Self::Accept(Box::new(Accepted {
                pre_store: o.pre_store,
                post_store: o.post_store,
                head: o.head,
                record: o.record,
            })),
            Err(e) => Self::Refuse(e),
        }
    }
}

/// A verifier under test. The V3 column implements this over `lc_v3` (native) and over the guest in the executor.
pub trait Column: Sync {
    fn name(&self) -> &'static str;
    /// Verify `update_ssz` against the store `witness` under `config` and apply it; never panics on purpose.
    fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict;
}

/// V1: Helios 0.12.0 `verify_*` + `apply_*` with our pre-checks (`lc::verify_and_apply`), the reference.
pub struct V1;

impl Column for V1 {
    fn name(&self) -> &'static str {
        "v1"
    }
    fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict {
        Verdict::from_outcome(lc::verify_and_apply(config, witness, update_ssz))
    }
}

/// V2: Helios's rules with the signature check replaced (`lc::verify_and_apply_v2`).
pub struct V2;

impl Column for V2 {
    fn name(&self) -> &'static str {
        "v2"
    }
    fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict {
        Verdict::from_outcome(lc::verify_and_apply_v2(config, witness, update_ssz))
    }
}

/// The V3 witness and absent keys a submitter would send for `update_ssz` on the V1 store `witness`.
#[cfg(feature = "v3")]
fn v3_inputs(
    config: &InstanceConfig,
    witness: &StoreWitness,
    update_ssz: &[u8],
) -> (attest_core::lc_v3::StoreWitnessV3, Vec<[u8; 96]>) {
    use attest_core::lc_v3;
    let signature_slot = lc::forks(config)
        .ok()
        .and_then(|f| lc::decode_update(update_ssz, &f).ok())
        .map_or(0, |u| u.signature_slot());
    let w3 = lc_v3::StoreWitnessV3::from_v1(witness, signature_slot);
    let keys = lc_v3::absent_keys(&w3.committee_ssz, update_ssz);
    (w3, keys)
}

#[cfg(feature = "v3")]
fn from_v3(r: Result<attest_core::lc_v3::Outcome, EsaError>) -> Verdict {
    match r {
        Ok(o) => Verdict::Accept(Box::new(Accepted {
            pre_store: o.pre_store,
            post_store: o.post_store,
            head: o.head,
            record: o.record,
        })),
        Err(e) => Verdict::Refuse(e),
    }
}

/// V3: the staged public-path verifier run in one execution (`lc_v3::verify_once`, the code of `VerifyOnce`), with
/// the witness and absent keys a submitter builds (`StoreWitnessV3::from_v1`, `lc_v3::absent_keys`).
#[cfg(feature = "v3")]
pub struct V3;

#[cfg(feature = "v3")]
impl Column for V3 {
    fn name(&self) -> &'static str {
        "v3"
    }
    fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict {
        let (w3, keys) = v3_inputs(config, witness, update_ssz);
        from_v3(attest_core::lc_v3::verify_once(config, &w3, update_ssz, &keys))
    }
}

/// V3 as three transactions: `Begin`, `Miller`, `Finish` through the program logic (`lc_v3::handle`) on the job
/// account, the job state carried between steps; the verdict is `Finish`'s `Apply` or the first refusal.
#[cfg(feature = "v3")]
pub struct V3Staged;

#[cfg(feature = "v3")]
impl Column for V3Staged {
    fn name(&self) -> &'static str {
        "v3_staged"
    }
    fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict {
        use attest_core::{AccountId, LcVerifyV3Message as M, instance_id, lc_v3};
        const ATTEST: AccountId = AccountId::new([0xA7; 32]);
        const PROGRAM: AccountId = AccountId::new([0x33; 32]);
        let (w3, keys) = v3_inputs(config, witness, update_ssz);
        let instance = instance_id(&ATTEST, config);
        // The submitter derives the job account from the commitment of its witness.
        let pre = w3.commitment().unwrap_or([0u8; 32]);
        let job = lc_v3::job_id(&PROGRAM, &lc_v3::job_seed(&instance, &pre, update_ssz));
        let mut state = Vec::new();
        let steps = [
            M::Begin {
                attest_program: ATTEST,
                instance,
                config: config.clone(),
                witness: w3,
                update_ssz: update_ssz.to_vec(),
                absent_keys: keys,
            },
            M::Miller,
            M::Finish,
        ];
        for m in steps {
            match lc_v3::handle(&PROGRAM, &job, &state, m) {
                Err(e) => return Verdict::Refuse(e),
                Ok(e) => {
                    if let Some((_, _, o)) = e.apply {
                        return from_v3(Ok(o));
                    }
                    if let Some(s) = e.state {
                        state = s;
                    }
                }
            }
        }
        Verdict::Panic("Finish sent no Apply".into())
    }
}

fn guarded(col: &dyn Column, c: &Case) -> Verdict {
    catch_unwind(AssertUnwindSafe(|| col.verify(&c.config, &c.witness, &c.update))).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default();
        Verdict::Panic(msg)
    })
}

/// Unmodified Helios on a case: `verify_*` (with `expected_current_slot = signature_slot`, as V1) and, when it
/// passes, `apply_*` on a copy of the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeliosRaw {
    /// `ok`, a `ConsensusError` name, `decode` (Helios's SSZ decoder refused the bytes) or `other`.
    pub verify: String,
    /// `apply_*` moved the finalized header or learned the next committee.
    pub store_moved: bool,
}

impl HeliosRaw {
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.verify == "ok"
    }
}

fn helios_err(e: &eyre::Report) -> String {
    e.downcast_ref::<ConsensusError>().map_or_else(
        || "other".into(),
        |c| {
            let s = format!("{c:?}");
            s.split('(').next().unwrap_or_default().to_string()
        },
    )
}

/// Runs unmodified Helios on a case. Decoding picks the container by size as `lc::decode_update` does (a full update
/// carries a 24,624-byte committee), without our fork check.
#[must_use]
pub fn helios_raw(c: &Case) -> HeliosRaw {
    let r = catch_unwind(AssertUnwindSafe(|| helios_raw_inner(c)));
    r.unwrap_or_else(|_| HeliosRaw {
        verify: "panic".into(),
        store_moved: false,
    })
}

fn helios_raw_inner(c: &Case) -> HeliosRaw {
    let no = |v: &str| HeliosRaw {
        verify: v.into(),
        store_moved: false,
    };
    let Ok(forks) = lc::forks(&c.config) else {
        return no("config");
    };
    let Ok(loaded) = c.witness.load() else {
        return no("witness");
    };
    let mut store = loaded.store;
    let gvr = B256::from(c.config.genesis_validators_root);
    let before = (
        store.finalized_header.beacon().slot,
        store.next_sync_committee.is_some(),
    );
    let b = &c.update;
    let res = if b.len() > layout::COMMITTEE_LEN {
        let Ok(u) = Update::<lc::Spec>::from_ssz_bytes(b) else {
            return no("decode");
        };
        verify_update(&u, *u.signature_slot(), &store, gvr, &forks).map(|()| apply_update(&mut store, &u))
    } else if let Ok(u) = FinalityUpdate::<lc::Spec>::from_ssz_bytes(b) {
        verify_finality_update(&u, *u.signature_slot(), &store, gvr, &forks)
            .map(|()| apply_finality_update(&mut store, &u))
    } else if let Ok(u) = OptimisticUpdate::<lc::Spec>::from_ssz_bytes(b) {
        verify_optimistic_update(&u, *u.signature_slot(), &store, gvr, &forks)
            .map(|()| apply_optimistic_update(&mut store, &u))
    } else {
        return no("decode");
    };
    match res {
        Ok(_) => HeliosRaw {
            verify: "ok".into(),
            store_moved: (
                store.finalized_header.beacon().slot,
                store.next_sync_committee.is_some(),
            ) != before,
        },
        Err(e) => no(&helios_err(&e)),
    }
}

/// The result of one case.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub helios: HeliosRaw,
    /// One verdict per column, in column order (the first is V1).
    pub verdicts: Vec<Verdict>,
}

/// Runs every case through Helios and every column, on `threads` workers. Order of results = order of cases.
#[must_use]
pub fn run(cases: &[Case], columns: &[&dyn Column], threads: usize) -> Vec<Outcome> {
    let threads = threads.max(1);
    let mut out: Vec<Option<Outcome>> = vec![None; cases.len()];
    std::thread::scope(|s| {
        let chunks: Vec<_> = out
            .chunks_mut(cases.len().div_ceil(threads).max(1))
            .zip(cases.chunks(cases.len().div_ceil(threads).max(1)))
            .map(|(o, c)| {
                s.spawn(move || {
                    for (slot, case) in o.iter_mut().zip(c) {
                        *slot = Some(Outcome {
                            helios: helios_raw(case),
                            verdicts: columns.iter().map(|col| guarded(*col, case)).collect(),
                        });
                    }
                })
            })
            .collect();
        for h in chunks {
            h.join().expect("worker");
        }
    });
    out.into_iter().map(|o| o.expect("every case ran")).collect()
}

/// Every case of the differential test, in a fixed order: mainnet, sepolia, hoodi (Fulu), fabricated.
pub fn all_cases() -> anyhow::Result<Vec<Case>> {
    let nets: Vec<load::Network> = ["mainnet", "sepolia", "hoodi"]
        .into_iter()
        .map(load::load_network)
        .collect::<anyhow::Result<_>>()?;
    let mut cases = Vec::new();
    for (i, net) in nets.iter().enumerate() {
        // Another network's genesis validators root for the wrong-domain class.
        let other = &nets[(i + 1) % nets.len()].config.genesis_validators_root;
        cases::real_cases(net, *other, &mut cases);
    }
    // Fabricated chain impersonating mainnet in a fetched period; the real store for the same signature period is a
    // mainnet handoff store taken from the generated cases.
    let mainnet = &nets[0];
    let period = mainnet.ranges[0].start_period + 7;
    let real_store = cases
        .iter()
        .find(|c| c.id == format!("mainnet/p{period}/handoff/valid"))
        .map(|c| c.witness.clone())
        .ok_or_else(|| anyhow::anyhow!("no mainnet handoff store for period {period}"))?;
    cases::fabricated_cases(mainnet, period, &real_store, &mut cases);
    Ok(cases)
}

/// Columns of a default run: V1 (reference) and V2; with feature `v3` also V3 (one execution) and V3 as three
/// steps.
#[must_use]
pub fn default_columns() -> Vec<&'static dyn Column> {
    #[cfg(feature = "v3")]
    return vec![&V1, &V2, &V3, &V3Staged];
    #[cfg(not(feature = "v3"))]
    vec![&V1, &V2]
}

/// Builds every case, runs them, writes `evidence/diff/*` and returns the summary.
pub fn run_and_report(columns: &[&dyn Column], threads: usize) -> anyhow::Result<report::Summary> {
    let mut log = String::new();
    let t0 = std::time::Instant::now();
    let cases = all_cases()?;
    log.push_str(&format!(
        "built {} cases in {:.1}s\n",
        cases.len(),
        t0.elapsed().as_secs_f64()
    ));
    let t1 = std::time::Instant::now();
    let outcomes = run(&cases, columns, threads);
    log.push_str(&format!(
        "ran {} cases x (helios + {} columns) on {threads} threads in {:.1}s\n",
        cases.len(),
        columns.len(),
        t1.elapsed().as_secs_f64()
    ));
    let names: Vec<&str> = columns.iter().map(|c| c.name()).collect();
    let s = report::summarize(
        &cases,
        &outcomes,
        &names,
        &report::fixture_manifests(&load::diff_fixtures_dir()),
    );
    log.push_str(&format!(
        "total {} valid {} gloas {} v1_v2_disagreements {} failures {}\n",
        s.json["total"],
        s.json["valid"],
        s.json["gloas_cases"],
        s.json["v1_v2_disagreements"],
        s.failures.len()
    ));
    let mut s = s;
    let exec_path = report::evidence_dir().join("executor.json");
    if let Ok(b) = std::fs::read(&exec_path) {
        let exec: serde_json::Value = serde_json::from_slice(&b)?;
        report::add_executor(&mut s, &cases, &outcomes, &names, &exec);
        log.push_str(&format!(
            "executor.json: executor_cases {} executor_mismatches {} (image {})\n",
            s.json["executor_cases"], s.json["executor_mismatches"], s.json["executor"]["image_id"]
        ));
    }
    for f in &s.failures {
        log.push_str(&format!("FAIL {f}\n"));
    }
    report::write(&report::evidence_dir(), &s, &log)?;
    Ok(s)
}

/// Worker threads: `BV_DIFF_THREADS`, default 4.
#[must_use]
pub fn threads() -> usize {
    std::env::var("BV_DIFF_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4)
}
