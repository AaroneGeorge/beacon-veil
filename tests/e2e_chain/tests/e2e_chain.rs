//! End-to-end light-client chain on a running LEZ v0.4.0-rc3 sequencer (accept (b), gate G1 dev-mode half).
//!
//! Needs: `scripts/seq.sh dev start`, `scripts/deploy_local.sh dev` (writes deploy/ids.toml), a funded public
//! payer (`BV_PAYER` or wallet label `bv-payer`) and a funded private submitter (`BV_SUBMITTER` or label
//! `bv-submitter`). Run with:
//!
//!   cargo test -p e2e_chain --test e2e_chain -- --ignored --nocapture --test-threads 1
//!
//! Chain (real mainnet fixtures): Init from bootstrap 15605760 (period 1905), then update[0] (learns the next
//! committee), update[1] (handoff to 1906), update[2] (handoff to 1907), each proven privately (Path A) in dev
//! mode and applied publicly; update[3] (handoff to 1908) from a private account created seconds earlier (F11).
//! Then: a replayed Apply (E14) and an instance whose config does not list the verifier (E12) are rejected.

use attest_core::{AccountId, EsaError, InstanceConfig, LcVerifyMessage, lc};
use attest_host::{DeployedIds, SubmitMode, esa_code};

fn expect_code(r: anyhow::Result<attest_host::TxOutcome>, want: EsaError, what: &str) {
    match r {
        Ok(o) => panic!("{what}: accepted (tx {}), expected {want}", o.tx),
        Err(e) => {
            let got = esa_code(&e);
            println!("{what}: rejected with {got:?}: {e:#}");
            assert_eq!(got, Some(want), "{what}: wrong code; error: {e:#}");
        }
    }
}

