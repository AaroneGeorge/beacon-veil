//! Host library (PLAN.md 2.3, owner W1; used by W2, W3, W4, W5, W6).
//!
//! Configuration comes from the environment, never from arguments (`scripts/env.sh` sets most of it):
//! - `LEE_WALLET_HOME_DIR`: wallet home; the sequencer address is read from its `wallet_config.json`.
//! - `LEE_WALLET_PASSWORD`: wallet password (empty on the dev wallets).
//! - `BV_STORE_DIR`: where the local light-client store files live (default `$LEE_WALLET_HOME_DIR/attest-stores`).
//! - `BV_IDS`: path of the deployment ids file (default `$BV_REPO/deploy/ids.toml`).
//! - `BV_PAYER`: public account (base58, key held by the wallet) that pays public transactions; default: the wallet
//!   label `bv-payer` that `scripts/deploy_local.sh` imports.
//! - `BV_SUBMITTER`: private account (base58, owned by the wallet, funded) that submits Path A updates and pays
//!   their in-proof fee; default: the wallet label `bv-submitter`.
//! - `BV_GUESTS`: directory with the programs' `.bin` files (needed to prove Path A); overrides the ids file's
//!   `[bins]` table (see [`guest_dir_for`]).
//! - `BV_GAS_LIMIT`: execution gas limit declared on public transactions (default [`MAX_GAS`], the rc3 cap).
//! - `BV_ALLOW_REVERT=1`: send a public transaction even when its simulation reverts (to show the sequencer
//!   rejecting it); default: refuse locally with the same `ESA:Exx` code.
//!
//! Inputs are canonical SSZ (`LightClientBootstrap`, `LightClientUpdate`, `LightClientFinalityUpdate`); the fork is
//! picked from the slot through `config.forks`.
//!
//! Every refusal (host-side or on chain) carries its [`EsaError`] as the root cause of the returned
//! `anyhow::Error` when the code is known: `err.root_cause().downcast_ref::<EsaError>()`, or [`esa_code`].

#![forbid(unsafe_code)]

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, anyhow, bail};
use attest_core::{
    AccountId, Actor, AttestMessage, EsaError, Head, HeaderRecord, InstanceConfig, InstanceState, LcVerifyMessage,
    Predicate, Statement, instance_id, instance_seed, lc, record_id, record_seed,
};
use borsh::{BorshDeserialize, BorshSerialize};
use lee::{privacy_preserving_transaction::circuit::ProgramCatalog, program::Program};
use lee_core::program::PdaSeed;
use wallet::{AccountIdentity, AccountMention};

/// The rc3 wallet over its JSON-RPC sequencer client.
pub type Wallet = wallet::WalletCore<sequencer_service_rpc::SequencerClient>;

pub use lee;
pub use wallet;

/// Gas (= cycles) cap per transaction and per block on LEZ rc3 (`lez/programs/fee/core/src/market.rs`).
pub const MAX_GAS: u64 = 10_000_000;

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

impl DeployedIds {
    /// Reads `[programs] attest = "..."`, `lc_verify = "..."` (base58 account ids) from an ids.toml file.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let doc = read_ids_toml(path)?;
        let get = |k: &str| -> anyhow::Result<AccountId> {
            doc.get("programs")
                .and_then(|p| p.get(k))
                .and_then(toml::Value::as_str)
                .ok_or_else(|| anyhow!("{}: [programs] {k} missing", path.display()))?
                .parse::<AccountId>()
                .map_err(|e| anyhow!("{}: [programs] {k}: {e:?}", path.display()))
        };
        Ok(Self {
            attest: get("attest")?,
            lc_verify: get("lc_verify")?,
        })
    }

    /// [`Self::load`] on `$BV_IDS`, else `$BV_REPO/deploy/ids.toml`.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::load(&ids_path())
    }
}

