//! Case generation: real updates on stores built from the fetched chain, one tampered copy per tamper class, and the
//! fabricated chain.
//!
//! Stores for old periods are built from the fetched updates themselves (no old bootstrap): the best update of period
//! `P` carries the committee of `P + 1`, so the store that update `P` is checked against ("handoff store") is
//! `{ finalized: finalized header of update P-1, current: committee of P-1 (from update P-2), next: committee of P
//! (from update P-1) }`.

use alloy_primitives::B256;
use attest_core::{
    EsaError, InstanceConfig,
    lc::{self, AnyUpdate, BeaconHeader, StoreWitness},
};
use bls12_381::{G1Affine, G1Projective};
use fixtures::{
    fabricate::{FabChain, FabParams, SLOTS_PER_EPOCH, SLOTS_PER_PERIOD, bits_first},
    ssz::{Header, UpdateParts},
};

use crate::{
    Case, Expect, Kind,
    layout::{COMMITTEE_LEN, Layout},
    load::{Fetched, Network},
};

/// A decoded fetched update with what case building needs.
struct Real<'a> {
    f: &'a Fetched,
    period: u64,
    finalized: BeaconHeader,
    next_ssz: Option<Vec<u8>>,
    participation: u64,
    attested_slot: u64,
}

fn decode<'a>(net: &Network, f: &'a Fetched) -> Real<'a> {
    let forks = lc::forks(&net.config).expect("forks");
    let u = lc::decode_update(&f.ssz, &forks).unwrap_or_else(|e| panic!("{}: {e}", f.source));
    let (finalized, next_ssz) = match &u {
        AnyUpdate::Full(x) => (
            BeaconHeader::from_helios(x.finalized_header().beacon()),
            Some(lc::encode_committee(x.next_sync_committee())),
        ),
        AnyUpdate::Finality(x) => (BeaconHeader::from_helios(x.finalized_header().beacon()), None),
    };
    Real {
        f,
        period: u.signature_slot() / SLOTS_PER_PERIOD,
        finalized,
        next_ssz,
        participation: u.participation(),
        attested_slot: u.attested_header().beacon().slot,
    }
}

fn store(fin: &BeaconHeader, current: &[u8], next: Option<&[u8]>, prev: u64, cur: u64) -> StoreWitness {
    StoreWitness {
        finalized: fin.clone(),
        current_committee_ssz: current.to_vec(),
        next_committee_ssz: next.map(<[u8]>::to_vec),
        previous_max_active_participants: prev,
        current_max_active_participants: cur,
    }
}

/// Fork name at a slot under the config's schedule (`fulu`/`electra` share the Electra light-client containers).
#[must_use]
pub fn fork_name(config: &InstanceConfig, slot: u64) -> &'static str {
    const NAMES: [&str; 8] = [
        "phase0",
        "altair",
        "bellatrix",
        "capella",
        "deneb",
        "electra",
        "fulu",
        "gloas",
    ];
    let epoch = slot / SLOTS_PER_EPOCH;
    let i = config.forks.iter().rposition(|(e, _)| *e <= epoch).unwrap_or(0);
    NAMES[i.min(7)]
}

