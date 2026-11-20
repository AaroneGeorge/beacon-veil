//! G3 with a REAL proof on seq-real: a private `Attest` (the mainnet USDT `Transfer` log at block 26397891)
//! proven for real in the user's private account, delivered to a public `consumer_erc20` instance in the same
//! transaction; then the identical transaction again, refused with `E11_ALREADY_CONSUMED`.
//!
//! ```text
//! g3_real --check-image <release IMAGE_IDS>        # consumer_erc20's on-chain header holds the release image id
//! g3_real --instance <base58> [--fixture mainnet_log_usdt_tx1_log2.json]
//! ```
//! Environment (as attest_host): `BV_IDS` (seq-real ids file), `LEE_WALLET_HOME_DIR` (wallet with `bv-payer` and
//! the funded private `bv-submitter`), `RISC0_DEV_MODE` unset. Run under `tools/cost/prove_job.sh`, which records
//! wall time and peak RSS. Prints one line `G3REAL {json}`.

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, Actor, AttestMessage, EsaError, Predicate, record_id};
use attest_host::{DeployedIds, esa_code, pda, wallet::AccountIdentity};
use consumer_erc20_program::{
    ConsumerMessage, ConsumerState, consumer_instance_id, consumer_instance_seed, nullifier_id, nullifier_seed,
};
use e2e_consumer::{Fixture, MAINNET_RECORD_SLOT};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    if let Some(f) = arg("--check-image") {
        return check_image(&f).await;
    }
    let t0 = std::time::Instant::now();
    let instance: AccountId = arg("--instance")
        .context("--instance")?
        .parse()
        .map_err(|e| anyhow!("--instance: {e:?}"))?;
    let fixture = arg("--fixture").unwrap_or_else(|| "mainnet_log_usdt_tx1_log2.json".into());
    let dev_mode = std::env::var("RISC0_DEV_MODE").ok();
    println!("RISC0_DEV_MODE={dev_mode:?}");

    let ids = DeployedIds::from_env()?;
    let cprog = attest_host::deployed_program("consumer_erc20")?;
    let record = attest_host::read_record(&ids, &instance, MAINNET_RECORD_SLOT)
        .await?
        .ok_or_else(|| anyhow!("instance {instance} has no record at slot {MAINNET_RECORD_SLOT}"))?;
    let f = Fixture::load(&fixture)?;
    if record.exec_hash != f.word(&f.0["block_hash"])? {
        bail!("record at {MAINNET_RECORD_SLOT} is not the fixture's block");
    }
    let claim = f.bytes("claim")?;
    let topics = f.0["expected"]["topics"].as_array().context("topics")?;
    let predicate = Predicate::All(vec![
        Predicate::LogTopicEq {
            i: 0,
            v: f.word(&topics[0])?,
        },
        Predicate::LogTopicEq {
            i: 2,
            v: f.word(&topics[2])?,
        },
    ]);
    let st =
        attest_host::dry_run_attest(&instance, &record, &claim, &predicate).map_err(|e| anyhow!("dry run: {e}"))?;

    // Consumer instance (public Init, no proof).
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let config = f.consumer_config(ids.attest, instance, salt)?;
    let me = consumer_instance_id(&cprog, &config);
    let mut w = attest_host::open_wallet()?;
    let init = attest_host::send_public(
        &mut w,
        vec![pda(cprog, consumer_instance_seed(&config)).select_program_actor_state(cprog)],
        borsh::to_vec(&ConsumerMessage::Init { config: config.clone() })?,
        "consumer init",
    )
    .await?;
    println!("consumer instance {me}: init tx {}", init.tx);
    let before = read_consumer(&w, cprog, me)
        .await?
        .context("consumer state after init")?;

    // The private Attest: root actor = the user's private account under `attest`.
    let submitter = attest_host::account_from_env_or_label(&w, "BV_SUBMITTER", "bv-submitter")?;
    let programs = attest_host::lee::privacy_preserving_transaction::circuit::ProgramCatalog::from([
        (
            ids.attest,
            attest_host::program_from(&attest_host::guest_dir_for("attest"), "attest")?,
        ),
        (
            cprog,
            attest_host::program_from(&attest_host::guest_dir_for("consumer_erc20"), "consumer_erc20")?,
        ),
    ]);
    let accounts = || {
        vec![
            AccountIdentity::PrivateOwned(submitter).select_program_actor_state(ids.attest),
            AccountIdentity::PublicNoSign(record_id(&ids.attest, &instance, record.slot))
                .select_program_actor_state(ids.attest),
            AccountIdentity::PublicNoSign(me).select_program_actor_state(cprog),
            pda(cprog, nullifier_seed(&me, &st.id)).select_program_actor_state(cprog),
        ]
    };
    let msg = borsh::to_vec(&AttestMessage::Attest {
        instance,
        record: record.clone(),
        claim: claim.clone(),
        predicate: predicate.clone(),
        deliver_to: Actor::new(me, cprog),
    })?;
    let t = std::time::Instant::now();
    let out = attest_host::send_private(&mut w, accounts(), msg.clone(), &programs, "attest (private, real)").await?;
    let prove_and_submit_s = t.elapsed().as_secs_f64();
    println!("private attest tx {} in {prove_and_submit_s:.1} s", out.tx);

    let hash = common::HashType::from_str(&out.tx).map_err(|e| anyhow!("tx hash: {e:?}"))?;
    let block_id = attest_host::confirm(&w, hash, "g3 real").await?;
    let block = w
        .get_block(block_id)
        .await?
        .ok_or_else(|| anyhow!("block {block_id}"))?;
    let tx = block
        .body
        .transactions
        .iter()
        .find(|t| t.hash() == hash)
        .context("tx not in block")?;
    let tx_bytes = borsh::to_vec(tx)?.len();
    let witness_bytes = match tx {
        common::transaction::LeeTransaction::PrivacyPreserving(p) => Some(borsh::to_vec(&p.witness_set)?.len()),
        _ => None,
    };
    let after = read_consumer(&w, cprog, me).await?.context("consumer state")?;
    let credited = after.count == before.count + 1 && after.last.as_ref().map(|c| c.statement_id) == Some(st.id);
    let nullifier = nullifier_id(&cprog, &me, &st.id);

    // The identical transaction again: refused (E11 from the public consumer leg) before anything is proven.
    let t = std::time::Instant::now();
    let replay = attest_host::send_private(&mut w, accounts(), msg, &programs, "attest (private, replay)").await;
    let replay_s = t.elapsed().as_secs_f64();
    let (replay_accepted, replay_code, replay_err) = match &replay {
        Ok(o) => (true, None, o.tx.clone()),
        Err(e) => (false, esa_code(e), format!("{e:#}")),
    };
    println!("replay: accepted {replay_accepted} code {replay_code:?}: {replay_err}");
    let final_state = read_consumer(&w, cprog, me).await?.context("consumer state")?;

    let doc = json!({
        "instance": instance.to_string(), "record_slot": record.slot, "exec_number": record.exec_number,
        "exec_hash": hex::encode(record.exec_hash), "fixture": fixture,
        "attest_program": ids.attest.to_string(), "consumer_program": cprog.to_string(),
        "consumer_instance": me.to_string(), "consumer_init_tx": init.tx, "nullifier": nullifier.to_string(),
        "statement_id": hex::encode(st.id), "submitter_private": true,
        "tx": out.tx, "block_id": block_id, "tx_wire_bytes": tx_bytes, "witness_set_bytes": witness_bytes,
        "prove_and_submit_s": prove_and_submit_s,
        "accepted": credited, "consumer_count_before": before.count, "consumer_count_after": after.count,
        "consumer_total_after": hex::encode(after.total),
        "replay": {"accepted": replay_accepted, "code": replay_code.map(EsaError::name), "error": replay_err,
                   "seconds": replay_s, "consumer_state_unchanged": final_state == after},
        "total_s": t0.elapsed().as_secs_f64(), "risc0_dev_mode": dev_mode,
    });
    println!("G3REAL {doc}");
    if !credited || replay_accepted || replay_code != Some(EsaError::E11AlreadyConsumed) || final_state != after {
        bail!("G3 real: expectations not met");
    }
    Ok(())
}