fn read_ids_toml(path: &Path) -> anyhow::Result<toml::Table> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    text.parse::<toml::Table>()
        .with_context(|| format!("parsing {}", path.display()))
}

/// Path of the ids file: `$BV_IDS`, else `$BV_REPO/deploy/ids.toml`, else `deploy/ids.toml`.
#[must_use]
pub fn ids_path() -> PathBuf {
    if let Ok(p) = std::env::var("BV_IDS") {
        return PathBuf::from(p);
    }
    std::env::var("BV_REPO").map_or_else(
        |_| PathBuf::from("deploy/ids.toml"),
        |r| Path::new(&r).join("deploy/ids.toml"),
    )
}

/// Directory of the local store files: `$BV_STORE_DIR`, else `$LEE_WALLET_HOME_DIR/attest-stores`.
#[must_use]
pub fn store_dir() -> PathBuf {
    if let Ok(p) = std::env::var("BV_STORE_DIR") {
        return PathBuf::from(p);
    }
    std::env::var("LEE_WALLET_HOME_DIR").map_or_else(
        |_| PathBuf::from("attest-stores"),
        |h| Path::new(&h).join("attest-stores"),
    )
}

/// Directory holding program `name`'s `.bin` (`<dir>/<name>.bin`). Precedence:
/// 1. `$BV_GUESTS`, when set explicitly (overrides everything, for local experiments);
/// 2. `[bins].<name>` of the ids file (`$BV_IDS`, else `$BV_REPO/deploy/ids.toml`), written per program by
///    `scripts/deploy_local.sh`;
/// 3. `[deploy].bins` of the ids file (legacy, one directory for all programs);
/// 4. `$CARGO_TARGET_DIR/guests` (`make guests`).
#[must_use]
pub fn guest_dir_for(name: &str) -> PathBuf {
    if let Ok(p) = std::env::var("BV_GUESTS") {
        return PathBuf::from(p);
    }
    if let Ok(doc) = read_ids_toml(&ids_path()) {
        let per_program = doc.get("bins").and_then(|b| b.get(name)).and_then(toml::Value::as_str);
        let legacy = doc
            .get("deploy")
            .and_then(|d| d.get("bins"))
            .and_then(toml::Value::as_str);
        if let Some(dir) = per_program.or(legacy) {
            return PathBuf::from(dir);
        }
    }
    std::env::var("CARGO_TARGET_DIR").map_or_else(|_| PathBuf::from("target/guests"), |t| Path::new(&t).join("guests"))
}

/// [`guest_dir_for`] of `lc_verify` (kept for callers that predate per-program bins).
#[must_use]
pub fn guests_dir() -> PathBuf {
    guest_dir_for("lc_verify")
}

/// The `[programs]` name under which `id` is deployed in the ids file (for example `lc_verify_v2`), if any.
#[must_use]
pub fn program_name(id: &AccountId) -> Option<String> {
    let doc = read_ids_toml(&ids_path()).ok()?;
    let programs = doc.get("programs")?.as_table()?;
    let want = id.to_string();
    programs
        .iter()
        .find(|(_, v)| v.as_str() == Some(want.as_str()))
        .map(|(k, _)| k.clone())
}

/// The account id deployed under `[programs].<name>` in the ids file (for example `lc_verify_v2`, the second
/// verifier; [`DeployedIds`] carries only `attest` and `lc_verify`).
pub fn deployed_program(name: &str) -> anyhow::Result<AccountId> {
    let path = ids_path();
    read_ids_toml(&path)?
        .get("programs")
        .and_then(|p| p.get(name))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow!("{}: [programs] {name} missing", path.display()))?
        .parse::<AccountId>()
        .map_err(|e| anyhow!("{}: [programs] {name}: {e:?}", path.display()))
}

/// Loads program `name` from `guest_dir_for(name)/<name>.bin`.
pub fn load_program(name: &str) -> anyhow::Result<Program> {
    program_from(&guest_dir_for(name), name)
}

