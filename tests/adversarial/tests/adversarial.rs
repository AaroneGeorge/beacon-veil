//! G2: the fabricated chain and the six negative cases of RFP-021 Verification Security 1 and 2 (021:650-660),
//! each asserting an exact error code (mapping approved 2026-11-12, see docs/DECISIONS.md).
//!
//! Every test has a native leg, which always runs: the pure functions the guests execute. It may also have an
//! on-chain leg, which runs only with `BV_G2_ONCHAIN=1` against `seq-dev` (see `onchain` below).
//! Run: `source scripts/env.sh && nice -n 15 cargo test -p adversarial`.

use adversarial::*;
use alloy_primitives::{B256, U256};
use attest_core::{
    ClaimKind, EsaError, Predicate, PredicateValue, Statement, instance_id,
    lc::{self, AnyUpdate, init_store, verify_and_apply},
    rules, statement_id_state,
};
use attest_inclusion::{Claim, ClaimProof, ClaimValue, LogValue, eval, verify_claim};
use consumer_erc20_program::{ConsumerConfig, ConsumerState, accept};
use fixtures::{
    fabricate::{Keys, QUORUM, bits_first},
    merkle::{gindex, root_from_branch},
    ssz::{Header, UpdateParts},
};
use helios_consensus_core::{consensus_spec::MainnetConsensusSpec, verify_update};

use EsaError::*;

/// The attacker's storage claim over the fabricated execution state.
fn fab_storage_claim() -> Vec<u8> {
    let s = &fab().state;
    Claim {
        exec_header: None,
        proof: ClaimProof::Storage {
            address: s.address.0.0,
            slot_key: s.slot_key.0,
            account_proof: s.account_proof.clone(),
            storage_proof: s.storage_proof.clone(),
        },
    }
    .to_bytes()
}

fn fab_value_predicate() -> Predicate {
    Predicate::Eq(PredicateValue::U256(fab().state.slot_value.to_be_bytes()))
}

fn real_update(i: usize) -> UpdateParts {
    UpdateParts::from_ssz(&mainnet().updates[i]).expect("mainnet update decodes as an Electra container")
}

