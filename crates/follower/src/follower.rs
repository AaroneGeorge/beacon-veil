//! The follower loop: read the verified store status, fetch the next update, check it is still needed
//! after a per-instance deferral, submit, log every step.
//!
//! Restart-resume (021 R4): all decisions start from the instance's on-chain head (through `attest_host`'s verified
//! store). The follower never re-initialises an instance and never replays updates from the checkpoint. The local
//! cache only remembers the time of the last applied update (for the cadence) and the last no-op candidate.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use attest_host::StoreStatus;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::chain::{AppliedTx, Candidate, CandidateKind, Chain, Step, Submitted, esa_code, write_atomic};
use crate::log::{LogLine, Logger};
use crate::source::{BeaconSource, LcObject, period_of};

/// Loop parameters (from the config file).
#[derive(Debug, Clone)]
pub struct Params {
    pub follower_id: String,
    pub instance: String,
    pub cadence: Duration,
    pub poll: Duration,
    pub jitter_max: Duration,
    pub min_participation: u32,
    pub state_dir: PathBuf,
}

/// What the finality update says, reduced to what [`decide`] needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalityMeta {
    pub finalized_slot: u64,
    pub signature_slot: u64,
}

impl FinalityMeta {
    pub fn of(fu: &LcObject) -> Result<Self> {
        Ok(Self {
            finalized_slot: fu.finalized_slot()?,
            signature_slot: fu.signature_slot()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to do now.
    Wait,
    /// Submit the period update of `period` (`updates_by_range(period, 1)`).
    Period(u64),
    /// Submit the current finality update.
    Finality { handoff: bool },
}

impl Decision {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Period(_) => "period",
            Self::Finality { handoff: false } => "finality",
            Self::Finality { handoff: true } => "finality_handoff",
        }
    }
}

/// The policy (pure, unit-tested):
/// 1. Next committee unknown: submit the period update of the store's period (Helios learns the next committee).
/// 2. The beacon chain is more than one period ahead: submit the period update of `period + 1` (signed by the next
///    committee, it rotates the store and carries the committee after it).
/// 3. The finalized header is in the next period: submit the finality update now (handoff).
/// 4. Otherwise submit the finality update once per cadence, if it is newer than the head.
pub fn decide(st: &StoreStatus, fu: &FinalityMeta, now: u64, last_applied: Option<u64>, cadence: Duration) -> Decision {
    let p = st.period;
    if !st.next_committee_known {
        return Decision::Period(p);
    }
    let fp = period_of(fu.finalized_slot);
    let sp = period_of(fu.signature_slot);
    if fp > p + 1 || sp > p + 1 {
        return Decision::Period(p + 1);
    }
    if fu.finalized_slot <= st.head_slot {
        return Decision::Wait;
    }
    if fp == p + 1 {
        return Decision::Finality { handoff: true };
    }
    match last_applied {
        Some(t) if now.saturating_sub(t) < cadence.as_secs() => Decision::Wait,
        _ => Decision::Finality { handoff: false },
    }
}

/// Deferral before submitting, from `sha256(instance || follower_id || slot)`, in `0..=max` seconds.
pub fn jitter(instance: &str, follower_id: &str, slot: u64, max: Duration) -> Duration {
    use sha2::{Digest, Sha256};
    let m = max.as_secs();
    if m == 0 {
        return Duration::ZERO;
    }
    let mut h = Sha256::new();
    h.update(instance.as_bytes());
    h.update([0]);
    h.update(follower_id.as_bytes());
    h.update(slot.to_le_bytes());
    let d = h.finalize();
    let x = u64::from_le_bytes(d[..8].try_into().expect("8 bytes"));
    Duration::from_secs(x % (m + 1))
}

/// One step of a public-path submission, as logged and as kept in the cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepRec {
    pub name: String,
    pub tx: Option<String>,
    /// Declared gas limit.
    pub gas: Option<u64>,
    /// Guest cycles when the submitter reports them (rc3's wallet does not).
    pub cycles: Option<u64>,
    /// The step's block and what it was charged (execution gas = metered cycles when it was the block's only charged
    /// transaction).
    #[serde(default)]
    pub charge: Option<crate::charge::Charge>,
}

impl From<&Step> for StepRec {
    fn from(s: &Step) -> Self {
        Self {
            name: s.name.clone(),
            tx: s.tx.as_ref().map(|t| t.tx.clone()),
            gas: s.tx.as_ref().and_then(|t| t.gas),
            cycles: s.tx.as_ref().and_then(|t| t.cycles),
            charge: s.charge.clone(),
        }
    }
}

