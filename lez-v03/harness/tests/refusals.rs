//! Refusals through LEZ v0.3.2. Each one exits with its `EsaError` code (never a panic), is charged less than the
//! step limit, and changes no shard of the transaction's accounts (the job, the instance, the record).
//!
//! Sequence: out-of-order steps (`Miller` before `Begin`, `Finish` before `Miller`); a duplicate `Finish`; `Begin`
//! again; `Finish` on a moved store; a top-level `Apply` (no caller); `Apply` from a program not in
//! `verifiers`; participation below 342; one signature byte changed; and every claim of the instruction changed in
//! turn (window start and end, each `Apply` field, `writes_record`, the `Init` record).

use attest_core::{EsaError, lc, lc_v3};
use attest_v03::core_id;
use fixtures::fabricate::{FabChain, FabParams, bits_first};
use lc_verify_v3_v03::{Claims, Instruction};
use lee_core::account::{AccountId, ProgramShardSelector};
use v03_harness::{
    fixtures as fx, guests, run,
    steps::{self, Ids, Tx},
    world::{STEP_LIMIT, World},
};

fn snapshot(w: &World, tx: &Tx) -> Vec<Vec<u8>> {
    tx.selectors
        .iter()
        .map(|s| w.shard(s.account_id, s.program_account_id))
        .collect()
}

/// `tx` must be refused with `code`, by exit code, charged under the limit, changing nothing.
fn refused(w: &mut World, tx: &Tx, ts: u64, code: EsaError, what: &str) {
    let before = snapshot(w, tx);
    let o = w.exec(tx, ts, STEP_LIMIT);
    assert!(o.result.is_err(), "{what}: applied");
    assert_eq!(o.exit_code, Some(u32::from(code as u8)), "{what}: {:?}", o.result);
    assert!(o.cycles < STEP_LIMIT, "{what}: charged {} cycles", o.cycles);
    assert_eq!(snapshot(w, tx), before, "{what}: a shard changed");
    println!("{what}: refused {} with {} cycles", code.code(), o.cycles);
}

fn ok(w: &mut World, tx: &Tx, ts: u64, what: &str) {
    let o = w.exec(tx, ts, STEP_LIMIT);
    assert_eq!(o.result, Ok(()), "{what}");
}

fn ts(tx: &Tx) -> u64 {
    tx.window_ms.map_or(0, |(from, _)| from)
}

fn with_claims(tx: &Tx, f: impl FnOnce(&mut Claims)) -> Tx {
    let mut ix: Instruction = borsh::from_slice(&tx.instruction).unwrap();
    f(&mut ix.claims);
    Tx {
        instruction: borsh::to_vec(&ix).unwrap(),
        ..tx.clone()
    }
}

