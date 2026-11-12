//! Host library (PLAN.md 2.3, owner W1; used by W2, W3, W4, W5, W6).
//!
//! Status: freeze-1 stubs. Signatures are the contract; bodies return "not implemented" until W1 lands.

#![forbid(unsafe_code)]

use attest_core::{AccountId, EsaError, Head, HeaderRecord, InstanceConfig, Predicate, Statement};

pub use lee;
pub use wallet;

/// How an update is submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitMode {
    /// Path A: `Verify` proven in a private actor of `lc_verify`, `Apply` on the public instance (D2).
    Private,
    /// B0 probe: the monolithic `Verify` run by the public executor, to record cycles and the cap rejection (D3).
    PublicProbe,
}

/// Outcome of a submitted transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxOutcome {
    /// Transaction hash as the sequencer reports it.
    pub tx: String,
    /// Guest cycles (executor session info), when known.
    pub cycles: Option<u64>,
    /// Gas charged, when known.
    pub gas: Option<u64>,
}

/// Program ids of a deployment, as written by `scripts/deploy_local.sh` into `deploy/ids.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployedIds {
    pub attest: AccountId,
    pub lc_verify: AccountId,
}

fn not_implemented<T>(what: &str) -> anyhow::Result<T> {
    anyhow::bail!("attest_host::{what}: not implemented (freeze-1 stub)")
}

/// Sends `Init{config, bootstrap}` to `instance_id(config)`; returns the instance id and the outcome.
pub async fn init(
    ids: &DeployedIds,
    config: &InstanceConfig,
    bootstrap_ssz: &[u8],
) -> anyhow::Result<(AccountId, TxOutcome)> {
    let _ = (ids, config, bootstrap_ssz);
    not_implemented("init")
}

/// Submits one light-client update to `instance`.
pub async fn submit_update(
    ids: &DeployedIds,
    instance: &AccountId,
    update_ssz: &[u8],
    mode: SubmitMode,
) -> anyhow::Result<TxOutcome> {
    let _ = (ids, instance, update_ssz, mode);
    not_implemented("submit_update")
}

/// Reads an instance's head from public state.
pub async fn read_head(instance: &AccountId) -> anyhow::Result<Option<Head>> {
    let _ = instance;
    not_implemented("read_head")
}

/// Reads the header record of `slot`.
pub async fn read_record(ids: &DeployedIds, instance: &AccountId, slot: u64) -> anyhow::Result<Option<HeaderRecord>> {
    let _ = (ids, instance, slot);
    not_implemented("read_record")
}

/// Sends `Attest` and returns the delivered statement.
pub async fn attest(
    ids: &DeployedIds,
    record: &HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
    deliver_to: &AccountId,
) -> anyhow::Result<(Statement, TxOutcome)> {
    let _ = (ids, record, claim, predicate, deliver_to);
    not_implemented("attest")
}

/// Runs the `Attest` checks on the host, with the same error codes as on chain (021 U2, U5).
pub fn dry_run_attest(record: &HeaderRecord, claim: &[u8], predicate: &Predicate) -> Result<Statement, EsaError> {
    let _ = (record, claim, predicate);
    Err(EsaError::E01Malformed)
}
