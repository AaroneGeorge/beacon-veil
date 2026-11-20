//! The LEZ side of the follower: read the instance's verified store status and submit updates.
//!
//! [`HostChain`] goes through `attest_host` to a LEZ sequencer: Path A (one private transaction), the B0 probe, or
//! the public path (`lc_verify_v3`: `Begin`, `Miller`, `Finish` as public transactions to a job account, through
//! `attest_host::submit_update_staged`). [`MockChain`] is a file-backed stand-in that
//! applies Helios's store rules on the update metadata only (no signatures). It exists to test the follower loop,
//! restart-resume and the G4 checker before and beside the real programs. A run on the mock is never G4 evidence: its
//! `start` line says `chain: "mock"`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, EsaError, lc_v3};
use attest_host::{DeployedIds, StoreStatus, SubmitMode, TxOutcome};
use serde::{Deserialize, Serialize};

use crate::source::{LcObject, period_of};
use crate::ssz::LcKind;

/// What the follower wants to submit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// `finality_update`, advances the head within the period or across a handoff.
    Finality { handoff: bool },
    /// `updates_by_range(period, 1)`: carries the next sync committee.
    Period { period: u64 },
}

impl CandidateKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Finality { handoff: false } => "finality",
            Self::Finality { handoff: true } => "finality_handoff",
            Self::Period { .. } => "period",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub version: String,
    pub ssz: Vec<u8>,
    pub finalized_slot: u64,
    pub attested_slot: u64,
    pub signature_slot: u64,
    pub participation: u32,
    pub has_next_committee: bool,
}

impl Candidate {
    pub fn from_object(kind: CandidateKind, obj: &LcObject) -> Result<Self> {
        if obj.kind == LcKind::Bootstrap {
            bail!("a bootstrap is not an update");
        }
        Ok(Self {
            kind,
            version: obj.version.clone(),
            ssz: obj.to_ssz()?,
            finalized_slot: obj.finalized_slot()?,
            attested_slot: obj.attested_slot()?,
            signature_slot: obj.signature_slot()?,
            participation: obj.participation()?,
            has_next_committee: obj.has_next_committee(),
        })
    }
}

/// One transaction of a staged (public path) submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// `Begin`, `BeginPrepare`, `BeginKeys`, `Miller`, `Finish`.
    pub name: String,
    /// `None` for a step this follower landed in an earlier attempt whose result was lost (the job's on-chain stage
    /// shows it landed; its hash is in the `submit_deferred` line's sequencer log, not here).
    pub tx: Option<TxOutcome>,
    /// What the sequencer charged, read back from the step's block (public path, when readable).
    pub charge: Option<crate::charge::Charge>,
}

/// An accepted submission.
#[derive(Debug, Clone, Default)]
pub struct AppliedTx {
    /// The transaction that applied the update (`Finish` on the public path).
    pub tx: TxOutcome,
    /// Public path: every step this follower sent for the update, in order (empty otherwise).
    pub steps: Vec<Step>,
    /// Public path: the job account (base58).
    pub job: Option<String>,
    /// Public path: `Begin` ran as `BeginPrepare` + `BeginKeys`.
    pub four_steps: bool,
}

impl From<TxOutcome> for AppliedTx {
    fn from(tx: TxOutcome) -> Self {
        Self { tx, ..Self::default() }
    }
}

/// Result of a submission.
#[derive(Debug, Clone)]
pub enum Submitted {
    /// Transaction(s) accepted; this follower's transaction applied the update.
    Applied(AppliedTx),
    /// Refused before submission because it would change nothing (E14). Not a failure.
    NoOp(String),
    /// Public path: another submitter moved the store first (it finished the same job, or applied another update);
    /// the steps this follower sent, if any, are listed. Not a failure: the instance is current.
    Superseded { detail: String, steps: Vec<Step> },
    /// Public path: a step failed without a refusal (not included, the block was full, a sequencer error) after
    /// `attest_host`'s own retries, and the store did not move. The job stays on chain at its stage; the follower
    /// sends the same update again at the next poll, which continues the job where it stands.
    Deferred { detail: String, landed: Vec<Step> },
}

pub trait Chain {
    /// Short name for the log (`host`, `mock`).
    fn name(&self) -> &'static str;
    fn status(&self) -> impl std::future::Future<Output = Result<Option<StoreStatus>>> + Send;
    /// Submits `c`. `earlier`: steps of the same update this follower landed in an attempt that was deferred
    /// (public path; empty otherwise).
    fn submit(&self, c: &Candidate, earlier: &[Step]) -> impl std::future::Future<Output = Result<Submitted>> + Send;
    /// On-chain head `(slot, beacon_root, period)` without the local store, for store recovery. `None` if the chain
    /// needs no recovery path (mock) or the instance does not exist.
    fn head_root(&self) -> impl std::future::Future<Output = Result<Option<(u64, [u8; 32], u64)>>> + Send;
    /// Rebuilds the local verified store from a bootstrap at the head plus updates (attest_host decision 3 (c)).
    fn rebuild(
        &self,
        bootstrap_ssz: &[u8],
        updates: &[Vec<u8>],
    ) -> impl std::future::Future<Output = Result<StoreStatus>> + Send;
}

