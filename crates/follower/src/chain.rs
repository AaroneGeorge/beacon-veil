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
use attest_core::{AccountId, EsaError};
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
    /// The transaction (hash, declared gas). `None` only on the staged mock for a step it did not send.
    pub tx: Option<TxOutcome>,
    /// What the sequencer charged, read back from the step's block (public path, when readable).
    pub charge: Option<crate::charge::Charge>,
    /// The transaction was sent but not included, or included and reverted (it is paid either way).
    pub error: Option<String>,
}

impl Step {
    /// A transaction `attest_host` reports as sent.
    #[must_use]
    pub fn from_sent(s: &attest_host::StagedSent) -> Self {
        Self {
            name: s.name.clone(),
            tx: Some(TxOutcome {
                tx: s.tx.clone(),
                cycles: None,
                gas: s.gas,
            }),
            charge: None,
            error: s.error.clone(),
        }
    }
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
    /// sends the same update again at the next poll, which continues the job where it stands. `landed`: every
    /// transaction sent for this update so far.
    Deferred { detail: String, landed: Vec<Step> },
    /// Public path: refused (an `EsaError`) while the store stayed where it was; the transactions sent for it are
    /// listed (they are paid).
    Failed {
        code: Option<String>,
        detail: String,
        steps: Vec<Step>,
    },
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
    /// Native balance of the account that pays the transactions, when the chain has one (`None` on the mock).
    fn payer_balance(&self) -> impl std::future::Future<Output = Option<u128>> + Send;
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

/// Classifies a staged submission: its result, every transaction it sent (`attest_host::submit_update_staged_traced`),
/// whether the instance's store moved during it, and the transactions sent for the same update in earlier, deferred
/// attempts. Applied only when this follower's own `Finish` was included; the store moving is checked before any
/// refusal code, so an E14 from a store another submitter moved is `Superseded`, and `NoOp` means nothing was sent.
pub fn classify_staged(
    result: Result<attest_host::StagedOutcome>,
    sent: &[attest_host::StagedSent],
    store_moved: bool,
    earlier: &[Step],
) -> Submitted {
    let mut steps = earlier.to_vec();
    steps.extend(sent.iter().map(Step::from_sent));
    let finish = sent.iter().rev().find(|s| s.name == "Finish" && s.error.is_none());
    match result {
        Ok(o) => match finish {
            Some(f) => Submitted::Applied(AppliedTx {
                tx: TxOutcome {
                    tx: f.tx.clone(),
                    cycles: None,
                    gas: f.gas,
                },
                steps,
                job: Some(o.job.to_string()),
                four_steps: o.four_steps,
            }),
            None => Submitted::Superseded {
                detail: format!("job {} finished by another submitter", o.job),
                steps,
            },
        },
        Err(e) if store_moved => Submitted::Superseded {
            detail: format!("the store moved during the submission (another submitter): {e:#}"),
            steps,
        },
        Err(e) if is_noop(&e) && steps.is_empty() => Submitted::NoOp(format!("{e:#}")),
        // A refusal, or a step that was included and reverted (charged its whole declared gas, never sent again).
        Err(e)
            if attest_host::esa_code_v3(&e).is_some()
                || sent
                    .iter()
                    .any(|s| s.error.as_deref().is_some_and(|x| x.contains(" reverted in block "))) =>
        {
            Submitted::Failed {
                code: esa_code(&e),
                detail: format!("{e:#}"),
                steps,
            }
        }
        Err(e) => Submitted::Deferred {
            detail: format!("{e:#}"),
            landed: steps,
        },
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
        let mut sent = Vec::new();
        let result = attest_host::submit_update_staged_traced(&self.ids, v3, &self.instance, &c.ssz, &mut sent).await;
        let after = self.head_mark().await;
        // `store_moved` matters only for an error (an applied result moved the store by construction).
        let store_moved = before.is_some() && after.is_some() && before != after;
        let mut out = classify_staged(result, &sent, store_moved, earlier);
        let steps = match &mut out {
            Submitted::Applied(AppliedTx { steps, .. })
            | Submitted::Superseded { steps, .. }
            | Submitted::Failed { steps, .. }
            | Submitted::Deferred { landed: steps, .. } => steps,
            Submitted::NoOp(_) => return Ok(out),
        };
        // Read the block of every included transaction (a reverted one is included; one that was never included
        // would make the read wait for blocks, so it is skipped).
        for s in steps.iter_mut().filter(|s| s.charge.is_none()) {
            let included = s.error.as_deref().is_none_or(|e| e.contains("reverted"));
            if let (Some(tx), true) = (&s.tx, included) {
                s.charge = crate::charge::charge_of(&tx.tx).await.ok();
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

    async fn payer_balance(&self) -> Option<u128> {
        let w = attest_host::open_wallet().ok()?;
        let payer = attest_host::payer().ok()?;
        w.get_account_balance(payer).await.ok()
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
    /// Tests: the payer's balance the mock reports (`None`: no payer).
    #[serde(default)]
    pub payer_balance: Option<u128>,
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
                    error: None,
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
                            error: None,
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

    async fn payer_balance(&self) -> Option<u128> {
        self.load().ok().flatten().and_then(|s| s.payer_balance)
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

#[cfg(test)]
mod tests {
    use attest_core::lc_v3::Outcome;
    use attest_host::{StagedOutcome, StagedSent};

    use super::*;

    fn sent(name: &str, error: Option<&str>) -> StagedSent {
        StagedSent {
            name: name.into(),
            tx: format!("tx-{name}"),
            gas: Some(9_000_000),
            error: error.map(Into::into),
        }
    }

    fn outcome() -> StagedOutcome {
        StagedOutcome {
            job: AccountId::new([7; 32]),
            steps: Vec::new(),
            outcome: Outcome {
                pre_store: [0; 32],
                post_store: [1; 32],
                head: attest_core::Head::default(),
                record: attest_core::HeaderRecord::default(),
                signature_slot: 0,
                participation: 512,
                new_slot: true,
                handoff: false,
            },
            four_steps: false,
        }
    }

    fn names(steps: &[Step]) -> Vec<&str> {
        steps.iter().map(|s| s.name.as_str()).collect()
    }

    fn refused(code: EsaError) -> anyhow::Error {
        anyhow::Error::new(code).context("v3 Finish: refused")
    }

    #[test]
    fn own_finish_applies() {
        let all = [sent("Begin", None), sent("Miller", None), sent("Finish", None)];
        let Submitted::Applied(a) = classify_staged(Ok(outcome()), &all, true, &[]) else {
            panic!()
        };
        assert_eq!(a.tx.tx, "tx-Finish");
        assert_eq!(names(&a.steps), ["Begin", "Miller", "Finish"]);
        assert_eq!(a.job.as_deref(), Some(AccountId::new([7; 32]).to_string().as_str()));
    }

    #[test]
    fn job_finished_by_another_submitter_is_superseded() {
        // This follower sent Begin and Miller; the other one landed Finish (its own Finish reverted).
        let mine = [
            sent("Begin", None),
            sent("Miller", None),
            sent("Finish", Some("reverted in block 9")),
        ];
        let Submitted::Superseded { steps, .. } = classify_staged(Ok(outcome()), &mine, true, &[]) else {
            panic!()
        };
        assert_eq!(names(&steps), ["Begin", "Miller", "Finish"]);
        assert!(steps[2].error.is_some(), "the paid, reverted Finish is listed");
    }

    #[test]
    fn store_moved_wins_over_a_refusal_code() {
        // E14 after Begin and Miller were paid, because another update moved the store: superseded, not a no-op.
        let mine = [sent("Begin", None), sent("Miller", None)];
        let r = classify_staged(Err(refused(EsaError::E14AlreadyApplied)), &mine, true, &[]);
        let Submitted::Superseded { steps, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn e14_before_anything_was_sent_is_a_noop() {
        let r = classify_staged(Err(refused(EsaError::E14AlreadyApplied)), &[], false, &[]);
        assert!(matches!(r, Submitted::NoOp(_)), "{r:?}");
        // The same code after a paid step is a failure, with the step listed.
        let r = classify_staged(
            Err(refused(EsaError::E14AlreadyApplied)),
            &[sent("Begin", None)],
            false,
            &[],
        );
        assert!(
            matches!(r, Submitted::Failed { ref steps, .. } if steps.len() == 1),
            "{r:?}"
        );
    }

    #[test]
    fn refusal_on_an_unmoved_store_fails_with_its_steps() {
        let mine = [
            sent("Begin", None),
            sent("Miller", None),
            sent("Finish", Some("reverted in block 4")),
        ];
        let r = classify_staged(Err(refused(EsaError::E03BadSignature)), &mine, false, &[]);
        let Submitted::Failed { code, steps, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(code.as_deref(), Some("E03"));
        assert_eq!(names(&steps), ["Begin", "Miller", "Finish"]);
    }

    #[test]
    fn reverted_step_fails_and_is_not_deferred() {
        // A window miss: included, reverted, no code in the error text.
        let mine = [sent(
            "Begin",
            Some("v3 Begin: tx tx-Begin reverted in block 7 (the panic code ...)"),
        )];
        let r = classify_staged(
            Err(anyhow!("v3 Begin: tx tx-Begin reverted in block 7")),
            &mine,
            false,
            &[],
        );
        assert!(
            matches!(r, Submitted::Failed { code: None, ref steps, .. } if steps.len() == 1),
            "{r:?}"
        );
    }

    #[test]
    fn transient_failure_defers_with_every_sent_step() {
        let earlier = vec![Step::from_sent(&sent("Begin", None))];
        let mine = [sent("Miller", Some("not included"))];
        let r = classify_staged(Err(anyhow!("v3 Miller: tx not included")), &mine, false, &earlier);
        let Submitted::Deferred { landed, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(names(&landed), ["Begin", "Miller"]);
        // The resumed attempt that lands the rest applies, listing the earlier steps first.
        let rest = [sent("Miller", None), sent("Finish", None)];
        let Submitted::Applied(a) = classify_staged(Ok(outcome()), &rest, true, &landed) else {
            panic!()
        };
        assert_eq!(names(&a.steps), ["Begin", "Miller", "Miller", "Finish"]);
    }
}