/// RFP-021 VS1: a self-consistent fabricated chain (fresh committee keys, valid branches, a valid inclusion proof
/// of an attacker-chosen deposit) is accepted only by an instance anchored to its own checkpoint, never by one
/// anchored to the canonical checkpoint.
#[test]
fn vs1_fabricated_chain() {
    let real = mainnet();
    let fab = fab();
    let fcfg = fab_config(0);

    // Positive control: the fabricated chain is internally valid. Helios accepts it, handoff included, when the
    // instance is anchored to the fabricated checkpoint, and the attacker's storage proof verifies under the
    // fabricated finalized header. So the anchor is the only thing that tells it apart.
    let (mut w, _) = init_store(&fcfg, &fab.bootstrap.ssz()).expect("fabricated bootstrap under its own checkpoint");
    let mut handoffs = 0;
    for (i, u) in fab.updates.iter().enumerate() {
        let out = verify_and_apply(&fcfg, &w, &u.ssz()).unwrap_or_else(|e| panic!("fabricated update {i}: {e}"));
        assert_eq!(out.record, fab.record(i), "record of fabricated update {i}");
        handoffs += usize::from(out.handoff);
        w = out.new_witness();
    }
    assert_eq!(handoffs, 1, "the fabricated chain hands off once");
    let fake_instance = instance_id(&NATIVE_ATTEST, &fcfg);
    let stmt = verify_claim(
        fake_instance,
        &fab.record(0),
        &fab_storage_claim(),
        &fab_value_predicate(),
    )
    .expect("the fabricated inclusion proof is valid under the fabricated root");

    // 1. Init: the fabricated bootstrap under the canonical checkpoint.
    expect_code(
        init_store(&real.config, &fab.bootstrap.ssz()),
        E06StaleCheckpoint,
        "vs1 init with fabricated bootstrap",
    );

    // 2. Update: the fabricated period-P update (same slots, chain id, genesis validators root and forks as mainnet,
    //    full participation) against the canonical store.
    let rw0 = store_after(&real.config, &real.bootstrap_ssz, &[]);
    expect_code(
        verify_and_apply(&real.config, &rw0, &fab.updates[0].ssz()),
        E03BadSignature,
        "vs1 fabricated update P",
    );
    expect_code(
        verify_and_apply(&real.config, &rw0, &fab.updates[2].ssz()),
        E05HandoffNotVerified,
        "vs1 fabricated finality update P+1, next committee unknown",
    );
    // After the canonical store learned committee P+1, the fabricated P+1 update (signed by fake committee P+1).
    let rw1 = store_after(&real.config, &real.bootstrap_ssz, &[&real.updates[0]]);
    expect_code(
        verify_and_apply(&real.config, &rw1, &fab.updates[1].ssz()),
        E03BadSignature,
        "vs1 fabricated update P+1",
    );
    expect_code(
        verify_and_apply(&real.config, &rw1, &fab.updates[2].ssz()),
        E03BadSignature,
        "vs1 fabricated finality update P+1",
    );

    // 3. Attest: a fabricated record is not a record of the canonical instance. No record at its slot: E02. At a
    //    slot the canonical instance did record, a record carrying the attacker's roots: E15. The inclusion proof
    //    itself verifies under the forged record, so CheckHeader is what stops it.
    let fab_rec = fab.record(0);
    expect_code(
        rules::check_header(&[], &fab_rec.hash()),
        E02NotFinalised,
        "vs1 attest on fabricated record",
    );
    let real_rec = verify_and_apply(&real.config, &rw0, &real.updates[0])
        .expect("real update 0")
        .record;
    let mut forged = real_rec.clone();
    forged.exec_state_root = fab.state.state_root.0;
    let real_instance = instance_id(&NATIVE_ATTEST, &real.config);
    verify_claim(real_instance, &forged, &fab_storage_claim(), &fab_value_predicate())
        .expect("inclusion alone accepts the forged record (021:112-125)");
    let real_state = borsh::to_vec(&real_rec).expect("borsh");
    expect_code(
        rules::check_header(&real_state, &forged.hash()),
        E15HeaderMismatch,
        "vs1 attest on forged record",
    );

    // 4. The attacker's own instance has another address, so a consumer pinned to the canonical instance refuses
    //    its statements.
    assert_ne!(
        fake_instance, real_instance,
        "fabricated checkpoint gives a different instance id"
    );
    let consumer = ConsumerState::new(ConsumerConfig {
        attest_program: NATIVE_ATTEST,
        instance: real_instance,
        chain_id: real.config.chain_id,
        token: [0xee; 20],
        recipient: [0xcc; 20],
        salt: 0,
    });
    expect_code(
        accept(&consumer, &NATIVE_ATTEST, &stmt),
        E09ChainMismatch,
        "vs1 consumer pinned to canonical instance",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let at_bootstrap =
                onchain::ensure_instance(&ids, &onchain::real_config(&ids, 343), &real.bootstrap_ssz).await;
            onchain::expect_chain(
                onchain::verify_raw(&ids, &at_bootstrap, &fab.updates[0].ssz()).await,
                E03BadSignature,
                "vs1 fabricated update P",
            );
            let never = onchain::real_config(&ids, 400);
            onchain::expect_chain(
                onchain::init_raw(&ids, &never, &never, &fab.bootstrap.ssz()).await,
                E06StaleCheckpoint,
                "vs1 init with fabricated bootstrap",
            );
            let advanced = onchain::ensure_instance(&ids, &onchain::real_config(&ids, 344), &real.bootstrap_ssz).await;
            onchain::ensure_applied(&ids, &advanced, &real.updates[0]).await;
            let slot = real_rec.slot;
            let on_chain = attest_host::read_record(&ids, &advanced, slot)
                .await
                .expect("read")
                .expect("record written");
            let mut forged = on_chain.clone();
            forged.exec_state_root = fab.state.state_root.0;
            onchain::expect_chain(
                onchain::attest_raw(&ids, &advanced, &forged, &fab_storage_claim(), &fab_value_predicate()).await,
                E15HeaderMismatch,
                "vs1 attest on forged record",
            );
            let mut absent = fab.record(0);
            absent.slot += 1;
            onchain::expect_chain(
                onchain::attest_raw(&ids, &advanced, &absent, &fab_storage_claim(), &fab_value_predicate()).await,
                E02NotFinalised,
                "vs1 attest on fabricated record",
            );
        });
    }
}