/// True if `e` is the "changes nothing" refusal (E14): `attest_host` gives every refusal its `EsaError` as root cause
/// (`lc_verify_v3` refusals by exit code included).
pub fn is_noop(e: &anyhow::Error) -> bool {
    attest_host::esa_code_v3(e) == Some(EsaError::E14AlreadyApplied)
}

/// ESA code of an error chain, for the log (`E13`, ...).
pub fn esa_code(e: &anyhow::Error) -> Option<String> {
    attest_host::esa_code_v3(e).map(|c| c.code().to_string())
}

/// How [`HostChain`] submits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostMode {
    /// `attest_host::submit_update` (Path A, or the B0 probe).
    Single(SubmitMode),
    /// The public path through `lc_verify_v3` (program id `v3`, which must be in the instance's `verifiers`).
    Public { v3: AccountId },
}

impl HostMode {
    /// The `[chain] mode` value.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Single(SubmitMode::Private) => "private",
            Self::Single(SubmitMode::PublicProbe) => "public_probe",
            Self::Public { .. } => "public",
        }
    }
}

/// Step names for the job stages a staged submission passed, from `before` (exclusive) to `after` (inclusive).
#[must_use]
pub fn steps_between(before: u8, after: u8, four_steps: bool) -> Vec<&'static str> {
    use lc_v3::stage::{BEGUN, DONE, MILLER, PREPARED};
    let all: &[(u8, &str)] = if four_steps {
        &[
            (PREPARED, "BeginPrepare"),
            (BEGUN, "BeginKeys"),
            (MILLER, "Miller"),
            (DONE, "Finish"),
        ]
    } else {
        &[(BEGUN, "Begin"), (MILLER, "Miller"), (DONE, "Finish")]
    };
    all.iter()
        .filter(|(s, _)| *s > before && *s <= after)
        .map(|(_, n)| *n)
        .collect()
}

