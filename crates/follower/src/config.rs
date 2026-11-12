//! `attest-followerd` config file (TOML). Example: `crates/follower/examples/followerd.sepolia.toml`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::source::{AnySource, DirectHttpSource, LogosModuleSource};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub follower: FollowerCfg,
    pub source: SourceCfg,
    pub chain: ChainCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowerCfg {
    /// Name of this follower; part of the deferral-jitter seed, so two followers of one instance spread out.
    pub id: String,
    pub chain_id: u64,
    /// Directory for the state cache, pid lock and (by default) the log.
    pub state_dir: PathBuf,
    /// JSON-lines log. Default `<state_dir>/follower.jsonl`.
    #[serde(default)]
    pub log_file: Option<PathBuf>,
    /// Seconds between finality updates within a period (16 epochs = 6144 s). Handoffs do not wait for it.
    #[serde(default = "d_cadence")]
    pub cadence_s: u64,
    /// Seconds between checks. Every check writes one line, so the log has no gap longer than this plus a fetch.
    #[serde(default = "d_poll")]
    pub poll_s: u64,
    /// Upper bound of the per-instance deferral before a submission.
    #[serde(default = "d_jitter")]
    pub jitter_max_s: u64,
    /// Updates below this participation (of 512) are not submitted (Helios needs 2/3 to apply).
    #[serde(default = "d_min_part")]
    pub min_participation: u32,
    /// Also echo log lines to stdout.
    #[serde(default = "d_true")]
    pub echo: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCfg {
    /// `logos_module` or `direct_http`.
    pub kind: String,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default = "d_timeout")]
    pub timeout_s: u64,
    #[serde(default)]
    pub logosctl: Option<PathBuf>,
    #[serde(default)]
    pub logosctl_config_dir: Option<PathBuf>,
    /// TMPDIR the logosctl daemon was started with (its local endpoint lives there). Default: inherit.
    #[serde(default)]
    pub logosctl_tmpdir: Option<PathBuf>,
    #[serde(default)]
    pub provider: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainCfg {
    /// `host` (attest_host to a LEZ sequencer) or `mock` (file-backed, tests only).
    pub kind: String,
    /// Instance account id (base58). Required for `host`.
    #[serde(default)]
    pub instance: Option<String>,
    /// `deploy/ids.toml`; default: `attest_host::ids_path()` ($BV_IDS or $BV_REPO/deploy/ids.toml).
    #[serde(default)]
    pub ids_file: Option<PathBuf>,
    /// `private` (Path A) or `public_probe` (B0).
    #[serde(default = "d_mode")]
    pub mode: String,
    /// Mock state file; default `<state_dir>/mock-chain.json`.
    #[serde(default)]
    pub mock_file: Option<PathBuf>,
    // attest_host reads LEE_WALLET_HOME_DIR, LEE_WALLET_PASSWORD and BV_STORE_DIR from the environment; the launcher
    // (scripts/follower/run_a.sh) sets them. Setting env vars in-process is `unsafe` in edition 2024, which the
    // workspace forbids.
}

fn d_cadence() -> u64 {
    6144
}
fn d_poll() -> u64 {
    300
}
fn d_jitter() -> u64 {
    60
}
fn d_min_part() -> u32 {
    342
}
fn d_true() -> bool {
    true
}
fn d_timeout() -> u64 {
    30
}
fn d_mode() -> String {
    "private".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
        let c: Self = toml::from_str(&s).with_context(|| format!("parse config {}", path.display()))?;
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<()> {
        if self.follower.poll_s == 0 || self.follower.cadence_s == 0 {
            bail!("poll_s and cadence_s must be positive");
        }
        if self.follower.poll_s * 2 > self.follower.cadence_s {
            bail!("poll_s must be at most half of cadence_s");
        }
        match self.source.kind.as_str() {
            "direct_http" if self.source.endpoints.is_empty() => bail!("source.endpoints is empty"),
            "direct_http" | "logos_module" => {}
            k => bail!("source.kind '{k}': expected logos_module or direct_http"),
        }
        match self.chain.kind.as_str() {
            "host" if self.chain.instance.is_none() => bail!("chain.instance is required for chain.kind = host"),
            "host" | "mock" => {}
            k => bail!("chain.kind '{k}': expected host or mock"),
        }
        Ok(())
    }

    pub fn log_file(&self) -> PathBuf {
        self.follower
            .log_file
            .clone()
            .unwrap_or_else(|| self.follower.state_dir.join("follower.jsonl"))
    }

    pub fn build_source(&self) -> Result<AnySource> {
        let timeout = Duration::from_secs(self.source.timeout_s);
        Ok(match self.source.kind.as_str() {
            "direct_http" => AnySource::Direct(DirectHttpSource::new(self.source.endpoints.clone(), timeout)?),
            _ => AnySource::Logos(LogosModuleSource {
                logosctl: self
                    .source
                    .logosctl
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("logosctl")),
                config_dir: self.source.logosctl_config_dir.clone(),
                tmpdir: self.source.logosctl_tmpdir.clone(),
                chain_id: self.follower.chain_id,
                provider: self.source.provider.clone(),
                deadline: timeout,
            }),
        })
    }
}