/// RFP-021 VS2: a header signed by a committee that was never validly handed off from the checkpoint.
#[test]
fn vs2a_committee_not_handed_off() {
    let real = mainnet();
    let cfg = &real.config;

    // Baseline: the canonical chain applies in order (bootstrap 1905, then 1905..1908, three handoffs).
    let all: Vec<&[u8]> = real.updates.iter().map(Vec::as_slice).collect();
    store_after(cfg, &real.bootstrap_ssz, &all);

    // 1. Next committee unknown: the canonical period-P+1 update, signed by committee P+1, which this store never
    //    learned.
    let w0 = store_after(cfg, &real.bootstrap_ssz, &[]);
    expect_code(
        verify_and_apply(cfg, &w0, &real.updates[1]),
        E05HandoffNotVerified,
        "vs2a skip the period update",
    );
    // 2. Next committee known (P+1), update signed in P+2: a period skipped.
    let w1 = store_after(cfg, &real.bootstrap_ssz, &[&real.updates[0]]);
    expect_code(
        verify_and_apply(cfg, &w1, &real.updates[2]),
        E05HandoffNotVerified,
        "vs2a skip a period",
    );

    // 3. Fabricated chain (keys known, so the signatures are valid): a period-P update signed by committee P that
    //    names a different next committee than the one already handed off.
    let fab = fab();
    let fcfg = fab_config(0);
    let fw1 = store_after(&fcfg, &fab.bootstrap.ssz(), &[&fab.updates[0].ssz()]);
    let evil = Keys::derive(b"evil committee", fab.keys[1].period);
    let p = fab.params.checkpoint_slot;
    let conflicting = fab.forge_update(p + 6 * 32, &fab.keys[0], Some(&evil));
    expect_code(
        verify_and_apply(&fcfg, &fw1, &conflicting.ssz()),
        E05HandoffNotVerified,
        "vs2a conflicting handoff",
    );
    // 4. A next committee the attested state does not commit to (branch of the real one, committee swapped).
    let fw0 = store_after(&fcfg, &fab.bootstrap.ssz(), &[]);
    let mut swapped = fab.updates[0].clone();
    swapped.next.as_mut().expect("next").0 = evil.committee.clone();
    expect_code(
        verify_and_apply(&fcfg, &fw0, &swapped.ssz()),
        E05HandoffNotVerified,
        "vs2a next committee not under state root",
    );
    // 5. The evil committee signs a period-P+1 update: it was never handed off, so its signature does not verify
    //    against the committee the store holds.
    let mut by_evil = fab.updates[1].clone();
    fixtures::fabricate::sign_with(
        &mut by_evil,
        &evil,
        &fab.params.forks,
        fab.params.genesis_validators_root,
    );
    expect_code(
        verify_and_apply(&fcfg, &fw1, &by_evil.ssz()),
        E03BadSignature,
        "vs2a signed by a committee never handed off",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let inst = onchain::ensure_instance(&ids, &onchain::real_config(&ids, 345), &real.bootstrap_ssz).await;
            onchain::expect_chain(
                onchain::verify_raw(&ids, &inst, &real.updates[1]).await,
                E05HandoffNotVerified,
                "vs2a skip the period update",
            );
        });
    }
}