#[test]
fn refusals() {
    let ids = run::IDS;
    let chain = fx::sepolia(vec![core_id(&ids.v3)]).unwrap();
    let mut w = run::world().unwrap();
    let (init, inst_state, _) = steps::init(&ids, &chain.config, &chain.bootstrap).unwrap();

    // Init claiming a different record: E15.
    let mut bad: attest_v03::Instruction = borsh::from_slice(&init.instruction).unwrap();
    if let attest_v03::Instruction::Init { record, .. } = &mut bad {
        record.exec_number += 1;
    }
    refused(
        &mut w,
        &Tx {
            instruction: borsh::to_vec(&bad).unwrap(),
            ..init.clone()
        },
        0,
        EsaError::E15HeaderMismatch,
        "Init with a wrong record claim",
    );
    ok(&mut w, &init, 0, "Init");
    refused(&mut w, &init, 0, EsaError::E14AlreadyApplied, "Init again");

    let (w0, _) = lc::init_store(&chain.config, &chain.bootstrap).unwrap();
    let (_, u0) = &chain.updates[0];
    let up = steps::update(&ids, &chain.config, &w0, &inst_state, u0).unwrap();
    let [begin, miller, finish] = [&up.txs[0], &up.txs[1], &up.txs[2]];

    refused(&mut w, miller, 0, EsaError::E01Malformed, "Miller before Begin");
    refused(
        &mut w,
        &with_claims(begin, |c| c.window_ms = c.window_ms.map(|(a, b)| (a + 1, b))),
        ts(begin) + 1,
        EsaError::E06StaleCheckpoint,
        "Begin with the window start changed",
    );
    refused(
        &mut w,
        &with_claims(begin, |c| c.window_ms = c.window_ms.map(|(a, b)| (a, b - 1))),
        ts(begin),
        EsaError::E06StaleCheckpoint,
        "Begin with the window end changed",
    );
    refused(
        &mut w,
        &with_claims(begin, |c| c.window_ms = None),
        ts(begin),
        EsaError::E06StaleCheckpoint,
        "Begin claiming no window",
    );
    ok(&mut w, begin, ts(begin), "Begin");
    refused(&mut w, begin, ts(begin), EsaError::E14AlreadyApplied, "Begin again");
    refused(
        &mut w,
        finish,
        ts(finish),
        EsaError::E01Malformed,
        "Finish before Miller",
    );
    ok(&mut w, miller, 0, "Miller");

    // Every Apply field of the Finish claim, then writes_record (checked by attest_v03), then a missing claim.
    let mutations: Vec<(&str, Box<dyn Fn(&mut Claims)>, EsaError)> = vec![
        (
            "pre_store",
            Box::new(|c: &mut Claims| c.finish.as_mut().unwrap().apply.pre_store[0] ^= 1),
            EsaError::E15HeaderMismatch,
        ),
        (
            "post_store",
            Box::new(|c: &mut Claims| c.finish.as_mut().unwrap().apply.post_store[0] ^= 1),
            EsaError::E15HeaderMismatch,
        ),
        (
            "head.slot",
            Box::new(|c: &mut Claims| c.finish.as_mut().unwrap().apply.head.slot += 1),
            EsaError::E15HeaderMismatch,
        ),
        (
            "record.exec_hash",
            Box::new(|c: &mut Claims| c.finish.as_mut().unwrap().apply.record.exec_hash[0] ^= 1),
            EsaError::E15HeaderMismatch,
        ),
        (
            "instance",
            Box::new(|c: &mut Claims| c.finish.as_mut().unwrap().instance[0] ^= 1),
            EsaError::E09ChainMismatch,
        ),
        (
            "writes_record",
            Box::new(|c: &mut Claims| {
                let a = &mut c.finish.as_mut().unwrap().apply;
                a.writes_record = !a.writes_record;
            }),
            EsaError::E15HeaderMismatch,
        ),
        (
            "window",
            Box::new(|c: &mut Claims| c.window_ms = c.window_ms.map(|(a, b)| (a, b + 1))),
            EsaError::E06StaleCheckpoint,
        ),
    ];
    for (what, f, code) in mutations {
        refused(
            &mut w,
            &with_claims(finish, f),
            ts(finish),
            code,
            &format!("Finish with {what} changed"),
        );
    }
    ok(&mut w, finish, ts(finish), "Finish");
    refused(&mut w, finish, ts(finish), EsaError::E14AlreadyApplied, "Finish again");

    // A top-level Apply (no calling program): E12.
    let claim = borsh::from_slice::<Instruction>(&finish.instruction)
        .unwrap()
        .claims
        .finish
        .unwrap();
    let inst = steps::instance(&ids, &chain.config);
    let direct = Tx {
        name: "Apply".into(),
        program: ids.attest,
        selectors: vec![
            ProgramShardSelector::new(inst, ids.attest),
            ProgramShardSelector::new(steps::record(&ids, &inst, claim.apply.record.slot), ids.attest),
        ],
        instruction: borsh::to_vec(&attest_v03::Instruction::Apply(claim.apply)).unwrap(),
        window_ms: None,
    };
    refused(&mut w, &direct, 0, EsaError::E12UnknownVerifier, "top-level Apply");
}