impl StepRec {
    fn to_step(&self) -> Step {
        Step {
            name: self.name.clone(),
            tx: self.tx.as_ref().map(|tx| attest_host::TxOutcome {
                tx: tx.clone(),
                cycles: self.cycles,
                gas: self.gas,
            }),
            charge: self.charge.clone(),
        }
    }
}

/// A public-path submission that was deferred (a step did not land after `attest_host`'s retries): sent again at
/// the next poll while the store is where it was, which continues its job on chain.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pending {
    /// Submission number of its `submit` line (its outcome line carries the same `n`).
    pub n: u64,
    pub kind: String,
    pub period: Option<u64>,
    pub version: String,
    pub ssz_hex: String,
    pub finalized_slot: u64,
    pub attested_slot: u64,
    pub signature_slot: u64,
    pub participation: u32,
    pub has_next_committee: bool,
    /// The store it was submitted on.
    pub head_slot: u64,
    pub next_committee_known: bool,
    pub landed: Vec<StepRec>,
    pub attempts: u32,
}

impl Pending {
    fn of(n: u64, c: &Candidate, st: &StoreStatus) -> Self {
        Self {
            n,
            kind: c.kind.label().into(),
            period: match c.kind {
                CandidateKind::Period { period } => Some(period),
                CandidateKind::Finality { .. } => None,
            },
            version: c.version.clone(),
            ssz_hex: hex::encode(&c.ssz),
            finalized_slot: c.finalized_slot,
            attested_slot: c.attested_slot,
            signature_slot: c.signature_slot,
            participation: c.participation,
            has_next_committee: c.has_next_committee,
            head_slot: st.head_slot,
            next_committee_known: st.next_committee_known,
            landed: Vec::new(),
            attempts: 0,
        }
    }

    fn candidate(&self) -> Result<Candidate> {
        Ok(Candidate {
            kind: match (self.kind.as_str(), self.period) {
                ("period", Some(period)) => CandidateKind::Period { period },
                ("finality_handoff", _) => CandidateKind::Finality { handoff: true },
                _ => CandidateKind::Finality { handoff: false },
            },
            version: self.version.clone(),
            ssz: hex::decode(&self.ssz_hex).context("pending ssz")?,
            finalized_slot: self.finalized_slot,
            attested_slot: self.attested_slot,
            signature_slot: self.signature_slot,
            participation: self.participation,
            has_next_committee: self.has_next_committee,
        })
    }
}

/// Local cache (not a source of truth; see module docs).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cache {
    pub instance: String,
    pub last_applied_unix: Option<u64>,
    pub last_applied_slot: Option<u64>,
    pub last_tx: Option<String>,
    /// `(kind, finalized_slot, signature_slot)` of the last candidate refused as a no-op.
    pub last_noop: Option<(String, u64, u64)>,
    pub starts: u64,
    pub applied_total: u64,
    /// Public path: a deferred submission to continue.
    #[serde(default)]
    pub pending: Option<Pending>,
}

impl Cache {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("follower-cache.json")
    }

    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let p = Self::path(dir);
        if !p.exists() {
            return Ok(None);
        }
        let b = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        Ok(Some(
            serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?,
        ))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        write_atomic(&Self::path(dir), &serde_json::to_vec_pretty(self)?)
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sleeps `d` unless a stop is requested first. Returns true if stopped.
pub async fn sleep_or_stop(d: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        () = tokio::time::sleep(d) => *stop.borrow(),
        r = stop.changed() => r.is_err() || *stop.borrow(),
    }
}

pub struct Follower<S, C> {
    pub source: S,
    pub chain: C,
    pub params: Params,
    pub log: Logger,
    cache: Cache,
    n: u64,
}

fn status_fields(l: &mut LogLine, st: &StoreStatus) {
    l.head_slot = Some(st.head_slot);
    l.period = Some(st.period);
    l.next_committee_known = Some(st.next_committee_known);
}

impl<S: BeaconSource + Sync, C: Chain + Sync> Follower<S, C> {
    pub fn new(source: S, chain: C, params: Params, log: Logger) -> Result<Self> {
        let cache = Cache::load(&params.state_dir)?.unwrap_or_default();
        let n = cache.pending.as_ref().map_or(0, |p| p.n);
        Ok(Self {
            source,
            chain,
            params,
            log,
            cache,
            n,
        })
    }

