//! On-chain legs of G2 (`BV_G2_ONCHAIN=1`): the same adversarial inputs sent to the deployed `attest` and
//! `lc_verify` programs on `seq-dev` through `attest_host`. Native pre-checks of `attest_host` are bypassed on
//! purpose (`submit_verify`, raw `Init` and `Attest` sends), so the refusal comes from the guest, and the test reads
//! the `ESA:Exx` code from the rejection (`attest_host::esa_code`).
//!
//! Each case uses its own instance (configs differ in `min_participants`, so instance ids differ) so the tests can
//! run in parallel and be re-run: an instance that already exists is reused.

use std::sync::OnceLock;

use adversarial::{fab, mainnet};
use attest_core::{
    AccountId, Actor, AttestMessage, EsaError, HeaderRecord, InstanceConfig, LcVerifyMessage, Predicate, instance_id,
    instance_seed, record_id, record_seed,
};
use attest_host::{self as host, DeployedIds, SubmitMode, TxOutcome, wallet::AccountIdentity};
use fixtures::ssz::UpdateParts;

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn ids() -> DeployedIds {
    DeployedIds::from_env().expect("deploy/ids.toml (scripts/deploy_local.sh)")
}

/// Blocks on `f`.
pub fn block<T>(f: impl Future<Output = T>) -> T {
    rt().block_on(f)
}

/// Asserts the guest refused with `want`.
#[track_caller]
pub fn expect_chain<T: std::fmt::Debug>(r: anyhow::Result<T>, want: EsaError, case: &str) {
    match r {
        Ok(v) => panic!("{case} (on chain): accepted ({v:?}), expected {want}"),
        Err(e) => assert_eq!(
            host::esa_code(&e),
            Some(want),
            "{case} (on chain): got `{e:#}`, expected {want}"
        ),
    }
}

/// The real mainnet config of a G2 instance on this deployment.
pub fn real_config(ids: &DeployedIds, min_participants: u16) -> InstanceConfig {
    let mut c = mainnet().config.clone();
    c.verifiers = vec![ids.lc_verify];
    c.min_participants = min_participants;
    c
}

/// The fabricated chain's own config on this deployment.
pub fn fab_config(ids: &DeployedIds, min_participants: u16) -> InstanceConfig {
    fab().config(vec![ids.lc_verify], min_participants)
}

/// Inits `config` with `bootstrap` unless the instance exists. Returns the instance id.
pub async fn ensure_instance(ids: &DeployedIds, config: &InstanceConfig, bootstrap: &[u8]) -> AccountId {
    let instance = instance_id(&ids.attest, config);
    if host::read_instance(ids, &instance)
        .await
        .expect("read instance")
        .is_none()
    {
        host::init(ids, config, bootstrap)
            .await
            .unwrap_or_else(|e| panic!("init {instance}: {e:#}"));
    }
    instance
}

/// Applies `update` through the normal path if the instance head is below its finalized slot.
pub async fn ensure_applied(ids: &DeployedIds, instance: &AccountId, update: &[u8]) {
    let slot = UpdateParts::from_ssz(update).map_or(0, |u| u.finalized.beacon.slot);
    let head = host::read_head(ids, instance)
        .await
        .expect("read head")
        .expect("instance exists");
    if head.slot < slot {
        host::submit_update(ids, instance, update, SubmitMode::Private)
            .await
            .unwrap_or_else(|e| panic!("apply update at {slot}: {e:#}"));
    }
}

/// Sends `Verify` with the instance's current witness and `update_ssz`, without the native pre-check.
pub async fn verify_raw(ids: &DeployedIds, instance: &AccountId, update_ssz: &[u8]) -> anyhow::Result<TxOutcome> {
    let state = host::read_instance(ids, instance)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no instance"))?;
    let file = host::load_verified_store(instance, &state.head)?;
    let record_slot = UpdateParts::from_ssz(update_ssz)
        .map(|u| u.finalized.beacon.slot)
        .filter(|s| *s > state.head.slot)
        .unwrap_or(state.head.slot + 1);
    let msg = LcVerifyMessage::Verify {
        attest_program: ids.attest,
        instance: *instance,
        config: state.config,
        store_witness: borsh::to_vec(&file.witness)?,
        update_ssz: update_ssz.to_vec(),
    };
    host::submit_verify(ids, ids.lc_verify, instance, record_slot, &msg, SubmitMode::Private).await
}

/// Sends `Init{config: msg_config}` to the instance address of `address_config`, without the native pre-check.
pub async fn init_raw(
    ids: &DeployedIds,
    address_config: &InstanceConfig,
    msg_config: &InstanceConfig,
    bootstrap: &[u8],
) -> anyhow::Result<TxOutcome> {
    let instance = instance_id(&ids.attest, address_config);
    let mut w = host::wallet::WalletCore::from_env()?;
    let accounts = vec![
        host::pda(ids.attest, instance_seed(address_config)).select_program_actor_state(ids.attest),
        host::pda(ids.attest, record_seed(&instance, address_config.checkpoint_slot))
            .select_program_actor_state(ids.attest),
    ];
    let msg = borsh::to_vec(&AttestMessage::Init {
        config: msg_config.clone(),
        bootstrap: bootstrap.to_vec(),
    })?;
    host::send_public(&mut w, accounts, msg, "init (raw)").await
}

/// Sends `Attest` without the host dry run. The statement is delivered to the payer's own actor under `attest`
/// (never reached in these cases).
pub async fn attest_raw(
    ids: &DeployedIds,
    instance: &AccountId,
    record: &HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
) -> anyhow::Result<TxOutcome> {
    let mut w = host::wallet::WalletCore::from_env()?;
    let payer = host::account_from_env_or_label(&w, "BV_PAYER", "bv-payer")?;
    let accounts = vec![
        AccountIdentity::Public(payer).select_program_actor_state(ids.attest),
        AccountIdentity::PublicNoSign(record_id(&ids.attest, instance, record.slot))
            .select_program_actor_state(ids.attest),
    ];
    let msg = borsh::to_vec(&AttestMessage::Attest {
        instance: *instance,
        record: record.clone(),
        claim: claim.to_vec(),
        predicate: predicate.clone(),
        deliver_to: Actor {
            account_id: payer,
            program_account_id: ids.attest,
        },
    })?;
    host::send_public(&mut w, accounts, msg, "attest (raw)").await
}

/// Deployment ids, or a panic naming what is missing.
pub fn deployment() -> DeployedIds {
    ids()
}