/// Classifies a staged submission that this follower started on a store at `pre` (head slot, next committee known)
/// with the job at stage `stage_before`: the job's stage now, the store now, and the result.
pub fn classify_staged(
    result: Result<attest_host::StagedOutcome>,
    stage_before: u8,
    stage_after: u8,
    store_moved: bool,
    earlier: &[Step],
) -> Result<Submitted> {
    match result {
        Ok(o) => {
            let mut steps = earlier.to_vec();
            steps.extend(o.steps.iter().map(|(name, tx)| Step {
                name: name.clone(),
                tx: Some(tx.clone()),
                charge: None,
            }));
            let finish = o.steps.iter().find(|(n, _)| n == "Finish").map(|(_, t)| t.clone());
            match finish {
                Some(tx) => Ok(Submitted::Applied(AppliedTx {
                    tx,
                    steps,
                    job: Some(o.job.to_string()),
                    four_steps: o.four_steps,
                })),
                None => Ok(Submitted::Superseded {
                    detail: format!("job {} finished by another submitter", o.job),
                    steps,
                }),
            }
        }
        Err(e) if store_moved => Ok(Submitted::Superseded {
            detail: format!("the store moved during the submission (another submitter): {e:#}"),
            steps: earlier.to_vec(),
        }),
        Err(e) if attest_host::esa_code_v3(&e).is_some() => Err(e),
        Err(e) => {
            let mut landed = earlier.to_vec();
            landed.extend(
                steps_between(stage_before, stage_after, stage_after == lc_v3::stage::PREPARED)
                    .into_iter()
                    .map(|n| Step {
                        name: n.into(),
                        tx: None,
                        charge: None,
                    }),
            );
            Ok(Submitted::Deferred {
                detail: format!("job at stage {stage_after} (was {stage_before}): {e:#}"),
                landed,
            })
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------

/// `attest_host` against a LEZ sequencer. Wallet and sequencer come from the environment (`LEE_WALLET_HOME_DIR`,
/// `LEE_WALLET_PASSWORD`, `BV_STORE_DIR`).
#[derive(Debug, Clone)]
pub struct HostChain {
    pub ids: DeployedIds,
    pub instance: AccountId,
    pub mode: HostMode,
}

impl HostChain {
    /// `(job account, its stage)` of `update_ssz` on the instance's current store (0 when the job does not exist).
    async fn job_stage(&self, v3: AccountId, update_ssz: &[u8]) -> Result<(AccountId, u8)> {
        let head = attest_host::read_head(&self.ids, &self.instance)
            .await?
            .ok_or_else(|| anyhow!("instance {} does not exist", self.instance))?;
        let job = lc_v3::job_id(
            &v3,
            &lc_v3::job_seed(&self.instance, &head.store_commitment, update_ssz),
        );
        let stage = attest_host::read_job(v3, &job).await?.map_or(0, |j| j.stage);
        Ok((job, stage))
    }

    async fn head_mark(&self) -> Option<(u64, [u8; 32])> {
        attest_host::read_head(&self.ids, &self.instance)
            .await
            .ok()
            .flatten()
            .map(|h| (h.slot, h.store_commitment))
    }
}

impl Chain for HostChain {
    fn name(&self) -> &'static str {
        "host"
    }

    async fn status(&self) -> Result<Option<StoreStatus>> {
        attest_host::read_store_status(&self.ids, &self.instance).await
    }

    async fn submit(&self, c: &Candidate, earlier: &[Step]) -> Result<Submitted> {
        let v3 = match self.mode {
            HostMode::Single(mode) => {
                return match attest_host::submit_update(&self.ids, &self.instance, &c.ssz, mode).await {
                    Ok(o) => Ok(Submitted::Applied(o.into())),
                    Err(e) if is_noop(&e) => Ok(Submitted::NoOp(format!("{e:#}"))),
                    Err(e) => Err(e),
                };
            }
            HostMode::Public { v3 } => v3,
        };
        let before = self.head_mark().await;
        let stage_before = match self.job_stage(v3, &c.ssz).await {
            Ok((_, s)) => s,
            Err(_) => 0,
        };
        let result = attest_host::submit_update_staged(&self.ids, v3, &self.instance, &c.ssz).await;
        if let Err(e) = &result
            && is_noop(e)
        {
            return Ok(Submitted::NoOp(format!("{e:#}")));
        }
        let after = self.head_mark().await;
        let store_moved = before.is_some() && after.is_some() && before != after;
        // The job of the old store, to see how far it got (only meaningful when the store did not move).
        let stage_after = if result.is_err() && !store_moved {
            self.job_stage(v3, &c.ssz).await.map_or(stage_before, |(_, s)| s)
        } else {
            stage_before
        };
        // `store_moved` matters only for an error (an applied result moved the store by construction).
        let mut out = classify_staged(result, stage_before, stage_after, store_moved, earlier)?;
        if let Submitted::Applied(AppliedTx { steps, .. }) | Submitted::Superseded { steps, .. } = &mut out {
            for s in steps.iter_mut().filter(|s| s.charge.is_none()) {
                if let Some(tx) = &s.tx {
                    s.charge = crate::charge::charge_of(&tx.tx).await.ok();
                }
            }
        }
        Ok(out)
    }

    async fn head_root(&self) -> Result<Option<(u64, [u8; 32], u64)>> {
        Ok(attest_host::read_head(&self.ids, &self.instance)
            .await?
            .map(|h| (h.slot, h.beacon_root, h.period)))
    }

    async fn rebuild(&self, bootstrap_ssz: &[u8], updates: &[Vec<u8>]) -> Result<StoreStatus> {
        attest_host::rebuild_store(&self.ids, &self.instance, bootstrap_ssz, updates).await
    }
}

// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MockState {
    pub head_slot: u64,
    pub next_committee_known: bool,
    pub applied: u64,
    /// Staged mock only (tests): the next submissions fail after `Begin` landed, without a refusal (a full block).
    #[serde(default)]
    pub fail_next: u32,
    /// Staged mock only (tests): another submitter applies the next submission first.
    #[serde(default)]
    pub race_next: bool,
}

/// File-backed mock chain (Helios store rules on metadata; no signature checks). `staged`: answer as the public path
/// does (three steps per update, deferrals and contention from [`MockState`]).
#[derive(Debug, Clone)]
pub struct MockChain {
    pub path: PathBuf,
    pub staged: bool,
}

impl MockChain {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            staged: false,
        }
    }

    #[must_use]
    pub fn staged(mut self) -> Self {
        self.staged = true;
        self
    }

    pub fn save(&self, s: &MockState) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(s)?)
    }

    /// Creates the mock instance at a bootstrap slot (the mock's `Init`).
    pub fn init(&self, bootstrap_slot: u64) -> Result<()> {
        if self.path.exists() {
            bail!("mock instance {} exists", self.path.display());
        }
        write_atomic(
            &self.path,
            &serde_json::to_vec_pretty(&MockState {
                head_slot: bootstrap_slot,
                ..Default::default()
            })?,
        )
    }

    pub fn load(&self) -> Result<Option<MockState>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let b = std::fs::read(&self.path).with_context(|| format!("read {}", self.path.display()))?;
        Ok(Some(serde_json::from_slice(&b)?))
    }

    /// Helios `verify_generic_update` period checks and `apply_generic_update`, on metadata.
    pub fn apply(s: &MockState, c: &Candidate) -> std::result::Result<MockState, EsaError> {
        let store_period = period_of(s.head_slot);
        let sig_period = period_of(c.signature_slot);
        let valid_period = if s.next_committee_known {
            sig_period == store_period || sig_period == store_period + 1
        } else {
            sig_period == store_period
        };
        if !valid_period {
            return Err(EsaError::E05HandoffNotVerified);
        }
        if c.participation * 3 < 512 * 2 {
            return Err(EsaError::E04LowParticipation);
        }
        if c.attested_slot < c.finalized_slot || c.signature_slot <= c.attested_slot {
            return Err(EsaError::E01Malformed);
        }
        let fin_period = period_of(c.finalized_slot);
        let has_fin_next = c.has_next_committee && !s.next_committee_known && fin_period == period_of(c.attested_slot);
        let newer = c.finalized_slot > s.head_slot;
        let mut n = s.clone();
        if newer || has_fin_next {
            // Learn the next committee, or rotate into the next period (which forgets it unless the update carries it).
            if !s.next_committee_known || fin_period == store_period + 1 {
                n.next_committee_known = c.has_next_committee;
            }
            if newer {
                n.head_slot = c.finalized_slot;
            }
        }
        if n.head_slot == s.head_slot && n.next_committee_known == s.next_committee_known {
            return Err(EsaError::E14AlreadyApplied);
        }
        n.applied += 1;
        Ok(n)
    }
}