    /// Runs until `stop` turns true. Errors inside a step are logged and retried at the next poll; the loop does not
    /// exit on them (unattended run).
    pub async fn run(&mut self, mut stop: watch::Receiver<bool>, start_detail: serde_json::Value) -> Result<()> {
        let resumed = self.cache.starts > 0;
        if !self.cache.instance.is_empty() && self.cache.instance != self.params.instance {
            anyhow::bail!(
                "state dir {} belongs to instance {}, not {}",
                self.params.state_dir.display(),
                self.cache.instance,
                self.params.instance
            );
        }
        self.cache.instance = self.params.instance.clone();
        self.cache.starts += 1;
        self.cache.save(&self.params.state_dir)?;

        let mut l = self.log.line("start");
        l.extra = Some(start_detail);
        self.log.write(l);

        // Resume point: the on-chain head, never the checkpoint.
        match self.chain.status().await {
            Ok(Some(st)) => {
                let mut l = self.log.line(if resumed { "resume" } else { "head" });
                status_fields(&mut l, &st);
                l.slot = Some(st.head_slot);
                l.tx = self.cache.last_tx.clone();
                l.detail = Some(format!(
                    "starts={} cached_last_applied_slot={:?}",
                    self.cache.starts, self.cache.last_applied_slot
                ));
                self.log.write(l);
            }
            Ok(None) => self.err_line("no_instance", None, "instance does not exist: run `attest init` first"),
            Err(e) => self.err_line("chain_error", None, &format!("{e:#}")),
        }

        loop {
            if *stop.borrow() {
                break;
            }
            if let Err(e) = self.step(&mut stop).await {
                self.err_line("step_error", None, &format!("{e:#}"));
            }
            if sleep_or_stop(self.params.poll, &mut stop).await {
                break;
            }
        }
        let mut l = self.log.line("stop");
        l.slot = self.cache.last_applied_slot;
        l.detail = Some("clean shutdown".into());
        self.log.write(l);
        Ok(())
    }

    fn err_line(&self, event: &str, code: Option<String>, detail: &str) {
        let mut l = self.log.line(event);
        l.code = code;
        l.detail = Some(detail.chars().take(2000).collect());
        self.log.write(l);
    }

    /// The verified local store is missing or behind the chain (for example after `kill -9` between an accepted
    /// transaction and the store write). Rebuild it from a bootstrap at the on-chain head, alone and then with the
    /// head period's update (attest_host decision 3 (c)). Never touches the instance.
    async fn recover_store(&mut self) {
        let (slot, root, period) = match self.chain.head_root().await {
            Ok(Some(h)) => h,
            Ok(None) => return,
            Err(e) => {
                self.err_line("rebuild_error", esa_code(&e), &format!("read head: {e:#}"));
                return;
            }
        };
        let boot = match self.source.bootstrap(&root).await.and_then(|b| b.to_ssz()) {
            Ok(b) => b,
            Err(e) => {
                self.err_line("rebuild_error", None, &format!("bootstrap at head slot {slot}: {e:#}"));
                return;
            }
        };
        let mut tries: Vec<Vec<Vec<u8>>> = vec![vec![]];
        match self.source.updates_by_range(period, 1).await {
            Ok(u) => {
                if let Some(ssz) = u.first().and_then(|x| x.to_ssz().ok()) {
                    tries.push(vec![ssz]);
                }
            }
            Err(e) => self.err_line("source_error", None, &format!("updates_by_range({period},1): {e:#}")),
        }
        let mut last = String::new();
        for t in tries {
            match self.chain.rebuild(&boot, &t).await {
                Ok(st) => {
                    let mut l = self.log.line("store_rebuilt");
                    status_fields(&mut l, &st);
                    l.slot = Some(st.head_slot);
                    l.detail = Some(format!("bootstrap at head + {} update(s)", t.len()));
                    self.log.write(l);
                    return;
                }
                Err(e) => last = format!("{e:#}"),
            }
        }
        self.err_line("rebuild_error", None, &last);
    }

