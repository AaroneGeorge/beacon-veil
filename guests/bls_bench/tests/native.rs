//! Native check of the bench pipeline: on every real update of the mainnet and Sepolia fixtures, all four variants
//! (zkcrypto or blst, hashed or cached committee) return exactly what V1 returns (accept with the same pre-store,
//! post-store, head and record), and tampered updates are refused with V1's code.

use attest_core::{
    EsaError, InstanceConfig,
    lc::{StoreWitness, init_store, verify_and_apply},
};
use bls_bench_guest::{CommitteeMode, Lib, Marks, Outcome, WholeJob, whole_update, witness_v3};

fn v1(config: &InstanceConfig, w: &StoreWitness, u: &[u8]) -> Result<Outcome, EsaError> {
    verify_and_apply(config, w, u).map(|o| Outcome {
        pre_store: o.pre_store,
        post_store: o.post_store,
        head: o.head,
        record: o.record,
    })
}

fn variants(
    config: &InstanceConfig,
    w: &StoreWitness,
    u: &[u8],
) -> Vec<((Lib, CommitteeMode), Result<Outcome, EsaError>)> {
    let (witness, root, keys) = match witness_v3(config, w, u) {
        Ok(x) => x,
        Err(e) => return vec![((Lib::Blst, CommitteeMode::Hashed), Err(e))],
    };
    let mut out = Vec::new();
    for lib in [Lib::Zkcrypto, Lib::Blst] {
        for mode in [CommitteeMode::Hashed, CommitteeMode::Cached] {
            let job = WholeJob {
                lib,
                mode,
                config: config.clone(),
                witness: witness.clone(),
                cached_root: root,
                update_ssz: u.to_vec(),
                keys: keys.clone(),
            };
            out.push(((lib, mode), whole_update(&job, &mut Marks::start())));
        }
    }
    out
}

/// `(label, update SSZ)` in submission order.
type Updates = Vec<(String, Vec<u8>)>;

fn chain(name: &str) -> (InstanceConfig, Vec<u8>, Updates) {
    let c = fixtures::real::load(name, vec![attest_core::AccountId::new([0x11; 32])], 0).expect("fixtures");
    let mut ups: Vec<(String, Vec<u8>)> = c
        .updates
        .iter()
        .enumerate()
        .map(|(i, u)| (format!("update[{i}]"), u.clone()))
        .collect();
    ups.push(("finality".into(), c.finality_ssz.clone()));
    (c.config, c.bootstrap_ssz, ups)
}

fn tampered(u: &[u8], at_end: usize) -> Vec<u8> {
    let mut t = u.to_vec();
    let i = t.len() - at_end;
    t[i] ^= 1;
    t
}

#[test]
fn all_variants_equal_v1_on_real_chains() {
    for net in ["mainnet", "sepolia"] {
        let (config, bootstrap, ups) = chain(net);
        let (mut w, _) = init_store(&config, &bootstrap).expect("init");
        for (label, u) in &ups {
            let want = v1(&config, &w, u);
            assert!(want.is_ok(), "{net} {label}: V1 refused {want:?}");
            for (v, got) in variants(&config, &w, u) {
                assert_eq!(got, want, "{net} {label} {v:?}");
            }
            // A few tampered forms, same code as V1 (the last 96 bytes are not always the signature; whatever V1
            // says is the expected code).
            for t in [tampered(u, 1), tampered(u, 200), with_bit_cleared(&config, u)] {
                let want_t = v1(&config, &w, &t);
                for (v, got) in variants(&config, &w, &t) {
                    assert_eq!(
                        got.map(|o| o.post_store),
                        want_t.clone().map(|o| o.post_store),
                        "{net} {label} tampered {v:?}"
                    );
                }
            }
            // Replay after apply: E14 on both.
            let next = verify_and_apply(&config, &w, u).expect("v1").new_witness();
            let want_r = v1(&config, &next, u);
            assert_eq!(
                want_r.clone().err(),
                Some(EsaError::E14AlreadyApplied),
                "{net} {label} replay"
            );
            for (v, got) in variants(&config, &next, u) {
                assert_eq!(got, want_r, "{net} {label} replay {v:?}");
            }
            w = next;
        }
    }
}

/// The update with participation bit 511 cleared (the signature then fails: E03 on V1, or E04 near the quorum).
/// The bitfield bytes are found as the bytes where "all set" and "none set" differ.
fn with_bit_cleared(config: &InstanceConfig, u: &[u8]) -> Vec<u8> {
    let all = bls_bench_guest::with_absent(config, u, 0).expect("bits");
    let none = bls_bench_guest::with_absent(config, u, 512).expect("bits");
    let last = all
        .iter()
        .zip(none.iter())
        .rposition(|(a, b)| a != b)
        .expect("bitfield bytes");
    let mut out = u.to_vec();
    out[last] &= 0x7f;
    out
}

#[test]
fn absent_key_witness_must_match_the_committee() {
    let (config, bootstrap, ups) = chain("mainnet");
    let (w, _) = init_store(&config, &bootstrap).expect("init");
    let u = &ups[0].1;
    let (witness, root, mut keys) = witness_v3(&config, &w, u).expect("witness");
    // Swap the aggregate key for another committee key: refused.
    let other = bls_blst::G1::from_compressed(&witness.committee_ssz[..48].try_into().expect("48")).expect("key");
    keys.aggregate = other.uncompressed();
    let job = WholeJob {
        lib: Lib::Blst,
        mode: CommitteeMode::Hashed,
        config,
        witness,
        cached_root: root,
        update_ssz: u.clone(),
        keys,
    };
    assert_eq!(whole_update(&job, &mut Marks::start()), Err(EsaError::E03BadSignature));
}

#[test]
fn fast_root_and_split_match_helios_on_real_updates() {
    for net in ["mainnet", "sepolia"] {
        let (config, bootstrap, ups) = chain(net);
        let forks = attest_core::lc::forks(&config).expect("forks");
        let (w, _) = init_store(&config, &bootstrap).expect("init");
        assert_eq!(
            bls_bench_guest::committee_root_fast(&w.current_committee_ssz),
            attest_core::lc::committee_root_ssz(&w.current_committee_ssz),
            "{net} root"
        );
        for (label, u) in &ups {
            let full = attest_core::lc::decode_update(u, &forks).expect("helios decode");
            let s = bls_bench_guest::split_update(u, &forks).expect("split");
            let g_full = match &full {
                attest_core::lc::AnyUpdate::Full(x) => helios_consensus_core::types::GenericUpdate::from(x),
                attest_core::lc::AnyUpdate::Finality(x) => helios_consensus_core::types::GenericUpdate::from(x),
            };
            assert_eq!(
                s.next_committee,
                g_full
                    .next_sync_committee
                    .as_ref()
                    .map(attest_core::lc::encode_committee),
                "{net} {label} committee"
            );
            assert_eq!(s.next_branch, g_full.next_sync_committee_branch, "{net} {label} branch");
            assert_eq!(
                s.update.attested_header(),
                full.attested_header(),
                "{net} {label} attested"
            );
            assert_eq!(
                s.update.finalized_slot(),
                full.finalized_slot(),
                "{net} {label} finalized"
            );
            assert_eq!(
                s.update.signature_slot(),
                full.signature_slot(),
                "{net} {label} signature slot"
            );
            assert_eq!(s.update.participation(), full.participation(), "{net} {label} bits");
            if u.len() > attest_core::lc::COMMITTEE_SSZ_LEN {
                assert!(
                    matches!(s.update, attest_core::lc::AnyUpdate::Finality(_)),
                    "{net} {label}: fast split not taken"
                );
            }
        }
    }
}