/// What the verified local store says about an instance (decision 3). Only returned after the local store file was
/// checked against the on-chain `store_commitment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreStatus {
    /// Slot of the finalized header (= on-chain `head.slot`).
    pub head_slot: u64,
    /// Sync-committee period of `head_slot`.
    pub period: u64,
    /// Whether the next sync committee is known. If not, the next update to submit is the period update
    /// (`updates_by_range` for `period`), which may only set the next committee (committee-only update).
    pub next_committee_known: bool,
}

/// The `EsaError` carried by an error returned from this crate, if any.
#[must_use]
pub fn esa_code(err: &anyhow::Error) -> Option<EsaError> {
    err.chain()
        .find_map(|e| e.downcast_ref::<EsaError>().copied())
        .or_else(|| err.chain().find_map(|e| EsaError::from_panic_text(&e.to_string())))
}

/// An error whose root cause is `code`.
fn refuse(code: EsaError, what: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(code).context(format!("{what}: {code}"))
}

/// Turns a wallet or sequencer error into an anyhow error with the `ESA:Exx` code as root cause when its text has one.
pub fn chain_err(what: &str, text: String) -> anyhow::Error {
    match EsaError::from_panic_text(&text) {
        Some(code) => anyhow::Error::new(code).context(format!("{what}: {text}")),
        None => anyhow!("{what}: {text}"),
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Local store files (decision 3)
// ------------------------------------------------------------------------------------------------------------------

/// Contents of a local store file: the instance config and the Helios store as a witness.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StoreFile {
    pub config: InstanceConfig,
    pub witness: lc::StoreWitness,
}

/// Path of an instance's store file.
#[must_use]
pub fn store_path(instance: &AccountId) -> PathBuf {
    store_dir().join(format!("{instance}.store"))
}

fn write_store(instance: &AccountId, file: &StoreFile) -> anyhow::Result<()> {
    let dir = store_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = store_path(instance);
    let tmp = path.with_extension("store.tmp");
    std::fs::write(&tmp, borsh::to_vec(file)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Reads the local store file of `instance` and checks it against `head.store_commitment` (E13 on mismatch).
pub fn load_verified_store(instance: &AccountId, head: &Head) -> anyhow::Result<StoreFile> {
    let path = store_path(instance);
    let bytes = std::fs::read(&path).with_context(|| format!("no local store file {}", path.display()))?;
    let file: StoreFile = borsh::from_slice(&bytes).with_context(|| format!("decoding {}", path.display()))?;
    let loaded = file.witness.load().map_err(|e| refuse(e, "local store"))?;
    if loaded.commitment() != head.store_commitment {
        return Err(refuse(
            EsaError::E13StoreMismatch,
            format!("local store {} is not the on-chain store", path.display()),
        ));
    }
    Ok(file)
}

/// Rebuilds the store witness that matches `head.store_commitment` (fallback (c) of decision 3), without the chain:
/// pure, so it is tested natively.
///
/// - `bootstrap_ssz` must be a bootstrap whose header is the head's (`head.beacon_root`, `head.slot`): it gives the
///   finalized header and the current committee, checked with the same `Init` checks.
/// - The next committee is unknown, or the `next_sync_committee` of one of `updates` (full `LightClientUpdate`s of the
///   head's period, for example the period's `updates_by_range` item); every candidate is tried.
/// - The two participation counters are part of the commitment but not recoverable from Ethereum data; each lies in
///   0..=512, so all 513 x 513 pairs are tried per candidate (a few hundred thousand SHA-256 calls natively).
pub fn rebuild_witness(
    config: &InstanceConfig,
    head: &Head,
    bootstrap_ssz: &[u8],
    updates: &[Vec<u8>],
) -> Result<lc::StoreWitness, EsaError> {
    let mut at_head = config.clone();
    at_head.checkpoint_root = head.beacon_root;
    at_head.checkpoint_slot = head.slot;
    let (base, _) = lc::init_store(&at_head, bootstrap_ssz)?;
    let forks = lc::forks(config)?;
    let mut nexts: Vec<Option<Vec<u8>>> = vec![None];
    for u in updates {
        if let Ok(lc::AnyUpdate::Full(full)) = lc::decode_update(u, &forks) {
            let c = lc::encode_committee(full.next_sync_committee());
            if !nexts.contains(&Some(c.clone())) {
                nexts.push(Some(c));
            }
        }
    }
    let finalized_root = base.finalized.root();
    let current_root = lc::committee_root_ssz(&base.current_committee_ssz)?;
    for next in nexts {
        let next_root = match &next {
            Some(b) => lc::committee_root_ssz(b)?,
            None => [0u8; 32],
        };
        for prev in 0..=512u64 {
            for cur in 0..=512u64 {
                let c = attest_core::StoreCommitmentInput {
                    finalized_header_root: finalized_root,
                    current_committee_root: current_root,
                    next_committee_root: next_root,
                    previous_max_active_participants: prev,
                    current_max_active_participants: cur,
                };
                if c.commitment() == head.store_commitment {
                    return Ok(lc::StoreWitness {
                        next_committee_ssz: next,
                        previous_max_active_participants: prev,
                        current_max_active_participants: cur,
                        ..base
                    });
                }
            }
        }
    }
    Err(EsaError::E13StoreMismatch)
}

/// Rebuilds and writes the local store file of `instance` from a bootstrap at the on-chain head plus candidate
/// period updates ([`rebuild_witness`]). Fails with E13 as root cause if nothing matches the on-chain commitment.
pub async fn rebuild_store(
    ids: &DeployedIds,
    instance: &AccountId,
    bootstrap_ssz: &[u8],
    updates: &[Vec<u8>],
) -> anyhow::Result<StoreStatus> {
    let state = read_instance(ids, instance)
        .await?
        .ok_or_else(|| anyhow!("instance {instance} does not exist"))?;
    let witness = rebuild_witness(&state.config, &state.head, bootstrap_ssz, updates)
        .map_err(|e| refuse(e, "rebuilding the local store"))?;
    write_store(
        instance,
        &StoreFile {
            config: state.config,
            witness: witness.clone(),
        },
    )?;
    Ok(status_of(&state.head, &witness))
}

fn status_of(head: &Head, witness: &lc::StoreWitness) -> StoreStatus {
    StoreStatus {
        head_slot: head.slot,
        period: head.period,
        next_committee_known: witness.next_committee_ssz.is_some(),
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Wallet plumbing
// ------------------------------------------------------------------------------------------------------------------

pub fn open_wallet() -> anyhow::Result<Wallet> {
    Wallet::from_env().context("opening the LEE wallet ($LEE_WALLET_HOME_DIR)")
}

pub fn account_from_env_or_label(w: &Wallet, var: &str, label: &str) -> anyhow::Result<AccountId> {
    if let Ok(v) = std::env::var(var) {
        let v = v.trim_start_matches("Public/").trim_start_matches("Private/");
        return v.parse().map_err(|e| anyhow!("${var}: {e:?}"));
    }
    let l = wallet::account::Label::new(label);
    let found = w
        .storage()
        .resolve_label(&l)
        .ok_or_else(|| anyhow!("set ${var} or create wallet label {label}"))?;
    Ok(match found {
        wallet::account::AccountIdWithPrivacy::Public(id) | wallet::account::AccountIdWithPrivacy::Private(id) => id,
    })
}

/// Public account paying public transactions (`$BV_PAYER`, else label `bv-payer`).
pub fn payer() -> anyhow::Result<AccountId> {
    account_from_env_or_label(&open_wallet()?, "BV_PAYER", "bv-payer")
}

pub fn program_from(dir: &Path, name: &str) -> anyhow::Result<Program> {
    let p = dir.join(format!("{name}.bin"));
    let bytes = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    Program::new(Cow::Owned(bytes)).map_err(|e| anyhow!("{}: {e}", p.display()))
}

pub fn pda(program: AccountId, seed: [u8; 32]) -> AccountIdentity {
    AccountIdentity::PublicPda {
        program,
        seed: PdaSeed::new(seed),
    }
}

/// Waits for `tx` to land and checks it did not revert. Returns the block id.
pub async fn confirm(w: &Wallet, tx: common::HashType, what: &str) -> anyhow::Result<u64> {
    let (_, block_id) = w
        .poll_transaction(tx)
        .await
        .map_err(|e| anyhow!("{what}: tx {tx} not included: {e:#}"))?;
    let block = w
        .get_block(block_id)
        .await?
        .ok_or_else(|| anyhow!("{what}: block {block_id} not found"))?;
    let pos = block
        .body
        .transactions
        .iter()
        .position(|t| t.hash() == tx)
        .ok_or_else(|| anyhow!("{what}: tx {tx} not in block {block_id}"))?;
    if block.body.is_reverted(pos) {
        bail!("{what}: tx {tx} reverted in block {block_id} (the panic code is in the sequencer log)");
    }
    Ok(block_id)
}

fn gas_of(w: &Wallet) -> Option<u64> {
    match w.take_declared_fee() {
        Some(wallet::DeclaredFee::Public(f)) => Some(f.gas_limit),
        _ => None,
    }
}

/// Sends a public transaction paid by [`payer`] and waits for it.
pub async fn send_public(
    w: &mut Wallet,
    accounts: Vec<AccountMention>,
    message: Vec<u8>,
    what: &str,
) -> anyhow::Result<TxOutcome> {
    let payer = account_from_env_or_label(w, "BV_PAYER", "bv-payer")?;
    if std::env::var("BV_ALLOW_REVERT").is_ok_and(|v| v == "1") {
        w.set_allow_revert(true);
    }
    // The wallet's default budget (config `gas_limit`, 2M on the local network) is below `Init`; simulate within the
    // per-transaction cap instead (`BV_GAS_LIMIT` overrides).
    let gas_limit = std::env::var("BV_GAS_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_GAS);
    w.set_fee_params(wallet::FeeParams {
        gas_limit: Some(gas_limit),
        ..wallet::FeeParams::default()
    });
    let tx = w
        .send_pub_tx_paid_by(accounts, 0, message, Some(payer))
        .await
        .map_err(|e| chain_err(what, format!("{e:#}")))?;
    let gas = gas_of(w);
    confirm(w, tx, what).await?;
    Ok(TxOutcome {
        tx: tx.to_string(),
        cycles: None,
        gas,
    })
}

// ------------------------------------------------------------------------------------------------------------------
// Public API
// ------------------------------------------------------------------------------------------------------------------

/// Reads an instance's state (config and head). `None` if the instance does not exist.
pub async fn read_instance(ids: &DeployedIds, instance: &AccountId) -> anyhow::Result<Option<InstanceState>> {
    let w = open_wallet()?;
    let view = w.get_account_view(Actor::new(*instance, ids.attest)).await?;
    let Some(account) = view else { return Ok(None) };
    let state = account.data.actor_state(ids.attest);
    if state.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        attest_core::rules::decode_instance(state).map_err(|e| refuse(e, "instance state"))?,
    ))
}

/// Reads an instance's head from public state. `None` if the instance does not exist.
pub async fn read_head(ids: &DeployedIds, instance: &AccountId) -> anyhow::Result<Option<Head>> {
    Ok(read_instance(ids, instance).await?.map(|s| s.head))
}

/// Head slot, period and whether the next committee is known, from the local store verified against the chain
/// (decision 3). `None` if the instance does not exist. Errors (E13 root cause) if no local store matches the
/// on-chain commitment; recover with [`rebuild_store`].
pub async fn read_store_status(ids: &DeployedIds, instance: &AccountId) -> anyhow::Result<Option<StoreStatus>> {
    let Some(state) = read_instance(ids, instance).await? else {
        return Ok(None);
    };
    let file = load_verified_store(instance, &state.head)?;
    Ok(Some(status_of(&state.head, &file.witness)))
}

/// Reads the header record of `slot`.
pub async fn read_record(ids: &DeployedIds, instance: &AccountId, slot: u64) -> anyhow::Result<Option<HeaderRecord>> {
    let w = open_wallet()?;
    let id = record_id(&ids.attest, instance, slot);
    let Some(account) = w.get_account_view(Actor::new(id, ids.attest)).await? else {
        return Ok(None);
    };
    let state = account.data.actor_state(ids.attest);
    if state.is_empty() {
        return Ok(None);
    }
    Ok(Some(borsh::from_slice(state).context("decoding header record")?))
}

/// Sends `Init{config, bootstrap}` to `instance_id(config)`; returns the instance id and the outcome. Also writes
/// the local store file of the new instance. Host pre-checks give the same codes as the program.
pub async fn init(
    ids: &DeployedIds,
    config: &InstanceConfig,
    bootstrap_ssz: &[u8],
) -> anyhow::Result<(AccountId, TxOutcome)> {
    let instance = instance_id(&ids.attest, config);
    let (witness, head) = lc::init_store(config, bootstrap_ssz).map_err(|e| refuse(e, "init"))?;
    if read_instance(ids, &instance).await?.is_some() {
        return Err(refuse(
            EsaError::E14AlreadyApplied,
            format!("instance {instance} exists"),
        ));
    }
    let mut w = open_wallet()?;
    let accounts = vec![
        pda(ids.attest, instance_seed(config)).select_program_actor_state(ids.attest),
        pda(ids.attest, record_seed(&instance, head.slot)).select_program_actor_state(ids.attest),
    ];
    let msg = borsh::to_vec(&AttestMessage::Init {
        config: config.clone(),
        bootstrap: bootstrap_ssz.to_vec(),
    })?;
    let out = send_public(&mut w, accounts, msg, "init").await?;
    write_store(
        &instance,
        &StoreFile {
            config: config.clone(),
            witness,
        },
    )?;
    Ok((instance, out))
}

/// Submits one light-client update (`LightClientUpdate` or `LightClientFinalityUpdate`, SSZ) to `instance`.
/// Uses the local store file as the witness, after checking it against the on-chain commitment; updates the file on
/// success. The update is first verified natively with the same code as the guest: a refusal there (including a
/// no-op update, E14) is returned before anything is sent.
pub async fn submit_update(
    ids: &DeployedIds,
    instance: &AccountId,
    update_ssz: &[u8],
    mode: SubmitMode,
) -> anyhow::Result<TxOutcome> {
    submit_update_via(ids, ids.lc_verify, instance, update_ssz, mode).await
}

/// [`submit_update`] through a chosen verifier program (tests: a verifier that is not in the config gives E12).
pub async fn submit_update_via(
    ids: &DeployedIds,
    verifier: AccountId,
    instance: &AccountId,
    update_ssz: &[u8],
    mode: SubmitMode,
) -> anyhow::Result<TxOutcome> {
    let state = read_instance(ids, instance)
        .await?
        .ok_or_else(|| anyhow!("instance {instance} does not exist"))?;
    let file = load_verified_store(instance, &state.head)?;
    let out = lc::verify_and_apply(&state.config, &file.witness, update_ssz).map_err(|e| refuse(e, "update"))?;
    let msg = LcVerifyMessage::Verify {
        attest_program: ids.attest,
        instance: *instance,
        config: state.config.clone(),
        store_witness: borsh::to_vec(&file.witness)?,
        update_ssz: update_ssz.to_vec(),
    };
    let tx = submit_verify_as(ids, verifier, None, instance, out.head.slot, &msg, mode).await?;
    write_store(
        instance,
        &StoreFile {
            config: state.config,
            witness: out.new_witness(),
        },
    )?;
    Ok(tx)
}

/// Sends an already-built `Verify` message (tests: replays and stale witnesses, which the native pre-check would
/// refuse before the chain sees them). `record_slot` is the slot whose record account the transaction declares.
pub async fn submit_verify(
    ids: &DeployedIds,
    verifier: AccountId,
    instance: &AccountId,
    record_slot: u64,
    msg: &LcVerifyMessage,
    mode: SubmitMode,
) -> anyhow::Result<TxOutcome> {
    submit_verify_as(ids, verifier, None, instance, record_slot, msg, mode).await
}

/// [`submit_update`] from a given private submitter account instead of `$BV_SUBMITTER` (F11: an account created
/// seconds earlier submits).
pub async fn submit_update_from(
    ids: &DeployedIds,
    submitter: AccountId,
    instance: &AccountId,
    update_ssz: &[u8],
) -> anyhow::Result<TxOutcome> {
    let state = read_instance(ids, instance)
        .await?
        .ok_or_else(|| anyhow!("instance {instance} does not exist"))?;
    let file = load_verified_store(instance, &state.head)?;
    let out = lc::verify_and_apply(&state.config, &file.witness, update_ssz).map_err(|e| refuse(e, "update"))?;
    let msg = LcVerifyMessage::Verify {
        attest_program: ids.attest,
        instance: *instance,
        config: state.config.clone(),
        store_witness: borsh::to_vec(&file.witness)?,
        update_ssz: update_ssz.to_vec(),
    };
    let tx = submit_verify_as(
        ids,
        ids.lc_verify,
        Some(submitter),
        instance,
        out.head.slot,
        &msg,
        SubmitMode::Private,
    )
    .await?;
    write_store(
        instance,
        &StoreFile {
            config: state.config,
            witness: out.new_witness(),
        },
    )?;
    Ok(tx)
}

/// [`submit_verify`] with an optional private submitter (default `$BV_SUBMITTER` / label `bv-submitter`).
#[allow(clippy::too_many_arguments)]
pub async fn submit_verify_as(
    ids: &DeployedIds,
    verifier: AccountId,
    submitter: Option<AccountId>,
    instance: &AccountId,
    record_slot: u64,
    msg: &LcVerifyMessage,
    mode: SubmitMode,
) -> anyhow::Result<TxOutcome> {
    let mut w = open_wallet()?;
    let message = borsh::to_vec(msg)?;
    let instance_mention = AccountIdentity::PublicNoSign(*instance).select_program_actor_state(ids.attest);
    let record_mention = pda(ids.attest, record_seed(instance, record_slot)).select_program_actor_state(ids.attest);
    match mode {
        SubmitMode::PublicProbe => {
            let payer = account_from_env_or_label(&w, "BV_PAYER", "bv-payer")?;
            let accounts = vec![
                AccountIdentity::Public(payer).select_program_actor_state(verifier),
                instance_mention,
                record_mention,
            ];
            send_public(&mut w, accounts, message, "verify (public probe)").await
        }
        SubmitMode::Private => {
            let submitter = match submitter {
                Some(s) => s,
                None => account_from_env_or_label(&w, "BV_SUBMITTER", "bv-submitter")?,
            };
            // The verifier may be lc_verify (V1) or a second verifier such as lc_verify_v2: load the binary by the
            // name it is deployed under.
            let verifier_name = program_name(&verifier).unwrap_or_else(|| "lc_verify".to_owned());
            let programs = ProgramCatalog::from([
                (ids.attest, load_program("attest")?),
                (verifier, load_program(&verifier_name)?),
            ]);
            let accounts = vec![
                AccountIdentity::PrivateOwned(submitter).select_program_actor_state(verifier),
                instance_mention,
                record_mention,
            ];
            send_private(&mut w, accounts, message, &programs, "verify (private)").await
        }
    }
}

/// Sends a privacy-preserving transaction rooted at `accounts[0]` (its in-proof fee is paid by a funded private
/// account among `accounts`, or the wallet's configured private payer) and waits for it. The wallet is synced to the
/// chain tip first (pending credits, other senders) and after inclusion (new private states), then stored, as the
/// wallet CLI does.
pub async fn send_private(
    w: &mut Wallet,
    accounts: Vec<AccountMention>,
    message: Vec<u8>,
    programs: &ProgramCatalog,
    what: &str,
) -> anyhow::Result<TxOutcome> {
    w.sync_to_latest_block().await.context("syncing the wallet")?;
    let (tx, _) = w
        .send_privacy_preserving_tx(accounts, 0, message, programs)
        .await
        .map_err(|e| chain_err(what, format!("{e:#}")))?;
    let block = confirm(w, tx, what).await;
    w.sync_to_latest_block()
        .await
        .context("syncing the wallet after the transaction")?;
    w.store_persistent_data().context("storing the wallet")?;
    block?;
    Ok(TxOutcome {
        tx: tx.to_string(),
        cycles: None,
        gas: None,
    })
}

/// Sends `Attest` and returns the delivered statement.
pub async fn attest(
    ids: &DeployedIds,
    instance: &AccountId,
    record: &HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
    deliver_to: &Actor,
) -> anyhow::Result<(Statement, TxOutcome)> {
    attest_with(ids, instance, record, claim, predicate, deliver_to, None, Vec::new()).await
}

/// [`attest`] with extra declared actors (for example the consumer's nullifier PDA, which the consumer writes in the
/// same transaction). `deliver_to_mention` declares the consumer actor (for example a `PublicPda` mention for a
/// consumer PDA that does not exist yet); `None` declares it as `PublicNoSign`. It is not declared again when
/// `extra` already declares that account. Public placement: the root actor is the payer's account under `attest`.
#[allow(clippy::too_many_arguments)]
pub async fn attest_with(
    ids: &DeployedIds,
    instance: &AccountId,
    record: &HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
    deliver_to: &Actor,
    deliver_to_mention: Option<AccountMention>,
    extra: Vec<AccountMention>,
) -> anyhow::Result<(Statement, TxOutcome)> {
    let statement = dry_run_attest(instance, record, claim, predicate).map_err(|e| refuse(e, "attest"))?;
    let mut w = open_wallet()?;
    let payer = account_from_env_or_label(&w, "BV_PAYER", "bv-payer")?;
    let mut accounts = vec![
        AccountIdentity::Public(payer).select_program_actor_state(ids.attest),
        AccountIdentity::PublicNoSign(record_id(&ids.attest, instance, record.slot))
            .select_program_actor_state(ids.attest),
    ];
    if !extra.iter().any(|m| m.identity.account_id() == deliver_to.account_id) {
        accounts.push(deliver_to_mention.unwrap_or_else(|| {
            AccountIdentity::PublicNoSign(deliver_to.account_id)
                .select_program_actor_state(deliver_to.program_account_id)
        }));
    }
    accounts.extend(extra);
    let msg = borsh::to_vec(&AttestMessage::Attest {
        instance: *instance,
        record: record.clone(),
        claim: claim.to_vec(),
        predicate: predicate.clone(),
        deliver_to: *deliver_to,
    })?;
    let out = send_public(&mut w, accounts, msg, "attest").await?;
    Ok((statement, out))
}

/// Runs the `Attest` checks on the host, with the same error codes as on chain (021 U2, U5). Does not check that
/// the record exists on chain (the on-chain `CheckHeader` does: E02 / E15).
pub fn dry_run_attest(
    instance: &AccountId,
    record: &HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
) -> Result<Statement, EsaError> {
    attest_inclusion::verify_claim(*instance, record, claim, predicate)
}