/// The tamper classes, in report order, with the code expected for each. `n/a` cases (a bit to set when
/// all 512 signed, a committee in a finality update) are not generated.
pub const TAMPER_CLASSES: [(&str, EsaError); 19] = [
    ("sig_byte_flipped", EsaError::E03BadSignature),
    ("sig_infinity", EsaError::E03BadSignature),
    ("bit_cleared", EsaError::E03BadSignature),
    ("bit_set", EsaError::E03BadSignature),
    ("attested_field_changed", EsaError::E03BadSignature),
    ("finalized_field_changed", EsaError::E02NotFinalised),
    ("finality_branch_node_changed", EsaError::E02NotFinalised),
    ("next_committee_key_byte_changed", EsaError::E05HandoffNotVerified),
    ("next_committee_branch_node_changed", EsaError::E05HandoffNotVerified),
    ("signature_slot_not_after_attested", EsaError::E01Malformed),
    ("finalized_execution_changed", EsaError::E15HeaderMismatch),
    ("attested_execution_changed", EsaError::E15HeaderMismatch),
    ("finalized_header_zeroed", EsaError::E02NotFinalised),
    ("other_network_genesis_root", EsaError::E03BadSignature),
    ("replay_after_apply", EsaError::E14AlreadyApplied),
    ("replay_bad_signature", EsaError::E03BadSignature),
    ("truncated_ssz", EsaError::E01Malformed),
    ("extra_data_shortened", EsaError::E15HeaderMismatch),
    ("two_periods_ahead", EsaError::E05HandoffNotVerified),
    ("optimistic_update", EsaError::E02NotFinalised),
];

fn expected(class: &str) -> EsaError {
    TAMPER_CLASSES
        .iter()
        .find(|(c, _)| *c == class)
        .map(|(_, e)| *e)
        .unwrap_or_else(|| panic!("unknown class {class}"))
}

fn flip(b: &mut [u8], at: usize) {
    b[at] ^= 0x01;
}

/// Byte-level tampered copies of `u` (one per class that applies to its container).
fn byte_tampers(u: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let l = Layout::of(u).expect("layout of a real update");
    let mut out = Vec::new();
    let mut t = |class: &'static str, f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = u.to_vec();
        f(&mut b);
        assert_ne!(b, u, "{class} changed nothing");
        out.push((class, b));
    };
    // Middle of the signature, away from the compression flag bits of byte 0.
    t("sig_byte_flipped", &|b| flip(b, l.signature.start + 40));
    t("sig_infinity", &|b| {
        b[l.signature.clone()].fill(0);
        b[l.signature.start] = 0xc0;
    });
    let first_set = (0..512).find(|i| l.bit(u, *i)).expect("a set bit");
    t("bit_cleared", &|b| l.set_bit(b, first_set, false));
    if let Some(first_clear) = (0..512).find(|i| !l.bit(u, *i)) {
        t("bit_set", &|b| l.set_bit(b, first_clear, true));
    }
    t("attested_field_changed", &|b| {
        flip(b, Layout::proposer_of(&l.attested).start)
    });
    t("finalized_field_changed", &|b| {
        flip(b, Layout::proposer_of(&l.finalized).start)
    });
    t("finality_branch_node_changed", &|b| {
        flip(b, l.finality_branch.start + 5)
    });
    if let (Some(c), Some(br)) = (&l.next_committee, &l.next_branch) {
        // Last byte of the first key: keeps the compression flags.
        t("next_committee_key_byte_changed", &|b| flip(b, c.start + 47));
        t("next_committee_branch_node_changed", &|b| flip(b, br.start + 5));
    }
    let att = l.attested_slot(u);
    t("signature_slot_not_after_attested", &|b| {
        b[l.signature_slot.clone()].copy_from_slice(&att.to_le_bytes());
    });
    let fx = l.execution_field(u, &l.finalized).expect("finalized execution field");
    t("finalized_execution_changed", &|b| flip(b, fx.start));
    let ax = l.execution_field(u, &l.attested).expect("attested execution field");
    t("attested_execution_changed", &|b| flip(b, ax.start));
    t("finalized_header_zeroed", &|b| {
        for r in l.zeroable(u, &l.finalized) {
            b[r].fill(0);
        }
    });
    // Cut inside the fixed part: offsets then point past the end (or a fixed container is short).
    t("truncated_ssz", &|b| b.truncate(b.len() * 2 / 3));
    if !l.gloas {
        // The Electra finalized header is the last field and ends with the variable `extra_data`: dropping its last
        // bytes still decodes, and changes the execution payload root.
        t("extra_data_shortened", &|b| b.truncate(b.len() - 7));
    }
    out
}

