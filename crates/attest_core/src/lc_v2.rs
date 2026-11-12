//! V2 verifier (PLAN.md D4, feature `v2`): the rules of Helios 0.12.0 `verify_generic_update`, in the same order and
//! with the same generalised indices, with ONE function replaced: the sync-committee signature check.
//!
//! Transcribed from a16z/helios tag 0.12.0, `ethereum/consensus-core/src/{consensus_core,proof,utils}.rs` and
//! `types/bls.rs` (MIT). Differences, and why:
//! - Signature check: Helios decompresses the signature with `G2Affine::from_compressed`, which in the bls12_381 fork
//!   already rejects points outside the prime-order subgroup (`is_torsion_free`), and then runs `subgroup_check_g2`
//!   (signature x r), a second subgroup check of the same point that costs 8.77M guest cycles (measured, B0). V2 keeps
//!   the decompression check and drops the repeat. Everything else (aggregate key as Helios builds it,
//!   `hash_to_curve` with the same DST, `e(S, -G1) * e(H, PK) == 1`) is unchanged.
//! - Next-committee branch: the committee root is computed by [`crate::lc::committee_root`] (equal to `tree_hash`,
//!   tested) instead of `tree_hash` (11M cycles).
//!
//! `apply_*` stays Helios's own (called by `lc::verify_and_apply_v2`). The test `v1_v2_agree` in `tests/e2e_chain`
//! runs both variants on every fixture and on tampered updates and compares outcomes and error codes.

use alloy_primitives::B256;
use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Prepared, Gt,
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
    multi_miller_loop,
};
use helios_consensus_core::{
    calc_sync_period, get_bits,
    types::{Forks, GenericUpdate},
};
use sha2::{Digest, Sha256};
use ssz::Encode;
use tree_hash::TreeHash;

use crate::{
    Bytes32, EsaError,
    lc::{Committee, Spec, Store, committee_root, header_valid, root_proof_valid},
};

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

fn h(a: &[u8], b: &[u8]) -> Bytes32 {
    let mut x = Sha256::new();
    x.update(a);
    x.update(b);
    x.finalize().into()
}

/// Helios `calculate_fork_version`.
fn fork_version(forks: &Forks, slot: u64) -> [u8; 4] {
    let epoch = slot / 32;
    let f = if epoch >= forks.gloas.epoch {
        &forks.gloas
    } else if epoch >= forks.fulu.epoch {
        &forks.fulu
    } else if epoch >= forks.electra.epoch {
        &forks.electra
    } else if epoch >= forks.deneb.epoch {
        &forks.deneb
    } else if epoch >= forks.capella.epoch {
        &forks.capella
    } else if epoch >= forks.bellatrix.epoch {
        &forks.bellatrix
    } else if epoch >= forks.altair.epoch {
        &forks.altair
    } else {
        &forks.genesis
    };
    f.fork_version.0
}

/// Helios `compute_committee_sign_root(header_root, compute_fork_data_root(version, genesis_root))`:
/// `fork_data_root = hash_tree_root(ForkData{version, genesis_root})`, `domain = 0x07000000 || fork_data_root[..28]`,
/// `signing_root = hash_tree_root(SigningData{header_root, domain})`.
fn signing_root(header_root: Bytes32, version: [u8; 4], genesis_root: Bytes32) -> Bytes32 {
    let mut v = [0u8; 32];
    v[..4].copy_from_slice(&version);
    let fork_data_root = h(&v, &genesis_root);
    let mut domain = [0u8; 32];
    domain[0] = 7;
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    h(&header_root, &domain)
}

fn g1(key: &impl Encode) -> Result<G1Affine, EsaError> {
    let b: [u8; 48] = key.as_ssz_bytes().try_into().map_err(|_| EsaError::E03BadSignature)?;
    Option::from(G1Affine::from_compressed(&b)).ok_or(EsaError::E03BadSignature)
}

/// Helios `get_participating_aggregate_pubkey`.
fn aggregate_key(committee: &Committee, update: &GenericUpdate<Spec>) -> Result<G1Affine, EsaError> {
    let bits = &update.sync_aggregate.sync_committee_bits;
    let total = bits.len();
    let participating = bits.iter().filter(|b| *b).count();
    if participating == 0 {
        return Err(EsaError::E03BadSignature);
    }
    let agg = if participating > total / 2 {
        let mut agg = G1Projective::from(g1(&committee.aggregate_pubkey)?);
        for (i, bit) in bits.iter().enumerate() {
            if !bit {
                agg -= G1Projective::from(g1(&committee.pubkeys[i])?);
            }
        }
        agg
    } else {
        let mut agg = G1Projective::identity();
        for (i, bit) in bits.iter().enumerate() {
            if bit {
                agg += G1Projective::from(g1(&committee.pubkeys[i])?);
            }
        }
        agg
    };
    Ok(G1Affine::from(agg))
}

