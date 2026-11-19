//! Measurement guest `bls_bench` (never deployed). One execution runs one [`Job`] and commits a [`JobOutput`]:
//! the verdict (with pre-store, post-store, head and record on accept) and the guest cycles of each phase.
//!
//! [`Job::Whole`] runs one light-client update through a pipeline shaped like the planned public-path verifier:
//! Helios 0.12.0 `verify_generic_update` rules in Helios's order (as `attest_core::lc_v2`), with
//!
//! - the store witness reduced to the committee that signs (SSZ bytes) plus the other committee as its root;
//! - committees handled as raw SSZ bytes: a full update is split into its next committee (raw), its branch and the
//!   finality-update part, which Helios decodes ([`split_update`]); roots are computed with the zkVM's SHA-256
//!   compression directly ([`committee_root_fast`]);
//! - [`CommitteeMode::Hashed`]: the signing committee's root is recomputed from its bytes;
//!   [`CommitteeMode::Cached`]: its root is taken as given (lower bound: the cost of a committee kept on chain);
//! - [`Lib::Zkcrypto`]: the BLS layer of V2 (RISC Zero `bls12_381` fork, keys decompressed as Helios does);
//!   [`Lib::Blst`]: `bls_blst` (RISC Zero `blst` fork, the aggregate key and the absent keys witnessed
//!   uncompressed and bound to the committee's compressed bytes);
//! - Helios `apply_generic_update` reproduced on committee roots, so the post-store commitment is computed without
//!   decoding a committee.
//!
//! [`Job::V2`] runs `attest_core::lc::verify_and_apply_v2` unchanged (today's V2, for reference).
//!
//! Native builds compute the same verdicts with zero cycle counts; `tools/cost bench-bls` compares them with V1.

use alloy_primitives::B256;
use attest_core::{
    Bytes32, EsaError, Head, HeaderRecord, InstanceConfig, StoreCommitmentInput,
    lc::{self, AnyUpdate, BeaconHeader, COMMITTEE_SSZ_LEN, QUORUM, Spec, StoreWitness},
};
use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt,
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
    multi_miller_loop,
};
use borsh::{BorshDeserialize, BorshSerialize};
use helios_consensus_core::types::FinalityUpdate;
use helios_consensus_core::{
    calc_sync_period,
    types::{Forks, GenericUpdate, LightClientHeader, LightClientHeaderBellatrix},
};
use risc0_zkvm::sha::{Digest as RDigest, Impl, SHA256_INIT, Sha256 as _};
use sha2::{Digest, Sha256};
use ssz::Encode;
use tree_hash::TreeHash;

/// Phase labels, in pipeline order. `begin` in the reports = the first six (everything before the Miller loop).
pub const PHASES: [&str; 9] = [
    "load",
    "decode",
    "checks",
    "aggregate",
    "sig_decode",
    "hash_to_g2",
    "miller",
    "final_exp",
    "apply",
];

/// Number of phases before the Miller loop.
pub const BEGIN_PHASES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Lib {
    Zkcrypto,
    Blst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum CommitteeMode {
    Hashed,
    Cached,
}

/// Store witness of the public-path shape.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct WitnessV3 {
    pub finalized: BeaconHeader,
    /// SSZ `SyncCommittee` of the committee that signs the update: the current one when the signature period equals
    /// the store period, the next one otherwise.
    pub committee_ssz: Vec<u8>,
    /// Root of the other committee (zero when the next committee is not known).
    pub other_root: Bytes32,
    pub previous_max_active_participants: u64,
    pub current_max_active_participants: u64,
}

