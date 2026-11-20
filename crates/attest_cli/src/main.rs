//! CLI `attest`: `init`, `update`, `status`, `config`, `attest [--dry-run]`,
//! `checkpoint verify`.
//!
//! LEZ access goes through `attest_host` (wallet and sequencer from the environment: `LEE_WALLET_HOME_DIR`,
//! `LEE_WALLET_PASSWORD`, `BV_STORE_DIR`; program ids from `--ids` or `$BV_IDS` or `$BV_REPO/deploy/ids.toml`).
//! Beacon data comes from Logos `beacon_api_module` (`--source logos_module`) or the Beacon API over HTTP
//! (`--source direct_http`, the default for the CLI).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, Actor, EsaError, HeaderRecord, InstanceConfig, Predicate, Statement};
use attest_host::{DeployedIds, SubmitMode};
use clap::{Args, Parser, Subcommand, ValueEnum};
use follower::chain::Candidate;
use follower::chain::CandidateKind;
use follower::source::period_of;
use follower::{AnySource, BeaconSource, DirectHttpSource, LogosModuleSource};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "attest", version, about = "beacon-veil Ethereum state attestation CLI")]
struct Cli {
    #[command(flatten)]
    g: Global,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Global {
    /// Ethereum chain id (1 mainnet, 11155111 Sepolia).
    #[arg(long, global = true, default_value_t = 11_155_111, env = "BV_CHAIN_ID")]
    chain_id: u64,
    /// Beacon data source.
    #[arg(long, global = true, value_enum, default_value_t = Src::DirectHttp)]
    source: Src,
    /// Beacon API endpoint(s) for direct_http (repeat; default: the public providers beacon_api_module seeds).
    #[arg(long = "endpoint", global = true)]
    endpoints: Vec<String>,
    /// logosctl binary for logos_module.
    #[arg(long, global = true, env = "BV_LOGOSCTL", default_value = "logosctl")]
    logosctl: PathBuf,
    /// logosctl --config-dir (session directory).
    #[arg(long, global = true, env = "BV_LOGOSCTL_CONFIG_DIR")]
    logosctl_config_dir: Option<PathBuf>,
    /// deploy/ids.toml (default: $BV_IDS, else $BV_REPO/deploy/ids.toml).
    #[arg(long, global = true)]
    ids: Option<PathBuf>,
    /// HTTP / module timeout, seconds.
    #[arg(long, global = true, default_value_t = 30)]
    timeout_s: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Src {
    DirectHttp,
    LogosModule,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create an instance from a finalized checkpoint (bootstrap), and print its id.
    Init {
        /// Checkpoint block root (hex), or `finalized` for the current finalized checkpoint.
        #[arg(long, default_value = "finalized")]
        checkpoint: String,
        /// Minimum sync-committee participants (of 512).
        #[arg(long, default_value_t = 342)]
        min_participants: u16,
        /// Extra verifier program id (base58) besides ids.lc_verify (at most one: config allows 1..=2).
        #[arg(long, conflicts_with = "only_verifier")]
        extra_verifier: Option<String>,
        /// The instance's only verifier (base58 program id, for example lc_verify_v3), instead of ids.lc_verify.
        #[arg(long)]
        only_verifier: Option<String>,
        /// Write the instance id and its config here (JSON), for the follower config and `checkpoint verify`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Only build and print the config; send nothing.
        #[arg(long)]
        print_only: bool,
    },
    /// Submit one light-client update now (what the follower would do in one step, without cadence).
    Update {
        #[arg(long)]
        instance: String,
        /// Submit the period update of this period (updates_by_range(P,1)) instead of the latest finality update.
        #[arg(long)]
        period: Option<u64>,
        #[arg(long, value_enum, default_value_t = Mode::Private)]
        mode: Mode,
    },
    /// Show an instance's head and verified store status.
    Status {
        #[arg(long)]
        instance: String,
    },
    /// Show the effective configuration (environment, ids, source).
    Config,
    /// Attest a claim against a header record. `--dry-run` runs the checks on the host with the on-chain error codes.
    Attest {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        instance: String,
        /// Header record: a slot (read from chain) or a file with the borsh hex of a HeaderRecord.
        #[arg(long)]
        record: String,
        /// Encoded inclusion claim (attest_inclusion layout): hex, or @file with hex.
        #[arg(long)]
        claim: String,
        /// borsh(Predicate) as hex, or @file with hex.
        #[arg(long)]
        predicate: String,
        /// Consumer actor `account_id:program_id` (base58), required without --dry-run.
        #[arg(long)]
        deliver_to: Option<String>,
    },
    /// Checkpoint tools.
    Checkpoint {
        #[command(subcommand)]
        cmd: CheckpointCmd,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Private,
    PublicProbe,
}

#[derive(Subcommand)]
enum CheckpointCmd {
    /// Compare a checkpoint with N public beacon sources (F9 fallback): each must report the same block root at the
    /// checkpoint slot, and a finalized slot at or above it.
    Verify {
        /// Instance file written by `attest init --out` (holds the config).
        #[arg(long, conflicts_with_all = ["root", "slot"])]
        instance_file: Option<PathBuf>,
        #[arg(long, requires = "slot")]
        root: Option<String>,
        #[arg(long, requires = "root")]
        slot: Option<u64>,
        /// Beacon API base URLs to ask (repeat). Default: every known public provider for the chain.
        #[arg(long = "against")]
        against: Vec<String>,
        /// How many sources must agree (default: all that answered, and at least 2).
        #[arg(long)]
        min_agree: Option<usize>,
    },
}

fn default_endpoints(chain_id: u64) -> Vec<String> {
    // The public defaults that Logos beacon_api_module 0.2.0 seeds (README table), https first.
    match chain_id {
        1 => vec![
            "https://lodestar-mainnet.chainsafe.io".into(),
            "http://testing.mainnet.beacon-api.nimbus.team".into(),
            "http://unstable.mainnet.beacon-api.nimbus.team".into(),
        ],
        11_155_111 => vec![
            "https://lodestar-sepolia.chainsafe.io".into(),
            "http://unstable.sepolia.beacon-api.nimbus.team".into(),
        ],
        560_048 => vec![
            "https://lodestar-hoodi.chainsafe.io".into(),
            "http://testing.hoodi.beacon-api.nimbus.team".into(),
        ],
        _ => vec![],
    }
}

impl Global {
    fn endpoints(&self) -> Vec<String> {
        if self.endpoints.is_empty() {
            default_endpoints(self.chain_id)
        } else {
            self.endpoints.clone()
        }
    }

    fn http(&self) -> Result<DirectHttpSource> {
        DirectHttpSource::new(self.endpoints(), Duration::from_secs(self.timeout_s))
    }

    fn source(&self) -> Result<AnySource> {
        Ok(match self.source {
            Src::DirectHttp => AnySource::Direct(self.http()?),
            Src::LogosModule => AnySource::Logos(self.logos()),
        })
    }

    fn logos(&self) -> LogosModuleSource {
        LogosModuleSource {
            logosctl: self.logosctl.clone(),
            config_dir: self.logosctl_config_dir.clone(),
            tmpdir: None,
            chain_id: self.chain_id,
            provider: String::new(),
            deadline: Duration::from_secs(self.timeout_s),
        }
    }

    /// A non-light-client Beacon API GET (genesis, fork schedule, headers): through the module's `raw_get` or HTTP.
    async fn beacon_get(&self, path: &str) -> Result<Value> {
        match self.source {
            Src::DirectHttp => Ok(self.http()?.get_json(path).await?.0),
            Src::LogosModule => {
                let ans = self
                    .logos()
                    .call(
                        "raw_get",
                        &[self.chain_id.to_string(), format!("str:{path}"), "str:".into()],
                    )
                    .await?;
                if ans.get("ok").and_then(Value::as_bool) != Some(true) {
                    bail!("beacon_api_module.raw_get {path}: {ans}");
                }
                let r = ans
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow!("raw_get: no result"))?;
                Ok(match r {
                    Value::String(s) => serde_json::from_str(&s)?,
                    v => v,
                })
            }
        }
    }

    fn ids(&self) -> Result<DeployedIds> {
        match &self.ids {
            Some(p) => DeployedIds::load(p),
            None => DeployedIds::from_env(),
        }
    }
}

/// The instance's verifiers: `only` alone when given; otherwise the deployment's `lc_verify` (when known) and the
/// optional extra one.
fn verifier_list(default: Option<AccountId>, extra: Option<AccountId>, only: Option<AccountId>) -> Vec<AccountId> {
    match only {
        Some(v) => vec![v],
        None => default.into_iter().chain(extra).collect(),
    }
}

fn parse_id(s: &str) -> Result<AccountId> {
    s.trim().parse().map_err(|e| anyhow!("bad account id '{s}': {e:?}"))
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let b = hex::decode(s.trim().trim_start_matches("0x")).with_context(|| format!("bad hex '{s}'"))?;
    b.try_into().map_err(|_| anyhow!("expected 32 bytes: '{s}'"))
}

fn hex_arg(s: &str) -> Result<Vec<u8>> {
    let text = match s.strip_prefix('@') {
        Some(p) => std::fs::read_to_string(p).with_context(|| format!("read {p}"))?,
        None => s.to_string(),
    };
    hex::decode(text.trim().trim_start_matches("0x")).context("bad hex")
}

fn str_u64(v: &Value, ptr: &str) -> Result<u64> {
    let x = v.pointer(ptr).ok_or_else(|| anyhow!("missing {ptr}"))?;
    match x {
        Value::String(s) => Ok(s.parse()?),
        Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("{ptr}")),
        _ => bail!("{ptr} not a number"),
    }
}

fn config_json(c: &InstanceConfig) -> Value {
    json!({
        "chain_id": c.chain_id,
        "genesis_validators_root": format!("0x{}", hex::encode(c.genesis_validators_root)),
        "genesis_time": c.genesis_time,
        "forks": c.forks.iter().map(|(e, v)| json!([e, format!("0x{}", hex::encode(v))])).collect::<Vec<_>>(),
        "checkpoint_root": format!("0x{}", hex::encode(c.checkpoint_root)),
        "checkpoint_slot": c.checkpoint_slot,
        "verifiers": c.verifiers.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "min_participants": c.min_participants,
        "borsh_hex": hex::encode(borsh::to_vec(c).expect("borsh")),
    })
}

fn statement_json(s: &Statement) -> Value {
    json!({
        "instance": s.instance.to_string(), "chain_id": s.chain_id, "slot": s.slot, "exec_number": s.exec_number,
        "kind": format!("{:?}", s.kind), "subject": hex::encode(&s.subject), "value": hex::encode(&s.value),
        "predicate_hash": hex::encode(s.predicate_hash), "id": hex::encode(s.id),
    })
}

fn record_json(r: &HeaderRecord) -> Value {
    json!({
        "chain_id": r.chain_id, "slot": r.slot, "beacon_root": hex::encode(r.beacon_root),
        "exec_number": r.exec_number, "exec_hash": hex::encode(r.exec_hash),
        "exec_state_root": hex::encode(r.exec_state_root), "exec_receipts_root": hex::encode(r.exec_receipts_root),
        "hash": hex::encode(r.hash()),
    })
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let g = cli.g;
    match cli.cmd {
        Cmd::Config => {
            let env = |k: &str| std::env::var(k).ok();
            print(&json!({
                "chain_id": g.chain_id,
                "source": match g.source { Src::DirectHttp => "direct_http", Src::LogosModule => "logos_module" },
                "endpoints": g.endpoints(),
                "logosctl": g.logosctl, "logosctl_config_dir": g.logosctl_config_dir,
                "ids_file": g.ids.clone().unwrap_or_else(attest_host::ids_path),
                "ids": g.ids().map(|i| json!({"attest": i.attest.to_string(), "lc_verify": i.lc_verify.to_string()}))
                    .unwrap_or_else(|e| json!({"error": format!("{e:#}")})),
                "store_dir": attest_host::store_dir(),
                "LEE_WALLET_HOME_DIR": env("LEE_WALLET_HOME_DIR"),
                "RISC0_DEV_MODE": env("RISC0_DEV_MODE"),
            }));
        }
        Cmd::Init {
            checkpoint,
            min_participants,
            extra_verifier,
            only_verifier,
            out,
            print_only,
        } => {
            let ids = g.ids();
            let genesis = g.beacon_get("/eth/v1/beacon/genesis").await?;
            let gvr = hex32(
                genesis
                    .pointer("/data/genesis_validators_root")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            )?;
            let genesis_time = str_u64(&genesis, "/data/genesis_time")?;
            let sched = g.beacon_get("/eth/v1/config/fork_schedule").await?;
            // Positional, as attest_core::lc::forks reads them (genesis, altair, ..., fulu, gloas): the first 8 entries
            // of the schedule, a far-future entry included (same rule as tests/e2e_chain config_from_beacon).
            let mut forks = Vec::new();
            for f in sched
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("fork_schedule: no data"))?
                .iter()
                .take(8)
            {
                let epoch = str_u64(f, "/epoch")?;
                let v = hex::decode(f["current_version"].as_str().unwrap_or("").trim_start_matches("0x"))?;
                let v: [u8; 4] = v.try_into().map_err(|_| anyhow!("fork version is not 4 bytes"))?;
                forks.push((epoch, v));
            }
            let (root, slot) = if checkpoint == "finalized" {
                let h = g.beacon_get("/eth/v1/beacon/headers/finalized").await?;
                (
                    hex32(h.pointer("/data/root").and_then(Value::as_str).unwrap_or(""))?,
                    str_u64(&h, "/data/header/message/slot")?,
                )
            } else {
                let root = hex32(&checkpoint)?;
                let h = g
                    .beacon_get(&format!("/eth/v1/beacon/headers/0x{}", hex::encode(root)))
                    .await?;
                (root, str_u64(&h, "/data/header/message/slot")?)
            };
            let src = g.source()?;
            let boot = src.bootstrap(&root).await?;
            let bslot = boot.finalized_slot()?;
            if bslot != slot {
                bail!("bootstrap header slot {bslot} != checkpoint slot {slot}");
            }
            let boot_ssz = boot.to_ssz()?;
            let default = match &ids {
                Ok(i) => Some(i.lc_verify),
                Err(_) if only_verifier.is_some() => None,
                Err(e) if print_only => {
                    eprintln!("note: no deploy ids ({e:#}); verifiers left empty");
                    None
                }
                Err(e) => bail!("deploy ids: {e:#}"),
            };
            let verifiers = verifier_list(
                default,
                extra_verifier.as_deref().map(parse_id).transpose()?,
                only_verifier.as_deref().map(parse_id).transpose()?,
            );
            let config = InstanceConfig {
                chain_id: g.chain_id,
                genesis_validators_root: gvr,
                genesis_time,
                forks,
                checkpoint_root: root,
                checkpoint_slot: slot,
                verifiers,
                min_participants,
            };
            let mut outv = json!({
                "config": config_json(&config), "bootstrap_version": boot.version, "bootstrap_ssz_bytes": boot_ssz.len(),
                "bootstrap_provider": boot.provider, "period": period_of(slot),
            });
            if let Ok(i) = &ids {
                outv["instance"] = json!(attest_core::instance_id(&i.attest, &config).to_string());
            }
            if !print_only {
                let ids = ids?;
                let (inst, o) = attest_host::init(&ids, &config, &boot_ssz).await?;
                outv["instance"] = json!(inst.to_string());
                outv["tx"] = json!(o.tx);
                outv["cycles"] = json!(o.cycles);
                outv["gas"] = json!(o.gas);
            }
            if let Some(p) = out {
                std::fs::write(&p, serde_json::to_vec_pretty(&outv)?)
                    .with_context(|| format!("write {}", p.display()))?;
            }
            print(&outv);
        }
        Cmd::Update { instance, period, mode } => {
            let ids = g.ids()?;
            let inst = parse_id(&instance)?;
            let src = g.source()?;
            let cand = match period {
                Some(p) => {
                    let u = src
                        .updates_by_range(p, 1)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow!("no update for period {p}"))?;
                    Candidate::from_object(CandidateKind::Period { period: p }, &u)?
                }
                None => Candidate::from_object(
                    CandidateKind::Finality { handoff: false },
                    &src.finality_update().await?,
                )?,
            };
            let mode = match mode {
                Mode::Private => SubmitMode::Private,
                Mode::PublicProbe => SubmitMode::PublicProbe,
            };
            let o = attest_host::submit_update(&ids, &inst, &cand.ssz, mode).await?;
            let st = attest_host::read_store_status(&ids, &inst).await.ok().flatten();
            print(&json!({
                "kind": cand.kind.label(), "finalized_slot": cand.finalized_slot, "signature_slot": cand.signature_slot,
                "participation": cand.participation, "tx": o.tx, "cycles": o.cycles, "gas": o.gas,
                "head_slot": st.map(|s| s.head_slot), "period": st.map(|s| s.period),
                "next_committee_known": st.map(|s| s.next_committee_known),
            }));
        }
        Cmd::Status { instance } => {
            let ids = g.ids()?;
            let inst = parse_id(&instance)?;
            let head = attest_host::read_head(&ids, &inst).await?;
            let st = attest_host::read_store_status(&ids, &inst).await;
            let Some(h) = head else {
                print(&json!({"instance": instance, "exists": false}));
                std::process::exit(3);
            };
            print(&json!({
                "instance": instance, "exists": true,
                "head": {
                    "slot": h.slot, "period": h.period, "beacon_root": hex::encode(h.beacon_root),
                    "exec_number": h.exec_number, "exec_hash": hex::encode(h.exec_hash),
                    "exec_state_root": hex::encode(h.exec_state_root), "exec_receipts_root": hex::encode(h.exec_receipts_root),
                    "store_commitment": hex::encode(h.store_commitment),
                },
                "store": match st {
                    Ok(Some(s)) => json!({"verified": true, "head_slot": s.head_slot, "period": s.period,
                                          "next_committee_known": s.next_committee_known}),
                    Ok(None) => json!({"verified": false}),
                    Err(e) => json!({"verified": false, "error": format!("{e:#}")}),
                },
            }));
        }
        Cmd::Attest {
            dry_run,
            instance,
            record,
            claim,
            predicate,
            deliver_to,
        } => {
            let inst = parse_id(&instance)?;
            let claim = hex_arg(&claim)?;
            let predicate: Predicate = borsh::from_slice(&hex_arg(&predicate)?).context("predicate: borsh")?;
            let record = load_record(&g, &inst, &record).await?;
            if dry_run {
                match attest_host::dry_run_attest(&inst, &record, &claim, &predicate) {
                    Ok(s) => {
                        print(&json!({"ok": true, "record": record_json(&record), "statement": statement_json(&s)}))
                    }
                    Err(e) => {
                        print(&json!({"ok": false, "code": e.code(), "error": e.panic_text()}));
                        std::process::exit(exit_code(e));
                    }
                }
            } else {
                let ids = g.ids()?;
                let d = deliver_to.ok_or_else(|| anyhow!("--deliver-to account:program is required"))?;
                let (a, p) = d
                    .split_once(':')
                    .ok_or_else(|| anyhow!("--deliver-to must be account:program"))?;
                let actor = Actor::new(parse_id(a)?, parse_id(p)?);
                let (s, o) = attest_host::attest(&ids, &inst, &record, &claim, &predicate, &actor).await?;
                print(
                    &json!({"ok": true, "tx": o.tx, "cycles": o.cycles, "gas": o.gas, "statement": statement_json(&s)}),
                );
            }
        }
        Cmd::Checkpoint {
            cmd:
                CheckpointCmd::Verify {
                    instance_file,
                    root,
                    slot,
                    against,
                    min_agree,
                },
        } => {
            let (root, slot, chain_id) = match (instance_file, root, slot) {
                (Some(f), _, _) => {
                    let v: Value =
                        serde_json::from_slice(&std::fs::read(&f).with_context(|| format!("read {}", f.display()))?)?;
                    (
                        hex32(
                            v.pointer("/config/checkpoint_root")
                                .and_then(Value::as_str)
                                .unwrap_or(""),
                        )?,
                        v.pointer("/config/checkpoint_slot")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| anyhow!("no checkpoint_slot"))?,
                        v.pointer("/config/chain_id")
                            .and_then(Value::as_u64)
                            .unwrap_or(g.chain_id),
                    )
                }
                (None, Some(r), Some(s)) => (hex32(&r)?, s, g.chain_id),
                _ => bail!("give --instance-file, or --root and --slot"),
            };
            let sources = if against.is_empty() {
                default_endpoints(chain_id)
            } else {
                against
            };
            let r = checkpoint_verify(&root, slot, &sources, g.timeout_s).await;
            let answered = r.iter().filter(|x| x["answered"] == json!(true)).count();
            let agree = r.iter().filter(|x| x["agrees"] == json!(true)).count();
            let disagree = r
                .iter()
                .filter(|x| x["answered"] == json!(true) && x["agrees"] != json!(true))
                .count();
            let need = min_agree.unwrap_or_else(|| answered.max(2));
            let ok = disagree == 0 && agree >= need;
            print(&json!({
                "ok": ok, "checkpoint_root": format!("0x{}", hex::encode(root)), "checkpoint_slot": slot,
                "chain_id": chain_id, "agree": agree, "answered": answered, "disagree": disagree, "need": need,
                "sources": r,
            }));
            if !ok {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// Exit code of a refused dry run: 10 + the numeric ESA code (E08 -> 18).
fn exit_code(e: EsaError) -> i32 {
    10 + i32::from(e as u8)
}

async fn load_record(g: &Global, inst: &AccountId, record: &str) -> Result<HeaderRecord> {
    if let Ok(slot) = record.parse::<u64>() {
        let ids = g.ids()?;
        return attest_host::read_record(&ids, inst, slot)
            .await?
            .ok_or_else(|| anyhow!("no header record at slot {slot}"));
    }
    let p = Path::new(record.strip_prefix('@').unwrap_or(record));
    let b = hex::decode(
        std::fs::read_to_string(p)
            .with_context(|| format!("read {}", p.display()))?
            .trim(),
    )?;
    borsh::from_slice(&b).context("record: borsh(HeaderRecord)")
}

/// Asks each source for the block root at `slot` and its finalized slot.
async fn checkpoint_verify(root: &[u8; 32], slot: u64, sources: &[String], timeout_s: u64) -> Vec<Value> {
    let mut out = Vec::new();
    for s in sources {
        let r = async {
            let src = DirectHttpSource::new(vec![s.clone()], Duration::from_secs(timeout_s))?;
            let (v, _) = src.get_json(&format!("/eth/v1/beacon/blocks/{slot}/root")).await?;
            let got = hex32(v.pointer("/data/root").and_then(Value::as_str).unwrap_or(""))?;
            let (f, _) = src.get_json("/eth/v1/beacon/headers/finalized").await?;
            let fin = str_u64(&f, "/data/header/message/slot")?;
            Ok::<_, anyhow::Error>((got, fin))
        }
        .await;
        out.push(match r {
            Ok((got, fin)) => json!({
                "source": s, "answered": true, "root": format!("0x{}", hex::encode(got)), "finalized_slot": fin,
                "agrees": got == *root && fin >= slot,
            }),
            Err(e) => json!({"source": s, "answered": false, "error": format!("{e:#}")}),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_lists() {
        let (v1, v3) = (AccountId::new([1; 32]), AccountId::new([3; 32]));
        assert_eq!(verifier_list(Some(v1), None, None), [v1]);
        assert_eq!(verifier_list(Some(v1), Some(v3), None), [v1, v3]);
        assert_eq!(verifier_list(Some(v1), None, Some(v3)), [v3], "only the given verifier");
        assert_eq!(verifier_list(None, None, Some(v3)), [v3]);
        assert!(verifier_list(None, None, None).is_empty());
    }

    #[test]
    fn only_and_extra_conflict() {
        use clap::Parser;
        let ok = Cli::try_parse_from(["attest", "init", "--only-verifier", "x"]);
        assert!(ok.is_ok());
        let both = Cli::try_parse_from(["attest", "init", "--only-verifier", "x", "--extra-verifier", "y"]);
        assert!(both.is_err());
    }
}
