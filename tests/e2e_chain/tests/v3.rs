//! V3 (`attest_core::lc_v3`, the staged public-path verifier) against V1 (Helios 0.12.0 unmodified), natively, on
//! the real mainnet and Sepolia (Gloas) fixture chains and on the fabricated chain; and the step logic of the
//! `lc_verify_v3` program (`lc_v3::handle`, which the program wraps) together with the `attest` rules (`Apply`).
//!
//!   cargo nextest run -p e2e_chain --features v3

#![cfg(feature = "v3")]

use attest_core::{
    AccountId, EsaError, InstanceConfig, InstanceState, LcVerifyV3Message, instance_id,
    lc::{self, StoreWitness},
    lc_v3::{self, Outcome, StoreWitnessV3, stage},
    rules,
};

const ATTEST: AccountId = AccountId::new([0xA7; 32]);
const V3: AccountId = AccountId::new([0x33; 32]);

type Out = (
    attest_core::Bytes32,
    attest_core::Bytes32,
    attest_core::Head,
    attest_core::HeaderRecord,
);

fn sig_slot(config: &InstanceConfig, u: &[u8]) -> u64 {
    lc::forks(config)
        .ok()
        .and_then(|f| lc::decode_update(u, &f).ok())
        .map_or(0, |x| x.signature_slot())
}