    /// One poll: continue a deferred submission, or decide, fetch, defer, re-check, submit.
    pub async fn step(&mut self, stop: &mut watch::Receiver<bool>) -> Result<()> {
        let st = match self.chain.status().await {
            Ok(Some(st)) => st,
            Ok(None) => {
                self.err_line("no_instance", None, "instance does not exist");
                return Ok(());
            }
            Err(e) => {
                self.err_line("chain_error", esa_code(&e), &format!("{e:#}"));
                self.recover_store().await;
                return Ok(());
            }
        };
        if let Some(p) = self.cache.pending.clone() {
            return self.resume_pending(p, &st).await;
        }
        let fu = match self.source.finality_update().await {
            Ok(fu) => fu,
            Err(e) => {
                let mut l = self.log.line("source_error");
                status_fields(&mut l, &st);
                l.detail = Some(format!("finality_update: {e:#}"));
                self.log.write(l);
                return Ok(());
            }
        };
        let meta = FinalityMeta::of(&fu)?;
        let now = now_unix();
        let d = decide(&st, &meta, now, self.cache.last_applied_unix, self.params.cadence);

        let mut l = self.log.line("tick");
        status_fields(&mut l, &st);
        l.slot = Some(meta.finalized_slot);
        l.kind = Some(d.label().into());
        l.provider = fu.provider.clone();
        self.log.write(l);

        let cand = match d {
            Decision::Wait => return Ok(()),
            Decision::Finality { handoff } => Candidate::from_object(CandidateKind::Finality { handoff }, &fu)?,
            Decision::Period(p) => {
                let ups = match self.source.updates_by_range(p, 1).await {
                    Ok(u) => u,
                    Err(e) => {
                        self.err_line("source_error", None, &format!("updates_by_range({p},1): {e:#}"));
                        return Ok(());
                    }
                };
                let Some(u) = ups.into_iter().next() else {
                    self.err_line(
                        "source_error",
                        None,
                        &format!("updates_by_range({p},1) returned no update"),
                    );
                    return Ok(());
                };
                let c = Candidate::from_object(CandidateKind::Period { period: p }, &u)?;
                if period_of(c.signature_slot) != p || !c.has_next_committee {
                    self.err_line(
                        "source_error",
                        None,
                        &format!(
                            "period update for {p} has signature slot {} (period {})",
                            c.signature_slot,
                            period_of(c.signature_slot)
                        ),
                    );
                    return Ok(());
                }
                c
            }
        };

        if cand.participation < self.params.min_participation {
            let mut l = self.log.line("skip_low_participation");
            l.slot = Some(cand.finalized_slot);
            l.kind = Some(cand.kind.label().into());
            l.detail = Some(format!(
                "participation {} < {}",
                cand.participation, self.params.min_participation
            ));
            self.log.write(l);
            return Ok(());
        }
        let key = (cand.kind.label().to_string(), cand.finalized_slot, cand.signature_slot);
        if self.cache.last_noop.as_ref() == Some(&key) {
            // Same candidate already refused as a no-op: wait for a new one.
            return Ok(());
        }

        // Per-instance deferral, then check the update is still needed (another follower may have submitted it).
        let wait = jitter(
            &self.params.instance,
            &self.params.follower_id,
            cand.finalized_slot,
            self.params.jitter_max,
        );
        if !wait.is_zero() {
            let mut l = self.log.line("defer");
            l.slot = Some(cand.finalized_slot);
            l.kind = Some(cand.kind.label().into());
            l.detail = Some(format!("{}s", wait.as_secs()));
            self.log.write(l);
            if sleep_or_stop(wait, stop).await {
                return Ok(());
            }
        }
        let st2 = match self.chain.status().await {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(()),
            Err(e) => {
                self.err_line("chain_error", esa_code(&e), &format!("{e:#}"));
                return Ok(());
            }
        };
        let d2 = decide(
            &st2,
            &meta,
            now_unix(),
            self.cache.last_applied_unix,
            self.params.cadence,
        );
        if d2 != d {
            let mut l = self.log.line("skip_not_needed");
            status_fields(&mut l, &st2);
            l.slot = Some(cand.finalized_slot);
            l.kind = Some(cand.kind.label().into());
            l.detail = Some(format!("decision now {}", d2.label()));
            self.log.write(l);
            return Ok(());
        }

        self.n += 1;
        let n = self.n;
        let mut l = self.log.line("submit");
        status_fields(&mut l, &st2);
        l.slot = Some(cand.finalized_slot);
        l.period = Some(period_of(cand.finalized_slot));
        l.kind = Some(cand.kind.label().into());
        l.n = Some(n);
        l.extra = Some(serde_json::json!({
            "version": cand.version, "signature_slot": cand.signature_slot, "attested_slot": cand.attested_slot,
            "participation": cand.participation, "ssz_bytes": cand.ssz.len(), "has_next_committee": cand.has_next_committee,
        }));
        self.log.write(l);

        let res = self.chain.submit(&cand, &[]).await;
        self.record_outcome(n, &cand, &st2, res, None).await?;
        Ok(())
    }