/// RFP-021 VS2: a sync-committee signature below the required participation threshold.
#[test]
fn vs2b_below_participation() {
    let fab = fab();
    let fcfg = fab_config(0);
    let fw = store_after(&fcfg, &fab.bootstrap.ssz(), &[]);

    // 341 of 512 members sign: the aggregate signature is valid. Helios's verify alone accepts it (it rejects only
    // zero participants, consensus_core.rs:256); our pre-check refuses it.
    let mut low = fab.updates[0].clone();
    low.bits = bits_first(QUORUM - 1);
    fab.resign(&mut low);
    let forks = lc::forks(&fcfg).expect("forks");
    let loaded = fw.load().expect("witness");
    let AnyUpdate::Full(hu) = lc::decode_update(&low.ssz(), &forks).expect("decode") else {
        panic!("full update")
    };
    assert!(
        verify_update::<MainnetConsensusSpec>(
            &hu,
            low.signature_slot,
            &loaded.store,
            fab.params.genesis_validators_root,
            &forks
        )
        .is_ok(),
        "Helios verify accepts 341/512 with a valid signature (the gap our pre-check closes)"
    );
    expect_code(
        verify_and_apply(&fcfg, &fw, &low.ssz()),
        E04LowParticipation,
        "vs2b 341/512 valid signature",
    );

    // The boundary: 342 is accepted.
    let mut at = fab.updates[0].clone();
    at.bits = bits_first(QUORUM);
    fab.resign(&mut at);
    verify_and_apply(&fcfg, &fw, &at.ssz()).expect("342/512 meets the 2/3 rule");

    // A stricter instance (min_participants 400): 399 refused, 400 accepted.
    let strict = fab_config(400);
    let sw = store_after(&strict, &fab.bootstrap.ssz(), &[]);
    for (n, ok) in [(399, false), (400, true)] {
        let mut u = fab.updates[0].clone();
        u.bits = bits_first(n);
        fab.resign(&mut u);
        let r = verify_and_apply(&strict, &sw, &u.ssz());
        if ok {
            r.unwrap_or_else(|e| panic!("{n}/512 under min 400: {e}"));
        } else {
            expect_code(r, E04LowParticipation, "vs2b 399/512 under min_participants 400");
        }
    }

    // Canonical update with participation cut to 300: the participation check runs before the signature check,
    // so the code is E04 (not E03).
    let real = mainnet();
    let rw = store_after(&real.config, &real.bootstrap_ssz, &[]);
    let mut cut = real_update(0);
    cut.bits = bits_first(300);
    expect_code(
        verify_and_apply(&real.config, &rw, &cut.ssz()),
        E04LowParticipation,
        "vs2b canonical update at 300/512",
    );
    // Zero participants.
    cut.bits = bits_first(0);
    expect_code(
        verify_and_apply(&real.config, &rw, &cut.ssz()),
        E04LowParticipation,
        "vs2b zero participants",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let inst = onchain::ensure_instance(&ids, &onchain::real_config(&ids, 346), &real.bootstrap_ssz).await;
            let mut cut = real_update(0);
            cut.bits = bits_first(300);
            onchain::expect_chain(
                onchain::verify_raw(&ids, &inst, &cut.ssz()).await,
                E04LowParticipation,
                "vs2b canonical update at 300/512",
            );
        });
    }
}