fn v1(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> Result<Out, EsaError> {
    lc::verify_and_apply(config, w, u).map(|o| (o.pre_store, o.post_store, o.head, o.record))
}

fn of(o: Outcome) -> Out {
    (o.pre_store, o.post_store, o.head, o.record)
}

fn witness(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> (StoreWitnessV3, Vec<[u8; 96]>) {
    let w3 = StoreWitnessV3::from_v1(w, sig_slot(config, u));
    let keys = lc_v3::absent_keys(&w3.committee_ssz, u);
    (w3, keys)
}

fn once(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> Result<Out, EsaError> {
    let (w3, keys) = witness(config, w, u);
    lc_v3::verify_once(config, &w3, u, &keys).map(of)
}

/// The job account the program would be called at.
fn job_account(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> AccountId {
    let w3 = StoreWitnessV3::from_v1(w, sig_slot(config, u));
    let pre = w3.commitment().expect("commitment");
    lc_v3::job_id(&V3, &lc_v3::job_seed(&instance_id(&ATTEST, config), &pre, u))
}

fn begin_msg(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> LcVerifyV3Message {
    let (witness, absent_keys) = witness(config, w, u);
    LcVerifyV3Message::Begin {
        attest_program: ATTEST,
        instance: instance_id(&ATTEST, config),
        config: config.clone(),
        witness,
        update_ssz: u.to_vec(),
        absent_keys,
    }
}

/// Runs `msgs` in order on one job account, as consecutive transactions; returns the job state and the outcome of
/// `Finish`.
fn run_steps(job: AccountId, msgs: Vec<LcVerifyV3Message>) -> Result<(Vec<u8>, Option<Outcome>), EsaError> {
    let mut state = Vec::new();
    let mut out = None;
    for m in msgs {
        let e = lc_v3::handle(&V3, &job, &state, m)?;
        if let Some(s) = e.state {
            state = s;
        }
        if let Some((_, _, o)) = e.apply {
            out = Some(o);
        }
    }
    Ok((state, out))
}

fn staged(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> Result<Out, EsaError> {
    let job = job_account(config, w, u);
    let (_, out) = run_steps(
        job,
        vec![
            begin_msg(config, w, u),
            LcVerifyV3Message::Miller,
            LcVerifyV3Message::Finish,
        ],
    )?;
    Ok(of(out.expect("Finish sends Apply")))
}

fn four_steps(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> Result<Out, EsaError> {
    let job = job_account(config, w, u);
    let (w3, absent_keys) = witness(config, w, u);
    let (_, out) = run_steps(
        job,
        vec![
            LcVerifyV3Message::BeginPrepare {
                attest_program: ATTEST,
                instance: instance_id(&ATTEST, config),
                config: config.clone(),
                witness: w3.clone(),
                update_ssz: u.to_vec(),
            },
            LcVerifyV3Message::BeginKeys {
                committee_ssz: w3.committee_ssz.clone(),
                absent_keys,
            },
            LcVerifyV3Message::Miller,
            LcVerifyV3Message::Finish,
        ],
    )?;
    Ok(of(out.expect("Finish sends Apply")))
}

fn chains() -> Vec<(&'static str, e2e_chain::FixtureChain)> {
    vec![
        ("mainnet", e2e_chain::mainnet_chain(vec![V3]).expect("mainnet fixtures")),
        ("sepolia", e2e_chain::sepolia_chain(vec![V3]).expect("sepolia fixtures")),
    ]
}

/// Replaces the 96 signature bytes of `u` with `sig`.
fn with_signature(config: &InstanceConfig, u: &[u8], sig: &[u8]) -> Vec<u8> {
    let own = signature_of(config, u);
    let at = u
        .windows(96)
        .position(|w| w == own.as_slice())
        .expect("signature bytes");
    let mut t = u.to_vec();
    t[at..at + 96].copy_from_slice(sig);
    t
}

fn signature_of(config: &InstanceConfig, u: &[u8]) -> Vec<u8> {
    use ssz::Encode;
    let f = lc::forks(config).expect("forks");
    match lc::decode_update(u, &f).expect("decode") {
        lc::AnyUpdate::Full(x) => x.sync_aggregate().sync_committee_signature.as_ssz_bytes(),
        lc::AnyUpdate::Finality(x) => x.sync_aggregate().sync_committee_signature.as_ssz_bytes(),
    }
}

#[test]
fn v3_equals_v1_on_real_chains() {
    let mut accepted = 0;
    let mut refused = 0;
    for (net, chain) in chains() {
        let (mut w, _) = lc::init_store(&chain.config, &chain.bootstrap).expect("init");
        for (label, u) in &chain.updates {
            let want = v1(&chain.config, &w, u);
            assert!(want.is_ok(), "{net} {label}: V1 refused {want:?}");
            assert_eq!(once(&chain.config, &w, u), want, "{net} {label} verify_once");
            assert_eq!(staged(&chain.config, &w, u), want, "{net} {label} staged");
            accepted += 1;
            // Tampered copies (as v1_v2): the same verdict and, on accept, the same outcome.
            for frac in [1u64, 7, 13, 29, 41, 53, 67, 71, 83, 97, 99] {
                let pos = (u.len() as u64 * frac / 100) as usize;
                for bit in [0u8, 3, 7] {
                    let mut t = u.clone();
                    t[pos] ^= 1 << bit;
                    let a = v1(&chain.config, &w, &t);
                    assert_eq!(once(&chain.config, &w, &t), a, "{net} {label} byte {pos} bit {bit}");
                    refused += usize::from(a.is_err());
                }
            }
            w = lc::verify_and_apply(&chain.config, &w, u).expect("v1").new_witness();
        }
    }
    println!("accepted by both: {accepted}; tampered copies refused by both with the same code: {refused}");
    assert_eq!(accepted, 10);
    assert!(refused > 200);
}

#[test]
fn v3_verify_once_equals_staged() {
    for (net, chain) in chains() {
        let (mut w, _) = lc::init_store(&chain.config, &chain.bootstrap).expect("init");
        for (label, u) in &chain.updates {
            let a = once(&chain.config, &w, u).expect("verify_once");
            assert_eq!(
                staged(&chain.config, &w, u).as_ref(),
                Ok(&a),
                "{net} {label} three steps"
            );
            assert_eq!(
                four_steps(&chain.config, &w, u).as_ref(),
                Ok(&a),
                "{net} {label} four steps"
            );
            // VerifyOnce through the program logic (private actor: any receiver, state kept).
            let (witness, absent_keys) = witness(&chain.config, &w, u);
            let e = lc_v3::handle(
                &V3,
                &AccountId::new([0x99; 32]),
                &[],
                LcVerifyV3Message::VerifyOnce {
                    attest_program: ATTEST,
                    instance: instance_id(&ATTEST, &chain.config),
                    config: chain.config.clone(),
                    witness,
                    update_ssz: u.clone(),
                    absent_keys,
                },
            )
            .expect("VerifyOnce");
            assert!(e.state.is_none());
            assert_eq!(
                e.apply.map(|(_, _, o)| of(o)),
                Some(a),
                "{net} {label} VerifyOnce message"
            );
            w = lc::verify_and_apply(&chain.config, &w, u).expect("v1").new_witness();
        }
    }
}

#[test]
fn v3_stage_order_enforced() {
    let chain = e2e_chain::mainnet_chain(vec![V3]).expect("fixtures");
    let (w, _) = lc::init_store(&chain.config, &chain.bootstrap).expect("init");
    let u = &chain.updates[0].1;
    let job = job_account(&chain.config, &w, u);
    let step = |state: &[u8], m: LcVerifyV3Message| lc_v3::handle(&V3, &job, state, m);
    let code = |r: Result<lc_v3::StepEffect, EsaError>| r.err();

    // Nothing before Begin.
    assert_eq!(code(step(&[], LcVerifyV3Message::Miller)), Some(EsaError::E01Malformed));
    assert_eq!(code(step(&[], LcVerifyV3Message::Finish)), Some(EsaError::E01Malformed));
    // Begin at an account that is not the job's PDA.
    assert_eq!(
        code(lc_v3::handle(
            &V3,
            &AccountId::new([5; 32]),
            &[],
            begin_msg(&chain.config, &w, u)
        )),
        Some(EsaError::E09ChainMismatch)
    );
    let s1 = step(&[], begin_msg(&chain.config, &w, u))
        .expect("begin")
        .state
        .expect("state");
    assert_eq!(borsh::from_slice::<lc_v3::JobV3>(&s1).expect("job").stage, stage::BEGUN);
    assert_eq!(
        code(step(&s1, LcVerifyV3Message::Finish)),
        Some(EsaError::E01Malformed),
        "Finish before Miller"
    );
    assert_eq!(
        code(step(
            &s1,
            LcVerifyV3Message::BeginKeys {
                committee_ssz: w.current_committee_ssz.clone(),
                absent_keys: vec![]
            }
        )),
        Some(EsaError::E01Malformed),
        "BeginKeys after Begin"
    );
    let s2 = step(&s1, LcVerifyV3Message::Miller)
        .expect("miller")
        .state
        .expect("state");
    assert_eq!(
        code(step(&s2, LcVerifyV3Message::Miller)),
        Some(EsaError::E01Malformed),
        "Miller twice"
    );
    let fin = step(&s2, LcVerifyV3Message::Finish).expect("finish");
    assert!(fin.apply.is_some());
    let s3 = fin.state.expect("state");
    assert_eq!(
        code(step(&s3, LcVerifyV3Message::Miller)),
        Some(EsaError::E14AlreadyApplied),
        "Miller on a done job"
    );
    assert_eq!(
        code(step(&s3, LcVerifyV3Message::Finish)),
        Some(EsaError::E14AlreadyApplied),
        "Finish twice"
    );
    // A malformed job state.
    assert_eq!(
        code(step(&[1, 2, 3], LcVerifyV3Message::Miller)),
        Some(EsaError::E01Malformed)
    );
}

#[test]
fn v3_duplicate_refused() {
    let chain = e2e_chain::mainnet_chain(vec![V3]).expect("fixtures");
    let config = &chain.config;
    let instance = instance_id(&ATTEST, config);
    let (st, _) = rules::init(&ATTEST, &instance, &[], config, &chain.bootstrap).expect("init");
    let (w, _) = lc::init_store(config, &chain.bootstrap).expect("init store");
    let u = &chain.updates[0].1;
    let job = job_account(config, &w, u);
    let s1 = lc_v3::handle(&V3, &job, &[], begin_msg(config, &w, u))
        .expect("begin")
        .state
        .expect("state");
    // Begin again on the same job account: E14.
    assert_eq!(
        lc_v3::handle(&V3, &job, &s1, begin_msg(config, &w, u)).err(),
        Some(EsaError::E14AlreadyApplied)
    );
    // Finish the job and apply it.
    let (_, out) = run_steps(
        job,
        vec![
            begin_msg(config, &w, u),
            LcVerifyV3Message::Miller,
            LcVerifyV3Message::Finish,
        ],
    )
    .expect("steps");
    let o = out.expect("outcome");
    let st_bytes = borsh::to_vec(&st).expect("borsh");
    let applied = rules::apply(
        &st_bytes,
        Some(V3),
        o.pre_store,
        o.post_store,
        o.head.clone(),
        o.record.clone(),
    )
    .expect("apply");
    let st2 = borsh::to_vec(&applied.new_state).expect("borsh");
    // The same outcome again (a second job of the same update finishing): attest refuses E14.
    assert_eq!(
        rules::apply(&st2, Some(V3), o.pre_store, o.post_store, o.head, o.record).err(),
        Some(EsaError::E14AlreadyApplied)
    );
    // The same update on the moved store: refused before any key work (E14), as V1.
    let w2 = lc::verify_and_apply(config, &w, u).expect("v1").new_witness();
    assert_eq!(once(config, &w2, u).err(), Some(EsaError::E14AlreadyApplied));
    assert_eq!(v1(config, &w2, u).err(), Some(EsaError::E14AlreadyApplied));
}

#[test]
fn v3_stale_job_refused() {
    let chain = e2e_chain::mainnet_chain(vec![V3]).expect("fixtures");
    let config = &chain.config;
    let instance = instance_id(&ATTEST, config);
    let (st0, _) = rules::init(&ATTEST, &instance, &[], config, &chain.bootstrap).expect("init");
    let (w0, _) = lc::init_store(config, &chain.bootstrap).expect("init store");
    let apply = |st: &InstanceState, o: Outcome| {
        rules::apply(
            &borsh::to_vec(st).expect("borsh"),
            Some(V3),
            o.pre_store,
            o.post_store,
            o.head,
            o.record,
        )
    };
    let staged_out = |w: &StoreWitness, u: &[u8]| {
        let job = job_account(config, w, u);
        run_steps(
            job,
            vec![
                begin_msg(config, w, u),
                LcVerifyV3Message::Miller,
                LcVerifyV3Message::Finish,
            ],
        )
        .expect("steps")
        .1
        .expect("outcome")
    };
    let (u0, u1, u2) = (&chain.updates[0].1, &chain.updates[1].1, &chain.updates[2].1);
    // Store after update[0] (store 1).
    let st1 = apply(&st0, staged_out(&w0, u0)).expect("apply u0").new_state;
    let w1 = lc::verify_and_apply(config, &w0, u0).expect("v1").new_witness();
    // A job for update[1] is built on store 1 ...
    let stale = staged_out(&w1, u1);
    // ... but update[1] and update[2] land first (store 3).
    let st2 = apply(&st1, staged_out(&w1, u1)).expect("apply u1").new_state;
    let w2 = lc::verify_and_apply(config, &w1, u1).expect("v1").new_witness();
    let st3 = apply(&st2, staged_out(&w2, u2)).expect("apply u2").new_state;
    // The stale job's Apply: its pre-store is store 1, the instance is at store 3: E13, and the head does not move.
    assert_eq!(apply(&st3, stale.clone()).err(), Some(EsaError::E13StoreMismatch));
    assert_eq!(
        st3.head.slot,
        lc::verify_and_apply(config, &w2, u2).expect("v1").head.slot
    );
    // Right after store 2 the same job reads as already applied (its post-store is the current one): E14.
    assert_eq!(apply(&st2, stale).err(), Some(EsaError::E14AlreadyApplied));
}

#[test]
fn v3_forged_signature_refused_at_finish() {
    let chain = e2e_chain::mainnet_chain(vec![V3]).expect("fixtures");
    let config = &chain.config;
    let (w, _) = lc::init_store(config, &chain.bootstrap).expect("init");
    // update[0] carrying update[1]'s signature: a valid G2 point in the subgroup, over another message.
    let other = signature_of(config, &chain.updates[1].1);
    let forged = with_signature(config, &chain.updates[0].1, &other);
    assert_eq!(v1(config, &w, &forged).err(), Some(EsaError::E03BadSignature));
    let job = job_account(config, &w, &forged);
    let s1 = lc_v3::handle(&V3, &job, &[], begin_msg(config, &w, &forged))
        .expect("Begin accepts: the signature decodes")
        .state
        .expect("state");
    let s2 = lc_v3::handle(&V3, &job, &s1, LcVerifyV3Message::Miller)
        .expect("Miller accepts")
        .state
        .expect("state");
    assert_eq!(
        lc_v3::handle(&V3, &job, &s2, LcVerifyV3Message::Finish).err(),
        Some(EsaError::E03BadSignature)
    );
    assert_eq!(once(config, &w, &forged).err(), Some(EsaError::E03BadSignature));
}

#[test]
fn v3_identity_key_refused() {
    use fixtures::fabricate::{FabChain, FabParams};
    let real = e2e_chain::mainnet_chain(vec![V3]).expect("fixtures");
    let chain = FabChain::build(FabParams::impersonating(&real.config, b"beacon-veil/v3/identity"));
    let config = chain.config(vec![V3], 0);
    let (w0, _) = lc::init_store(&config, &chain.bootstrap.ssz()).expect("fabricated bootstrap");
    // Committee member 5 is the identity; the aggregate key is the sum of the other 511; 511 sign.
    let idx = 5usize;
    let mut committee = w0.current_committee_ssz.clone();
    let key = |c: &[u8], i: usize| -> [u8; 48] { c[i * 48..i * 48 + 48].try_into().expect("48") };
    let mut sum: Option<bls_blst::G1Sum> = None;
    for i in (0..512).filter(|i| *i != idx) {
        let p = bls_blst::G1::from_compressed(&key(&committee, i)).expect("fabricated key");
        match sum.as_mut() {
            Some(s) => s.add(&p),
            None => sum = Some(bls_blst::G1Sum::new(&p)),
        }
    }
    committee[idx * 48..idx * 48 + 48].copy_from_slice(&lc_v3::IDENTITY_G1);
    committee[512 * 48..].copy_from_slice(&sum.expect("sum").to_affine().compressed());
    let mut wi = w0.clone();
    wi.current_committee_ssz = committee;
    let mut u = chain.updates[0].clone();
    u.bits = (0..512).map(|i| i != idx).collect();
    chain.resign(&mut u);
    // Absent identity: both accept, same outcome.
    assert!(v1(&config, &wi, &u.ssz()).is_ok());
    assert_eq!(once(&config, &wi, &u.ssz()), v1(&config, &wi, &u.ssz()));
    // Counted identity: Helios (V1) accepts, V3 refuses.
    u.bits[idx] = true;
    assert!(
        v1(&config, &wi, &u.ssz()).is_ok(),
        "V1 counts the identity key as a signer"
    );
    assert_eq!(once(&config, &wi, &u.ssz()).err(), Some(EsaError::E03BadSignature));
}

#[test]
fn v3_witness_round_trips_and_binds() {
    for (net, chain) in chains() {
        let (mut w, _) = lc::init_store(&chain.config, &chain.bootstrap).expect("init");
        for (label, u) in &chain.updates {
            let w3 = StoreWitnessV3::from_v1(&w, sig_slot(&chain.config, u));
            let v1_commitment = w.load().expect("load").commitment();
            assert_eq!(w3.commitment(), Ok(v1_commitment), "{net} {label}");
            assert_eq!(
                lc_v3::committee_root_fast(&w.current_committee_ssz),
                lc::committee_root_ssz(&w.current_committee_ssz)
            );
            // A witness claiming the wrong slot for its committee is refused.
            let mut wrong = w3.clone();
            wrong.signing = match w3.signing {
                lc_v3::CommitteeSlot::Current => lc_v3::CommitteeSlot::Next,
                lc_v3::CommitteeSlot::Next => lc_v3::CommitteeSlot::Current,
            };
            let keys = lc_v3::absent_keys(&w3.committee_ssz, u);
            assert!(
                lc_v3::verify_once(&chain.config, &wrong, u, &keys).is_err(),
                "{net} {label}"
            );
            w = lc::verify_and_apply(&chain.config, &w, u).expect("v1").new_witness();
        }
    }
}

#[test]
fn v3_infinity_signature_refused_at_miller() {
    let chain = e2e_chain::mainnet_chain(vec![V3]).expect("fixtures");
    let config = &chain.config;
    let (w, _) = lc::init_store(config, &chain.bootstrap).expect("init");
    let mut inf = [0u8; 96];
    inf[0] = 0xc0;
    let forged = with_signature(config, &chain.updates[0].1, &inf);
    assert_eq!(v1(config, &w, &forged).err(), Some(EsaError::E03BadSignature));
    let job = job_account(config, &w, &forged);
    let s1 = lc_v3::handle(&V3, &job, &[], begin_msg(config, &w, &forged))
        .expect("Begin accepts: it does not decode the signature")
        .state
        .expect("state");
    assert_eq!(
        lc_v3::handle(&V3, &job, &s1, LcVerifyV3Message::Miller).err(),
        Some(EsaError::E03BadSignature),
        "the identity signature is refused at Miller, before the Miller loop"
    );
    assert_eq!(once(config, &w, &forged).err(), Some(EsaError::E03BadSignature));
}

/// A replayed update whose signature is also wrong: both refuse. V1 reports the signature (E03: Helios decodes the
/// signature as part of the update); V3 reports the replay (E14: it checks replay before it decodes the signature, in
/// Miller). A designed difference, listed as such by the differential test.
#[test]
fn v3_replay_with_bad_signature() {
    for (net, chain) in chains() {
        let (mut w, _) = lc::init_store(&chain.config, &chain.bootstrap).expect("init");
        for (label, u) in &chain.updates {
            let next = lc::verify_and_apply(&chain.config, &w, u).expect("v1").new_witness();
            let mut t = u.clone();
            let own = signature_of(&chain.config, u);
            let at = t.windows(96).position(|x| x == own.as_slice()).expect("sig");
            t[at + 95] ^= 1;
            let a = v1(&chain.config, &next, &t).err();
            let b = once(&chain.config, &next, &t).err();
            println!("{net} {label}: replay with a changed signature byte: V1 {a:?}, V3 {b:?}");
            assert_eq!(
                (a, b),
                (Some(EsaError::E03BadSignature), Some(EsaError::E14AlreadyApplied)),
                "{net} {label}"
            );
            w = next;
        }
    }
}