/// The replaced function: Helios `Signature::verify` without the second subgroup check.
fn signature_valid(signature: &impl Encode, msg: &Bytes32, pk: &G1Affine) -> bool {
    let Ok(b) = <[u8; 96]>::try_from(signature.as_ssz_bytes()) else {
        return false;
    };
    // `from_compressed` = on-curve recovery + `is_torsion_free` (bls12_381 fork, src/g2.rs).
    let Some(sig) = Option::<G2Affine>::from(G2Affine::from_compressed(&b)) else {
        return false;
    };
    if bool::from(pk.is_identity()) {
        return false;
    }
    let hm = G2Affine::from(<bls12_381::G2Projective as HashToCurve<
        ExpandMsgXmd<sha2_v09::Sha256>,
    >>::hash_to_curve(&msg[..], DST));
    let neg_g1 = G1Affine::from(-G1Projective::generator());
    let ml = multi_miller_loop(&[(&neg_g1, &G2Prepared::from(sig)), (pk, &G2Prepared::from(hm))]);
    ml.final_exponentiation() == Gt::identity()
}

/// Helios `verify_generic_update`, same checks in the same order, mapped to the approved codes.
pub fn verify(
    update: &GenericUpdate<Spec>,
    expected_current_slot: u64,
    store: &Store,
    genesis_root: B256,
    forks: &Forks,
) -> Result<(), EsaError> {
    let bits = get_bits::<Spec>(&update.sync_aggregate.sync_committee_bits);
    if bits == 0 {
        return Err(EsaError::E04LowParticipation);
    }
    if !header_valid(&update.attested_header, forks) {
        return Err(EsaError::E15HeaderMismatch);
    }
    let attested = update.attested_header.beacon();
    let update_finalized_slot = update
        .finalized_header
        .as_ref()
        .map(|v| v.beacon().slot)
        .unwrap_or_default();
    let valid_time = expected_current_slot >= update.signature_slot
        && update.signature_slot > attested.slot
        && attested.slot >= update_finalized_slot;
    if !valid_time {
        return Err(EsaError::E01Malformed);
    }
    let store_period = calc_sync_period::<Spec>(store.finalized_header.beacon().slot);
    let sig_period = calc_sync_period::<Spec>(update.signature_slot);
    let valid_period = if store.next_sync_committee.is_some() {
        sig_period == store_period || sig_period == store_period + 1
    } else {
        sig_period == store_period
    };
    if !valid_period {
        return Err(EsaError::E05HandoffNotVerified);
    }
    let attested_period = calc_sync_period::<Spec>(attested.slot);
    let update_has_next_committee =
        store.next_sync_committee.is_none() && update.next_sync_committee.is_some() && attested_period == store_period;
    if attested.slot <= store.finalized_header.beacon().slot && !update_has_next_committee {
        return Err(EsaError::E14AlreadyApplied);
    }
    let attested_epoch = attested.slot / 32;
    if let Some(finalized) = &update.finalized_header {
        let Some(branch) = &update.finality_branch else {
            return Err(EsaError::E02NotFinalised);
        };
        if !header_valid(finalized, forks) {
            return Err(EsaError::E15HeaderMismatch);
        }
        let (index, depth) = if attested_epoch >= forks.gloas.epoch {
            (223, 9)
        } else if attested_epoch >= forks.electra.epoch {
            (41, 7)
        } else {
            (41, 6)
        };
        if !root_proof_valid(
            attested.state_root.0,
            finalized.beacon().tree_hash_root().0,
            branch,
            depth,
            index,
        ) {
            return Err(EsaError::E02NotFinalised);
        }
    }
    if update.next_sync_committee.is_some() != update.next_sync_committee_branch.is_some() {
        return Err(EsaError::E05HandoffNotVerified);
    }
    if let (Some(next), Some(branch)) = (&update.next_sync_committee, &update.next_sync_committee_branch) {
        if attested_period == store_period && store.next_sync_committee.as_ref().is_some_and(|known| known != next) {
            return Err(EsaError::E05HandoffNotVerified);
        }
        let (index, depth) = if attested_epoch >= forks.gloas.epoch {
            (898, 11)
        } else if attested_epoch >= forks.electra.epoch {
            (23, 6)
        } else {
            (23, 5)
        };
        if !root_proof_valid(attested.state_root.0, committee_root(next), branch, depth, index) {
            return Err(EsaError::E05HandoffNotVerified);
        }
    }
    let committee = if sig_period == store_period {
        &store.current_sync_committee
    } else {
        store
            .next_sync_committee
            .as_ref()
            .ok_or(EsaError::E05HandoffNotVerified)?
    };
    let pk = aggregate_key(committee, update)?;
    let msg = signing_root(
        attested.tree_hash_root().0,
        fork_version(forks, update.signature_slot.saturating_sub(1)),
        genesis_root.0,
    );
    if !signature_valid(&update.sync_aggregate.sync_committee_signature, &msg, &pk) {
        return Err(EsaError::E03BadSignature);
    }
    Ok(())
}
