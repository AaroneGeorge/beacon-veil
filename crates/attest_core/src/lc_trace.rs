//! Measurement-only cycle trace for `lc_verify` (feature `cycle-trace`, never enabled in a deployed build).
//!
//! B0 (docs/DECISIONS.md D-09) asks for cycles per phase. Helios stays unmodified, so the phases inside `verify_*` are measured by
//! running the same library calls Helios makes (bls12_381 risc0 fork: decompression with its torsion check, G2 scalar
//! multiplication by r, `hash_to_curve` with `ExpandMsgXmd<Sha256>` and the same DST, a 2-term multi-Miller loop and
//! final exponentiation) once more, on the same inputs, and printing their cycle cost. Figures from this build are
//! labelled "measured by duplicate computation".

use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt, Scalar,
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
    multi_miller_loop,
};
use risc0_zkvm::guest::env;
use tree_hash::TreeHash;

use crate::lc::{AnyUpdate, Store};

fn now() -> u64 {
    env::cycle_count()
}

/// Prints `label` and the cycles since the previous mark.
pub struct Marks {
    last: u64,
}

impl Marks {
    #[must_use]
    pub fn start() -> Self {
        let last = now();
        env::log(&format!("cycle-trace start at {last}"));
        Self { last }
    }

    pub fn mark(&mut self, label: &str) {
        let t = now();
        env::log(&format!("cycle-trace {label}: {}", t - self.last));
        self.last = now();
    }
}

fn g1(bytes: &[u8]) -> G1Affine {
    let b: [u8; 48] = bytes.try_into().expect("48 bytes");
    G1Affine::from_compressed(&b).expect("valid key")
}

/// Runs Helios's BLS steps once more and prints their cost. Call before the real `verify_*`.
pub fn probe(store: &Store, update: &AnyUpdate) {
    let (bits, sig) = match update {
        AnyUpdate::Full(u) => (
            u.sync_aggregate().sync_committee_bits.clone(),
            u.sync_aggregate().sync_committee_signature.clone(),
        ),
        AnyUpdate::Finality(u) => (
            u.sync_aggregate().sync_committee_bits.clone(),
            u.sync_aggregate().sync_committee_signature.clone(),
        ),
    };
    let committee = &store.current_sync_committee;

    let t = now();
    let _ = committee.tree_hash_root();
    let committee_root = now() - t;
    let t = now();
    let root = crate::lc::committee_root(committee);
    let committee_root_direct = now() - t;

    // Key aggregation as Helios does it with > 256 signers: aggregate key minus the non-signers.
    let t = now();
    let mut agg = G1Projective::from(g1(ssz::Encode::as_ssz_bytes(&committee.aggregate_pubkey).as_slice()));
    let mut missing = 0u32;
    for (i, bit) in bits.iter().enumerate() {
        if !bit {
            missing += 1;
            agg -= G1Projective::from(g1(ssz::Encode::as_ssz_bytes(&committee.pubkeys[i]).as_slice()));
        }
    }
    let agg = G1Affine::from(agg);
    let key_aggregation = now() - t;

    let sig_bytes: [u8; 96] = ssz::Encode::as_ssz_bytes(&sig).as_slice().try_into().expect("96 bytes");
    let t = now();
    let sig_point = G2Affine::from_compressed(&sig_bytes).unwrap_or_else(|| G2Affine::generator());
    let sig_decompress = now() - t;

    let t = now();
    let r = Scalar::from_raw([
        0xffff_ffff_0000_0001,
        0x53bd_a402_fffe_5bfe,
        0x3339_d808_09a1_d805,
        0x73ed_a753_299d_7d48,
    ]);
    let _ = bool::from((sig_point * r).is_identity());
    let subgroup = now() - t;

    let t = now();
    let msg = root;
    let h = <G2Projective as HashToCurve<ExpandMsgXmd<sha2_v09::Sha256>>>::hash_to_curve(
        &msg[..],
        b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_",
    );
    let h = G2Affine::from(h);
    let hash_to_curve = now() - t;

    let t = now();
    let neg_g1 = G1Affine::from(-G1Projective::generator());
    let ml = multi_miller_loop(&[(&neg_g1, &G2Prepared::from(sig_point)), (&agg, &G2Prepared::from(h))]);
    let miller = now() - t;
    let t = now();
    let _ = ml.final_exponentiation() == Gt::identity();
    let final_exp = now() - t;

    env::log(&format!(
        "cycle-trace phases (duplicate computation): committee_root tree_hash {committee_root}, direct {committee_root_direct}, key_aggregation {key_aggregation} \
         ({missing} non-signers), sig_decompress {sig_decompress}, g2_subgroup_check {subgroup}, hash_to_curve \
         {hash_to_curve}, miller_loop_2 {miller}, final_exponentiation {final_exp}"
    ));
}
