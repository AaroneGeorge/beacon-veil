//! Beacon light-client data sources: Logos `beacon_api_module` through `logosctl`, preferred; direct
//! HTTP to the same Beacon API routes, as the stated fallback. Both return the Beacon API JSON objects; SSZ for
//! submission is made by [`crate::ssz`], so the bytes submitted do not depend on which source ran.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::ssz::{self, LcKind};

/// Where Beacon API light-client data comes from (G4 records which one ran).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeaconSourceKind {
    /// Logos `beacon_api_module` through `lc_request`.
    LogosModule,
    /// Same Beacon API routes over HTTP.
    DirectHttp,
}

impl BeaconSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LogosModule => "logos_module",
            Self::DirectHttp => "direct_http",
        }
    }
}

/// One light-client object as the Beacon API serves it: `{ "version": "<fork>", "data": {...} }`.
#[derive(Debug, Clone, PartialEq)]
pub struct LcObject {
    pub kind: LcKind,
    pub version: String,
    pub data: Value,
    /// Provider that served it, when the source reports it.
    pub provider: Option<String>,
}

pub const SLOTS_PER_PERIOD: u64 = 8192;

pub fn period_of(slot: u64) -> u64 {
    slot / SLOTS_PER_PERIOD
}

fn slot_at(v: &Value, ptr: &str) -> Result<u64> {
    let x = v.pointer(ptr).ok_or_else(|| anyhow!("missing {ptr}"))?;
    match x {
        Value::String(s) => s.parse().with_context(|| format!("{ptr} is not a u64")),
        Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("{ptr} is not a u64")),
        _ => bail!("{ptr} is not a u64"),
    }
}

impl LcObject {
    fn from_envelope(kind: LcKind, v: Value, provider: Option<String>) -> Result<Self> {
        let version = v
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("light-client response has no 'version'"))?
            .to_string();
        let data = v
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("light-client response has no 'data'"))?;
        Ok(Self {
            kind,
            version,
            data,
            provider,
        })
    }

    /// Finalized header slot (updates) or header slot (bootstrap).
    pub fn finalized_slot(&self) -> Result<u64> {
        match self.kind {
            LcKind::Bootstrap => slot_at(&self.data, "/header/beacon/slot"),
            _ => slot_at(&self.data, "/finalized_header/beacon/slot"),
        }
    }

    pub fn attested_slot(&self) -> Result<u64> {
        slot_at(&self.data, "/attested_header/beacon/slot")
    }

    pub fn signature_slot(&self) -> Result<u64> {
        slot_at(&self.data, "/signature_slot")
    }

    /// Number of set bits in `sync_committee_bits`.
    pub fn participation(&self) -> Result<u32> {
        let s = self
            .data
            .pointer("/sync_aggregate/sync_committee_bits")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing sync_committee_bits"))?;
        let b = hex::decode(s.trim_start_matches("0x")).context("sync_committee_bits hex")?;
        Ok(b.iter().map(|x| x.count_ones()).sum())
    }

    pub fn has_next_committee(&self) -> bool {
        self.kind == LcKind::Update && self.data.get("next_sync_committee").is_some()
    }

    /// Canonical SSZ of `data`.
    pub fn to_ssz(&self) -> Result<Vec<u8>> {
        ssz::encode(self.kind, &self.version, &self.data)
    }
}

/// Source of light-client data.
pub trait BeaconSource {
    fn kind(&self) -> BeaconSourceKind;
    fn bootstrap(&self, block_root: &[u8; 32]) -> impl std::future::Future<Output = Result<LcObject>> + Send;
    fn updates_by_range(
        &self,
        start_period: u64,
        count: u64,
    ) -> impl std::future::Future<Output = Result<Vec<LcObject>>> + Send;
    fn finality_update(&self) -> impl std::future::Future<Output = Result<LcObject>> + Send;
}

