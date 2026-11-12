//! The LEZ side of the follower: read the instance's verified store status and submit updates.
//!
//! [`HostChain`] goes through `attest_host` (W1) to a LEZ sequencer. [`MockChain`] is a file-backed stand-in that
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

/// Result of a submission.
#[derive(Debug, Clone)]
pub enum Submitted {
    /// Transaction accepted.
    Applied(TxOutcome),
    /// Refused before submission because it would change nothing (E14). Not a failure.
    NoOp(String),
}

pub trait Chain {
    /// Short name for the log (`host`, `mock`).
    fn name(&self) -> &'static str;
    fn status(&self) -> impl std::future::Future<Output = Result<Option<StoreStatus>>> + Send;
    fn submit(&self, c: &Candidate) -> impl std::future::Future<Output = Result<Submitted>> + Send;
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

/// True if `e` is the "changes nothing" refusal (E14): `attest_host` gives every refusal its `EsaError` as root cause.
pub fn is_noop(e: &anyhow::Error) -> bool {
    attest_host::esa_code(e) == Some(EsaError::E14AlreadyApplied)
}

/// ESA code of an error chain, for the log (`E13`, ...).
pub fn esa_code(e: &anyhow::Error) -> Option<String> {
    attest_host::esa_code(e).map(|c| c.code().to_string())
}

// ---------------------------------------------------------------------------------------------------------------

/// `attest_host` against a LEZ sequencer. Wallet and sequencer come from the environment (`LEE_WALLET_HOME_DIR`,
/// `LEE_WALLET_PASSWORD`, `BV_STORE_DIR`).
#[derive(Debug, Clone)]
pub struct HostChain {
    pub ids: DeployedIds,
    pub instance: AccountId,
    pub mode: SubmitMode,
}

impl Chain for HostChain {
    fn name(&self) -> &'static str {
        "host"
    }

    async fn status(&self) -> Result<Option<StoreStatus>> {
        attest_host::read_store_status(&self.ids, &self.instance).await
    }

    async fn submit(&self, c: &Candidate) -> Result<Submitted> {
        match attest_host::submit_update(&self.ids, &self.instance, &c.ssz, self.mode).await {
            Ok(o) => Ok(Submitted::Applied(o)),
            Err(e) if is_noop(&e) => Ok(Submitted::NoOp(format!("{e:#}"))),
            Err(e) => Err(e),
        }
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
}

/// File-backed mock chain (Helios store rules on metadata; no signature checks).
#[derive(Debug, Clone)]
pub struct MockChain {
    pub path: PathBuf,
}

impl MockChain {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
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

    async fn submit(&self, c: &Candidate) -> Result<Submitted> {
        let s = self.load()?.ok_or_else(|| anyhow!("mock instance does not exist"))?;
        match Self::apply(&s, c) {
            Ok(n) => {
                write_atomic(&self.path, &serde_json::to_vec_pretty(&n)?)?;
                use sha2::Digest;
                let tx = hex::encode(sha2::Sha256::digest(&c.ssz));
                Ok(Submitted::Applied(TxOutcome {
                    tx: format!("mock:{}", &tx[..16]),
                    cycles: None,
                    gas: None,
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
