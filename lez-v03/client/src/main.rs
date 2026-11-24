//! `bv-v03`: the client of the LEZ v0.3 port on the public LEZ testnet 0.3 (`https://testnet.lez.logos.co`, LEZ
//! v0.3.2). Modelled on LEZ's own example runners (`examples/program_deployment`, branch
//! `fix/program-deployment-runners-declare-fees-v0.3.2`, MIT): deploy through the program loader, a fee paid by a
//! payer that co-signs without joining the inputs.
//!
//! Every transaction is simulated before it is sent: the signed transaction runs in LEZ v0.3.2's own state machine
//! (`lee::V03State`) seeded with the accounts just read from the testnet; it is sent only if it applies there (the
//! refusal demo is the one transaction sent because it is refused). Gas limit = simulated cycles + 3%, never above
//! 9,000,000. One transaction in flight at a time, the next only once the previous is in a block; at most one RPC
//! request a second; no send while `base_fee_exec` is above `--max-base-fee` (9).
//!
//! Home (`--home`, default `$V03_HOME`): `payer.key` (hex, mode 600), `deploy.json` (ids and keys of the program
//! headers), `progress.json` (every sent transaction). Nothing in the home goes into the repository; `record`
//! writes the public part (ids, hashes, raw bytes, states) to the evidence folder.

mod chain;
mod rpc;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use attest_core::{InstanceConfig, InstanceState, lc};
use attest_v03::core_id;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::{Parser, Subcommand};
use lee::{
    FeeDeclaration, PrivateKey, PublicKey, PublicTransaction, V03State, ValidatedStateDiff,
    public_transaction::{Message, WitnessSet},
};
use lee_core::{
    account::{Account, AccountId, Nonce, ProgramShardSelector},
    program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader},
};
use program_loader_core::MAX_SEGMENT_DATA_LEN;
use serde::{Deserialize, Serialize};
use serde_json::json;
use v03_harness::{
    attest,
    fixtures::{config_from_beacon, split_updates_by_range},
    steps::{self, Ids, Tx},
    world::STEP_LIMIT,
};

use crate::{chain::LeeTransaction, rpc::Rpc};

/// Gas limit of a program-loader transaction: the loader runs natively and meters no cycles; the fee is its bytes.
const LOADER_GAS: u64 = 100_000;