/// Committee keys a blst run needs, uncompressed (96 bytes each).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct KeysWitness {
    /// The signing committee's `aggregate_pubkey`.
    pub aggregate: [u8; 96],
    /// One key per cleared participation bit, in bit order.
    pub absent: Vec<[u8; 96]>,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct WholeJob {
    pub lib: Lib,
    pub mode: CommitteeMode,
    pub config: InstanceConfig,
    pub witness: WitnessV3,
    /// The signing committee's root as kept on chain ([`CommitteeMode::Cached`] only).
    pub cached_root: Bytes32,
    pub update_ssz: Vec<u8>,
    pub keys: KeysWitness,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Job {
    Whole(WholeJob),
    V2 {
        config: InstanceConfig,
        witness: StoreWitness,
        update_ssz: Vec<u8>,
    },
}

/// What an accepted update produces (the fields of `AttestMessage::Apply`).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Outcome {
    pub pre_store: Bytes32,
    pub post_store: Bytes32,
    pub head: Head,
    pub record: HeaderRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct JobOutput {
    pub result: Result<Outcome, EsaError>,
    /// `(phase, cycles)`; for [`Job::V2`] a single `("v2", cycles)`.
    pub marks: Vec<(String, u64)>,
}

fn now() -> u64 {
    #[cfg(target_os = "zkvm")]
    {
        risc0_zkvm::guest::env::cycle_count()
    }
    #[cfg(not(target_os = "zkvm"))]
    {
        0
    }
}

/// Cycle marks: each [`Marks::mark`] records the cycles since the previous mark.
pub struct Marks {
    last: u64,
    pub marks: Vec<(String, u64)>,
}

impl Marks {
    #[must_use]
    pub fn start() -> Self {
        Self {
            last: now(),
            marks: Vec::with_capacity(PHASES.len()),
        }
    }

    pub fn mark(&mut self, label: &str) {
        let t = now();
        self.marks.push((label.to_string(), t - self.last));
        self.last = now();
    }
}

/// Runs one job.
#[must_use]
pub fn run(job: &Job) -> JobOutput {
    match job {
        Job::Whole(w) => {
            let mut marks = Marks::start();
            let result = whole_update(w, &mut marks);
            JobOutput {
                result,
                marks: marks.marks,
            }
        }
        Job::V2 {
            config,
            witness,
            update_ssz,
        } => {
            let mut marks = Marks::start();
            let result = lc::verify_and_apply_v2(config, witness, update_ssz).map(|o| Outcome {
                pre_store: o.pre_store,
                post_store: o.post_store,
                head: o.head,
                record: o.record,
            });
            marks.mark("v2");
            JobOutput {
                result,
                marks: marks.marks,
            }
        }
    }
}

fn h(a: &[u8], b: &[u8]) -> Bytes32 {
    let mut x = Sha256::new();
    x.update(a);
    x.update(b);
    x.finalize().into()
}

/// Helios `calculate_fork_version` (as `attest_core::lc_v2`).
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

/// Helios `compute_committee_sign_root` (as `attest_core::lc_v2`).
fn signing_root(header_root: Bytes32, version: [u8; 4], genesis_root: Bytes32) -> Bytes32 {
    let mut v = [0u8; 32];
    v[..4].copy_from_slice(&version);
    let fork_data_root = h(&v, &genesis_root);
    let mut domain = [0u8; 32];
    domain[0] = 7;
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    h(&header_root, &domain)
}

/// Helios `is_valid_header` (same as the crate-private `attest_core::lc::header_valid`).
fn header_valid(header: &LightClientHeader, forks: &Forks) -> bool {
    let beacon = header.beacon();
    let epoch = beacon.slot / 32;
    if let LightClientHeader::Gloas(hd) = header {
        let (index, depth) = if epoch >= forks.gloas.epoch {
            (808, 11)
        } else if epoch >= forks.deneb.epoch {
            (300, 9)
        } else if epoch >= forks.capella.epoch {
            (156, 8)
        } else {
            return hd.execution_block_hash.is_zero() && hd.execution_branch.iter().all(B256::is_zero);
        };
        root_proof_valid(
            beacon.body_root.0,
            hd.execution_block_hash.0,
            &hd.execution_branch,
            depth,
            index,
        )
    } else if epoch >= forks.gloas.epoch {
        false
    } else if epoch < forks.capella.epoch {
        header.execution().is_err() && header.execution_branch().is_err()
    } else if let (Ok(execution), Ok(branch)) = (header.execution(), header.execution_branch()) {
        root_proof_valid(beacon.body_root.0, execution.tree_hash_root().0, branch, 4, 9)
    } else {
        false
    }
}

/// Helios `is_root_proof_valid` (same as the crate-private `attest_core::lc::root_proof_valid`).
fn root_proof_valid(root: Bytes32, leaf: Bytes32, branch: &[B256], depth: usize, index: usize) -> bool {
    let Some(extra) = branch.len().checked_sub(depth) else {
        return false;
    };
    if branch[..extra].iter().any(|n| !n.is_zero()) {
        return false;
    }
    let mut node = leaf;
    for (i, sibling) in branch[extra..].iter().enumerate() {
        node = if (index >> i) & 1 == 1 {
            h(&sibling.0, &node)
        } else {
            h(&node, &sibling.0)
        };
    }
    node == root
}

/// SHA-256 of exactly 64 bytes (`a || b`) with two calls of the zkVM's compression function: the message block,
/// then the constant padding block of a 64-byte message (0x80, zeros, bit length 512).
fn sha64(a: &[u8; 32], b: &[u8; 32]) -> Bytes32 {
    const PAD1: RDigest = RDigest::from_bytes({
        let mut p = [0u8; 32];
        p[0] = 0x80;
        p
    });
    const PAD2: RDigest = RDigest::from_bytes({
        let mut p = [0u8; 32];
        p[30] = 0x02;
        p
    });
    let s = Impl::compress(&SHA256_INIT, &RDigest::from_bytes(*a), &RDigest::from_bytes(*b));
    let out = Impl::compress(&s, &PAD1, &PAD2);
    out.as_bytes().try_into().expect("32 bytes")
}

/// `hash_tree_root(SyncCommittee)` from its SSZ bytes, equal to `attest_core::lc::committee_root_ssz` (tested), with
/// [`sha64`] for every node: a 48-byte key's root is `sha256(key || 0^16)`.
pub fn committee_root_fast(ssz: &[u8]) -> Result<Bytes32, EsaError> {
    fn key_root(k: &[u8]) -> Bytes32 {
        let a: [u8; 32] = k[..32].try_into().expect("32");
        let mut b = [0u8; 32];
        b[..16].copy_from_slice(&k[32..48]);
        sha64(&a, &b)
    }
    if ssz.len() != COMMITTEE_SSZ_LEN {
        return Err(EsaError::E01Malformed);
    }
    let mut layer: Vec<Bytes32> = ssz[..512 * 48]
        .as_chunks::<48>()
        .0
        .iter()
        .map(|k| key_root(k))
        .collect();
    while layer.len() > 1 {
        layer = layer.as_chunks::<2>().0.iter().map(|p| sha64(&p[0], &p[1])).collect();
    }
    Ok(sha64(&layer[0], &key_root(&ssz[512 * 48..])))
}

/// An update as the pipeline reads it: the finality part decoded by Helios, the next committee kept as raw bytes.
pub struct SplitUpdate {
    pub update: AnyUpdate,
    /// Raw SSZ of the next committee; `None` when absent or all zero (Helios `default_to_none`).
    pub next_committee: Option<Vec<u8>>,
    /// `None` when absent or all zero (Helios `default_branch_to_none`).
    pub next_branch: Option<Vec<B256>>,
}

/// Fixed part of an Electra `LightClientUpdate` (attested-header offset, committee, 6-node branch, finalized-header
/// offset, 7-node branch, sync aggregate, signature slot): the value of its first offset.
const ELECTRA_UPDATE_FIXED: usize = 4 + COMMITTEE_SSZ_LEN + 6 * 32 + 4 + 7 * 32 + 160 + 8;

/// Splits an SSZ `LightClientUpdate` into its next committee (raw), its branch and the `LightClientFinalityUpdate`
/// made of the remaining fields, which is what Helios then decodes. A full update of fork F is
/// `attested | next_committee | next_branch | finalized | finality_branch | sync_aggregate | signature_slot`, and a
/// finality update of F is the same without the two middle fields, so cutting them out (and moving the two
/// offsets of the Electra layout) is a bijection for a fixed F. The cut is used only when the result decodes as a
/// finality update of the layout's own fork (Electra: headers by offset, 6-node next branch; Gloas: fixed-size
/// headers, 11-node branch); anything else takes Helios's full decode (`lc::decode_update`), so every input is read
/// as V1 reads it.
pub fn split_update(bytes: &[u8], forks: &Forks) -> Result<SplitUpdate, EsaError> {
    if bytes.len() <= COMMITTEE_SSZ_LEN {
        return Ok(SplitUpdate {
            update: lc::decode_update(bytes, forks)?,
            next_committee: None,
            next_branch: None,
        });
    }
    let try_cut = |p: usize, n1: usize, electra: bool| -> Option<SplitUpdate> {
        let cut = COMMITTEE_SSZ_LEN + n1 * 32;
        let end = p.checked_add(cut)?;
        if end > bytes.len() {
            return None;
        }
        let mut fin = Vec::with_capacity(bytes.len() - cut);
        fin.extend_from_slice(&bytes[..p]);
        fin.extend_from_slice(&bytes[end..]);
        if electra {
            for at in [0usize, 4] {
                let o = u32::from_le_bytes(fin.get(at..at + 4)?.try_into().ok()?);
                let o = o.checked_sub(u32::try_from(cut).ok()?)?;
                fin[at..at + 4].copy_from_slice(&o.to_le_bytes());
            }
        }
        let update = lc::decode_update(&fin, forks).ok()?;
        let ok = matches!(
            (&update, electra),
            (AnyUpdate::Finality(FinalityUpdate::Electra(_)), true)
                | (AnyUpdate::Finality(FinalityUpdate::Gloas(_)), false)
        );
        if !ok {
            return None;
        }
        let committee = &bytes[p..p + COMMITTEE_SSZ_LEN];
        let branch: Vec<B256> = bytes[p + COMMITTEE_SSZ_LEN..end]
            .as_chunks::<32>()
            .0
            .iter()
            .map(|c| B256::from(*c))
            .collect();
        Some(SplitUpdate {
            update,
            next_committee: committee.iter().any(|b| *b != 0).then(|| committee.to_vec()),
            next_branch: branch.iter().any(|n| !n.is_zero()).then_some(branch),
        })
    };
    let first = u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
    let cut = if first == ELECTRA_UPDATE_FIXED {
        try_cut(4, 6, true)
    } else {
        // Gloas: all fixed; two headers of equal size H around the committee, branches of 11 and 9 nodes.
        let rest = bytes.len().checked_sub(COMMITTEE_SSZ_LEN + 11 * 32 + 9 * 32 + 160 + 8);
        rest.filter(|r| r % 2 == 0 && *r > 0)
            .and_then(|r| try_cut(r / 2, 11, false))
    };
    if let Some(s) = cut {
        return Ok(s);
    }
    // Any other shape: Helios's own decode of the full update (slow, same verdicts).
    let update = lc::decode_update(bytes, forks)?;
    let g = generic(&update);
    Ok(SplitUpdate {
        next_committee: g.next_sync_committee.as_ref().map(lc::encode_committee),
        next_branch: g.next_sync_committee_branch.clone(),
        update,
    })
}

fn key48(committee_ssz: &[u8], i: usize) -> [u8; 48] {
    committee_ssz[i * 48..(i + 1) * 48].try_into().expect("48-byte key")
}

fn generic(update: &AnyUpdate) -> GenericUpdate<Spec> {
    match update {
        AnyUpdate::Full(u) => GenericUpdate::from(u),
        AnyUpdate::Finality(u) => GenericUpdate::from(u),
    }
}

/// The aggregate key and the signature point, per library.
enum Points {
    Zk(G1Affine, G2Affine, G2Affine),
    Blst(bls_blst::G1, bls_blst::G2, bls_blst::G2),
}

/// Whole update, one execution. See the crate docs.
pub fn whole_update(job: &WholeJob, marks: &mut Marks) -> Result<Outcome, EsaError> {
    let WholeJob {
        lib,
        mode,
        config,
        witness: w,
        cached_root,
        update_ssz,
        keys,
    } = job;
    let forks = lc::forks(config)?;
    if w.committee_ssz.len() != COMMITTEE_SSZ_LEN {
        return Err(EsaError::E01Malformed);
    }
    let committee_root = match mode {
        CommitteeMode::Hashed => committee_root_fast(&w.committee_ssz)?,
        CommitteeMode::Cached => *cached_root,
    };
    marks.mark("load");

    // Pre-checks, same order as V1: decode (E01), finalized header present (E02), quorum (E04).
    let SplitUpdate {
        update,
        next_committee,
        next_branch,
    } = split_update(update_ssz, &forks)?;
    if update.finalized_slot() == 0 {
        return Err(EsaError::E02NotFinalised);
    }
    let signature_slot = update.signature_slot();
    let participation = update.participation();
    if participation < u64::from(config.min_participants).max(QUORUM) {
        return Err(EsaError::E04LowParticipation);
    }
    let store_slot = w.finalized.slot;
    let store_period = calc_sync_period::<Spec>(store_slot);
    let sig_period = calc_sync_period::<Spec>(signature_slot);
    let signing_is_current = sig_period == store_period;
    let (current_root, next_root) = if signing_is_current {
        (committee_root, w.other_root)
    } else {
        (w.other_root, committee_root)
    };
    let pre_store = StoreCommitmentInput {
        finalized_header_root: w.finalized.root(),
        current_committee_root: current_root,
        next_committee_root: next_root,
        previous_max_active_participants: w.previous_max_active_participants,
        current_max_active_participants: w.current_max_active_participants,
    }
    .commitment();
    let next_known = next_root != [0u8; 32];
    let g = generic(&update);
    marks.mark("decode");

    // Helios `verify_generic_update`, same order as attest_core::lc_v2::verify, on roots.
    if !header_valid(&g.attested_header, &forks) {
        return Err(EsaError::E15HeaderMismatch);
    }
    let attested = g.attested_header.beacon();
    let update_finalized_slot = g.finalized_header.as_ref().map(|v| v.beacon().slot).unwrap_or_default();
    if !(signature_slot > attested.slot && attested.slot >= update_finalized_slot) {
        return Err(EsaError::E01Malformed);
    }
    let valid_period = if next_known {
        sig_period == store_period || sig_period == store_period + 1
    } else {
        sig_period == store_period
    };
    if !valid_period {
        return Err(EsaError::E05HandoffNotVerified);
    }
    let attested_period = calc_sync_period::<Spec>(attested.slot);
    let update_has_next_committee = !next_known && next_committee.is_some() && attested_period == store_period;
    if attested.slot <= store_slot && !update_has_next_committee {
        return Err(EsaError::E14AlreadyApplied);
    }
    let attested_epoch = attested.slot / 32;
    if let Some(finalized) = &g.finalized_header {
        let Some(branch) = &g.finality_branch else {
            return Err(EsaError::E02NotFinalised);
        };
        if !header_valid(finalized, &forks) {
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
    if next_committee.is_some() != next_branch.is_some() {
        return Err(EsaError::E05HandoffNotVerified);
    }
    let mut update_next_root = None;
    if let (Some(next), Some(branch)) = (&next_committee, &next_branch) {
        let r = committee_root_fast(next)?;
        if attested_period == store_period && next_known && next_root != r {
            return Err(EsaError::E05HandoffNotVerified);
        }
        let (index, depth) = if attested_epoch >= forks.gloas.epoch {
            (898, 11)
        } else if attested_epoch >= forks.electra.epoch {
            (23, 6)
        } else {
            (23, 5)
        };
        if !root_proof_valid(attested.state_root.0, r, branch, depth, index) {
            return Err(EsaError::E05HandoffNotVerified);
        }
        update_next_root = Some(r);
    }
    let msg = signing_root(
        attested.tree_hash_root().0,
        fork_version(&forks, signature_slot.saturating_sub(1)),
        config.genesis_validators_root,
    );
    marks.mark("checks");

    // BLS: aggregate key, signature decode, hash to G2, Miller loop, final exponentiation.
    let bits = &g.sync_aggregate.sync_committee_bits;
    let sig_bytes: [u8; 96] = g
        .sync_aggregate
        .sync_committee_signature
        .as_ssz_bytes()
        .try_into()
        .map_err(|_| EsaError::E03BadSignature)?;
    let c = &w.committee_ssz;
    let points = match lib {
        Lib::Zkcrypto => {
            let dec =
                |b: [u8; 48]| Option::<G1Affine>::from(G1Affine::from_compressed(&b)).ok_or(EsaError::E03BadSignature);
            // Helios `get_participating_aggregate_pubkey`, more than half participating (quorum checked above).
            let mut agg = G1Projective::from(dec(key48(c, 512))?);
            for (i, bit) in bits.iter().enumerate() {
                if !bit {
                    agg -= G1Projective::from(dec(key48(c, i))?);
                }
            }
            let pk = G1Affine::from(agg);
            marks.mark("aggregate");
            let sig =
                Option::<G2Affine>::from(G2Affine::from_compressed(&sig_bytes)).ok_or(EsaError::E03BadSignature)?;
            if bool::from(pk.is_identity()) {
                return Err(EsaError::E03BadSignature);
            }
            marks.mark("sig_decode");
            let hm = G2Affine::from(
                <G2Projective as HashToCurve<ExpandMsgXmd<sha2_v09::Sha256>>>::hash_to_curve(&msg[..], bls_blst::DST),
            );
            marks.mark("hash_to_g2");
            Points::Zk(pk, sig, hm)
        }
        Lib::Blst => {
            let agg = bls_blst::G1::from_uncompressed_matching(&keys.aggregate, &key48(c, 512))
                .ok_or(EsaError::E03BadSignature)?;
            let mut sum = bls_blst::G1Sum::new(&agg);
            let mut absent = keys.absent.iter();
            for (i, bit) in bits.iter().enumerate() {
                if !bit {
                    let k = absent.next().ok_or(EsaError::E03BadSignature)?;
                    let p =
                        bls_blst::G1::from_uncompressed_matching(k, &key48(c, i)).ok_or(EsaError::E03BadSignature)?;
                    sum.sub(&p);
                }
            }
            if absent.next().is_some() {
                return Err(EsaError::E03BadSignature);
            }
            let pk = sum.to_affine();
            marks.mark("aggregate");
            let sig = bls_blst::G2::signature_from_compressed(&sig_bytes).ok_or(EsaError::E03BadSignature)?;
            if pk.is_identity() {
                return Err(EsaError::E03BadSignature);
            }
            marks.mark("sig_decode");
            let hm = bls_blst::G2::hash_to_curve(&msg);
            marks.mark("hash_to_g2");
            Points::Blst(pk, sig, hm)
        }
    };
    let ok = match points {
        Points::Zk(pk, sig, hm) => {
            let neg_g1 = G1Affine::from(-G1Projective::generator());
            let ml = multi_miller_loop(&[(&neg_g1, &G2Prepared::from(sig)), (&pk, &G2Prepared::from(hm))]);
            marks.mark("miller");
            let ok = ml.final_exponentiation() == Gt::identity();
            marks.mark("final_exp");
            ok
        }
        Points::Blst(pk, sig, hm) => {
            let ml = bls_blst::miller_loop_2([&bls_blst::G1::neg_generator(), &pk], [&sig, &hm]);
            marks.mark("miller");
            let ok = ml.final_exp_is_one();
            marks.mark("final_exp");
            ok
        }
    };
    if !ok {
        return Err(EsaError::E03BadSignature);
    }

    // Helios `apply_generic_update` (finality path), on roots.
    let previous_max = w.previous_max_active_participants;
    let current_max = w.current_max_active_participants.max(participation);
    let update_finalized_period = calc_sync_period::<Spec>(update_finalized_slot);
    let has_sync_update = next_committee.is_some() && next_branch.is_some();
    let has_finalized_next =
        !next_known && has_sync_update && g.finality_branch.is_some() && update_finalized_period == attested_period;
    let has_majority = participation * 3 >= 512 * 2;
    let newer = update_finalized_slot > store_slot;
    let (mut cur_root, mut nxt_root, mut prev_max, mut cur_max) = (current_root, next_root, previous_max, current_max);
    let mut finalized_moved = false;
    if has_majority && (newer || has_finalized_next) {
        let mut stop = false;
        if !next_known {
            if update_finalized_period == store_period {
                nxt_root = update_next_root.unwrap_or([0u8; 32]);
            } else {
                stop = true;
            }
        } else if update_finalized_period == store_period + 1 {
            cur_root = next_root;
            nxt_root = update_next_root.unwrap_or([0u8; 32]);
            prev_max = cur_max;
            cur_max = 0;
        }
        if !stop && newer {
            finalized_moved = true;
        }
    }
    let learned_next = !next_known && nxt_root != [0u8; 32];
    if !finalized_moved && !learned_next {
        return Err(EsaError::E14AlreadyApplied);
    }
    let finalized_header = if finalized_moved {
        g.finalized_header.clone().ok_or(EsaError::E02NotFinalised)?
    } else {
        LightClientHeader::Bellatrix(LightClientHeaderBellatrix {
            beacon: w.finalized.to_helios(),
        })
    };
    let post_store = StoreCommitmentInput {
        finalized_header_root: finalized_header.beacon().tree_hash_root().0,
        current_committee_root: cur_root,
        next_committee_root: nxt_root,
        previous_max_active_participants: prev_max,
        current_max_active_participants: cur_max,
    }
    .commitment();
    let (head, record) = lc::head_from(config, &finalized_header, post_store);
    marks.mark("apply");
    Ok(Outcome {
        pre_store,
        post_store,
        head,
        record,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Host side: building jobs from a V1 store witness. Not called in the guest.

/// The public-path witness, the signing committee's root and its uncompressed keys, from a V1 store witness and an
/// update.
pub fn witness_v3(
    config: &InstanceConfig,
    v1: &StoreWitness,
    update_ssz: &[u8],
) -> Result<(WitnessV3, Bytes32, KeysWitness), EsaError> {
    let forks = lc::forks(config)?;
    let update = lc::decode_update(update_ssz, &forks)?;
    let store_period = calc_sync_period::<Spec>(v1.finalized.slot);
    let sig_period = calc_sync_period::<Spec>(update.signature_slot());
    let current_root = lc::committee_root_ssz(&v1.current_committee_ssz)?;
    let next_root = match &v1.next_committee_ssz {
        Some(b) => lc::committee_root_ssz(b)?,
        None => [0u8; 32],
    };
    let (committee_ssz, root, other_root) = if sig_period == store_period {
        (v1.current_committee_ssz.clone(), current_root, next_root)
    } else {
        let next = v1.next_committee_ssz.clone().ok_or(EsaError::E05HandoffNotVerified)?;
        (next, next_root, current_root)
    };
    let un = |b: [u8; 48]| {
        bls_blst::G1::from_compressed(&b)
            .map(|p| p.uncompressed())
            .ok_or(EsaError::E03BadSignature)
    };
    let g = generic(&update);
    let mut absent = Vec::new();
    for (i, bit) in g.sync_aggregate.sync_committee_bits.iter().enumerate() {
        if !bit {
            absent.push(un(key48(&committee_ssz, i))?);
        }
    }
    let keys = KeysWitness {
        aggregate: un(key48(&committee_ssz, 512))?,
        absent,
    };
    Ok((
        WitnessV3 {
            finalized: v1.finalized.clone(),
            committee_ssz,
            other_root,
            previous_max_active_participants: v1.previous_max_active_participants,
            current_max_active_participants: v1.current_max_active_participants,
        },
        root,
        keys,
    ))
}

/// A copy of `update_ssz` whose participation bits are all set except the first `absent` ones (the signature then
/// no longer verifies: phase costs only). The 64 bitfield bytes sit right before the 96 signature bytes.
pub fn with_absent(config: &InstanceConfig, update_ssz: &[u8], absent: usize) -> Result<Vec<u8>, EsaError> {
    let forks = lc::forks(config)?;
    let update = lc::decode_update(update_ssz, &forks)?;
    let sig = generic(&update).sync_aggregate.sync_committee_signature.as_ssz_bytes();
    let hits: Vec<usize> = update_ssz
        .windows(96)
        .enumerate()
        .filter(|(_, win)| *win == sig.as_slice())
        .map(|(i, _)| i)
        .collect();
    let [at] = hits[..] else {
        return Err(EsaError::E01Malformed);
    };
    let start = at.checked_sub(64).ok_or(EsaError::E01Malformed)?;
    let mut out = update_ssz.to_vec();
    for i in 0..512 {
        let (byte, bit) = (start + i / 8, 1u8 << (i % 8));
        if i < absent {
            out[byte] &= !bit;
        } else {
            out[byte] |= bit;
        }
    }
    Ok(out)
}