/// RFP-021 VS2: a valid inclusion proof against a header that is not finalised.
#[test]
fn vs2c_header_not_finalised() {
    let fab = fab();
    let fcfg = fab_config(0);
    let fw = store_after(&fcfg, &fab.bootstrap.ssz(), &[]);

    // 1. Validly signed attested header, no finality (the zero header, as in an optimistic update), as a full
    //    update and as a finality update.
    let mut nofin = fab.updates[0].clone();
    nofin.finalized = Header::empty();
    nofin.finality_branch = vec![B256::ZERO; 7];
    expect_code(
        verify_and_apply(&fcfg, &fw, &nofin.ssz()),
        E02NotFinalised,
        "vs2c full update without finality",
    );
    nofin.next = None;
    expect_code(
        verify_and_apply(&fcfg, &fw, &nofin.ssz()),
        E02NotFinalised,
        "vs2c finality update without finality",
    );

    // 2. The attested (signed, not finalised) header presented as finalised.
    let mut claimed = fab.updates[0].clone();
    claimed.finalized = claimed.attested.clone();
    expect_code(
        verify_and_apply(&fcfg, &fw, &claimed.ssz()),
        E02NotFinalised,
        "vs2c attested header claimed as finalised",
    );

    // 3. Canonical data: a real update with its finality stripped, and the real optimistic update.
    let real = mainnet();
    let rw = store_after(&real.config, &real.bootstrap_ssz, &[]);
    let mut stripped = real_update(0);
    stripped.finalized = Header::empty();
    stripped.finality_branch = vec![B256::ZERO; 7];
    expect_code(
        verify_and_apply(&real.config, &rw, &stripped.ssz()),
        E02NotFinalised,
        "vs2c canonical update, finality stripped",
    );
    let opt = std::fs::read(real.dir.join(optimistic_name(&real.dir))).expect("optimistic update");
    let rw_late = store_after(
        &real.config,
        &real.bootstrap_ssz,
        &real.updates.iter().map(Vec::as_slice).collect::<Vec<_>>(),
    );
    expect_code(
        verify_and_apply(&real.config, &rw_late, &opt),
        E02NotFinalised,
        "vs2c canonical optimistic update",
    );

    // 4. Attest: a valid inclusion proof under the attested header's execution state root. That header was never
    //    finalised, so no record exists at its slot.
    let a = &fab.updates[0].attested;
    let mut rec = fab.record(0);
    rec.slot = a.beacon.slot;
    rec.beacon_root = a.root().0;
    verify_claim(
        instance_id(&NATIVE_ATTEST, &fcfg),
        &rec,
        &fab_storage_claim(),
        &fab_value_predicate(),
    )
    .expect("proof valid under the attested header's state root");
    expect_code(
        rules::check_header(&[], &rec.hash()),
        E02NotFinalised,
        "vs2c attest on a non-finalised header",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let inst = onchain::ensure_instance(&ids, &onchain::real_config(&ids, 347), &real.bootstrap_ssz).await;
            onchain::expect_chain(
                onchain::verify_raw(&ids, &inst, &stripped.ssz()).await,
                E02NotFinalised,
                "vs2c canonical update, finality stripped",
            );
            onchain::expect_chain(
                onchain::verify_raw(&ids, &inst, &opt).await,
                E02NotFinalised,
                "vs2c canonical optimistic update",
            );
        });
    }
}

