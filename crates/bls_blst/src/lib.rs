//! The BLS12-381 operations of a sync-committee signature check, over `blst` (in the guest: the RISC Zero fork
//! `risc0/blst` tag `v0.3.16-risczero.0`, whose field arithmetic calls the zkVM's bigint accelerator).
//!
//! The check `e(-G1, S) * e(PK, H(m)) == 1` is split where a staged verifier splits it:
//!
//! 1. inputs: [`G1::from_uncompressed_matching`] (committee keys witnessed uncompressed), [`G1Sum`] (aggregate key
//!    minus the absent keys), [`G2::signature_from_compressed`] (decode and subgroup check), [`G2::hash_to_curve`];
//! 2. [`miller_loop_2`]: both pairs in one Miller loop, result an [`Fp12`] that can travel as 576 bytes;
//! 3. [`Fp12::final_exp_is_one`].
//!
//! Committee keys are not subgroup-checked here. A sync-committee key reaches a beacon state only after its
//! validator's deposit carried a valid proof of possession, and the committee bytes used by the caller are bound to
//! that state through the committee root. The caller binds each uncompressed key to its compressed bytes with
//! [`G1::from_uncompressed_matching`] (on the curve, compresses to exactly those bytes). The signature IS
//! subgroup-checked: it is attacker-supplied.
//!
//! Most calls go through blst's safe Rust API. The few that have none (G1 projective negation and mixed addition,
//! hash to G2, projective-to-affine in G2, field-element decoding) are `unsafe` blocks with a `SAFETY:` note each.

use blst::{
    blst_fp, blst_fp_from_bendian, blst_fp12, blst_hash_to_g2, blst_p1, blst_p1_add_or_double_affine, blst_p1_affine,
    blst_p1_affine_generator, blst_p1_cneg, blst_p2, blst_p2_affine, blst_p2_to_affine,
    min_pk::{AggregatePublicKey, PublicKey, Signature},
};

/// Domain separation tag of the Ethereum consensus BLS signature scheme (proof-of-possession ciphersuite).
pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

/// Bytes of an [`Fp12`] as it travels between steps (twelve big-endian 48-byte field elements, blst's order).
pub const FP12_BYTES: usize = 576;

/// A point of E1 (affine), the group of public keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct G1(blst_p1_affine);

/// A point of E2 (affine), the group of signatures and message points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct G2(blst_p2_affine);

/// An element of Fp12: the Miller-loop output before the final exponentiation.
#[derive(Clone, Copy, Debug)]
pub struct Fp12(blst_fp12);

impl PartialEq for Fp12 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Fp12 {}

impl G1 {
    /// A committee key given uncompressed (96 bytes) and bound to its compressed form (48 bytes, as stored in the
    /// committee): `None` unless the coordinates are field elements, the point is on the curve (both checked by
    /// `blst_p1_deserialize`) and the point compresses to exactly `compressed`. No subgroup check (crate docs).
    #[must_use]
    pub fn from_uncompressed_matching(uncompressed: &[u8; 96], compressed: &[u8; 48]) -> Option<Self> {
        let pk = PublicKey::deserialize(uncompressed).ok()?;
        (pk.compress() == *compressed).then(|| Self(pk.into()))
    }

    /// Decompresses a 48-byte key (square root in Fp). Host side: it produces the uncompressed keys a submitter
    /// sends. No subgroup check.
    #[must_use]
    pub fn from_compressed(compressed: &[u8; 48]) -> Option<Self> {
        PublicKey::uncompress(compressed).ok().map(|pk| Self(pk.into()))
    }

    /// A point stored by the caller itself (for example in its own job state): decoded from 96 uncompressed bytes,
    /// checked on the curve, nothing else.
    #[must_use]
    pub fn from_uncompressed_stored(uncompressed: &[u8; 96]) -> Option<Self> {
        PublicKey::deserialize(uncompressed).ok().map(|pk| Self(pk.into()))
    }

    /// 96-byte uncompressed encoding (`x || y`, big-endian, flags clear; the identity is `0x40 || 0^95`).
    #[must_use]
    pub fn uncompressed(&self) -> [u8; 96] {
        PublicKey::from(self.0).serialize()
    }

    /// 48-byte compressed encoding.
    #[must_use]
    pub fn compressed(&self) -> [u8; 48] {
        PublicKey::from(self.0).compress()
    }