#[test]
fn foreign_verifier_low_participation_and_bad_signature() {
    let ids = run::IDS;
    // A second copy of lc_verify_v3_v03 at another id, not in the instance's verifiers.
    let other = Ids {
        attest: ids.attest,
        v3: AccountId::new([0x44; 32]),
    };
    let mut w = World::new(
        ids,
        &guests::bin("attest_v03").unwrap(),
        &guests::bin("lc_verify_v3_v03").unwrap(),
    )
    .unwrap();
    w.state = w.state.clone().with_named_programs([(
        other.v3,
        v03_harness::world::program(&guests::bin("lc_verify_v3_v03").unwrap()).unwrap(),
    )]);
    let chain = fx::sepolia(vec![core_id(&ids.v3)]).unwrap();
    let (init, inst_state, _) = steps::init(&ids, &chain.config, &chain.bootstrap).unwrap();
    ok(&mut w, &init, 0, "Init");
    let (w0, _) = lc::init_store(&chain.config, &chain.bootstrap).unwrap();
    let (_, u0) = &chain.updates[0];
    // The steps through the other program: Begin and Miller pass (lc_v3 does not know the instance's verifiers),
    // Finish's chained Apply comes from a caller not in `verifiers`: E12, the whole transaction reverts.
    let foreign = steps::update(&other, &chain.config, &w0, &inst_state, u0);
    // The native claim builder refuses too (rules::apply sees the caller): build the claims from the real ones.
    assert!(foreign.is_err());
    let real = steps::update(&ids, &chain.config, &w0, &inst_state, u0).unwrap();
    let (_, job_other, msgs) = steps::messages(&other, &chain.config, &w0, u0).unwrap();
    let mut job_state = Vec::new();
    for (i, (name, msg)) in msgs.into_iter().enumerate() {
        let step = lc_v3::handle(&core_id(&other.v3), &core_id(&job_other), &job_state, msg.clone()).unwrap();
        let claims: Claims = borsh::from_slice::<Instruction>(&real.txs[i].instruction)
            .unwrap()
            .claims;
        let tx = Tx {
            name: name.into(),
            program: other.v3,
            selectors: real.txs[i]
                .selectors
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    if k == 0 {
                        ProgramShardSelector::new(job_other, other.v3)
                    } else {
                        *s
                    }
                })
                .collect(),
            instruction: borsh::to_vec(&Instruction::new(claims, &msg)).unwrap(),
            window_ms: step.window_ms,
        };
        if i < 2 {
            ok(&mut w, &tx, ts(&tx), &format!("foreign {name}"));
        } else {
            refused(
                &mut w,
                &tx,
                ts(&tx),
                EsaError::E12UnknownVerifier,
                "Apply from a program not in verifiers",
            );
        }
        job_state = step.state.unwrap();
    }

    // Participation below 342 (fabricated chain impersonating mainnet, 341 signers): Begin refuses with E04.
    let real_main = fx::mainnet(vec![core_id(&ids.v3)]).unwrap();
    let fab = FabChain::build(FabParams::impersonating(&real_main.config, b"beacon-veil/v3/floor"));
    let config = fab.config(vec![core_id(&ids.v3)], 0);
    let (finit, finst, _) = steps::init(&ids, &config, &fab.bootstrap.ssz()).unwrap();
    ok(&mut w, &finit, 0, "fabricated Init");
    let (fw0, _) = lc::init_store(&config, &fab.bootstrap.ssz()).unwrap();
    let good = steps::update(&ids, &config, &fw0, &finst, &fab.updates[0].ssz()).unwrap();
    for (i, tx) in good.txs.iter().enumerate() {
        ok(&mut w, tx, ts(tx), &format!("fabricated update[0] step {i}"));
    }
    // Finish on a moved store: the same update re-signed by 400 members (another job, the same pre-store) passes
    // Begin and Miller; its Apply names the bootstrap commitment, which the instance no longer holds.
    let mut other_sig = fab.updates[0].clone();
    other_sig.bits = bits_first(400);
    fab.resign(&mut other_sig);
    let stale = steps::update(&ids, &config, &fw0, &finst, &other_sig.ssz()).unwrap();
    let code = if stale.native.post_store == good.native.post_store {
        EsaError::E14AlreadyApplied
    } else {
        EsaError::E13StoreMismatch
    };
    ok(&mut w, &stale.txs[0], ts(&stale.txs[0]), "stale Begin");
    ok(&mut w, &stale.txs[1], 0, "stale Miller");
    refused(
        &mut w,
        &stale.txs[2],
        ts(&stale.txs[2]),
        code,
        "Finish on a moved store",
    );

    let mut low = fab.updates[0].clone();
    low.bits = bits_first(341);
    fab.resign(&mut low);
    let (_, _, msgs) = steps::messages(&ids, &config, &fw0, &fab.updates[0].ssz()).unwrap();
    let lc_v3_begin = |update_ssz: Vec<u8>| {
        let (_, mut m) = msgs[0].clone();
        if let attest_core::LcVerifyV3Message::Begin {
            update_ssz: u,
            absent_keys,
            witness,
            ..
        } = &mut m
        {
            *absent_keys = lc_v3::absent_keys(&witness.committee_ssz, &update_ssz);
            *u = update_ssz;
        }
        m
    };
    let job_for = |u: &[u8]| {
        let inst = core_id(&steps::instance(&ids, &config));
        let pre = lc::verify_and_apply(&config, &fw0, &fab.updates[0].ssz())
            .unwrap()
            .pre_store;
        attest_v03::v03_id(&lc_v3::job_id(&core_id(&ids.v3), &lc_v3::job_seed(&inst, &pre, u)))
    };
    let begin_of = |u: Vec<u8>| {
        let claims: Claims = borsh::from_slice::<Instruction>(&good.txs[0].instruction)
            .unwrap()
            .claims;
        Tx {
            selectors: vec![ProgramShardSelector::new(job_for(&u), ids.v3)],
            instruction: borsh::to_vec(&Instruction::new(claims, &lc_v3_begin(u))).unwrap(),
            ..good.txs[0].clone()
        }
    };
    refused(
        &mut w,
        &begin_of(low.ssz()),
        ts(&good.txs[0]),
        EsaError::E04LowParticipation,
        "participation 341",
    );

    // One signature byte changed: refused with E03 at the first step that checks the signature (Begin decompresses
    // it, Miller and Finish use it); the instance never changes.
    let mut bad = fab.updates[0].clone();
    bad.signature[10] ^= 0x01;
    let inst = steps::instance(&ids, &config);
    let inst_before = w.shard(inst, ids.attest);
    let b = begin_of(bad.ssz());
    let job = b.selectors[0].account_id;
    let m = Tx {
        selectors: vec![ProgramShardSelector::new(job, ids.v3)],
        ..good.txs[1].clone()
    };
    let f = Tx {
        selectors: std::iter::once(ProgramShardSelector::new(job, ids.v3))
            .chain(good.txs[2].selectors[1..].iter().copied())
            .collect(),
        ..good.txs[2].clone()
    };
    let mut refused_at = None;
    for (name, tx) in [("Begin", &b), ("Miller", &m), ("Finish", &f)] {
        let before = snapshot(&w, tx);
        let o = w.exec(tx, ts(tx), STEP_LIMIT);
        if o.result.is_err() {
            assert_eq!(
                o.exit_code,
                Some(3),
                "{name} with one signature byte changed: {:?}",
                o.result
            );
            assert!(o.cycles < STEP_LIMIT);
            assert_eq!(snapshot(&w, tx), before, "{name}: a shard changed");
            println!(
                "one signature byte changed: refused E03 at {name} with {} cycles",
                o.cycles
            );
            refused_at = Some(name);
            break;
        }
    }
    assert!(refused_at.is_some(), "a changed signature was accepted");
    assert_eq!(w.shard(inst, ids.attest), inst_before, "the instance changed");
}