fn optimistic_name(dir: &std::path::Path) -> String {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .expect("fixtures dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("optimistic_update_") && n.ends_with(".ssz"))
        .collect();
    v.sort();
    v.pop().expect("an optimistic_update_*.ssz fixture")
}

/// RFP-021 VS2: a valid finalised header with a tampered state or receipts root.
#[test]
fn vs2d_tampered_roots() {
    let real = mainnet();
    let cfg = &real.config;
    let rw = store_after(cfg, &real.bootstrap_ssz, &[]);
    let x = B256::repeat_byte(0x5a);

    // 1. Light-client update: finalized (or attested) execution roots changed under an unchanged beacon header.
    for case in [
        "finalized state_root",
        "finalized receipts_root",
        "attested state_root",
        "attested receipts_root",
    ] {
        let mut u = real_update(0);
        let e = match case {
            "finalized state_root" | "finalized receipts_root" => u.finalized.execution_mut(),
            _ => u.attested.execution_mut(),
        };
        if case.ends_with("state_root") {
            e.state_root = x;
        } else {
            e.receipts_root = x;
        }
        expect_code(
            verify_and_apply(cfg, &rw, &u.ssz()),
            E15HeaderMismatch,
            &format!("vs2d tampered {case}"),
        );
    }
    // 2. Tampered and the body root recomputed so the execution branch holds: the beacon header root moves, so the
    //    finality branch (finalized) or the committee signature (attested) breaks.
    let mut u = real_update(0);
    u.finalized.execution_mut().state_root = x;
    u.finalized.beacon.body_root = root_from_branch(
        u.finalized.execution_root(),
        &u.finalized.execution_branch,
        gindex::EXECUTION_PAYLOAD,
    );
    expect_code(
        verify_and_apply(cfg, &rw, &u.ssz()),
        E02NotFinalised,
        "vs2d finalized root tampered, body root recomputed",
    );
    let mut u = real_update(0);
    u.attested.execution_mut().state_root = x;
    u.attested.beacon.body_root = root_from_branch(
        u.attested.execution_root(),
        &u.attested.execution_branch,
        gindex::EXECUTION_PAYLOAD,
    );
    expect_code(
        verify_and_apply(cfg, &rw, &u.ssz()),
        E03BadSignature,
        "vs2d attested root tampered, body root recomputed",
    );

    // 3. Attest: the canonical record at that slot, with a tampered state or receipts root. The attacker's proof is
    //    valid under the tampered state root; CheckHeader refuses the record.
    let rec = verify_and_apply(cfg, &rw, &real.updates[0])
        .expect("real update 0")
        .record;
    let stored = borsh::to_vec(&rec).expect("borsh");
    rules::check_header(&stored, &rec.hash()).expect("the untampered record passes");
    let mut t = rec.clone();
    t.exec_state_root = fab().state.state_root.0;
    verify_claim(
        instance_id(&NATIVE_ATTEST, cfg),
        &t,
        &fab_storage_claim(),
        &fab_value_predicate(),
    )
    .expect("proof valid under the tampered state root");
    expect_code(
        rules::check_header(&stored, &t.hash()),
        E15HeaderMismatch,
        "vs2d record with tampered state root",
    );
    let mut t = rec.clone();
    t.exec_receipts_root = x.0;
    expect_code(
        rules::check_header(&stored, &t.hash()),
        E15HeaderMismatch,
        "vs2d record with tampered receipts root",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let inst = onchain::ensure_instance(&ids, &onchain::real_config(&ids, 348), &real.bootstrap_ssz).await;
            let mut u = real_update(0);
            u.finalized.execution_mut().state_root = x;
            onchain::expect_chain(
                onchain::verify_raw(&ids, &inst, &u.ssz()).await,
                E15HeaderMismatch,
                "vs2d tampered finalized state_root",
            );
            let adv = onchain::ensure_instance(&ids, &onchain::real_config(&ids, 349), &real.bootstrap_ssz).await;
            onchain::ensure_applied(&ids, &adv, &real.updates[0]).await;
            let on_chain = attest_host::read_record(&ids, &adv, rec.slot)
                .await
                .expect("read")
                .expect("record written");
            let mut t = on_chain.clone();
            t.exec_state_root = fab().state.state_root.0;
            onchain::expect_chain(
                onchain::attest_raw(&ids, &adv, &t, &fab_storage_claim(), &fab_value_predicate()).await,
                E15HeaderMismatch,
                "vs2d record with tampered state root",
            );
        });
    }
}