    /// The point at infinity (blst represents it as the all-zero affine point).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.0 == blst_p1_affine::default()
    }

    /// `-G1`, the negated generator used on the signature side of the pairing check.
    #[must_use]
    #[allow(unsafe_code)]
    pub fn neg_generator() -> Self {
        // SAFETY: `blst_p1_affine_generator` returns a pointer to a static constant of the library; it is valid for
        // reads for the whole program and is copied out immediately.
        let g = unsafe { *blst_p1_affine_generator() };
        let mut s = G1Sum::new(&Self(g));
        s.negate();
        s.to_affine()
    }
}

/// A running sum in E1 (projective): the aggregate key minus the absent keys.
#[derive(Clone, Copy, Debug)]
pub struct G1Sum(blst_p1);

impl G1Sum {
    #[must_use]
    pub fn new(start: &G1) -> Self {
        Self(AggregatePublicKey::from_public_key(&PublicKey::from(start.0)).into())
    }

    /// `self += p` (complete formula: handles doubling and the identity).
    #[allow(unsafe_code)]
    pub fn add(&mut self, p: &G1) {
        let a = self.0;
        // SAFETY: `out`, `a` and `b` point to valid, initialised blst point structs owned by this frame or by
        // `self`; blst allows `out` to alias `a`, and here they are distinct anyway.
        unsafe { blst_p1_add_or_double_affine(&raw mut self.0, &raw const a, &raw const p.0) };
    }

    /// `self -= p`, as `-(-self + p)`.
    pub fn sub(&mut self, p: &G1) {
        self.negate();
        self.add(p);
        self.negate();
    }

    #[allow(unsafe_code)]
    fn negate(&mut self) {
        // SAFETY: `self.0` is a valid, initialised projective point; `blst_p1_cneg` negates it in place.
        unsafe { blst_p1_cneg(&raw mut self.0, true) };
    }

    #[must_use]
    pub fn to_affine(&self) -> G1 {
        G1(AggregatePublicKey::from(self.0).to_public_key().into())
    }
}

impl G2 {
    /// A 96-byte compressed signature: decoded (point on the curve) and checked to be in the prime-order subgroup.
    /// The identity decodes (as in the Ethereum consensus `Signature` type); it never verifies against a key that is
    /// not the identity, and callers refuse an identity aggregate key.
    #[must_use]
    pub fn signature_from_compressed(bytes: &[u8; 96]) -> Option<Self> {
        let s = Signature::uncompress(bytes).ok()?;
        s.subgroup_check().then(|| Self(s.into()))
    }

    /// Compressed encoding.
    #[must_use]
    pub fn compressed(&self) -> [u8; 96] {
        Signature::from(self.0).compress()
    }

    /// The point at infinity (blst represents it as the all-zero affine point).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.0 == blst_p2_affine::default()
    }

    /// 192-byte uncompressed encoding.
    #[must_use]
    pub fn uncompressed(&self) -> [u8; 192] {
        Signature::from(self.0).serialize()
    }

    /// A point stored by the caller itself after [`G2::signature_from_compressed`]: decoded from 192 uncompressed
    /// bytes, checked on the curve; the subgroup check was done when the point was first decoded.
    #[must_use]
    pub fn from_uncompressed_stored(uncompressed: &[u8; 192]) -> Option<Self> {
        Signature::deserialize(uncompressed).ok().map(|s| Self(s.into()))
    }

    /// `hash_to_curve` (RFC 9380, `expand_message_xmd` with SHA-256, SSWU, random oracle) with [`DST`].
    #[must_use]
    #[allow(unsafe_code)]
    pub fn hash_to_curve(msg: &[u8]) -> Self {
        let mut p = blst_p2::default();
        let mut a = blst_p2_affine::default();
        // SAFETY: `msg` and `DST` are valid for reads of the given lengths; `aug` is empty (null, length 0), which
        // blst accepts; `p` and `a` are valid, writable structs of this frame.
        unsafe {
            blst_hash_to_g2(
                &raw mut p,
                msg.as_ptr(),
                msg.len(),
                DST.as_ptr(),
                DST.len(),
                core::ptr::null(),
                0,
            );
            blst_p2_to_affine(&raw mut a, &raw const p);
        }
        Self(a)
    }
}

