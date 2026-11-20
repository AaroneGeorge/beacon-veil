//! `attest-followerd --config <file.toml>`: follows a beacon chain and keeps an `attest` instance current.
//!
//! - SIGTERM / SIGINT: finish the step in progress (a submission is never cut in half), log `stop`, exit 0.
//!   A second signal exits at once with 143.
//! - Restart: resumes from the instance's on-chain head (021 R4). A pid lock in the state dir refuses a second
//!   follower on the same state dir.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use follower::chain::{HostChain, HostMode, MockChain};
use follower::config::Config;
use follower::follower::{Follower, Params};
use follower::log::Logger;
use follower::{AnySource, BeaconSource, Chain};
use tokio::signal::unix::{SignalKind, signal};

fn usage() -> ! {
    eprintln!("usage: attest-followerd --config <file.toml> [--once]");
    std::process::exit(2)
}

/// The program id under `[programs] <name>` in an ids file (`deploy/ids-pub.toml`, ...).
fn program_id(ids_file: &Path, name: &str) -> Result<attest_core::AccountId> {
    let text = std::fs::read_to_string(ids_file).with_context(|| format!("read {}", ids_file.display()))?;
    let doc: toml::Table = toml::from_str(&text).with_context(|| format!("parse {}", ids_file.display()))?;
    doc.get("programs")
        .and_then(|p| p.get(name))
        .and_then(toml::Value::as_str)
        .with_context(|| format!("{}: [programs] {name} missing", ids_file.display()))?
        .parse()
        .map_err(|e| anyhow::anyhow!("{}: [programs] {name}: {e:?}", ids_file.display()))
}

struct PidLock(PathBuf);

impl PidLock {
    fn acquire(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let p = dir.join("followerd.pid");
        if let Ok(s) = std::fs::read_to_string(&p)
            && let Ok(pid) = s.trim().parse::<u32>()
        {
            let alive = Path::new(&format!("/proc/{pid}")).exists();
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if alive && pid != std::process::id() && String::from_utf8_lossy(&cmd).contains("attest-followerd") {
                bail!("another attest-followerd (pid {pid}) holds {}", p.display());
            }
        }
        std::fs::write(&p, format!("{}\n", std::process::id()))?;
        Ok(Self(p))
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut config: Option<PathBuf> = None;
    let mut once = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" | "-c" => config = Some(args.next().unwrap_or_else(|| usage()).into()),
            "--once" => once = true,
            "--help" | "-h" => usage(),
            _ => usage(),
        }
    }
    let cfg_path = config.unwrap_or_else(|| usage());
    let cfg = Config::load(&cfg_path)?;
    let _lock = PidLock::acquire(&cfg.follower.state_dir)?;
    let source = cfg.build_source()?;

    match cfg.chain.kind.as_str() {
        "mock" => {
            let path = cfg
                .chain
                .mock_file
                .clone()
                .unwrap_or_else(|| cfg.follower.state_dir.join("mock-chain.json"));
            let instance = cfg
                .chain
                .instance
                .clone()
                .unwrap_or_else(|| format!("mock:{}", path.display()));
            let mock = MockChain::new(path);
            let mock = if cfg.chain.mode == "public" {
                mock.staged()
            } else {
                mock
            };
            run(&cfg, &cfg_path, source, mock, instance, once).await
        }
        _ => {
            let ids = match &cfg.chain.ids_file {
                Some(p) => attest_host::DeployedIds::load(p)?,
                None => attest_host::DeployedIds::from_env()?,
            };
            let instance_s = cfg.chain.instance.clone().context("chain.instance")?;
            let instance = instance_s
                .parse()
                .map_err(|e| anyhow::anyhow!("chain.instance: {e:?}"))?;
            let ids_file = cfg.chain.ids_file.clone().unwrap_or_else(attest_host::ids_path);
            let mode = match cfg.chain.mode.as_str() {
                "private" => HostMode::Single(attest_host::SubmitMode::Private),
                "public_probe" => HostMode::Single(attest_host::SubmitMode::PublicProbe),
                "public" => {
                    let name = cfg.chain.verifier.as_deref().unwrap_or("lc_verify_v3");
                    HostMode::Public {
                        v3: program_id(&ids_file, name)?,
                    }
                }
                m => bail!("chain.mode '{m}': expected private, public_probe or public"),
            };
            run(
                &cfg,
                &cfg_path,
                source,
                HostChain { ids, instance, mode },
                instance_s,
                once,
            )
            .await
        }
    }
}

async fn run<C: Chain + Sync>(
    cfg: &Config,
    cfg_path: &Path,
    source: AnySource,
    chain: C,
    instance: String,
    once: bool,
) -> Result<()> {
    let log = Logger::open(&cfg.log_file(), source.kind(), cfg.follower.echo)?;
    let params = Params {
        follower_id: cfg.follower.id.clone(),
        instance: instance.clone(),
        cadence: Duration::from_secs(cfg.follower.cadence_s),
        poll: Duration::from_secs(cfg.follower.poll_s),
        jitter_max: Duration::from_secs(cfg.follower.jitter_max_s),
        min_participation: cfg.follower.min_participation,
        min_payer_balance: cfg.min_payer_balance(),
        state_dir: cfg.follower.state_dir.clone(),
    };
    let detail = serde_json::json!({
        "config": cfg_path.display().to_string(),
        "follower_id": cfg.follower.id,
        "chain_id": cfg.follower.chain_id,
        "chain": chain.name(),
        "instance": instance,
        "mode": cfg.chain.mode,
        "verifier": (cfg.chain.mode == "public").then(|| cfg.chain.verifier.clone().unwrap_or_else(|| "lc_verify_v3".into())),
        "sequencer": cfg.chain.sequencer,
        "cadence_s": cfg.follower.cadence_s,
        "poll_s": cfg.follower.poll_s,
        "jitter_max_s": cfg.follower.jitter_max_s,
        "source_kind": source.kind().as_str(),
        "endpoints": cfg.source.endpoints,
        "risc0_dev_mode": std::env::var("RISC0_DEV_MODE").ok(),
        "version": env!("CARGO_PKG_VERSION"),
    });
    let mut f = Follower::new(source, chain, params, log)?;

    let (tx, rx) = tokio::sync::watch::channel(false);
    if once {
        let mut rx2 = rx.clone();
        f.step(&mut rx2).await?;
        return Ok(());
    }
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::spawn(async move {
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
        let _ = tx.send(true);
        // Second signal: exit now.
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
        eprintln!("attest-followerd: second signal, exiting now");
        std::process::exit(143);
    });
    f.run(rx, detail).await
}