/// RFP-021 VS2: a proof replayed against a different chain ID.
#[test]
fn vs2e_wrong_chain_id() {
    let real = mainnet();
    let cfg = &real.config;
    const SEPOLIA: u64 = 11_155_111;

    // 1. Apply: a header record carrying another chain id (attest program, rules::apply).
    let instance = instance_id(&NATIVE_ATTEST, cfg);
    let (st, _) = rules::init(&NATIVE_ATTEST, &instance, &[], cfg, &real.bootstrap_ssz).expect("init");
    let state = borsh::to_vec(&st).expect("borsh");
    let rw = store_after(cfg, &real.bootstrap_ssz, &[]);
    let out = verify_and_apply(cfg, &rw, &real.updates[0]).expect("real update 0");
    rules::apply(
        &state,
        Some(NATIVE_VERIFIER),
        out.pre_store,
        out.post_store,
        out.head.clone(),
        out.record.clone(),
    )
    .expect("the canonical Apply passes");
    let mut other = out.record.clone();
    other.chain_id = SEPOLIA;
    expect_code(
        rules::apply(
            &state,
            Some(NATIVE_VERIFIER),
            out.pre_store,
            out.post_store,
            out.head.clone(),
            other,
        ),
        E09ChainMismatch,
        "vs2e Apply with a record of another chain",
    );

    // 2. Init: the same checkpoint configured for another chain, sent to this chain's instance address.
    let mut cfg_b = cfg.clone();
    cfg_b.chain_id = SEPOLIA;
    expect_code(
        rules::check_init_target(&NATIVE_ATTEST, &instance, &[], &cfg_b),
        E09ChainMismatch,
        "vs2e Init for another chain",
    );

    // 3. Consumer: a statement replayed from another chain (or another instance) is refused; statement ids bind
    //    the chain id, so the replay is also a different statement.
    let base = Statement {
        instance,
        chain_id: cfg.chain_id,
        slot: out.record.slot,
        exec_number: out.record.exec_number,
        kind: ClaimKind::Log,
        subject: vec![0; 36],
        value: vec![],
        predicate_hash: [0; 32],
        id: [1; 32],
    };
    let consumer = ConsumerState::new(ConsumerConfig {
        attest_program: NATIVE_ATTEST,
        instance,
        chain_id: cfg.chain_id,
        token: [0xee; 20],
        recipient: [0xcc; 20],
        salt: 0,
    });
    let mut replay = base.clone();
    replay.chain_id = SEPOLIA;
    expect_code(
        accept(&consumer, &NATIVE_ATTEST, &replay),
        E09ChainMismatch,
        "vs2e statement from another chain",
    );
    let mut replay = base;
    replay.instance = instance_id(&NATIVE_ATTEST, &sepolia().config);
    expect_code(
        accept(&consumer, &NATIVE_ATTEST, &replay),
        E09ChainMismatch,
        "vs2e statement from another instance",
    );
    let (a, k, v) = ([0xab; 20], [0u8; 32], [7u8; 32]);
    assert_ne!(
        statement_id_state(1, &a, &k, &v),
        statement_id_state(SEPOLIA, &a, &k, &v),
        "statement id binds chain id"
    );

    // 4. A light-client update signed for one chain, replayed to an instance of another chain with the same
    //    checkpoint and committee: the signing domain (genesis validators root) differs.
    let fab = fab();
    let mut fcfg_b = fab_config(0);
    fcfg_b.chain_id = SEPOLIA;
    fcfg_b.genesis_validators_root = [0x99; 32];
    let fw_b = store_after(&fcfg_b, &fab.bootstrap.ssz(), &[]);
    expect_code(
        verify_and_apply(&fcfg_b, &fw_b, &fab.updates[0].ssz()),
        E03BadSignature,
        "vs2e update replayed to another chain",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let a = onchain::real_config(&ids, 401);
            let mut b = a.clone();
            b.chain_id = SEPOLIA;
            onchain::expect_chain(
                onchain::init_raw(&ids, &a, &b, &real.bootstrap_ssz).await,
                E09ChainMismatch,
                "vs2e Init for another chain",
            );
            let mut fb = onchain::fab_config(&ids, 351);
            fb.chain_id = SEPOLIA;
            fb.genesis_validators_root = [0x99; 32];
            let inst = onchain::ensure_instance(&ids, &fb, &fab.bootstrap.ssz()).await;
            onchain::expect_chain(
                onchain::verify_raw(&ids, &inst, &fab.updates[0].ssz()).await,
                E03BadSignature,
                "vs2e update replayed to another chain",
            );
        });
    }
}