#[derive(Parser)]
#[command(name = "bv-v03", about = "beacon-veil LEZ v0.3 port: public testnet client")]
struct Cli {
    #[arg(long, env = "V03_RPC", default_value = "https://testnet.lez.logos.co")]
    rpc: String,
    #[arg(long, env = "V03_HOME")]
    home: PathBuf,
    /// Built guests (`attest_v03.bin`, `lc_verify_v3_v03.bin`).
    #[arg(long, env = "V03_GUESTS")]
    guests: PathBuf,
    /// No send while the head's `base_fee_exec` is above this.
    #[arg(long, default_value_t = 9)]
    max_base_fee: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the payer key in the home (refuses if one exists); prints its account id.
    PayerNew,
    /// The payer's account id, nonce and balance.
    Payer,
    /// Everything the run would send, simulated in-process from the payer's live account: sizes, gas, fees, funds
    /// needed. Sends nothing.
    Plan {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Deploy programs through the program loader, immutable headers: `--program NAME` (repeatable), default
    /// `attest_v03` and `lc_verify_v3_v03`.
    Deploy {
        #[arg(long = "program")]
        programs: Vec<String>,
    },
    /// `Init` of the instance from the data folder's bootstrap.
    Init {
        #[arg(long)]
        data: PathBuf,
    },
    /// Apply the data folder's updates (`updates_by_range`, then finality updates by slot, or the manifest's
    /// `apply_order`), each as Begin, Miller, Finish; resumes from the chain's instance and job state. `--max N` stops
    /// after N applied updates.
    Update {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        max: Option<usize>,
    },
    /// Send the last update's `Finish` again: it must be refused (exit code) and change nothing.
    RefuseDemo {
        #[arg(long)]
        data: PathBuf,
    },
    /// Move `amount` lepta of the home payer's native balance to `to` (a public native transfer; the home payer
    /// signs and pays the fee). Used to top up a run's payer from a second funded account.
    Transfer {
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: u128,
    },
    /// `Attest` of `attest_fact_v03`: the claim folder's storage fact (`tools/fixtures/fetch_claim.py`) under the
    /// header record that `attest_v03` holds for its beacon slot. `--plan`: deploy `attest_fact_v03` and attest in
    /// process, from the chain's accounts, and print the fees and the funds needed; sends nothing.
    Attest {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        claim: PathBuf,
        #[arg(long)]
        plan: bool,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Add the `attest_fact_v03` deployment and the `Attest` to an evidence folder written by `record`: rows of
    /// `txs.tsv` and raw bytes for the transactions not yet in it, the program in `deployment.json` and `IMAGE_IDS`,
    /// `attest` in `run.json`. Nothing else in the folder changes.
    RecordAttest {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        claim: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Write the evidence folder from the home's progress and the chain's state.
    Record {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

// ---------------------------------------------------------------------------------------------------------------
// Home

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Deployed {
    header: String,
    /// Key of the header account (it signs `CreateHeader`; the header is immutable afterwards).
    header_key: String,
    segments: Vec<String>,
    image_id: String,
    elf_sha256: String,
    txs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Deployment {
    programs: BTreeMap<String, Deployed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sent {
    name: String,
    hash: String,
    block: u64,
    bytes: u64,
    gas_limit: u64,
    simulated_cycles: u64,
    /// `None` for a transaction that is expected to apply; `Some(code)` for the refusal demo.
    expected_exit: Option<u32>,
    base_fee_exec: u64,
    base_fee_stor: u64,
    payer_before: String,
    payer_after: String,
    sent_utc: String,
    raw_b64: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Progress {
    sent: Vec<Sent>,
}

struct Home {
    dir: PathBuf,
}

impl Home {
    fn path(&self, f: &str) -> PathBuf {
        self.dir.join(f)
    }

    fn payer_key(&self) -> Result<PrivateKey> {
        let hex_key = std::fs::read_to_string(self.path("payer.key")).context("no payer.key in the home")?;
        let bytes: [u8; 32] = hex::decode(hex_key.trim())?
            .try_into()
            .map_err(|_| anyhow!("payer.key: 32 bytes"))?;
        PrivateKey::try_new(bytes).map_err(|e| anyhow!("payer.key: {e}"))
    }

    fn load<T: Default + for<'a> Deserialize<'a>>(&self, f: &str) -> Result<T> {
        match std::fs::read(self.path(f)) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save<T: Serialize>(&self, f: &str, v: &T) -> Result<()> {
        let tmp = self.path(&format!("{f}.tmp"));
        write_private(&tmp, serde_json::to_string_pretty(v)?.as_bytes())?;
        std::fs::rename(tmp, self.path(f))?;
        Ok(())
    }
}

fn write_private(p: &Path, bytes: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(p)?;
    f.write_all(bytes)?;
    Ok(())
}

fn id_of(key: &PrivateKey) -> AccountId {
    AccountId::from(&PublicKey::new_from_private_key(key))
}

fn bal(a: &Account) -> u128 {
    a.data.native_balance().unwrap_or(0)
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn utc() -> String {
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let (days, rem) = (s / 86_400, s % 86_400);
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// ---------------------------------------------------------------------------------------------------------------
// Data folder (written by the root workspace's `tools/fixtures/fetch.py`)

struct Data {
    config: InstanceConfig,
    bootstrap: Vec<u8>,
    /// `(label, update_ssz)`: `updates_by_range` items, then finality updates by attested slot; or the manifest's
    /// `apply_order`.
    updates: Vec<(String, Vec<u8>)>,
}

fn load_data(dir: &Path, ids: &Ids) -> Result<Data> {
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
    let files = manifest["files"].as_object().context("manifest.files")?;
    let chain_id = manifest["chain_id"].as_u64().context("manifest.chain_id")?;
    let pick = |prefix: &str| -> Vec<(String, &serde_json::Value)> {
        let mut v: Vec<_> = files
            .iter()
            .filter(|(k, _)| k.starts_with(prefix) && k.ends_with(".ssz"))
            .map(|(k, e)| (k.clone(), e))
            .collect();
        v.sort_by_key(|(k, e)| (e["attested_slot"].as_u64().unwrap_or(0), k.clone()));
        v
    };
    let (bname, _) = pick("bootstrap_").into_iter().next().context("no bootstrap_*.ssz")?;
    let bootstrap = std::fs::read(dir.join(&bname))?;
    let config = config_from_beacon(
        chain_id,
        &std::fs::read(dir.join("genesis.json"))?,
        &std::fs::read(dir.join("fork_schedule.json"))?,
        &bootstrap,
        vec![core_id(&ids.v3)],
        0,
    )?;
    // `apply_order` (written by the keeper, which adds updates over many rounds): the update files in the order they
    // were applied on chain. Without it: `updates_by_range` items, then finality updates by attested slot.
    let names: Vec<String> = match manifest.get("apply_order") {
        Some(order) => order
            .as_array()
            .context("manifest.apply_order: a list")?
            .iter()
            .map(|n| {
                let n = n.as_str().context("manifest.apply_order: file names")?;
                ensure!(
                    files.contains_key(n),
                    "manifest.apply_order: {n} is not in manifest.files"
                );
                Ok(n.to_string())
            })
            .collect::<Result<_>>()?,
        None => pick("updates_")
            .into_iter()
            .chain(pick("finality_update_"))
            .map(|(k, _)| k)
            .collect(),
    };
    let mut updates = Vec::new();
    for name in names {
        let bytes = std::fs::read(dir.join(&name))?;
        let label = name.trim_end_matches(".ssz");
        if name.starts_with("updates_") {
            for (i, u) in split_updates_by_range(&bytes)?.into_iter().enumerate() {
                updates.push((format!("{label}[{i}]"), u));
            }
        } else {
            updates.push((label.to_string(), bytes));
        }
    }
    Ok(Data {
        config,
        bootstrap,
        updates,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Transactions

/// One row of `txs.tsv`. `derived_cycles`: the cycles the payer's balance change implies at the row's base fees.
fn tsv_row(s: &Sent) -> String {
    let before: u128 = s.payer_before.parse().unwrap_or(0);
    let after: u128 = s.payer_after.parse().unwrap_or(0);
    let derived = before
        .checked_sub(after)
        .and_then(|delta| delta.checked_sub(u128::from(s.bytes) * u128::from(s.base_fee_stor)))
        .map(|x| (x / u128::from(s.base_fee_exec.max(1))).to_string())
        .unwrap_or_else(|| "n/a".into());
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        s.name,
        s.hash,
        s.block,
        s.bytes,
        s.gas_limit,
        s.simulated_cycles,
        s.expected_exit.map_or_else(|| "-".to_string(), |c| c.to_string()),
        s.base_fee_exec,
        s.base_fee_stor,
        s.payer_before,
        s.payer_after,
        derived,
        s.sent_utc
    )
}

struct Fee {
    gas_limit: u64,
    max_fee: u128,
}

fn signed(
    program: AccountId,
    selectors: Vec<ProgramShardSelector>,
    instruction: Vec<u8>,
    signers: &[(&PrivateKey, Nonce)],
    payer: AccountId,
    fee: &Fee,
) -> PublicTransaction {
    let message = Message::new_preserialized(
        program,
        selectors,
        signers.iter().map(|(_, n)| *n).collect(),
        instruction,
        Some(FeeDeclaration::new(payer, fee.gas_limit, 0, fee.max_fee)),
    );
    let keys: Vec<&PrivateKey> = signers.iter().map(|(k, _)| *k).collect();
    let witness = WitnessSet::for_message(&message, &keys);
    PublicTransaction::new(message, witness)
}

fn wire_len(tx: &PublicTransaction) -> u64 {
    u64::try_from(
        borsh::to_vec(&LeeTransaction::Public(tx.clone()))
            .map(|v| v.len())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

/// The reserve the sequencer holds for `tx` at fee state `(exec, stor)`.
fn reserve(gas_limit: u64, bytes: u64, exec: u64, stor: u64) -> u128 {
    u128::from(gas_limit) * u128::from(exec) + u128::from(bytes) * u128::from(stor)
}

/// Runs `tx` in `state` as the sequencer's action phase does. `Ok(cycles)` if it applies (and applies it to
/// `state`); `Err((cycles, exit code, error))` if it is refused.
fn run(
    state: &mut V03State,
    tx: &PublicTransaction,
    block: u64,
    ts: u64,
    budget: u64,
) -> std::result::Result<u64, (u64, Option<u32>, String)> {
    match ValidatedStateDiff::from_public_transaction_with_cycle_budget(tx, state, block, ts, budget) {
        Ok((diff, charge)) => {
            drop(state.apply_state_diff(diff));
            Ok(charge.cycles)
        }
        Err(e) => {
            let (charge, _) = ValidatedStateDiff::from_public_transaction_metered(tx, state, block, ts, budget);
            let code = match &e {
                lee::error::LeeError::ProgramExitedWithCode { code, .. } => Some(*code),
                _ => None,
            };
            Err((charge.cycles, code, e.to_string()))
        }
    }
}

/// In-process run of a sequence of transactions at the head's fees (`attest --plan`), as `plan`'s steps.
struct Sim {
    fee: rpc::FeeState,
    block: u64,
    rows: Vec<serde_json::Value>,
    total_fee: u128,
    /// The largest (fees before a transaction + its reserve).
    need: u128,
}

impl Sim {
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        state: &mut V03State,
        name: &str,
        program: AccountId,
        sel: Vec<ProgramShardSelector>,
        ix: Vec<u8>,
        extra: &[&PrivateKey],
        payer: &PrivateKey,
    ) -> Result<()> {
        self.block += 1;
        let payer_id = id_of(payer);
        let ts = now_ms();
        let nonce = |s: &V03State, id| s.get_account_by_id(id).nonce;
        let mk = |s: &V03State, gas: u64| {
            let mut signers: Vec<(&PrivateKey, Nonce)> = extra.iter().map(|k| (*k, nonce(s, id_of(k)))).collect();
            signers.push((payer, nonce(s, payer_id)));
            signed(
                program,
                sel.clone(),
                ix.clone(),
                &signers,
                payer_id,
                &Fee {
                    gas_limit: gas,
                    max_fee: reserve(gas, 200_000, 64, 64),
                },
            )
        };
        let probe = mk(state, STEP_LIMIT);
        let cycles =
            run(&mut state.clone(), &probe, self.block, ts, STEP_LIMIT).map_err(|e| anyhow!("{name}: {e:?}"))?;
        let gas = if program == PROGRAM_LOADER_ACCOUNT_ID {
            LOADER_GAS
        } else {
            (cycles + cycles.div_ceil(100) * 3).min(STEP_LIMIT)
        };
        let tx = mk(state, gas);
        let bytes = wire_len(&tx);
        run(state, &tx, self.block, ts, gas).map_err(|e| anyhow!("{name}: {e:?}"))?;
        let fee_paid = u128::from(cycles.min(gas)) * u128::from(self.fee.base_fee_exec)
            + u128::from(bytes) * u128::from(self.fee.base_fee_stor);
        let res = reserve(gas, bytes, self.fee.base_fee_exec, self.fee.base_fee_stor);
        self.need = self.need.max(self.total_fee + res);
        self.total_fee += fee_paid;
        println!("{name:<44} {bytes:>7} bytes  gas limit {gas:>9}  simulated {cycles:>9}  fee {fee_paid:>11}");
        self.rows.push(
            json!({"tx": name, "bytes": bytes, "gas_limit": gas, "simulated_cycles": cycles,
            "fee_at_head": fee_paid.to_string(), "reserve_at_head": res.to_string()}),
        );
        Ok(())
    }
}

struct Client {
    rpc: Rpc,
    home: Home,
    payer: PrivateKey,
    payer_id: AccountId,
    guests: PathBuf,
    max_base_fee: u64,
    /// Program accounts (headers and segments) read once: they never change.
    program_accounts: BTreeMap<AccountId, Account>,
    rpc_url: String,
}

impl Client {
    fn ids(&self) -> Result<Ids> {
        let d: Deployment = self.home.load("deploy.json")?;
        let id = |n: &str| -> Result<AccountId> {
            d.programs
                .get(n)
                .with_context(|| format!("{n} is not deployed (run deploy)"))?
                .header
                .parse()
                .map_err(|e| anyhow!("{e:?}"))
        };
        Ok(Ids {
            attest: id("attest_v03")?,
            v3: id("lc_verify_v3_v03")?,
        })
    }

    /// A state seeded from the chain: the payer, `ids` as read now, and both programs' accounts.
    fn seeded(&mut self, ids: &[AccountId]) -> Result<V03State> {
        let mut accounts: Vec<(AccountId, Account)> = Vec::new();
        for id in ids.iter().chain(std::iter::once(&self.payer_id)) {
            accounts.push((*id, self.rpc.account(id)?));
        }
        let d: Deployment = self.home.load("deploy.json")?;
        for p in d.programs.values() {
            for a in std::iter::once(&p.header).chain(p.segments.iter()) {
                let id: AccountId = a.parse().map_err(|e| anyhow!("{e:?}"))?;
                if !self.program_accounts.contains_key(&id) {
                    let acc = self.rpc.account(&id)?;
                    if !acc.data.shard(PROGRAM_LOADER_ACCOUNT_ID).is_empty() {
                        self.program_accounts.insert(id, acc);
                    }
                }
            }
        }
        accounts.extend(self.program_accounts.iter().map(|(k, v)| (*k, v.clone())));
        Ok(V03State::new().with_public_accounts(accounts))
    }

    /// Waits until the head's base fee is at most `max_base_fee`; returns the fee state.
    fn fee_ok(&self) -> Result<rpc::FeeState> {
        loop {
            let f = self.rpc.fee_state()?;
            if f.base_fee_exec <= self.max_base_fee {
                return Ok(f);
            }
            eprintln!("base_fee_exec {} > {}: waiting", f.base_fee_exec, self.max_base_fee);
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    /// Simulates, sizes, signs, simulates again at the final limit, sends, waits for the block; records.
    /// `extra_signers` sign before the payer (deploy headers). `expect_exit`: the refusal demo.
    fn send(
        &mut self,
        name: &str,
        program: AccountId,
        selectors: Vec<ProgramShardSelector>,
        instruction: Vec<u8>,
        extra_signers: &[&PrivateKey],
        expect_exit: Option<u32>,
    ) -> Result<Sent> {
        let fee_state = self.fee_ok()?;
        let mut ids: Vec<AccountId> = selectors.iter().map(|s| s.account_id).collect();
        ids.extend(extra_signers.iter().map(|k| id_of(k)));
        let state0 = self.seeded(&ids)?;
        let nonce_of = |s: &V03State, id: AccountId| s.get_account_by_id(id).nonce;
        let payer_before = bal(&state0.get_account_by_id(self.payer_id));
        let head = self.rpc.last_block_id()?;
        let ts = now_ms();
        let is_loader = program == PROGRAM_LOADER_ACCOUNT_ID;
        let build = |s: &V03State, fee: &Fee| {
            let mut signers: Vec<(&PrivateKey, Nonce)> =
                extra_signers.iter().map(|k| (*k, nonce_of(s, id_of(k)))).collect();
            signers.push((&self.payer, nonce_of(s, self.payer_id)));
            signed(
                program,
                selectors.clone(),
                instruction.clone(),
                &signers,
                self.payer_id,
                fee,
            )
        };
        let max_fee = |gas: u64, bytes: u64| {
            reserve(
                gas,
                bytes,
                fee_state.next_base_fee_exec_ceiling + 1,
                fee_state.next_base_fee_stor_ceiling + 1,
            )
        };
        // First pass at the step limit, for the cycles.
        let probe = build(
            &state0,
            &Fee {
                gas_limit: STEP_LIMIT,
                max_fee: max_fee(STEP_LIMIT, 200_000),
            },
        );
        let simulated = match (run(&mut state0.clone(), &probe, head + 1, ts, STEP_LIMIT), expect_exit) {
            (Ok(c), None) => c,
            (Err((c, Some(code), _)), Some(want)) if code == want => c,
            (Ok(_), Some(want)) => bail!("{name}: expected a refusal with exit {want}, but it applies"),
            (Err((c, code, e)), _) => bail!("{name}: simulation refused ({c} cycles, exit {code:?}): {e}; not sent"),
        };
        let gas_limit = if is_loader {
            LOADER_GAS
        } else {
            (simulated + simulated.div_ceil(100) * 3).min(STEP_LIMIT)
        };
        ensure!(
            is_loader || simulated <= STEP_LIMIT,
            "{name}: {simulated} cycles above the step limit"
        );
        let mut tx = build(
            &state0,
            &Fee {
                gas_limit,
                max_fee: max_fee(gas_limit, 200_000),
            },
        );
        let bytes = wire_len(&tx);
        tx = build(
            &state0,
            &Fee {
                gas_limit,
                max_fee: max_fee(gas_limit, bytes),
            },
        );
        ensure!(wire_len(&tx) == bytes, "transaction size moved with the fee");
        let need = reserve(gas_limit, bytes, fee_state.base_fee_exec, fee_state.base_fee_stor);
        ensure!(
            payer_before >= need,
            "{name}: payer holds {payer_before}, the reserve is {need}"
        );
        let check = run(&mut state0.clone(), &tx, head + 1, ts, gas_limit);
        match (&check, expect_exit) {
            (Ok(_), None) => {}
            (Err((_, Some(code), _)), Some(want)) if *code == want => {}
            (r, _) => bail!("{name}: the final transaction does not simulate as expected: {r:?}; not sent"),
        }
        let lee_tx = LeeTransaction::Public(tx.clone());
        let hash = lee_tx.hash();
        let raw_b64 = B64.encode(borsh::to_vec(&lee_tx)?);
        eprintln!(
            "{name}: sending {} ({bytes} bytes, gas limit {gas_limit}, simulated {simulated} cycles, reserve {need})",
            hex::encode(hash)
        );
        let sent_utc = utc();
        let answer = self.rpc.send(&lee_tx)?;
        ensure!(
            answer == hex::encode(hash),
            "{name}: sequencer hash {answer} differs from ours"
        );
        // Wait for the block (up to 30 min: the testnet stalls at times).
        let deadline = std::time::Instant::now() + Duration::from_secs(1800);
        let block = loop {
            if let Some((_, b)) = self.rpc.transaction(&hash)? {
                break b;
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "{name}: not in a block after 30 min (hash {})",
                hex::encode(hash)
            );
            std::thread::sleep(Duration::from_secs(5));
        };
        // One transaction per block: wait for the next block before going on.
        while self.rpc.last_block_id()? <= block {
            std::thread::sleep(Duration::from_secs(5));
        }
        let payer_after = self.rpc.balance(&self.payer_id)?;
        let s = Sent {
            name: name.to_string(),
            hash: hex::encode(hash),
            block,
            bytes,
            gas_limit,
            simulated_cycles: simulated,
            expected_exit: expect_exit,
            base_fee_exec: fee_state.base_fee_exec,
            base_fee_stor: fee_state.base_fee_stor,
            payer_before: payer_before.to_string(),
            payer_after: payer_after.to_string(),
            sent_utc,
            raw_b64,
        };
        let mut p: Progress = self.home.load("progress.json")?;
        p.sent.push(s.clone());
        self.home.save("progress.json", &p)?;
        eprintln!("{name}: in block {block}; payer {payer_before} -> {payer_after}");
        Ok(s)
    }

    fn guest(&self, name: &str) -> Result<Vec<u8>> {
        std::fs::read(self.guests.join(format!("{name}.bin"))).with_context(|| format!("{name}.bin"))
    }

    // -----------------------------------------------------------------------------------------------------------

    fn deploy(&mut self, programs: &[String]) -> Result<()> {
        let mut d: Deployment = self.home.load("deploy.json")?;
        let default = ["attest_v03".to_string(), "lc_verify_v3_v03".to_string()];
        for name in if programs.is_empty() { &default[..] } else { programs } {
            let name = name.as_str();
            if d.programs
                .get(name)
                .is_some_and(|p| !p.txs.is_empty() && p.txs.len() == p.segments.len() + 1)
            {
                eprintln!("{name}: deployed at {}", d.programs[name].header);
                continue;
            }
            ensure!(
                !d.programs.contains_key(name),
                "{name}: a partial deployment is in deploy.json; resume is not supported, remove it to start again"
            );
            let bin = self.guest(name)?;
            let binary = risc0_binfmt::ProgramBinary::decode(&bin).map_err(|e| anyhow!("{e}"))?;
            ensure!(
                binary.kernel_elf == risc0_zkos_v1compat::V1COMPAT_ELF,
                "{name}: not the v1compat kernel"
            );
            let image_id = risc0_binfmt::compute_image_id(&bin).map_err(|e| anyhow!("{e}"))?;
            let chunks: Vec<Vec<u8>> = binary
                .user_elf
                .chunks(MAX_SEGMENT_DATA_LEN)
                .map(<[u8]>::to_vec)
                .collect();
            // Segment accounts are written without authorization: fresh random ids. The header signs.
            let segs: Vec<AccountId> = chunks.iter().map(|_| id_of(&PrivateKey::new_os_random())).collect();
            let header_key = PrivateKey::new_os_random();
            let header = id_of(&header_key);
            let mut entry = Deployed {
                header: header.to_string(),
                header_key: hex::encode(header_key.value()),
                segments: segs.iter().map(ToString::to_string).collect(),
                image_id: image_id.to_string(),
                elf_sha256: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bin)),
                txs: Vec::new(),
            };
            d.programs.insert(name.to_string(), entry.clone());
            self.home.save("deploy.json", &d)?;
            for i in (0..chunks.len()).rev() {
                let next = segs.get(i + 1).copied();
                let mut sel = vec![ProgramShardSelector::new(segs[i], PROGRAM_LOADER_ACCOUNT_ID)];
                sel.extend(next.map(|n| ProgramShardSelector::new(n, PROGRAM_LOADER_ACCOUNT_ID)));
                let ix = borsh::to_vec(&program_loader_core::Instruction::WriteSegment {
                    bytecode: chunks[i].clone(),
                    next_segment: next,
                })?;
                let s = self.send(
                    &format!("deploy {name} segment {i}"),
                    PROGRAM_LOADER_ACCOUNT_ID,
                    sel,
                    ix,
                    &[],
                    None,
                )?;
                entry.txs.push(s.hash);
                d.programs.insert(name.to_string(), entry.clone());
                self.home.save("deploy.json", &d)?;
            }
            let mut sel = vec![ProgramShardSelector::new(header, PROGRAM_LOADER_ACCOUNT_ID)];
            sel.extend(
                segs.iter()
                    .map(|s| ProgramShardSelector::new(*s, PROGRAM_LOADER_ACCOUNT_ID)),
            );
            let ix = borsh::to_vec(&program_loader_core::Instruction::CreateHeader {
                first_segment: segs[0],
                immutable: true,
            })?;
            let s = self.send(
                &format!("deploy {name} header"),
                PROGRAM_LOADER_ACCOUNT_ID,
                sel,
                ix,
                &[&header_key],
                None,
            )?;
            entry.txs.push(s.hash);
            d.programs.insert(name.to_string(), entry.clone());
            self.home.save("deploy.json", &d)?;
            // The header must now hold the image id, immutable.
            let acc = self.rpc.account(&header)?;
            let h = ProgramHeader::from_bytes(acc.data.shard(PROGRAM_LOADER_ACCOUNT_ID)).context("no header")?;
            ensure!(h.immutable, "{name}: header not immutable");
            ensure!(
                h.image_id == <[u32; 8]>::from(image_id),
                "{name}: header image id differs"
            );
            eprintln!("{name}: deployed at {header}, image id {image_id}");
        }
        Ok(())
    }

    fn instance_state(&self, ids: &Ids, config: &InstanceConfig) -> Result<Vec<u8>> {
        let inst = steps::instance(ids, config);
        Ok(self.rpc.account(&inst)?.data.shard(ids.attest).clone().into_inner())
    }

    fn init(&mut self, data: &Path) -> Result<()> {
        let ids = self.ids()?;
        let d = load_data(data, &ids)?;
        if !self.instance_state(&ids, &d.config)?.is_empty() {
            eprintln!("instance exists");
            return Ok(());
        }
        let (tx, _, _) = steps::init(&ids, &d.config, &d.bootstrap)?;
        self.send("Init", tx.program, tx.selectors, tx.instruction, &[], None)?;
        Ok(())
    }

    /// Walks the data's updates from the bootstrap: `(witness before, label, update, applied on chain?)`.
    fn walk(&self, ids: &Ids, d: &Data) -> Result<Vec<(lc::StoreWitness, String, Vec<u8>, bool)>> {
        let chain_commit = borsh::from_slice::<InstanceState>(&self.instance_state(ids, &d.config)?)
            .context("no instance (run init)")?
            .head
            .store_commitment;
        let (mut w, head) = lc::init_store(&d.config, &d.bootstrap).map_err(|e| anyhow!("init store: {e}"))?;
        let mut known = head.store_commitment;
        let mut reached = known == chain_commit;
        let mut out = Vec::new();
        for (label, u) in &d.updates {
            let Ok(native) = lc::verify_and_apply(&d.config, &w, u) else {
                eprintln!("{label}: does not apply to the store (not relevant or refused natively); skipped");
                continue;
            };
            ensure!(native.pre_store == known, "{label}: pre-store does not follow");
            out.push((w.clone(), label.clone(), u.clone(), !reached));
            known = native.post_store;
            if known == chain_commit {
                reached = true;
            }
            w = native.new_witness();
        }
        ensure!(
            reached,
            "the chain's instance commitment is not reached by the data folder's updates"
        );
        Ok(out)
    }

    fn update(&mut self, data: &Path, max: Option<usize>) -> Result<()> {
        let ids = self.ids()?;
        let d = load_data(data, &ids)?;
        let mut applied = 0;
        for (w, label, u, done) in self.walk(&ids, &d)? {
            if done {
                eprintln!("{label}: applied already");
                continue;
            }
            if max.is_some_and(|m| applied >= m) {
                break;
            }
            let inst_state = self.instance_state(&ids, &d.config)?;
            let up = steps::update(&ids, &d.config, &w, &inst_state, &u)?;
            // Resume from the job's state on chain: k steps are done when it equals the state after step k.
            let job_now = self.rpc.account(&up.job)?.data.shard(ids.v3).clone().into_inner();
            let done_steps = if job_now.is_empty() {
                0
            } else {
                1 + up
                    .job_states
                    .iter()
                    .position(|s| *s == job_now)
                    .with_context(|| format!("{label}: the job on chain is in no expected state"))?
            };
            for tx in up.txs.iter().skip(done_steps) {
                self.send_step(&format!("{label} {}", tx.name), tx)?;
            }
            let after = self.instance_state(&ids, &d.config)?;
            ensure!(
                after == up.instance_state,
                "{label}: instance state after Finish differs from the native one"
            );
            eprintln!("{label}: applied; instance head slot {}", up.native.head.slot);
            applied += 1;
        }
        Ok(())
    }

    fn send_step(&mut self, name: &str, tx: &Tx) -> Result<Sent> {
        if let Some((from, to)) = tx.window_ms {
            let now = now_ms();
            ensure!(
                now >= from && now + 120_000 < to,
                "{name}: now ({now}) is outside the window [{from}, {to})"
            );
        }
        self.send(
            name,
            tx.program,
            tx.selectors.clone(),
            tx.instruction.clone(),
            &[],
            None,
        )
    }

    fn refuse_demo(&mut self, data: &Path) -> Result<()> {
        let ids = self.ids()?;
        let p: Progress = self.home.load("progress.json")?;
        let last = p
            .sent
            .iter()
            .rev()
            .find(|s| s.name.ends_with(" Finish") && s.expected_exit.is_none())
            .context("no Finish sent yet")?;
        let raw = B64.decode(&last.raw_b64)?;
        let LeeTransaction::Public(old) = borsh::from_slice::<LeeTransaction>(&raw)? else {
            bail!("not a public transaction")
        };
        let d = load_data(data, &ids)?;
        let before = self.instance_state(&ids, &d.config)?;
        let m = old.message();
        let name = format!("{} again (refusal demo)", last.name);
        // A finished job refuses every step with E14.
        self.send(
            &name,
            m.program_account_id,
            m.shard_selectors.clone(),
            m.instruction_data.clone(),
            &[],
            Some(14),
        )?;
        ensure!(
            self.instance_state(&ids, &d.config)? == before,
            "the refused transaction changed the instance"
        );
        eprintln!("refused as expected; the instance is unchanged");
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------------

    fn plan(&mut self, data: &Path, json_out: Option<&Path>) -> Result<()> {
        let fee = self.rpc.fee_state()?;
        let payer_acc = self.rpc.account(&self.payer_id)?;
        let mut state = V03State::new().with_public_accounts([(self.payer_id, payer_acc.clone())]);
        let head = self.rpc.last_block_id()?;
        let mut rows = Vec::new();
        let mut total_fee: u128 = 0;
        let mut max_reserve: u128 = 0;
        let mut block = head;
        let mut sim = |state: &mut V03State,
                       name: String,
                       program: AccountId,
                       sel: Vec<ProgramShardSelector>,
                       ix: Vec<u8>,
                       extra: &[&PrivateKey],
                       ts: u64,
                       payer: &PrivateKey,
                       payer_id: AccountId|
         -> Result<()> {
            block += 1;
            let nonce = |s: &V03State, id| s.get_account_by_id(id).nonce;
            let mk = |s: &V03State, gas: u64| {
                let mut signers: Vec<(&PrivateKey, Nonce)> = extra.iter().map(|k| (*k, nonce(s, id_of(k)))).collect();
                signers.push((payer, nonce(s, payer_id)));
                signed(
                    program,
                    sel.clone(),
                    ix.clone(),
                    &signers,
                    payer_id,
                    &Fee {
                        gas_limit: gas,
                        max_fee: reserve(gas, 200_000, 64, 64),
                    },
                )
            };
            let probe = mk(state, STEP_LIMIT);
            let cycles =
                run(&mut state.clone(), &probe, block, ts, STEP_LIMIT).map_err(|e| anyhow!("{name}: {e:?}"))?;
            let gas = if program == PROGRAM_LOADER_ACCOUNT_ID {
                LOADER_GAS
            } else {
                (cycles + cycles.div_ceil(100) * 3).min(STEP_LIMIT)
            };
            let tx = mk(state, gas);
            let bytes = wire_len(&tx);
            // Nonces advance as the sequencer would (the payer's).
            run(state, &tx, block, ts, gas).map_err(|e| anyhow!("{name}: {e:?}"))?;
            let fee_paid = u128::from(cycles.min(gas)) * u128::from(fee.base_fee_exec)
                + u128::from(bytes) * u128::from(fee.base_fee_stor);
            let res = reserve(gas, bytes, fee.base_fee_exec, fee.base_fee_stor);
            total_fee += fee_paid;
            max_reserve = max_reserve.max(res);
            println!("{name:<44} {bytes:>7} bytes  gas limit {gas:>9}  simulated {cycles:>9}  fee {fee_paid:>11}");
            rows.push(
                json!({"tx": name, "bytes": bytes, "gas_limit": gas, "simulated_cycles": cycles,
                "fee_at_head": fee_paid.to_string(), "reserve_at_head": res.to_string()}),
            );
            Ok(())
        };
        // Deployment into the in-process state, with throwaway ids.
        let mut ids_map = BTreeMap::new();
        for name in ["attest_v03", "lc_verify_v3_v03"] {
            let bin = self.guest(name)?;
            let binary = risc0_binfmt::ProgramBinary::decode(&bin).map_err(|e| anyhow!("{e}"))?;
            let chunks: Vec<Vec<u8>> = binary
                .user_elf
                .chunks(MAX_SEGMENT_DATA_LEN)
                .map(<[u8]>::to_vec)
                .collect();
            let segs: Vec<AccountId> = chunks.iter().map(|_| id_of(&PrivateKey::new_os_random())).collect();
            let hk = PrivateKey::new_os_random();
            for i in (0..chunks.len()).rev() {
                let next = segs.get(i + 1).copied();
                let mut sel = vec![ProgramShardSelector::new(segs[i], PROGRAM_LOADER_ACCOUNT_ID)];
                sel.extend(next.map(|n| ProgramShardSelector::new(n, PROGRAM_LOADER_ACCOUNT_ID)));
                let ix = borsh::to_vec(&program_loader_core::Instruction::WriteSegment {
                    bytecode: chunks[i].clone(),
                    next_segment: next,
                })?;
                sim(
                    &mut state,
                    format!("deploy {name} segment {i}"),
                    PROGRAM_LOADER_ACCOUNT_ID,
                    sel,
                    ix,
                    &[],
                    now_ms(),
                    &self.payer,
                    self.payer_id,
                )?;
            }
            let mut sel = vec![ProgramShardSelector::new(id_of(&hk), PROGRAM_LOADER_ACCOUNT_ID)];
            sel.extend(
                segs.iter()
                    .map(|s| ProgramShardSelector::new(*s, PROGRAM_LOADER_ACCOUNT_ID)),
            );
            let ix = borsh::to_vec(&program_loader_core::Instruction::CreateHeader {
                first_segment: segs[0],
                immutable: true,
            })?;
            sim(
                &mut state,
                format!("deploy {name} header"),
                PROGRAM_LOADER_ACCOUNT_ID,
                sel,
                ix,
                &[&hk],
                now_ms(),
                &self.payer,
                self.payer_id,
            )?;
            ids_map.insert(name, id_of(&hk));
        }
        let ids = Ids {
            attest: ids_map["attest_v03"],
            v3: ids_map["lc_verify_v3_v03"],
        };
        let d = load_data(data, &ids)?;
        let (init, mut inst_state, _) = steps::init(&ids, &d.config, &d.bootstrap)?;
        sim(
            &mut state,
            "Init".into(),
            init.program,
            init.selectors,
            init.instruction,
            &[],
            now_ms(),
            &self.payer,
            self.payer_id,
        )?;
        let (mut w, _) = lc::init_store(&d.config, &d.bootstrap).map_err(|e| anyhow!("{e}"))?;
        let mut last_finish = None;
        for (label, u) in &d.updates {
            let Ok(up) = steps::update(&ids, &d.config, &w, &inst_state, u) else {
                println!("{label}: does not apply natively; skipped");
                continue;
            };
            for tx in &up.txs {
                let ts = tx.window_ms.map_or_else(now_ms, |(from, _)| from.max(now_ms()));
                sim(
                    &mut state,
                    format!("{label} {}", tx.name),
                    tx.program,
                    tx.selectors.clone(),
                    tx.instruction.clone(),
                    &[],
                    ts,
                    &self.payer,
                    self.payer_id,
                )?;
            }
            last_finish = up.txs.last().cloned();
            inst_state = up.instance_state;
            w = up.native.new_witness();
        }
        drop(sim);
        if let Some(f) = last_finish {
            // The refusal demo: charged its cycles.
            let probe = signed(
                f.program,
                f.selectors.clone(),
                f.instruction.clone(),
                &[(&self.payer, state.get_account_by_id(self.payer_id).nonce)],
                self.payer_id,
                &Fee {
                    gas_limit: STEP_LIMIT,
                    max_fee: reserve(STEP_LIMIT, 200_000, 64, 64),
                },
            );
            match run(&mut state.clone(), &probe, block + 1, now_ms(), STEP_LIMIT) {
                Err((c, Some(code), _)) => {
                    let fee_paid = u128::from(c) * u128::from(fee.base_fee_exec)
                        + u128::from(wire_len(&probe)) * u128::from(fee.base_fee_stor);
                    total_fee += fee_paid;
                    println!(
                        "{:<44} refused, exit {code}, {c} cycles, fee {fee_paid}",
                        "Finish again (refusal demo)"
                    );
                    rows.push(json!({"tx": "Finish again (refusal demo)", "exit": code, "simulated_cycles": c, "fee_at_head": fee_paid.to_string()}));
                }
                other => bail!("refusal demo: expected an exit code, got {other:?}"),
            }
        }
        let balance = bal(&payer_acc);
        println!(
            "\nfee state at block {}: base fees {} / {}; {} transactions; fees at head {total_fee}; largest reserve {max_reserve}; payer {} holds {balance}; funds needed at least {}",
            fee.height,
            fee.base_fee_exec,
            fee.base_fee_stor,
            rows.len(),
            self.payer_id,
            total_fee + max_reserve
        );
        if let Some(p) = json_out {
            std::fs::write(
                p,
                serde_json::to_string_pretty(&json!({
                    "fee_state": fee, "payer": self.payer_id.to_string(), "payer_balance": balance.to_string(),
                    "transactions": rows, "fees_at_head": total_fee.to_string(), "largest_reserve": max_reserve.to_string(),
                    "funds_needed": (total_fee + max_reserve).to_string(), "head_block": head, "channel_id": self.rpc.channel_id()?,
                }))?,
            )?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------------

    fn record(&mut self, data: &Path, out: &Path) -> Result<()> {
        let ids = self.ids()?;
        let d = load_data(data, &ids)?;
        let dep: Deployment = self.home.load("deploy.json")?;
        let p: Progress = self.home.load("progress.json")?;
        std::fs::create_dir_all(out.join("raw"))?;
        // deployment.json: ids, image ids, ELF hashes, segment ids, transactions. No key.
        let programs: BTreeMap<_, _> = dep
            .programs
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    json!({"header": v.header, "image_id": v.image_id, "bin_sha256": v.elf_sha256,
                    "segments": v.segments, "txs": v.txs, "immutable": true}),
                )
            })
            .collect();
        std::fs::write(
            out.join("deployment.json"),
            serde_json::to_string_pretty(
                &json!({"rpc": self.rpc_url(), "lez_release": "v0.3.2", "programs": programs}),
            )?,
        )?;
        // txs.tsv and raw bytes.
        let mut tsv = String::from(
            "step\thash\tblock\tbytes\tgas_limit\tsimulated_cycles\texpected_exit\tbase_fee_exec\tbase_fee_stor\tpayer_before\tpayer_after\tderived_cycles\tsent_utc\n",
        );
        let mut blocks = std::collections::BTreeSet::new();
        for s in &p.sent {
            std::fs::write(out.join(format!("raw/{}.tx.b64", s.hash)), &s.raw_b64)?;
            blocks.insert(s.block);
            tsv.push_str(&tsv_row(s));
        }
        std::fs::write(out.join("txs.tsv"), tsv)?;
        for b in blocks {
            if let Some((_, raw)) = self.rpc.block(b)? {
                std::fs::write(out.join(format!("raw/{b}.block.b64")), raw)?;
            }
        }
        // run.json: the instance and the records, as the chain holds them now.
        let inst = steps::instance(&ids, &d.config);
        let inst_state = self.instance_state(&ids, &d.config)?;
        let st: InstanceState = borsh::from_slice(&inst_state).context("instance")?;
        let mut records = Vec::new();
        let mut slots = vec![d.config.checkpoint_slot];
        let (mut w, _) = lc::init_store(&d.config, &d.bootstrap).map_err(|e| anyhow!("{e}"))?;
        for (_, u) in &d.updates {
            if let Ok(n) = lc::verify_and_apply(&d.config, &w, u) {
                if n.new_slot && n.head.slot <= st.head.slot {
                    slots.push(n.head.slot);
                }
                if n.head.slot <= st.head.slot {
                    w = n.new_witness();
                }
            }
        }
        for slot in slots {
            let rid = steps::record(&ids, &inst, slot);
            let r = self.rpc.account(&rid)?.data.shard(ids.attest).clone().into_inner();
            if !r.is_empty() {
                records.push(json!({"slot": slot, "account": rid.to_string(), "bytes_hex": hex::encode(&r)}));
            }
        }
        std::fs::write(
            out.join("run.json"),
            serde_json::to_string_pretty(&json!({
                "network": "LEZ testnet 0.3 (v0.3.2)", "rpc": self.rpc_url(), "chain_id": d.config.chain_id,
                "attest_v03": ids.attest.to_string(), "lc_verify_v3_v03": ids.v3.to_string(),
                "instance": inst.to_string(), "instance_state_hex": hex::encode(&inst_state),
                "checkpoint_slot": d.config.checkpoint_slot, "checkpoint_root": hex::encode(d.config.checkpoint_root),
                "head_slot": st.head.slot, "head_beacon_root": hex::encode(st.head.beacon_root),
                "store_commitment": hex::encode(st.head.store_commitment), "records": records,
                "payer": self.payer_id.to_string(), "recorded_utc": utc(),
            }))?,
        )?;
        eprintln!("recorded {} transactions into {}", p.sent.len(), out.display());
        Ok(())
    }

    /// The id of a deployed program (its program-loader header), from `deploy.json`.
    fn program_id(&self, name: &str) -> Result<AccountId> {
        let d: Deployment = self.home.load("deploy.json")?;
        d.programs
            .get(name)
            .filter(|p| !p.txs.is_empty() && p.txs.len() == p.segments.len() + 1)
            .with_context(|| format!("{name} is not deployed (run deploy --program {name})"))?
            .header
            .parse()
            .map_err(|e| anyhow!("{e:?}"))
    }

    /// The `attest_v03` instance of `data`'s config, the record account of `slot` and the record bytes the chain
    /// holds there (empty if none).
    fn chain_record(&self, ids: &Ids, data: &Path, slot: u64) -> Result<([u8; 32], AccountId, Vec<u8>)> {
        ensure!(
            ids.attest == AccountId::new(attest_fact_v03::ATTEST_V03),
            "deploy.json's attest_v03 ({}) is not the one attest_fact_v03 trusts",
            ids.attest
        );
        let d = load_data(data, ids)?;
        let inst = steps::instance(ids, &d.config);
        let rid = steps::record(ids, &inst, slot);
        let bytes = self.rpc.account(&rid)?.data.shard(ids.attest).clone().into_inner();
        Ok((*inst.value(), rid, bytes))
    }

    /// `attest --plan`: `attest_fact_v03` deployed through the program loader and the `Attest`, in a state seeded
    /// from the chain (payer, record, deployed programs), at the head's base fees. The funds needed are the largest
    /// (fees of the transactions before one + that one's reserve).
    fn attest_plan(&mut self, data: &Path, claim: &Path, json_out: Option<&Path>) -> Result<()> {
        let ids = self.ids()?;
        let fact = attest::load(claim)?;
        let (instance, rid, rec_bytes) = self.chain_record(&ids, data, fact.beacon_slot)?;
        ensure!(
            !rec_bytes.is_empty(),
            "no header record of slot {} on chain at {rid}",
            fact.beacon_slot
        );
        let record: attest_core::HeaderRecord = borsh::from_slice(&rec_bytes)?;
        let fee = self.rpc.fee_state()?;
        let mut state = self.seeded(&[rid])?;
        let head = self.rpc.last_block_id()?;
        let mut sim = Sim {
            fee,
            block: head,
            rows: Vec::new(),
            total_fee: 0,
            need: 0,
        };
        let bin = self.guest("attest_fact_v03")?;
        let image_id = risc0_binfmt::compute_image_id(&bin).map_err(|e| anyhow!("{e}"))?;
        let binary = risc0_binfmt::ProgramBinary::decode(&bin).map_err(|e| anyhow!("{e}"))?;
        let chunks: Vec<Vec<u8>> = binary
            .user_elf
            .chunks(MAX_SEGMENT_DATA_LEN)
            .map(<[u8]>::to_vec)
            .collect();
        let segs: Vec<AccountId> = chunks.iter().map(|_| id_of(&PrivateKey::new_os_random())).collect();
        let hk = PrivateKey::new_os_random();
        for i in (0..chunks.len()).rev() {
            let next = segs.get(i + 1).copied();
            let mut sel = vec![ProgramShardSelector::new(segs[i], PROGRAM_LOADER_ACCOUNT_ID)];
            sel.extend(next.map(|n| ProgramShardSelector::new(n, PROGRAM_LOADER_ACCOUNT_ID)));
            let ix = borsh::to_vec(&program_loader_core::Instruction::WriteSegment {
                bytecode: chunks[i].clone(),
                next_segment: next,
            })?;
            let name = format!("deploy attest_fact_v03 segment {i}");
            sim.step(&mut state, &name, PROGRAM_LOADER_ACCOUNT_ID, sel, ix, &[], &self.payer)?;
        }
        let mut sel = vec![ProgramShardSelector::new(id_of(&hk), PROGRAM_LOADER_ACCOUNT_ID)];
        sel.extend(
            segs.iter()
                .map(|s| ProgramShardSelector::new(*s, PROGRAM_LOADER_ACCOUNT_ID)),
        );
        let ix = borsh::to_vec(&program_loader_core::Instruction::CreateHeader {
            first_segment: segs[0],
            immutable: true,
        })?;
        sim.step(
            &mut state,
            "deploy attest_fact_v03 header",
            PROGRAM_LOADER_ACCOUNT_ID,
            sel,
            ix,
            &[&hk],
            &self.payer,
        )?;
        let program = id_of(&hk);
        let (tx, st, fact_acc) = attest::tx(program, &instance, &record, &fact)?;
        sim.step(
            &mut state,
            "Attest",
            tx.program,
            tx.selectors,
            tx.instruction,
            &[],
            &self.payer,
        )?;
        ensure!(
            state
                .get_account_by_id(fact_acc)
                .data
                .shard(program)
                .clone()
                .into_inner()
                == borsh::to_vec(&st)?,
            "Attest applied in process but did not write the statement"
        );
        let balance = self.rpc.balance(&self.payer_id)?;
        println!(
            "\nattest_fact_v03 image id {image_id}; record slot {} at {rid}; fact {}: slot {} of {} = {}\nfee state at block {}: base fees {} / {}; fees at head {}; payer {} holds {balance}; funds needed {}",
            record.slot,
            hex::encode(st.id),
            hex::encode(fact.slot_key),
            hex::encode(fact.address),
            hex::encode(fact.value),
            fee.height,
            fee.base_fee_exec,
            fee.base_fee_stor,
            sim.total_fee,
            self.payer_id,
            sim.need
        );
        if let Some(p) = json_out {
            std::fs::write(
                p,
                serde_json::to_string_pretty(&json!({
                    "fee_state": fee, "payer": self.payer_id.to_string(), "payer_balance": balance.to_string(),
                    "image_id": image_id.to_string(), "record_slot": record.slot, "record_account": rid.to_string(),
                    "transactions": sim.rows, "fees_at_head": sim.total_fee.to_string(),
                    "funds_needed": sim.need.to_string(), "head_block": head,
                }))?,
            )?;
        }
        Ok(())
    }

    /// `attest`: the claim folder's fact under the chain's record, sent through `send` (simulated first); then the
    /// fact account must hold the statement the native `verify_claim` computes.
    fn attest(&mut self, data: &Path, claim: &Path) -> Result<()> {
        let ids = self.ids()?;
        let program = self.program_id("attest_fact_v03")?;
        let fact = attest::load(claim)?;
        let (instance, rid, rec_bytes) = self.chain_record(&ids, data, fact.beacon_slot)?;
        ensure!(
            !rec_bytes.is_empty(),
            "no header record of slot {} on chain at {rid}",
            fact.beacon_slot
        );
        let record: attest_core::HeaderRecord = borsh::from_slice(&rec_bytes)?;
        let (tx, st, fact_acc) = attest::tx(program, &instance, &record, &fact)?;
        let want = borsh::to_vec(&st)?;
        let now = self.rpc.account(&fact_acc)?.data.shard(program).clone().into_inner();
        if !now.is_empty() {
            ensure!(now == want, "the fact account {fact_acc} holds another statement");
            eprintln!("attested already: fact {fact_acc}");
            return Ok(());
        }
        self.send("Attest", tx.program, tx.selectors, tx.instruction, &[], None)?;
        let after = self.rpc.account(&fact_acc)?.data.shard(program).clone().into_inner();
        ensure!(
            after == want,
            "the fact account {fact_acc} does not hold the statement after Attest"
        );
        eprintln!(
            "attested: statement {} at {fact_acc} (record slot {})",
            hex::encode(st.id),
            record.slot
        );
        Ok(())
    }

    /// `record-attest`: adds `attest_fact_v03` and the `Attest` to an evidence folder written by `record`.
    fn record_attest(&mut self, data: &Path, claim: &Path, out: &Path) -> Result<()> {
        let ids = self.ids()?;
        let program = self.program_id("attest_fact_v03")?;
        let dep: Deployment = self.home.load("deploy.json")?;
        let fp = &dep.programs["attest_fact_v03"];
        let fact = attest::load(claim)?;
        let (instance, rid, rec_bytes) = self.chain_record(&ids, data, fact.beacon_slot)?;
        let record: attest_core::HeaderRecord = borsh::from_slice(&rec_bytes).context("no record on chain")?;
        let (_, st, fact_acc) = attest::tx(program, &instance, &record, &fact)?;
        let stored = self.rpc.account(&fact_acc)?.data.shard(program).clone().into_inner();
        ensure!(
            stored == borsh::to_vec(&st)?,
            "the fact account {fact_acc} does not hold the statement"
        );
        let p: Progress = self.home.load("progress.json")?;
        let sent = p
            .sent
            .iter()
            .rev()
            .find(|s| s.name == "Attest")
            .context("no Attest in progress.json")?;
        // txs.tsv and raw bytes: the transactions sent since the folder was recorded.
        let tsv_path = out.join("txs.tsv");
        let mut tsv = std::fs::read_to_string(&tsv_path)?;
        let have: std::collections::BTreeSet<String> = tsv
            .lines()
            .filter_map(|l| l.split('\t').nth(1).map(str::to_string))
            .collect();
        let mut blocks = std::collections::BTreeSet::new();
        for s in p.sent.iter().filter(|s| !have.contains(&s.hash)) {
            std::fs::write(out.join(format!("raw/{}.tx.b64", s.hash)), &s.raw_b64)?;
            blocks.insert(s.block);
            tsv.push_str(&tsv_row(s));
        }
        std::fs::write(&tsv_path, tsv)?;
        for b in blocks {
            if let Some((_, raw)) = self.rpc.block(b)? {
                std::fs::write(out.join(format!("raw/{b}.block.b64")), raw)?;
            }
        }
        // deployment.json (written by `record`): the program, no key.
        let dpath = out.join("deployment.json");
        let mut dj: serde_json::Value = serde_json::from_slice(&std::fs::read(&dpath)?)?;
        dj["programs"]["attest_fact_v03"] = json!({"header": fp.header, "image_id": fp.image_id,
            "bin_sha256": fp.elf_sha256, "segments": fp.segments, "txs": fp.txs, "immutable": true});
        std::fs::write(&dpath, serde_json::to_string_pretty(&dj)?)?;
        let ipath = out.join("IMAGE_IDS");
        let mut ids_txt = std::fs::read_to_string(&ipath)?;
        if !ids_txt.lines().any(|l| l.starts_with("attest_fact_v03 ")) {
            ids_txt.push_str(&format!("attest_fact_v03 {}\n", fp.image_id));
            std::fs::write(&ipath, ids_txt)?;
        }
        // run.json: `attest`, appended as the last field (the file's other fields are kept byte for byte).
        let before: u128 = sent.payer_before.parse().unwrap_or(0);
        let after: u128 = sent.payer_after.parse().unwrap_or(0);
        let obj = json!({
            "what": "one Attest of attest_fact_v03: a Sepolia storage slot proven under a header record of attest_v03's instance, the statement written once at the fact account",
            "program": program.to_string(), "image_id": fp.image_id, "attest_v03": ids.attest.to_string(),
            "instance": AccountId::new(instance).to_string(),
            "record": {"slot": record.slot, "account": rid.to_string(), "bytes_hex": hex::encode(&rec_bytes),
                "exec_hash": hex::encode(record.exec_hash)},
            "claim": {"kind": "Storage", "chain_id": record.chain_id, "beacon_slot": fact.beacon_slot,
                "block_hash": hex::encode(fact.block_hash), "block_number": fact.block_number,
                "address": hex::encode(fact.address), "slot_key": hex::encode(fact.slot_key),
                "value": hex::encode(fact.value), "predicate": "Eq(Bytes32(value))", "witnesses": "attest/data"},
            "fact_account": fact_acc.to_string(), "statement_hex": hex::encode(&stored),
            "statement": {"id": hex::encode(st.id), "chain_id": st.chain_id, "slot": st.slot,
                "exec_number": st.exec_number, "subject": hex::encode(&st.subject), "value": hex::encode(&st.value),
                "predicate_hash": hex::encode(st.predicate_hash)},
            "tx": sent.hash, "block": sent.block, "bytes": sent.bytes, "gas_limit": sent.gas_limit,
            "simulated_cycles": sent.simulated_cycles,
            "fee": before.checked_sub(after).map_or_else(|| "n/a".into(), |f| f.to_string()),
            "deploy_txs": fp.txs, "recorded_utc": utc(),
        });
        let rpath = out.join("run.json");
        let text = std::fs::read_to_string(&rpath)?;
        ensure!(
            !text.contains("\n \"attest\": "),
            "run.json has an attest field already"
        );
        let body = text
            .trim_end()
            .strip_suffix('}')
            .context("run.json does not end with }")?;
        let mut buf = Vec::new();
        let mut ser =
            serde_json::Serializer::with_formatter(&mut buf, serde_json::ser::PrettyFormatter::with_indent(b" "));
        serde::Serialize::serialize(&obj, &mut ser)?;
        let field = String::from_utf8(buf)?.replace('\n', "\n ");
        std::fs::write(&rpath, format!("{},\n \"attest\": {field}\n}}\n", body.trim_end()))?;
        eprintln!(
            "recorded the Attest {} (block {}) into {}",
            sent.hash,
            sent.block,
            out.display()
        );
        Ok(())
    }

    fn rpc_url(&self) -> String {
        self.rpc_url.clone()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = Home { dir: cli.home.clone() };
    std::fs::create_dir_all(&home.dir)?;
    std::fs::set_permissions(&home.dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    if matches!(cli.cmd, Cmd::PayerNew) {
        ensure!(!home.path("payer.key").exists(), "payer.key exists");
        let k = PrivateKey::new_os_random();
        write_private(&home.path("payer.key"), hex::encode(k.value()).as_bytes())?;
        println!("{}", id_of(&k));
        return Ok(());
    }
    let payer = home.payer_key()?;
    let mut c = Client {
        rpc: Rpc::new(&cli.rpc)?,
        payer_id: id_of(&payer),
        payer,
        home,
        guests: cli.guests.clone(),
        max_base_fee: cli.max_base_fee,
        program_accounts: BTreeMap::new(),
        rpc_url: cli.rpc.clone(),
    };
    match &cli.cmd {
        Cmd::PayerNew => unreachable!(),
        Cmd::Payer => {
            let a = c.rpc.account(&c.payer_id)?;
            println!("{} nonce {} balance {}", c.payer_id, a.nonce.0, bal(&a));
        }
        Cmd::Plan { data, json } => c.plan(data, json.as_deref())?,
        Cmd::Deploy { programs } => c.deploy(programs)?,
        Cmd::Attest {
            data,
            claim,
            plan,
            json,
        } => {
            if *plan {
                c.attest_plan(data, claim, json.as_deref())?;
            } else {
                c.attest(data, claim)?;
            }
        }
        Cmd::RecordAttest { data, claim, out } => c.record_attest(data, claim, out)?,
        Cmd::Init { data } => c.init(data)?,
        Cmd::Update { data, max } => c.update(data, *max)?,
        Cmd::RefuseDemo { data } => c.refuse_demo(data)?,
        Cmd::Record { data, out } => c.record(data, out)?,
        Cmd::Transfer { to, amount } => {
            let to: AccountId = to
                .trim_start_matches("Public/")
                .parse()
                .map_err(|e| anyhow!("--to: {e:?}"))?;
            ensure!(to != c.payer_id, "--to is the home payer itself");
            let ix = borsh::to_vec(&lee_core::native_token::Instruction::Transfer { amount: *amount })?;
            let sel = vec![
                ProgramShardSelector::new(c.payer_id, lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID),
                ProgramShardSelector::new(to, lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID),
            ];
            let s = c.send(
                "transfer",
                lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID,
                sel,
                ix,
                &[],
                None,
            )?;
            println!("{} block {} to {} now {}", s.hash, s.block, to, c.rpc.balance(&to)?);
        }
    }
    eprintln!("{} RPC requests", c.rpc.calls.get());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn segment_layout_is_the_loaders() {
        // The loader's segment format, as written by `deploy`, reads back.
        let seg = lee_core::program::ProgramSegment {
            bytecode: vec![1, 2, 3],
            next_segment: None,
        };
        assert_eq!(
            lee_core::program::ProgramSegment::from_bytes(&seg.to_bytes()),
            Some(seg)
        );
    }
}
