//! G3 end to end on a running LEZ v0.4.0-rc3 sequencer: one attested ERC-20 `Transfer` lands in the
//! consumer's own transaction and changes its state; the identical transaction is refused with
//! `E11_ALREADY_CONSUMED`. Plus the private placement: `Attest` run in the user's private account, the statement
//! delivered to the public consumer, in dev mode.
//!
//! Needs `scripts/seq.sh dev start`, `attest`, `lc_verify` and `consumer_erc20` deployed (`consumer_erc20` id in
//! `$BV_CONSUMER_ERC20` or `[programs] consumer_erc20` of the ids file), a funded payer (`bv-payer`) and, for the
//! private test and for building a fresh instance, a funded private submitter (`bv-submitter`).
//!
//!   cargo test -p e2e_consumer --test e2e_consumer -- --ignored --nocapture --test-threads 1
//!
//! The light-client instance: `$BV_E2E_INSTANCE` (base58) if set and its record at beacon slot 15637568 exists;
//! else a fresh mainnet instance is built from fixtures (`e2e_chain::mainnet_chain`: Init + 5 updates, Path A
//! in dev mode), whose last update finalises execution block 26397891, the block of the claim fixtures.
//! Each run uses a fresh consumer instance (config `salt` = unix time), so reruns start clean.
//! A line `G3RESULT {json}` is printed for `scripts/gate/g3_consumer.sh`.

use attest_core::{AccountId, Actor, EsaError, Predicate};
use attest_host::{DeployedIds, SubmitMode, esa_code, open_wallet as wallet, pda};
use consumer_erc20_program::{
    ConsumerConfig, ConsumerMessage, ConsumerState, consumer_instance_id, consumer_instance_seed, nullifier_id,
    nullifier_seed,
};
use e2e_consumer::{Fixture, MAINNET_RECORD_SLOT};
use serde_json::json;
use wallet_alias::AccountIdentity;

mod wallet_alias {
    pub use attest_host::wallet::AccountIdentity;
}

fn consumer_program() -> AccountId {
    if let Ok(v) = std::env::var("BV_CONSUMER_ERC20") {
        return v.trim_start_matches("Public/").parse().expect("$BV_CONSUMER_ERC20");
    }
    let text = std::fs::read_to_string(attest_host::ids_path()).expect("ids file");
    let doc: toml::Table = text.parse().expect("ids toml");
    doc["programs"]["consumer_erc20"]
        .as_str()
        .expect("[programs] consumer_erc20 in the ids file")
        .parse()
        .expect("id")
}

/// Directory with `consumer_erc20.bin` (the deployed binary): `$BV_W3_GUESTS`, else `$B/run/w3-guests`.
fn consumer_guests_dir() -> std::path::PathBuf {
    std::env::var("BV_W3_GUESTS").map_or_else(
        |_| std::path::PathBuf::from(std::env::var("B").expect("$B (source scripts/env.sh)")).join("run/w3-guests"),
        std::path::PathBuf::from,
    )
}

fn salt() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// An instance whose record at `MAINNET_RECORD_SLOT` exists: `$BV_E2E_INSTANCE`, else built now.
async fn instance_with_record(ids: &DeployedIds) -> (AccountId, attest_core::HeaderRecord) {
    if let Ok(v) = std::env::var("BV_E2E_INSTANCE") {
        let inst: AccountId = v.parse().expect("$BV_E2E_INSTANCE");
        if let Some(r) = attest_host::read_record(ids, &inst, MAINNET_RECORD_SLOT).await.unwrap() {
            println!("reusing instance {inst}");
            return (inst, r);
        }
        println!("$BV_E2E_INSTANCE has no record at {MAINNET_RECORD_SLOT}: building a fresh instance");
    }
    let chain = e2e_chain::mainnet_chain(vec![ids.lc_verify]).expect("fixtures");
    let mut config = chain.config.clone();
    for k in 0..1000u16 {
        config.min_participants = 342 + k;
        if attest_host::read_head(ids, &attest_core::instance_id(&ids.attest, &config))
            .await
            .unwrap()
            .is_none()
        {
            break;
        }
    }
    let (inst, out) = attest_host::init(ids, &config, &chain.bootstrap).await.expect("init");
    println!("init instance {inst}: tx {}", out.tx);
    for (label, update) in &chain.updates {
        let out = attest_host::submit_update(ids, &inst, update, SubmitMode::Private)
            .await
            .expect(label);
        println!("{label}: tx {}", out.tx);
    }
    let r = attest_host::read_record(ids, &inst, MAINNET_RECORD_SLOT)
        .await
        .unwrap()
        .expect("record at 15637568");
    (inst, r)
}

async fn read_consumer(program: AccountId, id: AccountId) -> Option<ConsumerState> {
    let w = wallet().unwrap();
    let view = w.get_account_view(Actor::new(id, program)).await.unwrap()?;
    let s = view.data.actor_state(program);
    if s.is_empty() {
        None
    } else {
        Some(borsh::from_slice(s).expect("consumer state"))
    }
}