/// `e(p[0], q[0]) * e(p[1], q[1])` before the final exponentiation (one Miller loop over both pairs).
#[must_use]
pub fn miller_loop_2(p: [&G1; 2], q: [&G2; 2]) -> Fp12 {
    Fp12(blst_fp12::miller_loop_n(&[q[0].0, q[1].0], &[p[0].0, p[1].0]))
}

impl Fp12 {
    /// Final exponentiation, then comparison with one.
    #[must_use]
    pub fn final_exp_is_one(&self) -> bool {
        self.0.final_exp() == blst_fp12::default()
    }

    /// Twelve canonical big-endian field elements (blst `blst_bendian_from_fp12` order).
    #[must_use]
    pub fn to_bytes(&self) -> [u8; FP12_BYTES] {
        self.0.to_bendian()
    }

    /// Inverse of [`Fp12::to_bytes`]; `None` unless every element is canonical (less than the modulus).
    #[must_use]
    #[allow(unsafe_code)]
    pub fn from_bytes(bytes: &[u8; FP12_BYTES]) -> Option<Self> {
        let mut f = blst_fp12::default();
        let mut chunks = bytes.as_chunks::<48>().0.iter();
        for i in 0..3 {
            for j in 0..2 {
                for k in 0..2 {
                    let c = chunks.next()?;
                    let mut e = blst_fp::default();
                    // SAFETY: `c` is a 48-byte array, the length `blst_fp_from_bendian` reads; `e` is a valid,
                    // writable field element of this frame.
                    unsafe { blst_fp_from_bendian(&raw mut e, c.as_ptr()) };
                    f.fp6[j].fp2[i].fp[k] = e;
                }
            }
        }
        let out = Self(f);
        (out.to_bytes() == *bytes).then_some(out)
    }
}

/// The whole check in one call: `e(-G1, sig) * e(agg, H(msg)) == 1`, refusing an identity aggregate key.
#[must_use]
pub fn verify(agg: &G1, sig: &G2, msg_point: &G2) -> bool {
    if agg.is_identity() {
        return false;
    }
    miller_loop_2([&G1::neg_generator(), agg], [sig, msg_point]).final_exp_is_one()
}

#[cfg(test)]
mod tests {
    use blst::min_pk::SecretKey;

    use super::*;

    fn sk(seed: u8) -> SecretKey {
        SecretKey::key_gen(&[seed; 32], &[]).expect("key")
    }

    fn g1_of(sk: &SecretKey) -> G1 {
        G1(sk.sk_to_pk().into())
    }

    #[test]
    fn sign_verify_and_aggregate_minus_absent() {
        let msg = [7u8; 32];
        let keys: Vec<SecretKey> = (1..=5).map(sk).collect();
        // Keys 0, 2, 4 sign; 1 and 3 are absent.
        let sigs: Vec<Signature> = [0usize, 2, 4].iter().map(|&i| keys[i].sign(&msg, DST, &[])).collect();
        let refs: Vec<&Signature> = sigs.iter().collect();
        let agg_sig = blst::min_pk::AggregateSignature::aggregate(&refs, false)
            .expect("agg")
            .to_signature();
        let sig = G2::signature_from_compressed(&agg_sig.compress()).expect("decode");

        let pks: Vec<G1> = keys.iter().map(g1_of).collect();
        let mut all = G1Sum::new(&pks[0]);
        for p in &pks[1..] {
            all.add(p);
        }
        let all = all.to_affine();
        // Round trip of the witness form: uncompressed bound to compressed.
        let all_w = G1::from_uncompressed_matching(&all.uncompressed(), &all.compressed()).expect("matching");
        let mut agg = G1Sum::new(&all_w);
        for i in [1usize, 3] {
            let w = G1::from_uncompressed_matching(&pks[i].uncompressed(), &pks[i].compressed()).expect("matching");
            agg.sub(&w);
        }
        let agg = agg.to_affine();
        let h = G2::hash_to_curve(&msg);
        assert!(verify(&agg, &sig, &h));
        // Staged form equals the one-call form, also after the Fp12 travels as bytes.
        let ml = miller_loop_2([&G1::neg_generator(), &agg], [&sig, &h]);
        let back = Fp12::from_bytes(&ml.to_bytes()).expect("canonical");
        assert_eq!(back, ml);
        assert!(back.final_exp_is_one());
        // Wrong message, wrong key set.
        assert!(!verify(&agg, &sig, &G2::hash_to_curve(&[8u8; 32])));
        assert!(!verify(&all, &sig, &h));
    }