struct Ctx<'a> {
    net: &'a Network,
    other_gvr: [u8; 32],
    out: &'a mut Vec<Case>,
}

impl Ctx<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        id: String,
        kind: Kind,
        class: &'static str,
        crossing: bool,
        source: &str,
        config: InstanceConfig,
        witness: StoreWitness,
        update: Vec<u8>,
        expect: Expect,
    ) {
        let slot = Layout::of(&update).map_or(0, |l| l.attested_slot(&update));
        let fork = if slot == 0 {
            // Optimistic or truncated: the fork of the source update decides.
            fork_name(&config, witness.finalized.slot)
        } else {
            fork_name(&config, slot)
        };
        self.out.push(Case {
            id,
            network: self.net.name.clone(),
            fork,
            kind,
            class,
            crossing,
            source: source.to_string(),
            config,
            witness,
            update,
            expect,
            divergence: None,
        });
    }

    /// The valid case and every tamper of it. `older` is the store two periods back (for `two_periods_ahead`).
    #[allow(clippy::too_many_arguments)]
    fn valid_and_tampers(
        &mut self,
        base_id: &str,
        valid_class: &'static str,
        crossing: bool,
        src: &str,
        w: &StoreWitness,
        older: Option<&StoreWitness>,
        u: &[u8],
    ) {
        let cfg = self.net.config.clone();
        let applied = lc::verify_and_apply(&cfg, w, u)
            .unwrap_or_else(|e| panic!("{base_id}: real update refused by V1 on its constructed store: {e}"));
        self.push(
            format!("{base_id}/valid"),
            Kind::Valid,
            valid_class,
            crossing,
            src,
            cfg.clone(),
            w.clone(),
            u.to_vec(),
            Expect::Accept,
        );
        for (class, b) in byte_tampers(u) {
            self.push(
                format!("{base_id}/{class}"),
                Kind::Tampered,
                class,
                crossing,
                src,
                cfg.clone(),
                w.clone(),
                b,
                Expect::Refuse(expected(class)),
            );
        }
        let mut other = cfg.clone();
        other.genesis_validators_root = self.other_gvr;
        self.push(
            format!("{base_id}/other_network_genesis_root"),
            Kind::Tampered,
            "other_network_genesis_root",
            crossing,
            src,
            other,
            w.clone(),
            u.to_vec(),
            Expect::Refuse(expected("other_network_genesis_root")),
        );
        self.push(
            format!("{base_id}/replay_after_apply"),
            Kind::Tampered,
            "replay_after_apply",
            crossing,
            src,
            cfg.clone(),
            applied.new_witness(),
            u.to_vec(),
            Expect::Refuse(expected("replay_after_apply")),
        );
        // The same replay with a signature byte changed: V1 (Helios decodes the signature when it decodes the update)
        // refuses E03; V3 decides the replay first (E14) and decodes the signature only in Miller.
        if let Some(l) = Layout::of(u) {
            let mut b = u.to_vec();
            b[l.signature.start + 40] ^= 1;
            self.push(
                format!("{base_id}/replay_bad_signature"),
                Kind::Tampered,
                "replay_bad_signature",
                crossing,
                src,
                cfg.clone(),
                applied.new_witness(),
                b,
                Expect::Refuse(expected("replay_bad_signature")),
            );
            if let Some(c) = self.out.last_mut() {
                c.divergence = Some(
                    "a replayed update whose signature is also wrong: both refuse; V1 reports the signature (E03), \
                     the staged verifier reports the replay (E14) because it checks replay before it decodes the \
                     signature",
                );
            }
        }
        if let Some(o) = older {
            self.push(
                format!("{base_id}/two_periods_ahead"),
                Kind::Tampered,
                "two_periods_ahead",
                crossing,
                src,
                cfg,
                o.clone(),
                u.to_vec(),
                Expect::Refuse(expected("two_periods_ahead")),
            );
        }
    }
}