    /// Sends a deferred public-path submission again while the store is where it was (its job continues from its
    /// on-chain stage). If the store moved meanwhile, the update was superseded: say so under its `n` and drop it.
    async fn resume_pending(&mut self, p: Pending, st: &StoreStatus) -> Result<()> {
        let cand = p.candidate()?;
        if st.head_slot != p.head_slot || st.next_committee_known != p.next_committee_known {
            let mut l = self.log.line("superseded");
            status_fields(&mut l, st);
            l.slot = Some(p.finalized_slot);
            l.kind = Some(p.kind.clone());
            l.n = Some(p.n);
            l.detail = Some(format!(
                "the store moved from head {} while the submission was deferred",
                p.head_slot
            ));
            l.extra = Some(serde_json::json!({ "steps": p.landed, "attempts": p.attempts }));
            self.log.write(l);
            self.cache.pending = None;
            self.cache.save(&self.params.state_dir)?;
            return Ok(());
        }
        let mut l = self.log.line("submit_resume");
        status_fields(&mut l, st);
        l.slot = Some(p.finalized_slot);
        l.kind = Some(p.kind.clone());
        l.n = Some(p.n);
        l.detail = Some(format!("attempt {}", p.attempts + 1));
        self.log.write(l);
        let earlier: Vec<Step> = p.landed.iter().map(StepRec::to_step).collect();
        let res = self.chain.submit(&cand, &earlier).await;
        self.record_outcome(p.n, &cand, st, res, Some(p)).await
    }