/// RFP-021 VS2: a predicate asserted over state the inclusion proof does not cover.
#[test]
fn vs2f_predicate_not_covered() {
    let fab = fab();
    let fcfg = fab_config(0);
    let inst = instance_id(&NATIVE_ATTEST, &fcfg);
    let rec = fab.record(0);
    let claim = fab_storage_claim();
    let w = |b: u8| [b; 32];

    // Controls: the storage claim proves slot_key = slot_value. A true predicate passes; a false one is E08.
    verify_claim(inst, &rec, &claim, &fab_value_predicate()).expect("true predicate over the proven slot");
    expect_code(
        verify_claim(
            inst,
            &rec,
            &claim,
            &Predicate::Eq(PredicateValue::U256(U256::from(1u64).to_be_bytes())),
        ),
        E08PredicateFalse,
        "vs2f control: false predicate",
    );

    // A storage proof covers one word. Log predicates assert over a log it does not carry.
    for (case, p) in [
        (
            "log topic over a storage proof",
            Predicate::LogTopicEq { i: 0, v: w(1) },
        ),
        (
            "log data word over a storage proof",
            Predicate::LogDataWordEq { offset: 0, v: w(1) },
        ),
        ("ordering over bytes32", Predicate::Lt(PredicateValue::Bytes32(w(9)))),
        (
            "compound with one uncovered term",
            Predicate::All(vec![fab_value_predicate(), Predicate::LogTopicEq { i: 0, v: w(1) }]),
        ),
    ] {
        expect_code(
            verify_claim(inst, &rec, &claim, &p),
            E10PredicateNotCovered,
            &format!("vs2f {case}"),
        );
    }

    // A log covers its topics and data only.
    let log = ClaimValue::Log(LogValue {
        address: [0xee; 20],
        topics: vec![w(1), w(2), w(3)],
        data: vec![0; 32],
    });
    eval(&Predicate::LogTopicEq { i: 2, v: w(3) }, &log).expect("topic 2 is covered");
    expect_code(
        eval(&Predicate::LogTopicEq { i: 3, v: w(3) }, &log),
        E10PredicateNotCovered,
        "vs2f topic beyond the log",
    );
    expect_code(
        eval(&Predicate::LogDataWordEq { offset: 1, v: w(0) }, &log),
        E10PredicateNotCovered,
        "vs2f data word beyond the log",
    );
    expect_code(
        eval(&Predicate::Eq(PredicateValue::U256(w(0))), &log),
        E10PredicateNotCovered,
        "vs2f word predicate over a log",
    );

    // The ERC-20 consumer accepts only log statements: a storage statement asserts nothing it can check.
    let stmt = verify_claim(inst, &rec, &claim, &fab_value_predicate()).expect("statement");
    let consumer = ConsumerState::new(ConsumerConfig {
        attest_program: NATIVE_ATTEST,
        instance: inst,
        chain_id: fcfg.chain_id,
        token: [0xee; 20],
        recipient: [0xcc; 20],
        salt: 0,
    });
    expect_code(
        accept(&consumer, &NATIVE_ATTEST, &stmt),
        E10PredicateNotCovered,
        "vs2f storage statement to a log consumer",
    );

    if onchain() {
        let ids = onchain::deployment();
        onchain::block(async {
            let inst = onchain::ensure_instance(&ids, &onchain::fab_config(&ids, 353), &fab.bootstrap.ssz()).await;
            onchain::ensure_applied(&ids, &inst, &fab.updates[0].ssz()).await;
            let on_chain = attest_host::read_record(&ids, &inst, rec.slot)
                .await
                .expect("read")
                .expect("record written");
            onchain::expect_chain(
                onchain::attest_raw(&ids, &inst, &on_chain, &claim, &Predicate::LogTopicEq { i: 0, v: w(1) }).await,
                E10PredicateNotCovered,
                "vs2f log topic over a storage proof",
            );
        });
    }
}

/// On-chain legs: the same inputs through `attest_host` on `seq-dev` (`BV_G2_ONCHAIN=1`).
mod onchain;