fn envelopes_to_updates(v: Value, provider: Option<String>) -> Result<Vec<LcObject>> {
    let arr = match v {
        Value::Array(a) => a,
        other => bail!("updates: expected a JSON array, got {}", short(&other.to_string())),
    };
    arr.into_iter()
        .map(|e| LcObject::from_envelope(LcKind::Update, e, provider.clone()))
        .collect()
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Direct HTTP
// ---------------------------------------------------------------------------------------------------------------

/// The Beacon API light-client routes over HTTP, with failover across `endpoints` in order.
#[derive(Debug, Clone)]
pub struct DirectHttpSource {
    endpoints: Vec<String>,
    client: reqwest::Client,
}

impl DirectHttpSource {
    pub fn new(endpoints: Vec<String>, timeout: Duration) -> Result<Self> {
        if endpoints.is_empty() {
            bail!("direct_http source needs at least one endpoint");
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent("attest-followerd")
            .build()
            .context("http client")?;
        Ok(Self {
            endpoints: endpoints
                .into_iter()
                .map(|e| e.trim_end_matches('/').to_string())
                .collect(),
            client,
        })
    }

    /// GET `path` as JSON from the first endpoint that answers 200 with JSON.
    pub async fn get_json(&self, path: &str) -> Result<(Value, String)> {
        let mut errs = Vec::new();
        for ep in &self.endpoints {
            let url = format!("{ep}{path}");
            let r = async {
                let resp = self
                    .client
                    .get(&url)
                    .header("Accept", "application/json")
                    .send()
                    .await?;
                let status = resp.status();
                if !status.is_success() {
                    bail!("HTTP {status}");
                }
                Ok::<Value, anyhow::Error>(resp.json::<Value>().await?)
            }
            .await;
            match r {
                Ok(v) => return Ok((v, ep.clone())),
                Err(e) => errs.push(format!("{ep}: {e:#}")),
            }
        }
        bail!("all endpoints failed for {path}: {}", errs.join("; "))
    }

    /// GET `path` as SSZ (`application/octet-stream`), for cross-checks of the encoder.
    pub async fn get_ssz(&self, path: &str) -> Result<Vec<u8>> {
        let mut errs = Vec::new();
        for ep in &self.endpoints {
            let url = format!("{ep}{path}");
            let r = async {
                let resp = self
                    .client
                    .get(&url)
                    .header("Accept", "application/octet-stream")
                    .send()
                    .await?;
                let status = resp.status();
                if !status.is_success() {
                    bail!("HTTP {status}");
                }
                Ok::<Vec<u8>, anyhow::Error>(resp.bytes().await?.to_vec())
            }
            .await;
            match r {
                Ok(v) => return Ok(v),
                Err(e) => errs.push(format!("{ep}: {e:#}")),
            }
        }
        bail!("all endpoints failed for {path}: {}", errs.join("; "))
    }
}

const LC: &str = "/eth/v1/beacon/light_client";

impl BeaconSource for DirectHttpSource {
    fn kind(&self) -> BeaconSourceKind {
        BeaconSourceKind::DirectHttp
    }

    async fn bootstrap(&self, block_root: &[u8; 32]) -> Result<LcObject> {
        let (v, ep) = self
            .get_json(&format!("{LC}/bootstrap/0x{}", hex::encode(block_root)))
            .await?;
        LcObject::from_envelope(LcKind::Bootstrap, v, Some(ep))
    }

    async fn updates_by_range(&self, start_period: u64, count: u64) -> Result<Vec<LcObject>> {
        let (v, ep) = self
            .get_json(&format!("{LC}/updates?start_period={start_period}&count={count}"))
            .await?;
        envelopes_to_updates(v, Some(ep))
    }

    async fn finality_update(&self) -> Result<LcObject> {
        let (v, ep) = self.get_json(&format!("{LC}/finality_update")).await?;
        LcObject::from_envelope(LcKind::FinalityUpdate, v, Some(ep))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Logos beacon_api_module through logosctl
// ---------------------------------------------------------------------------------------------------------------

/// Logos `beacon_api_module` (v0.2.0) called as `logosctl call beacon_api_module lc_request ...` against a running
/// `logosctl` daemon that has the module installed and loaded. Provider choice, failover and proxy policy are the
/// module's.
#[derive(Debug, Clone)]
pub struct LogosModuleSource {
    pub logosctl: PathBuf,
    /// `--config-dir` of the logosctl session; `None` uses logosctl's default.
    pub config_dir: Option<PathBuf>,
    /// TMPDIR for logosctl: the daemon's local endpoint lives under the daemon's TMPDIR, so the client must use the
    /// same one. `None` inherits.
    pub tmpdir: Option<PathBuf>,
    pub chain_id: u64,
    /// Preferred provider id (may be empty: the module's order).
    pub provider: String,
    pub deadline: Duration,
}

impl LogosModuleSource {
    /// Runs `logosctl call beacon_api_module <method> <args...>` and returns the method's JSON answer.
    pub async fn call(&self, method: &str, args: &[String]) -> Result<Value> {
        let mut cmd = tokio::process::Command::new(&self.logosctl);
        if let Some(d) = &self.config_dir {
            cmd.arg("--config-dir").arg(d);
        }
        cmd.arg("--json")
            .arg("call")
            .arg("beacon_api_module")
            .arg(method)
            .args(args);
        if let Some(t) = &self.tmpdir {
            cmd.env("TMPDIR", t);
        }
        cmd.kill_on_drop(true);
        cmd.stdin(std::process::Stdio::null());
        let fut = cmd.output();
        let out = tokio::time::timeout(self.deadline + Duration::from_secs(30), fut)
            .await
            .map_err(|_| anyhow!("logosctl call {method}: timed out"))?
            .with_context(|| format!("spawn {}", self.logosctl.display()))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            bail!(
                "logosctl call beacon_api_module {method}: exit {:?}: {} {}",
                out.status.code(),
                short(stdout.trim()),
                short(String::from_utf8_lossy(&out.stderr).trim())
            );
        }
        parse_module_answer(&stdout).with_context(|| format!("logosctl call beacon_api_module {method}"))
    }

    async fn lc_request(&self, endpoint: &str, params: Value) -> Result<(Value, Option<String>)> {
        let args = vec![
            self.chain_id.to_string(),
            format!("str:{endpoint}"),
            format!("str:{params}"),
            format!("str:{}", self.provider),
            u64::try_from(self.deadline.as_millis()).unwrap_or(600_000).to_string(),
        ];
        let ans = self.call("lc_request", &args).await?;
        if ans.get("ok").and_then(Value::as_bool) != Some(true) {
            bail!("beacon_api_module.lc_request {endpoint}: {}", short(&ans.to_string()));
        }
        let provider = ans.get("provider").and_then(Value::as_str).map(str::to_string);
        let body = match ans.get("body") {
            Some(Value::String(s)) => serde_json::from_str(s).context("lc_request body is not JSON")?,
            Some(v) => v.clone(),
            None => bail!("lc_request answer has no body"),
        };
        Ok((body, provider))
    }
}

/// `logosctl --json call` prints the method's return value. For this module that is a JSON string holding a JSON
/// object (`{ "ok": ..., ... }`); depending on the logosctl version it may arrive wrapped (`{"result": ...}`) or as
/// the bare string. Accept each shape.
pub fn parse_module_answer(stdout: &str) -> Result<Value> {
    let t = stdout.trim();
    let v: Value = serde_json::from_str(t)
        .or_else(|_| {
            // Last JSON line, in case logosctl printed notices before it.
            t.lines()
                .rev()
                .find_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
                .ok_or(())
        })
        .map_err(|()| anyhow!("logosctl output is not JSON: {}", short(t)))?;
    unwrap_answer(v, 0)
}

fn unwrap_answer(v: Value, depth: u8) -> Result<Value> {
    if depth > 4 {
        bail!("logosctl answer nested too deeply");
    }
    match v {
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(inner) => unwrap_answer(inner, depth + 1),
            Err(_) => bail!("module answered a non-JSON string: {}", short(&s)),
        },
        Value::Object(ref o) if o.contains_key("ok") => Ok(v),
        Value::Object(mut o) => {
            for key in ["result", "value", "return", "data"] {
                if let Some(x) = o.remove(key) {
                    return unwrap_answer(x, depth + 1);
                }
            }
            bail!("unrecognised logosctl answer: {}", short(&Value::Object(o).to_string()))
        }
        other => bail!("unrecognised logosctl answer: {}", short(&other.to_string())),
    }
}

impl BeaconSource for LogosModuleSource {
    fn kind(&self) -> BeaconSourceKind {
        BeaconSourceKind::LogosModule
    }

    async fn bootstrap(&self, block_root: &[u8; 32]) -> Result<LcObject> {
        let p = serde_json::json!({ "block_root": format!("0x{}", hex::encode(block_root)) });
        let (v, prov) = self.lc_request("getLightClientBootstrap", p).await?;
        LcObject::from_envelope(LcKind::Bootstrap, v, prov)
    }

    async fn updates_by_range(&self, start_period: u64, count: u64) -> Result<Vec<LcObject>> {
        let p = serde_json::json!({ "start_period": start_period, "count": count });
        let (v, prov) = self.lc_request("getLightClientUpdatesByRange", p).await?;
        envelopes_to_updates(v, prov)
    }

    async fn finality_update(&self) -> Result<LcObject> {
        let (v, prov) = self
            .lc_request("getLightClientFinalityUpdate", serde_json::json!({}))
            .await?;
        LcObject::from_envelope(LcKind::FinalityUpdate, v, prov)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Either source, chosen by config
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum AnySource {
    Direct(DirectHttpSource),
    Logos(LogosModuleSource),
}

impl BeaconSource for AnySource {
    fn kind(&self) -> BeaconSourceKind {
        match self {
            Self::Direct(s) => s.kind(),
            Self::Logos(s) => s.kind(),
        }
    }

    async fn bootstrap(&self, block_root: &[u8; 32]) -> Result<LcObject> {
        match self {
            Self::Direct(s) => s.bootstrap(block_root).await,
            Self::Logos(s) => s.bootstrap(block_root).await,
        }
    }

    async fn updates_by_range(&self, start_period: u64, count: u64) -> Result<Vec<LcObject>> {
        match self {
            Self::Direct(s) => s.updates_by_range(start_period, count).await,
            Self::Logos(s) => s.updates_by_range(start_period, count).await,
        }
    }

    async fn finality_update(&self) -> Result<LcObject> {
        match self {
            Self::Direct(s) => s.finality_update().await,
            Self::Logos(s) => s.finality_update().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_answer_shapes() {
        let inner = r#"{"ok":true,"provider":"lodestar","body":"{\"version\":\"gloas\",\"data\":{}}","attempts":[]}"#;
        let as_string = serde_json::to_string(inner).unwrap();
        for s in [
            inner.to_string(),
            as_string.clone(),
            format!("{{\"result\":{as_string}}}"),
        ] {
            let v = parse_module_answer(&s).unwrap();
            assert_eq!(v["ok"], Value::Bool(true));
            assert_eq!(v["provider"], "lodestar");
        }
    }
}