/// Every case of one real network.
pub fn real_cases(net: &Network, other_gvr: [u8; 32], out: &mut Vec<Case>) {
    let mut cx = Ctx { net, other_gvr, out };
    let cfg = net.config.clone();
    // Handoff stores by signature period, for the finality and optimistic updates.
    let mut handoff_store: Vec<(u64, StoreWitness, Option<StoreWitness>, StoreWitness)> = Vec::new();
    for range in &net.ranges {
        let us: Vec<Real> = range.updates.iter().map(|f| decode(net, f)).collect();
        for (i, u) in us.iter().enumerate() {
            assert_eq!(u.period, range.start_period + i as u64, "{}: period", u.f.source);
        }
        let next = |i: usize| {
            us[i]
                .next_ssz
                .as_deref()
                .expect("a range update carries the next committee")
        };
        // Store at the end of period `i` (finalized header of update i, committees of i and i+1).
        let end_of = |i: usize| {
            store(
                &us[i].finalized,
                next(i - 1),
                Some(next(i)),
                us[i - 1].participation,
                us[i].participation,
            )
        };
        for i in 1..us.len() {
            let u = &us[i];
            let id = format!("{}/p{}", net.name, u.period);
            // Learn the next committee in the same period (no new finality): store finalized at the update's own
            // finalized header, next committee unknown.
            let w = store(&u.finalized, next(i - 1), None, us[i - 1].participation, 0);
            cx.push(
                format!("{id}/learn_next/valid"),
                Kind::Valid,
                "valid_learn_next",
                false,
                &u.f.source,
                cfg.clone(),
                w,
                u.f.ssz.clone(),
                Expect::Accept,
            );
            if i < 2 {
                continue;
            }
            let w = end_of(i - 1);
            let older = (i >= 3).then(|| end_of(i - 2));
            let crossing = fork_name(&cfg, us[i - 1].attested_slot) != fork_name(&cfg, u.attested_slot);
            cx.valid_and_tampers(
                &format!("{id}/handoff"),
                "valid_handoff_update",
                crossing,
                &u.f.source,
                &w,
                older.as_ref(),
                &u.f.ssz,
            );
            // The store after this update (same period), for finality updates of period P.
            let after = lc::verify_and_apply(&cfg, &w, &u.f.ssz).expect("valid").new_witness();
            handoff_store.push((u.period, w, older, after));
        }
    }
    for f in &net.finality {
        let u = decode(net, f);
        let Some((_, w, older, after)) = handoff_store.iter().find(|(p, ..)| *p == u.period) else {
            eprintln!("{}: no store for period {}, skipped", f.source, u.period);
            continue;
        };
        let id = format!("{}/finality{}", net.name, u.attested_slot);
        cx.valid_and_tampers(
            &format!("{id}/handoff"),
            "valid_handoff_finality",
            false,
            &f.source,
            w,
            older.as_ref(),
            &f.ssz,
        );
        // Same period, after the period's best update, when the finality update finalizes something newer.
        if u.finalized.slot > after.finalized.slot {
            cx.push(
                format!("{id}/same_period/valid"),
                Kind::Valid,
                "valid_same_period_finality",
                false,
                &f.source,
                cfg.clone(),
                after.clone(),
                f.ssz.clone(),
                Expect::Accept,
            );
        }
    }
    for f in &net.optimistic {
        // An optimistic update's signature slot: after the sync aggregate (Electra: offset 4 + aggregate 160, then
        // the variable attested header; Gloas: fixed 496-byte header, aggregate, slot last).
        let at = if f.ssz[..4] == 172u32.to_le_bytes() {
            164
        } else {
            f.ssz.len() - 8
        };
        let sig_slot = u64::from_le_bytes(f.ssz[at..at + 8].try_into().expect("8 bytes"));
        let period = sig_slot / SLOTS_PER_PERIOD;
        let Some((_, w, ..)) = handoff_store.iter().find(|(p, ..)| *p == period) else {
            continue;
        };
        cx.push(
            format!("{}/optimistic{}/no_finality", net.name, sig_slot),
            Kind::Optimistic,
            "optimistic_update",
            false,
            &f.source,
            cfg.clone(),
            w.clone(),
            f.ssz.clone(),
            Expect::Refuse(expected("optimistic_update")),
        );
    }
}