/// A config not yet used on this chain: the instance address is the PDA of the config, so a rerun picks another
/// `min_participants` (>= 342, so the effective quorum is unchanged).
async fn fresh_config(ids: &DeployedIds, base: &InstanceConfig) -> InstanceConfig {
    let mut c = base.clone();
    for k in 0..1000u16 {
        c.min_participants = 342 + k;
        let id = attest_core::instance_id(&ids.attest, &c);
        if attest_host::read_head(ids, &id).await.expect("read_head").is_none() {
            return c;
        }
    }
    panic!("no fresh config");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running seq-dev with attest and lc_verify deployed"]
async fn mainnet_chain_init_three_updates_handoff_and_rejections() {
    // libtest prints the test name without a newline: start our lines at column 0 (scripts/gate/g1_chain.sh parses
    // `^init:`, `^update[i]:`, `^F11:` and the two `rejected with` lines).
    println!();
    let ids = DeployedIds::from_env().expect("deploy/ids.toml");
    let chain = e2e_chain::mainnet_chain(vec![ids.lc_verify]).expect("fixtures");
    let config = fresh_config(&ids, &chain.config).await;

    // Init from a real bootstrap.
    let (instance, out) = attest_host::init(&ids, &config, &chain.bootstrap).await.expect("init");
    println!("init: instance {instance} tx {} gas {:?}", out.tx, out.gas);
    let head = attest_host::read_head(&ids, &instance)
        .await
        .unwrap()
        .expect("instance exists");
    assert_eq!((head.slot, head.period), (15_605_760, 1905));
    let rec = attest_host::read_record(&ids, &instance, head.slot)
        .await
        .unwrap()
        .expect("checkpoint record");
    assert_eq!(rec.beacon_root, config.checkpoint_root);

    // Three consecutive updates, Path A, with handoffs.
    let expect_period = [1905u64, 1906, 1907];
    let mut last_msg = None;
    for (i, (label, update)) in chain.updates.iter().take(3).enumerate() {
        let before = attest_host::read_head(&ids, &instance).await.unwrap().unwrap();
        // Keep the exact message for the replay test.
        let file = attest_host::load_verified_store(&instance, &before).expect("local store");
        let msg = LcVerifyMessage::Verify {
            attest_program: ids.attest,
            instance,
            config: config.clone(),
            store_witness: borsh::to_vec(&file.witness).unwrap(),
            update_ssz: update.clone(),
        };
        let native = lc::verify_and_apply(&config, &file.witness, update).expect("native verify");
        let out = attest_host::submit_update(&ids, &instance, update, SubmitMode::Private)
            .await
            .expect(label);
        let head = attest_host::read_head(&ids, &instance).await.unwrap().unwrap();
        println!(
            "{label}: tx {} -> head slot {} period {} next known {} (handoff {})",
            out.tx,
            head.slot,
            head.period,
            attest_host::read_store_status(&ids, &instance)
                .await
                .unwrap()
                .unwrap()
                .next_committee_known,
            native.handoff
        );
        assert_eq!(head.store_commitment, native.post_store);
        assert_eq!(head.period, expect_period[i]);
        assert!(head.slot >= before.slot);
        if head.slot > before.slot {
            let rec = attest_host::read_record(&ids, &instance, head.slot)
                .await
                .unwrap()
                .expect("record");
            assert_eq!(rec, native.record);
        }
        last_msg = Some((msg, native.head.slot));
    }
    let head = attest_host::read_head(&ids, &instance).await.unwrap().unwrap();
    assert!(head.period >= 1906, "at least one handoff");

    // Replayed Apply: the same Verify message again (pre-store is no longer the instance's) -> E14.
    let (msg, slot) = last_msg.unwrap();
    expect_code(
        attest_host::submit_verify(&ids, ids.lc_verify, &instance, slot, &msg, SubmitMode::Private).await,
        EsaError::E14AlreadyApplied,
        "replayed Apply",
    );

    // Wrong verifier: an instance whose config lists another verifier program; our lc_verify -> E12.
    let mut other = chain.config.clone();
    other.verifiers = vec![AccountId::new([0x0B; 32])];
    let other = fresh_config(&ids, &other).await;
    let (other_instance, _) = attest_host::init(&ids, &other, &chain.bootstrap).await.expect("init 2");
    let file = attest_host::load_verified_store(
        &other_instance,
        &attest_host::read_head(&ids, &other_instance).await.unwrap().unwrap(),
    )
    .unwrap();
    let native = lc::verify_and_apply(&other, &file.witness, &chain.updates[0].1).unwrap();
    let msg = LcVerifyMessage::Verify {
        attest_program: ids.attest,
        instance: other_instance,
        config: other.clone(),
        store_witness: borsh::to_vec(&file.witness).unwrap(),
        update_ssz: chain.updates[0].1.clone(),
    };
    expect_code(
        attest_host::submit_verify(
            &ids,
            ids.lc_verify,
            &other_instance,
            native.head.slot,
            &msg,
            SubmitMode::Private,
        )
        .await,
        EsaError::E12UnknownVerifier,
        "wrong verifier",
    );

    // F11: a private account created and funded seconds earlier submits the next update (update[3], handoff to
    // 1908).
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/new_submitter.sh");
    let created = std::process::Command::new(script).output().expect("new_submitter.sh");
    assert!(
        created.status.success(),
        "new_submitter.sh: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let fresh: AccountId = String::from_utf8(created.stdout)
        .unwrap()
        .trim()
        .parse()
        .expect("account id");
    let t = std::time::Instant::now();
    let out = attest_host::submit_update_from(&ids, fresh, &instance, &chain.updates[3].1)
        .await
        .expect("F11 submit");
    let head = attest_host::read_head(&ids, &instance).await.unwrap().unwrap();
    println!(
        "F11: fresh account {fresh} submitted update[3]: tx {} -> period {} ({:?})",
        out.tx,
        head.period,
        t.elapsed()
    );
    assert_eq!(head.period, 1908);
}

/// B0 (D-09): the same monolithic `Verify` sent as a PUBLIC transaction (sequencer-executed, metered). Expected:
/// refused at the 10M per-transaction cap. With `BV_ALLOW_REVERT=1` the wallet sends it anyway and the sequencer
/// includes it as reverted (fee charged); otherwise the wallet's simulation refuses it before sending.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running seq-dev with attest and lc_verify deployed"]
async fn b0_public_verify_hits_the_cap() {
    println!();
    let ids = DeployedIds::from_env().expect("deploy/ids.toml");
    let chain = e2e_chain::mainnet_chain(vec![ids.lc_verify]).expect("fixtures");
    let config = fresh_config(&ids, &chain.config).await;
    let (instance, _) = attest_host::init(&ids, &config, &chain.bootstrap).await.expect("init");
    let before = attest_host::read_head(&ids, &instance).await.unwrap().unwrap();
    let r = attest_host::submit_update(&ids, &instance, &chain.updates[0].1, SubmitMode::PublicProbe).await;
    match r {
        Ok(o) => panic!(
            "public Verify accepted (tx {}): expected the 10M cap to refuse it",
            o.tx
        ),
        Err(e) => {
            let text = format!("{e:#}");
            println!("B0 public Verify: refused: {text}");
            assert!(
                text.contains("cycle budget") || text.contains("reverted") || text.contains("OutOfGas"),
                "unexpected refusal: {text}"
            );
        }
    }
    let after = attest_host::read_head(&ids, &instance).await.unwrap().unwrap();
    assert_eq!(before, after, "a refused public Verify must not change the instance");
}