async fn read_consumer(w: &attest_host::Wallet, program: AccountId, id: AccountId) -> Result<Option<ConsumerState>> {
    let Some(view) = w.get_account_view(Actor::new(id, program)).await? else {
        return Ok(None);
    };
    let s = view.data.actor_state(program);
    if s.is_empty() {
        Ok(None)
    } else {
        Ok(Some(borsh::from_slice(s)?))
    }
}

/// The on-chain program header of `consumer_erc20` holds the release image id (same check as g1_real_proof).
async fn check_image(image_ids_file: &str) -> Result<()> {
    let text = std::fs::read_to_string(image_ids_file)?;
    let want = text
        .lines()
        .find(|l| l.starts_with("consumer_erc20.bin:"))
        .and_then(|l| l.rsplit("image id ").next())
        .map(str::trim)
        .context("consumer_erc20 not in IMAGE_IDS")?;
    let want_bytes = hex::decode(want)?;
    let header = attest_host::deployed_program("consumer_erc20")?;
    let w = attest_host::open_wallet()?;
    let loader = lee_core::program::PROGRAM_LOADER_ACCOUNT_ID;
    let view = w.get_account_view(Actor::new(header, loader)).await?;
    let state = view.map(|a| a.data.actor_state(loader).to_vec()).unwrap_or_default();
    let found = state.windows(32).any(|win| win == want_bytes.as_slice());
    println!(
        "consumer_erc20: header {header} state {} bytes, image id {want}: {}",
        state.len(),
        if found { "MATCH" } else { "NOT FOUND" }
    );
    if !found {
        bail!("on-chain header does not hold the release image id");
    }
    Ok(())
}