    /// Logs the outcome line of submission `n` and updates the cache.
    async fn record_outcome(
        &mut self,
        n: u64,
        cand: &Candidate,
        st: &StoreStatus,
        res: Result<Submitted>,
        pending: Option<Pending>,
    ) -> Result<()> {
        let key = (cand.kind.label().to_string(), cand.finalized_slot, cand.signature_slot);
        let attempts = pending.as_ref().map_or(0, |p| p.attempts) + 1;
        let steps_json = |steps: &[Step]| -> serde_json::Value {
            serde_json::to_value(steps.iter().map(StepRec::from).collect::<Vec<_>>()).unwrap_or_default()
        };
        match res {
            Ok(Submitted::Applied(a)) => {
                let AppliedTx {
                    tx: o,
                    steps,
                    job,
                    four_steps,
                } = a;
                let after = self.chain.status().await.ok().flatten();
                let mut l = self.log.line("applied");
                if let Some(a) = &after {
                    status_fields(&mut l, a);
                }
                l.slot = Some(after.map_or(cand.finalized_slot, |a| a.head_slot));
                l.period = after.map(|a| a.period).or(Some(period_of(cand.finalized_slot)));
                l.tx = Some(o.tx.clone());
                l.cycles = o.cycles;
                l.gas = o.gas;
                l.kind = Some(cand.kind.label().into());
                l.n = Some(n);
                let handoff = after.is_some_and(|a| a.period > st.period);
                let mut extra = serde_json::json!({ "handoff": handoff, "candidate_slot": cand.finalized_slot });
                if !steps.is_empty() {
                    extra["mode"] = "public".into();
                    extra["txs"] = steps.len().into();
                    extra["steps"] = steps_json(&steps);
                    extra["job"] = job.into();
                    extra["four_steps"] = four_steps.into();
                    extra["attempts"] = attempts.into();
                }
                l.extra = Some(extra);
                self.log.write(l);
                self.cache.last_applied_unix = Some(now_unix());
                self.cache.last_applied_slot = Some(after.map_or(cand.finalized_slot, |a| a.head_slot));
                self.cache.last_tx = Some(o.tx);
                self.cache.applied_total += 1;
                self.cache.last_noop = None;
                self.cache.pending = None;
                self.cache.save(&self.params.state_dir)?;
            }
            Ok(Submitted::NoOp(why)) => {
                let mut l = self.log.line("noop");
                l.slot = Some(cand.finalized_slot);
                l.kind = Some(cand.kind.label().into());
                l.n = Some(n);
                l.code = Some("E14".into());
                l.detail = Some(why.chars().take(500).collect());
                self.log.write(l);
                self.cache.last_noop = Some(key);
                self.cache.pending = None;
                self.cache.save(&self.params.state_dir)?;
            }
            Ok(Submitted::Superseded { detail, steps }) => {
                let mut l = self.log.line("superseded");
                status_fields(&mut l, st);
                l.slot = Some(cand.finalized_slot);
                l.kind = Some(cand.kind.label().into());
                l.n = Some(n);
                l.detail = Some(detail.chars().take(1000).collect());
                l.extra =
                    Some(serde_json::json!({ "txs": steps.len(), "steps": steps_json(&steps), "attempts": attempts }));
                self.log.write(l);
                self.cache.pending = None;
                self.cache.save(&self.params.state_dir)?;
            }
            Ok(Submitted::Deferred { detail, landed }) => {
                let mut l = self.log.line("submit_deferred");
                status_fields(&mut l, st);
                l.slot = Some(cand.finalized_slot);
                l.kind = Some(cand.kind.label().into());
                l.n = Some(n);
                l.detail = Some(detail.chars().take(1000).collect());
                l.extra = Some(serde_json::json!({ "landed": steps_json(&landed), "attempts": attempts }));
                self.log.write(l);
                let mut p = pending.unwrap_or_else(|| Pending::of(n, cand, st));
                p.landed = landed.iter().map(StepRec::from).collect();
                p.attempts = attempts;
                self.cache.pending = Some(p);
                self.cache.save(&self.params.state_dir)?;
            }
            Err(e) => {
                let mut l = self.log.line("submit_failed");
                l.slot = Some(cand.finalized_slot);
                l.kind = Some(cand.kind.label().into());
                l.n = Some(n);
                l.code = esa_code(&e);
                l.detail = Some(format!("{e:#}").chars().take(2000).collect());
                self.log.write(l);
                self.cache.pending = None;
                self.cache.save(&self.params.state_dir)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(head: u64, next: bool) -> StoreStatus {
        StoreStatus {
            head_slot: head,
            period: period_of(head),
            next_committee_known: next,
        }
    }
    const P: u64 = 8192;
    const CAD: Duration = Duration::from_secs(6144);

    #[test]
    fn needs_committee_first() {
        let fu = FinalityMeta {
            finalized_slot: 10 * P + 500,
            signature_slot: 10 * P + 600,
        };
        assert_eq!(
            decide(&st(10 * P + 100, false), &fu, 0, None, CAD),
            Decision::Period(10)
        );
    }

    #[test]
    fn cadence_within_period() {
        let fu = FinalityMeta {
            finalized_slot: 10 * P + 500,
            signature_slot: 10 * P + 600,
        };
        let s = st(10 * P + 100, true);
        assert_eq!(decide(&s, &fu, 1000, None, CAD), Decision::Finality { handoff: false });
        assert_eq!(decide(&s, &fu, 1000, Some(500), CAD), Decision::Wait);
        assert_eq!(
            decide(&s, &fu, 7000, Some(500), CAD),
            Decision::Finality { handoff: false }
        );
        // Not newer than the head.
        assert_eq!(
            decide(&st(10 * P + 500, true), &fu, 7000, Some(500), CAD),
            Decision::Wait
        );
    }

    #[test]
    fn handoff_ignores_cadence() {
        let fu = FinalityMeta {
            finalized_slot: 11 * P + 32,
            signature_slot: 11 * P + 97,
        };
        assert_eq!(
            decide(&st(10 * P + 100, true), &fu, 1000, Some(999), CAD),
            Decision::Finality { handoff: true }
        );
    }

    #[test]
    fn far_behind_catches_up_by_period() {
        let fu = FinalityMeta {
            finalized_slot: 13 * P,
            signature_slot: 13 * P + 70,
        };
        assert_eq!(decide(&st(10 * P + 100, true), &fu, 0, None, CAD), Decision::Period(11));
        // Signed by the committee two periods ahead even though finalized one ahead.
        let fu = FinalityMeta {
            finalized_slot: 12 * P - 10,
            signature_slot: 12 * P + 50,
        };
        assert_eq!(decide(&st(10 * P + 100, true), &fu, 0, None, CAD), Decision::Period(11));
    }

    #[test]
    fn jitter_bounded_and_stable() {
        let m = Duration::from_secs(60);
        let a = jitter("inst", "run-a", 42, m);
        assert_eq!(a, jitter("inst", "run-a", 42, m));
        assert!(a <= m);
        assert_eq!(jitter("inst", "run-a", 42, Duration::ZERO), Duration::ZERO);
    }
}