impl Chain for MockChain {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn status(&self) -> Result<Option<StoreStatus>> {
        Ok(self.load()?.map(|s| StoreStatus {
            head_slot: s.head_slot,
            period: period_of(s.head_slot),
            next_committee_known: s.next_committee_known,
        }))
    }

    async fn submit(&self, c: &Candidate, earlier: &[Step]) -> Result<Submitted> {
        use sha2::Digest;
        let mut s = self.load()?.ok_or_else(|| anyhow!("mock instance does not exist"))?;
        let tx = |step: &str| TxOutcome {
            tx: format!(
                "mock:{}",
                &hex::encode(sha2::Sha256::digest([&c.ssz[..], step.as_bytes()].concat()))[..16]
            ),
            cycles: None,
            gas: None,
        };
        if self.staged && s.race_next {
            s.race_next = false;
            if let Ok(n) = Self::apply(&s, c) {
                self.save(&n)?;
            }
            return Ok(Submitted::Superseded {
                detail: "job finished by another submitter (mock)".into(),
                steps: earlier.to_vec(),
            });
        }
        if self.staged && s.fail_next > 0 {
            s.fail_next -= 1;
            self.save(&s)?;
            let mut landed = earlier.to_vec();
            if !landed.iter().any(|x| x.name == "Begin") {
                landed.push(Step {
                    name: "Begin".into(),
                    tx: None,
                    charge: None,
                });
            }
            return Ok(Submitted::Deferred {
                detail: "Miller not included: block full (mock)".into(),
                landed,
            });
        }
        match Self::apply(&s, c) {
            Ok(n) => {
                self.save(&n)?;
                if !self.staged {
                    return Ok(Submitted::Applied(tx("").into()));
                }
                let mut steps = earlier.to_vec();
                for name in ["Begin", "Miller", "Finish"] {
                    if !steps.iter().any(|x| x.name == name) {
                        steps.push(Step {
                            name: name.into(),
                            tx: Some(tx(name)),
                            charge: None,
                        });
                    }
                }
                Ok(Submitted::Applied(AppliedTx {
                    tx: tx("Finish"),
                    steps,
                    job: Some("mock-job".into()),
                    four_steps: false,
                }))
            }
            Err(EsaError::E14AlreadyApplied) => Ok(Submitted::NoOp("ESA:E14 (mock)".into())),
            Err(e) => Err(anyhow::Error::new(e).context("mock chain rejected the update")),
        }
    }

    async fn head_root(&self) -> Result<Option<(u64, [u8; 32], u64)>> {
        Ok(None)
    }

    async fn rebuild(&self, _bootstrap_ssz: &[u8], _updates: &[Vec<u8>]) -> Result<StoreStatus> {
        bail!("the mock chain has no local store to rebuild")
    }
}

/// Write-then-rename, fsynced.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}