    #[test]
    fn uncompressed_key_must_match_its_compressed_bytes() {
        let a = g1_of(&sk(1));
        let b = g1_of(&sk(2));
        assert!(G1::from_uncompressed_matching(&a.uncompressed(), &a.compressed()).is_some());
        assert!(G1::from_uncompressed_matching(&a.uncompressed(), &b.compressed()).is_none());
        // Off the curve: change y.
        let mut bad = a.uncompressed();
        bad[95] ^= 1;
        assert!(G1::from_uncompressed_matching(&bad, &a.compressed()).is_none());
        // Negated point: same x, different sign bit in the compressed form.
        let mut s = G1Sum::new(&a);
        s.negate();
        let neg = s.to_affine();
        assert!(G1::from_uncompressed_matching(&neg.uncompressed(), &a.compressed()).is_none());
        assert_eq!(G1::from_compressed(&a.compressed()), Some(a));
    }

    #[test]
    fn identity_aggregate_is_refused_and_subtraction_reaches_it() {
        let a = g1_of(&sk(3));
        let mut s = G1Sum::new(&a);
        s.sub(&a);
        let id = s.to_affine();
        assert!(id.is_identity());
        let sig = G2::signature_from_compressed(&sk(3).sign(b"m", DST, &[]).compress()).expect("sig");
        assert!(!verify(&id, &sig, &G2::hash_to_curve(b"m")));
    }

    #[test]
    fn signature_outside_the_subgroup_is_refused() {
        // A point on E2 outside the r-torsion: cofactor-uncleared points are found by trying x = 1, 2, ...;
        // instead, flip a byte of a valid signature until it still decodes, then require the subgroup check to fail.
        let good = sk(4).sign(b"m", DST, &[]).compress();
        let mut found = false;
        for i in 1..=255u8 {
            let mut b = good;
            b[95] ^= i;
            if let Ok(s) = Signature::uncompress(&b) {
                assert!(!s.subgroup_check());
                assert!(G2::signature_from_compressed(&b).is_none());
                found = true;
                break;
            }
        }
        assert!(found, "no on-curve neighbour found");
    }

    #[test]
    fn infinity_signature_decodes_as_identity() {
        let mut b = [0u8; 96];
        b[0] = 0xc0;
        let s = G2::signature_from_compressed(&b).expect("the identity decodes");
        assert!(s.is_identity());
        let good = G2::signature_from_compressed(&sk(8).sign(b"m", DST, &[]).compress()).expect("sig");
        assert!(!good.is_identity());
    }

    #[test]
    fn stored_points_round_trip() {
        let a = g1_of(&sk(7));
        assert_eq!(G1::from_uncompressed_stored(&a.uncompressed()), Some(a));
        let sig = G2::signature_from_compressed(&sk(7).sign(b"x", DST, &[]).compress()).expect("sig");
        assert_eq!(G2::from_uncompressed_stored(&sig.uncompressed()), Some(sig));
        let mut bad = sig.uncompressed();
        bad[191] ^= 1;
        assert!(G2::from_uncompressed_stored(&bad).is_none());
    }

    #[test]
    fn fp12_bytes_refuse_non_canonical() {
        let ml = miller_loop_2(
            [&G1::neg_generator(), &g1_of(&sk(5))],
            [&G2::hash_to_curve(b"a"), &G2::hash_to_curve(b"b")],
        );
        let mut b = ml.to_bytes();
        // Set the first element to p + small: top byte 0xff makes it larger than the modulus.
        b[0] = 0xff;
        assert!(Fp12::from_bytes(&b).is_none());
    }

    #[test]
    fn hash_to_curve_matches_blst_signing() {
        // sign(sk, m) = sk * H(m): check e(G1, sig) == e(pk, H(m)) through verify().
        let k = sk(6);
        let sig = G2::signature_from_compressed(&k.sign(b"msg", DST, &[]).compress()).expect("sig");
        assert!(verify(&g1_of(&k), &sig, &G2::hash_to_curve(b"msg")));
    }
}