async fn read_nullifier(program: AccountId, id: AccountId) -> Vec<u8> {
    let w = wallet().unwrap();
    match w.get_account_view(Actor::new(id, program)).await.unwrap() {
        Some(v) => v.data.actor_state(program).to_vec(),
        None => Vec::new(),
    }
}

async fn init_consumer(program: AccountId, config: &ConsumerConfig) -> AccountId {
    let id = consumer_instance_id(&program, config);
    let mut w = wallet().unwrap();
    let msg = borsh::to_vec(&ConsumerMessage::Init { config: config.clone() }).unwrap();
    let out = attest_host::send_public(
        &mut w,
        vec![pda(program, consumer_instance_seed(config)).select_program_actor_state(program)],
        msg,
        "consumer init",
    )
    .await
    .expect("consumer init");
    println!("consumer instance {id}: init tx {} gas {:?}", out.tx, out.gas);
    id
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running seq-dev with attest, lc_verify and consumer_erc20 deployed"]
async fn g3_consumer_accepts_once_and_refuses_replay() {
    let ids = DeployedIds::from_env().expect("ids");
    let cprog = consumer_program();
    let (instance, record) = instance_with_record(&ids).await;

    let f = Fixture::load("mainnet_log_usdt_tx1_log2.json").unwrap();
    assert_eq!(
        record.exec_hash,
        f.word(&f.0["block_hash"]).unwrap(),
        "record holds the fixture block"
    );
    let claim = f.bytes("claim").unwrap();
    let t = f.0["expected"]["topics"].as_array().unwrap();
    let predicate = Predicate::All(vec![
        Predicate::LogTopicEq {
            i: 0,
            v: f.word(&t[0]).unwrap(),
        },
        Predicate::LogTopicEq {
            i: 2,
            v: f.word(&t[2]).unwrap(),
        },
    ]);
    let config = f.consumer_config(ids.attest, instance, salt()).unwrap();
    let me = init_consumer(cprog, &config).await;
    let before = read_consumer(cprog, me).await.expect("consumer state after init");
    assert_eq!(before.count, 0);

    let deliver_to = Actor::new(me, cprog);
    let st = attest_host::dry_run_attest(&instance, &record, &claim, &predicate).expect("dry run");
    let nullifier = nullifier_id(&cprog, &me, &st.id);
    let extra = || vec![pda(cprog, nullifier_seed(&me, &st.id)).select_program_actor_state(cprog)];

    // tx 1: accepted, consumer state changed, nullifier written.
    let (st1, out1) =
        attest_host::attest_with(&ids, &instance, &record, &claim, &predicate, &deliver_to, None, extra())
            .await
            .expect("tx 1");
    assert_eq!(st1, st);
    let after = read_consumer(cprog, me).await.unwrap();
    assert_eq!(after.count, 1);
    assert_eq!(after.total, f.amount().unwrap());
    let credit = after.last.clone().unwrap();
    assert_eq!(credit.statement_id, st.id);
    assert_eq!(read_nullifier(cprog, nullifier).await, me.value().to_vec());
    println!(
        "tx 1 {} accepted: consumer count {} total 0x{}",
        out1.tx,
        after.count,
        hex::encode(after.total)
    );

    // tx 2, identical: refused with E11 by the transaction simulation...
    let r2 = attest_host::attest_with(&ids, &instance, &record, &claim, &predicate, &deliver_to, None, extra()).await;
    let code2 = match &r2 {
        Ok(o) => panic!("tx 2 accepted: {}", o.1.tx),
        Err(e) => {
            println!("tx 2 refused: {e:#}");
            esa_code(e)
        }
    };
    assert_eq!(code2, Some(EsaError::E11AlreadyConsumed));
    // ...and, sent anyway (the wallet's allow-revert flag, as BV_ALLOW_REVERT=1 sets it), included as reverted by
    // the sequencer with the consumer state unchanged.
    let r3 = send_attest_allow_revert(&ids, &instance, &record, &claim, &predicate, &deliver_to, extra()).await;
    let sent_reverted = match &r3 {
        Ok(tx) => panic!("tx 2 (sent) accepted: {tx}"),
        Err(e) => {
            println!("tx 2 sent with allow-revert: {e:#}");
            format!("{e:#}")
        }
    };
    let final_state = read_consumer(cprog, me).await.unwrap();
    assert_eq!(final_state, after, "consumer state unchanged by the replay");

    println!(
        "G3RESULT {}",
        json!({
            "instance": instance.to_string(), "record_slot": record.slot, "exec_number": record.exec_number,
            "exec_hash": hex::encode(record.exec_hash),
            "attest_program": ids.attest.to_string(), "consumer_program": cprog.to_string(),
            "consumer_instance": me.to_string(), "nullifier": nullifier.to_string(),
            "statement_id": hex::encode(st.id), "tx1": {"tx": out1.tx, "gas_limit_declared": out1.gas, "accepted": true,
                "consumer_count_before": before.count, "consumer_count_after": after.count,
                "consumer_total_after": hex::encode(after.total)},
            "tx2_simulated": {"accepted": false, "code": code2.map(|c| c.name())},
            "tx2_sent": {"accepted": false, "error": sent_reverted, "consumer_state_unchanged": true},
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running seq-dev with attest, lc_verify and consumer_erc20 deployed, and a funded bv-submitter"]
async fn private_placement_attest_in_private_account() {
    let ids = DeployedIds::from_env().expect("ids");
    let cprog = consumer_program();
    let (instance, record) = instance_with_record(&ids).await;
    // Another USDT transfer in the same receipt (tx 1, log 4), another recipient: its own consumer instance.
    let f = Fixture::load("mainnet_log_usdt_tx1_log4.json").unwrap();
    let claim = f.bytes("claim").unwrap();
    let predicate = Predicate::All(vec![]);
    let config = f.consumer_config(ids.attest, instance, salt()).unwrap();
    let me = init_consumer(cprog, &config).await;
    let st = attest_host::dry_run_attest(&instance, &record, &claim, &predicate).expect("dry run");

    let mut w = wallet().unwrap();
    let submitter = attest_host::account_from_env_or_label(&w, "BV_SUBMITTER", "bv-submitter").unwrap();
    let dir = attest_host::guests_dir();
    let programs = attest_host::lee::privacy_preserving_transaction::circuit::ProgramCatalog::from([
        (ids.attest, attest_host::program_from(&dir, "attest").unwrap()),
        (
            cprog,
            attest_host::program_from(&consumer_guests_dir(), "consumer_erc20").unwrap(),
        ),
    ]);
    let accounts = vec![
        AccountIdentity::PrivateOwned(submitter).select_program_actor_state(ids.attest),
        AccountIdentity::PublicNoSign(attest_core::record_id(&ids.attest, &instance, record.slot))
            .select_program_actor_state(ids.attest),
        AccountIdentity::PublicNoSign(me).select_program_actor_state(cprog),
        pda(cprog, nullifier_seed(&me, &st.id)).select_program_actor_state(cprog),
    ];
    let msg = borsh::to_vec(&attest_core::AttestMessage::Attest {
        instance,
        record: record.clone(),
        claim: claim.clone(),
        predicate: predicate.clone(),
        deliver_to: Actor::new(me, cprog),
    })
    .unwrap();
    let t = std::time::Instant::now();
    let out = attest_host::send_private(&mut w, accounts.clone(), msg.clone(), &programs, "attest (private)")
        .await
        .expect("private attest");
    let secs = t.elapsed().as_secs_f64();
    let after = read_consumer(cprog, me).await.unwrap();
    assert_eq!(after.count, 1);
    assert_eq!(after.last.unwrap().statement_id, st.id);
    println!(
        "private attest tx {} in {secs:.1} s (dev mode): consumer count 1",
        out.tx
    );

    // Replay from the private account: refused (E11 from the public consumer leg).
    let r = attest_host::send_private(&mut w, accounts, msg, &programs, "attest (private, replay)").await;
    let code = r.as_ref().err().and_then(esa_code);
    println!(
        "private replay: {:?}",
        r.as_ref().map(|o| &o.tx).map_err(|e| format!("{e:#}"))
    );
    assert!(r.is_err(), "private replay accepted");
    println!(
        "PRIVATERESULT {}",
        json!({"instance": instance.to_string(), "consumer_instance": me.to_string(), "statement_id": hex::encode(st.id),
               "tx": out.tx, "wall_s": secs, "dev_mode": std::env::var("RISC0_DEV_MODE").ok(),
               "replay_code": code.map(|c| c.name())})
    );
}

/// The same transaction as `attest_host::attest_with` builds, sent with the wallet's allow-revert flag so the
/// sequencer itself judges it (no local refusal). `Err` when it is not included or included as reverted.
async fn send_attest_allow_revert(
    ids: &DeployedIds,
    instance: &AccountId,
    record: &attest_core::HeaderRecord,
    claim: &[u8],
    predicate: &Predicate,
    deliver_to: &Actor,
    extra: Vec<attest_host::wallet::AccountMention>,
) -> anyhow::Result<String> {
    let mut w = wallet()?;
    let payer = attest_host::account_from_env_or_label(&w, "BV_PAYER", "bv-payer")?;
    let mut accounts = vec![
        AccountIdentity::Public(payer).select_program_actor_state(ids.attest),
        AccountIdentity::PublicNoSign(attest_core::record_id(&ids.attest, instance, record.slot))
            .select_program_actor_state(ids.attest),
        AccountIdentity::PublicNoSign(deliver_to.account_id).select_program_actor_state(deliver_to.program_account_id),
    ];
    accounts.extend(extra);
    let msg = borsh::to_vec(&attest_core::AttestMessage::Attest {
        instance: *instance,
        record: record.clone(),
        claim: claim.to_vec(),
        predicate: predicate.clone(),
        deliver_to: *deliver_to,
    })?;
    w.set_allow_revert(true);
    let tx = w
        .send_pub_tx_paid_by(accounts, 0, msg, Some(payer))
        .await
        .map_err(|e| attest_host::chain_err("attest (allow revert)", format!("{e:#}")))?;
    attest_host::confirm(&w, tx, "attest (allow revert)").await?;
    Ok(tx.to_string())
}