/// Participation levels of the fabricated chain (512 is the chain's own period update, `chain0`).
pub const FAB_PARTICIPATION: [usize; 9] = [512, 511, 400, 343, 342, 341, 256, 1, 0];

/// The fabricated chain: our own keys, impersonating `like` (same chain id, genesis root, forks), checkpoint early in
/// `period`. `real_store` is a real handoff store whose next committee is the real committee of `period`.
pub fn fabricated_cases(like: &Network, period: u64, real_store: &StoreWitness, out: &mut Vec<Case>) {
    let mut real = like.config.clone();
    real.checkpoint_slot = period * SLOTS_PER_PERIOD + 2 * SLOTS_PER_EPOCH;
    let chain = FabChain::build(FabParams::impersonating(&real, b"beacon-veil/diff/v1"));
    let cfg = chain.config(like.config.verifiers.clone(), 0);
    let (w0, _) = lc::init_store(&cfg, &chain.bootstrap.ssz()).expect("fabricated bootstrap");
    let mut push = |id: String, class: &'static str, config: &InstanceConfig, w: &StoreWitness, u: Vec<u8>, e| {
        out.push(Case {
            id: format!("fabricated/{id}"),
            network: "fabricated".into(),
            fork: fork_name(config, period * SLOTS_PER_PERIOD),
            kind: Kind::Fabricated,
            class,
            crossing: false,
            source: format!("FabChain seed beacon-veil/diff/v1, period {period}"),
            config: config.clone(),
            witness: w.clone(),
            update: u,
            expect: e,
            divergence: None,
        });
    };
    // The chain itself: period update, handoff update, finality update (positive control on its own checkpoint).
    let mut w = w0.clone();
    for (i, (u, class)) in chain
        .updates
        .iter()
        .zip(["fab_period_update", "fab_handoff_update", "fab_finality_update"])
        .enumerate()
    {
        push(format!("chain{i}"), class, &cfg, &w, u.ssz(), Expect::Accept);
        w = lc::verify_and_apply(&cfg, &w, &u.ssz())
            .expect("fabricated chain applies")
            .new_witness();
    }
    // Participation sweep on the period update. 512 of 512 is `chain0` itself (same bytes, same store), so it is not
    // generated twice.
    for p in FAB_PARTICIPATION.into_iter().filter(|p| *p != 512) {
        let mut u = chain.updates[0].clone();
        u.bits = bits_first(p);
        chain.resign(&mut u);
        let e = if p >= 342 {
            Expect::Accept
        } else {
            Expect::Refuse(EsaError::E04LowParticipation)
        };
        push(format!("participation{p}"), "fab_participation", &cfg, &w0, u.ssz(), e);
    }
    // Where Helios `verify_*` passes and our pre-checks refuse (a16z/helios #872 included): all signatures valid,
    // made by the fabricated committee on its own checkpoint.
    let resigned = |mut u: UpdateParts, bits: usize| {
        u.bits = bits_first(bits);
        chain.resign(&mut u);
        u
    };
    // 1 of 512 as a finality update (the full-update form is `participation1`).
    let mut fin_one = chain.updates[0].clone();
    fin_one.next = None;
    let fin_one = resigned(fin_one, 1);
    push(
        "finality_one_signer".into(),
        "fab_finality_one_signer",
        &cfg,
        &w0,
        fin_one.ssz(),
        Expect::Refuse(EsaError::E04LowParticipation),
    );
    // No finality: empty finalized header, zero finality branch, 512 of 512 (the signature covers the attested
    // header only, so it stays valid).
    let mut nofin = chain.updates[0].clone();
    nofin.finalized = Header::empty();
    nofin.finality_branch = vec![B256::ZERO; nofin.finality_branch.len()];
    push(
        "no_finality".into(),
        "fab_no_finality",
        &cfg,
        &w0,
        nofin.ssz(),
        Expect::Refuse(EsaError::E02NotFinalised),
    );
    // No finality and 1 of 512 together: E02 is checked first.
    push(
        "no_finality_one_signer".into(),
        "fab_no_finality_one_signer",
        &cfg,
        &w0,
        resigned(nofin.clone(), 1).ssz(),
        Expect::Refuse(EsaError::E02NotFinalised),
    );
    // Empty finalized header with a non-zero finality branch that Helios never checks (a16z/helios #872).
    let mut junk = nofin;
    junk.finality_branch = vec![B256::repeat_byte(0xab); junk.finality_branch.len()];
    push(
        "empty_finalized_junk_branch".into(),
        "fab_empty_finalized_junk_branch",
        &cfg,
        &w0,
        junk.ssz(),
        Expect::Refuse(EsaError::E02NotFinalised),
    );
    // Signed by the previous period's committee, on the store that knows the right next committee.
    let w1 = lc::verify_and_apply(&cfg, &w0, &chain.updates[0].ssz())
        .expect("u0")
        .new_witness();
    let p1 = (period + 1) * SLOTS_PER_PERIOD;
    let forged = chain.forge_update(p1 + 3 * SLOTS_PER_EPOCH, &chain.keys[0], Some(&chain.keys[2]));
    push(
        "signed_by_previous_committee".into(),
        "fab_previous_committee",
        &cfg,
        &w1,
        forged.ssz(),
        Expect::Refuse(EsaError::E03BadSignature),
    );
    // The fabricated period update against the real store (RFP-021 Verification Security 1).
    push(
        "against_real_store".into(),
        "fab_against_real_store",
        &like.config,
        real_store,
        chain.updates[0].ssz(),
        Expect::Refuse(EsaError::E03BadSignature),
    );
    // A committee member whose key is the identity (G1 point at infinity), aggregate key consistent with the other
    // 511. The committee enters through the store witness (on a real chain it would come through a next-committee
    // branch; the verify step trusts the store either way). Signed by the other 511.
    let idx = 5usize;
    let mut committee = w0.current_committee_ssz.clone();
    let mut agg = G1Projective::identity();
    for i in (0..512).filter(|i| *i != idx) {
        let k: [u8; 48] = committee[i * 48..i * 48 + 48].try_into().expect("48");
        agg += G1Projective::from(G1Affine::from_compressed(&k).expect("fabricated key"));
    }
    committee[idx * 48..idx * 48 + 48].copy_from_slice(&G1Affine::identity().to_compressed());
    committee[512 * 48..].copy_from_slice(&G1Affine::from(agg).to_compressed());
    assert_eq!(committee.len(), COMMITTEE_LEN);
    let mut wi = w0.clone();
    wi.current_committee_ssz = committee;
    for (name, bit) in [("identity_member_counted", true), ("identity_member_absent", false)] {
        let mut u: UpdateParts = chain.updates[0].clone();
        u.bits = (0..512).map(|i| i != idx).collect();
        chain.resign(&mut u);
        u.bits[idx] = bit;
        push(name.into(), "fab_identity_member", &cfg, &wi, u.ssz(), Expect::Accept);
    }
    if let Some(c) = out.iter_mut().find(|c| c.id == "fabricated/identity_member_counted") {
        c.divergence = Some(
            "a participating committee key equal to the identity: Helios counts it, the staged verifier refuses it \
             (a16z/helios issue #871 class)",
        );
    }
}
