//! V3 verifier (feature `v3`): the public-path light client, run as up to three public transactions.
//!
//! Rules: Helios 0.12.0 `verify_generic_update`, transcribed in the same order and with the same generalised
//! indices as [`crate::lc_v2`] (a16z/helios tag 0.12.0, `ethereum/consensus-core/src/consensus_core.rs`, MIT), and
//! the pre-checks of V1 in V1's order (decode E01, finalized header present E02, participation at least
//! `max(min_participants, 342)` E04). Three things are replaced:
//!
//! 1. **BLS layer**: `bls_blst` (RISC Zero fork of `blst`) instead of the `bls12_381` fork. The committee's aggregate
//!    key is decompressed once; each absent key arrives uncompressed in the message and must lie on the curve and
//!    compress to the 48 bytes at its index; the signature is decoded with a subgroup check. Committee keys get no
//!    subgroup check: their bytes are bound, through the committee root and the store commitment, to a beacon
//!    state that admitted each key only with a proof of possession (`docs/DECISIONS.md`). A participating key equal
//!    to the identity is refused (E03): Helios counts it as a signer (a16z/helios issue 871).
//! 2. **Store witness** ([`StoreWitnessV3`]): the SSZ bytes of the committee that signs, the other committee as its
//!    32-byte root. The pre-store commitment is recomputed from these exactly as [`StoreCommitmentInput::commitment`]
//!    defines it, so the instance's on-chain commitment binds the witness. Committees stay raw bytes: a full update
//!    is split into its next committee, its branch and the finality-update part ([`split_update`]), and roots are
//!    computed with the SHA-256 compression function directly ([`committee_root_fast`]).
//! 3. **Apply on roots**: Helios `apply_generic_update` (finality path) reproduced on committee roots and counters.
//!
//! On every input the result (accept with pre-store, post-store, head and record, or the refusal code) must equal
//! V1's; `tests/diff_helios` and `tests/e2e_chain` check it. One designed difference: an identity key among the
//! participants is refused here (V1 accepts it, as Helios does).
//!
//! Steps. [`begin`] = [`prepare`] + [`keys`] (checks, `Apply` fields, aggregate key; the job keeps the signing root
//! and the compressed signature), [`miller`] (signature decode with its subgroup check, hash to G2, one Miller loop
//! over both pairs), [`finish`] (final exponentiation, then the `Apply` fields). A job that would not fit one
//! transaction can run [`prepare`] and [`keys`] as two steps (four in all): [`keys`] then re-checks the committee
//! bytes against the root [`prepare`] stored. [`verify_once`] runs all of it in one execution (private path).
//! No function here panics on any input.

use alloy_primitives::B256;
use borsh::{BorshDeserialize, BorshSerialize};
use helios_consensus_core::{
    calc_sync_period,
    types::{FinalityUpdate, Forks, GenericUpdate, LightClientHeader, LightClientHeaderBellatrix, Update},
};
use lee_core::{account::AccountId, program::PdaSeed};
use risc0_zkvm::sha::{Digest as RDigest, Impl, SHA256_INIT, Sha256 as _};
use sha2::{Digest, Sha256};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

use crate::{
    Bytes32, EsaError, Head, HeaderRecord, InstanceConfig, StoreCommitmentInput,
    lc::{self, AnyUpdate, BeaconHeader, COMMITTEE_SSZ_LEN, QUORUM, SECONDS_PER_SLOT, Spec, StoreWitness},
};

/// Domain of the job seed: `sha256("bv/job/v1" || instance || pre_store || sha256(update_ssz))`.
pub const JOB_DOMAIN: &[u8] = b"bv/job/v1";

/// Weak-subjectivity bound: a step is valid only until `time(finalized head of the store) + WS_PERIOD_SECS`. Same
/// value as Helios 0.12.0's mainnet `max_checkpoint_age` (`ethereum/src/config/networks.rs`: 1_209_600, 14 days).
pub const WS_PERIOD_SECS: u64 = 1_209_600;

/// Compressed encoding of the G1 identity.
pub const IDENTITY_G1: [u8; 48] = {
    let mut b = [0u8; 48];
    b[0] = 0xc0;
    b
};

/// Job stages, in order.
pub mod stage {
    /// Update checked, `Apply` fields computed, keys not yet aggregated (four-step path only).
    pub const PREPARED: u8 = 1;
    /// Aggregate key and signature point stored: ready for the Miller loop.
    pub const BEGUN: u8 = 2;
    /// Miller-loop product stored.
    pub const MILLER: u8 = 3;
    /// Final exponentiation done, `Apply` sent.
    pub const DONE: u8 = 4;
}

/// Which store slot the witnessed committee occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum CommitteeSlot {
    Current,
    Next,
}

/// Store witness of the public path.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StoreWitnessV3 {
    pub finalized: BeaconHeader,
    /// SSZ `SyncCommittee` of the committee that signs the update.
    pub committee_ssz: Vec<u8>,
    /// Current when the update's signature period equals the store period, next otherwise.
    pub signing: CommitteeSlot,
    /// Root of the other committee (zero when the next committee is not known).
    pub other_root: Bytes32,
    pub previous_max_active_participants: u64,
    pub current_max_active_participants: u64,
}

impl StoreWitnessV3 {
    /// The V3 witness of a V1 store for an update signed at `signature_slot`: the next committee when the
    /// signature period is past the store period and the next committee is known, the current one otherwise.
    /// Never fails; the commitment equals the V1 witness's.
    #[must_use]
    pub fn from_v1(w: &StoreWitness, signature_slot: u64) -> Self {
        let root = |b: &[u8]| committee_root_fast(b).unwrap_or([0u8; 32]);
        let store_period = calc_sync_period::<Spec>(w.finalized.slot);
        let sig_period = calc_sync_period::<Spec>(signature_slot);
        let (committee_ssz, signing, other_root) = match &w.next_committee_ssz {
            Some(next) if sig_period != store_period => {
                (next.clone(), CommitteeSlot::Next, root(&w.current_committee_ssz))
            }
            Some(next) => (w.current_committee_ssz.clone(), CommitteeSlot::Current, root(next)),
            None => (w.current_committee_ssz.clone(), CommitteeSlot::Current, [0u8; 32]),
        };
        Self {
            finalized: w.finalized.clone(),
            committee_ssz,
            signing,
            other_root,
            previous_max_active_participants: w.previous_max_active_participants,
            current_max_active_participants: w.current_max_active_participants,
        }
    }

    /// `(current_root, next_root)` given the witnessed committee's root.
    fn roots(&self, committee_root: Bytes32) -> (Bytes32, Bytes32) {
        match self.signing {
            CommitteeSlot::Current => (committee_root, self.other_root),
            CommitteeSlot::Next => (self.other_root, committee_root),
        }
    }

    /// The store commitment this witness describes (hashes the committee).
    pub fn commitment(&self) -> Result<Bytes32, EsaError> {
        let (c, n) = self.roots(committee_root_fast(&self.committee_ssz)?);
        Ok(commitment(
            &self.finalized.root(),
            c,
            n,
            self.previous_max_active_participants,
            self.current_max_active_participants,
        ))
    }
}

fn commitment(
    finalized_root: &Bytes32,
    current: Bytes32,
    next: Bytes32,
    previous_max: u64,
    current_max: u64,
) -> Bytes32 {
    StoreCommitmentInput {
        finalized_header_root: *finalized_root,
        current_committee_root: current,
        next_committee_root: next,
        previous_max_active_participants: previous_max,
        current_max_active_participants: current_max,
    }
    .commitment()
}

/// The fields `attest.Apply` takes, plus facts about the update.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Outcome {
    pub pre_store: Bytes32,
    pub post_store: Bytes32,
    pub head: Head,
    pub record: HeaderRecord,
    pub signature_slot: u64,
    pub participation: u64,
    /// The finalized slot moved forward.
    pub new_slot: bool,
    /// A sync-committee handoff happened (current := old next).
    pub handoff: bool,
}

/// State of a job account (`PDA(lc_verify_v3, job_seed)`), borsh.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct JobV3 {
    pub stage: u8,
    pub attest_program: AccountId,
    pub instance: AccountId,
    /// Timestamp window of `Begin` and `Finish` (milliseconds): from the signature slot's time to the
    /// weak-subjectivity bound of the store's finalized head.
    pub not_before_ms: u64,
    pub not_after_ms: u64,
    /// Root of the signing committee (checked again by [`keys`] on the four-step path).
    pub committee_root: Bytes32,
    /// Participation bits as in the update (512 bits, SSZ order).
    pub bits: [u8; 64],
    /// The update's signature, compressed.
    pub signature: [u8; 96],
    /// `compute_signing_root(attested header, domain)`: the message that was signed.
    pub signing_root: Bytes32,
    /// From [`stage::BEGUN`]: aggregate key of the participants, uncompressed.
    pub aggregate_key: [u8; 96],
    /// From [`stage::MILLER`]: the Miller-loop product.
    pub miller: [u8; bls_blst::FP12_BYTES],
    pub outcome: Outcome,
}

/// `sha256("bv/job/v1" || instance || pre_store || sha256(update_ssz))`.
#[must_use]
pub fn job_seed(instance: &AccountId, pre_store: &Bytes32, update_ssz: &[u8]) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(JOB_DOMAIN);
    h.update(instance.value());
    h.update(pre_store);
    h.update(Sha256::digest(update_ssz));
    h.finalize().into()
}

/// Job account id: `PDA(v3_program, job_seed(..))`.
#[must_use]
pub fn job_id(v3_program: &AccountId, seed: &Bytes32) -> AccountId {
    AccountId::for_public_pda(v3_program, &PdaSeed::new(*seed))
}

/// Measurement build only (feature `v3-trace`, in the guest): writes `v3-mark <label> <cycles>` to stderr, where
/// `<cycles>` is the guest's user-cycle count so far. Does nothing in a deployed build.
pub fn mark(label: &str) {
    #[cfg(all(feature = "v3-trace", target_os = "zkvm"))]
    {
        use std::io::Write as _;
        let c = risc0_zkvm::guest::env::cycle_count();
        let _ = writeln!(risc0_zkvm::guest::env::stderr(), "v3-mark {label} {c}");
    }
    #[cfg(not(all(feature = "v3-trace", target_os = "zkvm")))]
    let _ = label;
}

// ---------------------------------------------------------------------------------------------------------------
// Hashing and SSZ

/// SHA-256 of exactly 64 bytes (`a || b`): the message block, then the constant padding block of a 64-byte message.
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
    let mut r = [0u8; 32];
    r.copy_from_slice(out.as_bytes());
    r
}

fn key48(committee_ssz: &[u8], i: usize) -> Option<[u8; 48]> {
    committee_ssz.get(i * 48..(i + 1) * 48)?.try_into().ok()
}

/// `hash_tree_root(SyncCommittee)` from its SSZ bytes; equals [`lc::committee_root_ssz`] (tested). A 48-byte key's
/// root is `sha256(key || 0^16)`.
pub fn committee_root_fast(ssz: &[u8]) -> Result<Bytes32, EsaError> {
    fn key_root(k: &[u8; 48]) -> Bytes32 {
        let mut a = [0u8; 32];
        a.copy_from_slice(&k[..32]);
        let mut b = [0u8; 32];
        b[..16].copy_from_slice(&k[32..]);
        sha64(&a, &b)
    }
    if ssz.len() != COMMITTEE_SSZ_LEN {
        return Err(EsaError::E01Malformed);
    }
    let (keys, _) = ssz.as_chunks::<48>();
    let mut layer: Vec<Bytes32> = keys[..512].iter().map(key_root).collect();
    while layer.len() > 1 {
        layer = layer.as_chunks::<2>().0.iter().map(|p| sha64(&p[0], &p[1])).collect();
    }
    Ok(sha64(&layer[0], &key_root(&keys[512])))
}

/// An update as the verifier reads it.
pub struct SplitUpdate {
    pub update: AnyUpdate,
    /// Raw SSZ of the next committee; `None` when absent or all zero (Helios `default_to_none`).
    pub next_committee: Option<Vec<u8>>,
    /// `None` when absent or all zero (Helios `default_branch_to_none`).
    pub next_branch: Option<Vec<B256>>,
}

/// Fixed part of an Electra `LightClientUpdate`: the value of its first offset.
const ELECTRA_UPDATE_FIXED: usize = 4 + COMMITTEE_SSZ_LEN + 6 * 32 + 4 + 7 * 32 + 160 + 8;

fn generic(update: &AnyUpdate) -> GenericUpdate<Spec> {
    match update {
        AnyUpdate::Full(u) => GenericUpdate::from(u),
        AnyUpdate::Finality(u) => GenericUpdate::from(u),
    }
}

/// Splits an SSZ `LightClientUpdate` into its next committee (raw), its branch and the `LightClientFinalityUpdate`
/// made of the remaining fields, which Helios decodes. For a fixed fork F a full update is
/// `attested | next_committee | next_branch | finalized | finality_branch | sync_aggregate | signature_slot` and a
/// finality update is the same without the two middle fields, so cutting them out (and moving the two offsets of
/// the Electra layout) is a bijection. The cut is used only when the result decodes as a finality update of the
/// layout's own fork (Electra: headers by offset, 6-node next branch; Gloas: fixed-size headers, 11-node branch);
/// any other input takes Helios's full decode ([`lc::decode_update`]), so every input is read as V1 reads it.
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
        let mut fin = Vec::with_capacity(bytes.len().checked_sub(cut)?);
        fin.extend_from_slice(bytes.get(..p)?);
        fin.extend_from_slice(bytes.get(end..)?);
        if electra {
            for at in [0usize, 4] {
                let o = u32::from_le_bytes(fin.get(at..at + 4)?.try_into().ok()?);
                let o = o.checked_sub(u32::try_from(cut).ok()?)?;
                fin.get_mut(at..at + 4)?.copy_from_slice(&o.to_le_bytes());
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
        let committee = bytes.get(p..p + COMMITTEE_SSZ_LEN)?;
        let branch: Vec<B256> = bytes
            .get(p + COMMITTEE_SSZ_LEN..end)?
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
    let first = bytes
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .map(|b: [u8; 4]| u32::from_le_bytes(b) as usize);
    let cut = if first == Some(ELECTRA_UPDATE_FIXED) {
        try_cut(4, 6, true)
    } else {
        // Gloas: all fixed; two headers of equal size around the committee, branches of 11 and 9 nodes.
        let rest = bytes.len().checked_sub(COMMITTEE_SSZ_LEN + 11 * 32 + 9 * 32 + 160 + 8);
        rest.filter(|r| r % 2 == 0 && *r > 0)
            .and_then(|r| try_cut(r / 2, 11, false))
    };
    if let Some(s) = cut {
        return Ok(s);
    }
    let update = lc::decode_update(bytes, forks)?;
    let g = generic(&update);
    Ok(SplitUpdate {
        next_committee: g.next_sync_committee.as_ref().map(lc::encode_committee),
        next_branch: g.next_sync_committee_branch.clone(),
        update,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Helios rules (as lc_v2)

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

/// Helios `compute_committee_sign_root`.
fn signing_root(header_root: Bytes32, version: [u8; 4], genesis_root: Bytes32) -> Bytes32 {
    let mut v = [0u8; 32];
    v[..4].copy_from_slice(&version);
    let fork_data_root = sha64(&v, &genesis_root);
    let mut domain = [0u8; 32];
    domain[0] = 7;
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    sha64(&header_root, &domain)
}

/// Helios `is_valid_header` (same as `lc::header_valid`).
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

/// Helios `is_root_proof_valid` (same as `lc::root_proof_valid`).
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
            sha64(&sibling.0, &node)
        } else {
            sha64(&node, &sibling.0)
        };
    }
    node == root
}

fn bit(bits: &[u8; 64], i: usize) -> bool {
    bits[i / 8] >> (i % 8) & 1 == 1
}

// ---------------------------------------------------------------------------------------------------------------
// Steps

/// Everything that needs no key: decode, pre-checks, Helios's rule checks, signing root, apply on roots. The
/// returned job is at [`stage::PREPARED`].
pub fn prepare(
    attest_program: AccountId,
    instance: AccountId,
    config: &InstanceConfig,
    w: &StoreWitnessV3,
    update_ssz: &[u8],
) -> Result<JobV3, EsaError> {
    let forks = lc::forks(config)?;
    let committee_root = committee_root_fast(&w.committee_ssz)?;
    mark("committee_root");

    // Pre-checks in V1's order.
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
    let (current_root, next_root) = w.roots(committee_root);
    let pre_store = commitment(
        &w.finalized.root(),
        current_root,
        next_root,
        w.previous_max_active_participants,
        w.current_max_active_participants,
    );
    let store_slot = w.finalized.slot;
    let store_period = calc_sync_period::<Spec>(store_slot);
    let sig_period = calc_sync_period::<Spec>(signature_slot);
    let next_known = next_root != [0u8; 32];
    let g = generic(&update);
    mark("decode");

    // Helios `verify_generic_update`, same order as lc_v2::verify (expected_current_slot = signature_slot; chain time
    // is the transaction's timestamp window).
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
    // The witnessed committee must be the one that signs.
    let want = if sig_period == store_period {
        CommitteeSlot::Current
    } else {
        CommitteeSlot::Next
    };
    if w.signing != want {
        return Err(EsaError::E13StoreMismatch);
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
    let mut bits = [0u8; 64];
    let raw_bits = g.sync_aggregate.sync_committee_bits.as_ssz_bytes();
    if raw_bits.len() != 64 {
        return Err(EsaError::E01Malformed);
    }
    bits.copy_from_slice(&raw_bits);
    let signature: [u8; 96] = g
        .sync_aggregate
        .sync_committee_signature
        .as_ssz_bytes()
        .try_into()
        .map_err(|_| EsaError::E03BadSignature)?;

    mark("checks");

    // Helios `apply_generic_update` (finality path), on roots.
    let current_max = w.current_max_active_participants.max(participation);
    let update_finalized_period = calc_sync_period::<Spec>(update_finalized_slot);
    let has_sync_update = next_committee.is_some() && next_branch.is_some();
    let has_finalized_next =
        !next_known && has_sync_update && g.finality_branch.is_some() && update_finalized_period == attested_period;
    let has_majority = participation * 3 >= 512 * 2;
    let newer = update_finalized_slot > store_slot;
    let (mut cur_root, mut nxt_root, mut prev_max, mut cur_max) =
        (current_root, next_root, w.previous_max_active_participants, current_max);
    let mut finalized_moved = false;
    let mut handoff = false;
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
            handoff = true;
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
    let post_store = commitment(
        &finalized_header.beacon().tree_hash_root().0,
        cur_root,
        nxt_root,
        prev_max,
        cur_max,
    );
    let (head, record) = lc::head_from(config, &finalized_header, post_store);
    mark("apply");

    let slot_ms = |slot: u64| {
        config
            .genesis_time
            .saturating_add(SECONDS_PER_SLOT.saturating_mul(slot))
            .saturating_mul(1000)
    };
    Ok(JobV3 {
        stage: stage::PREPARED,
        attest_program,
        instance,
        not_before_ms: slot_ms(signature_slot),
        not_after_ms: slot_ms(store_slot).saturating_add(WS_PERIOD_SECS.saturating_mul(1000)),
        committee_root,
        bits,
        signature,
        signing_root: msg,
        aggregate_key: [0u8; 96],
        miller: [0u8; bls_blst::FP12_BYTES],
        outcome: Outcome {
            pre_store,
            post_store,
            head,
            record,
            signature_slot,
            participation,
            new_slot: finalized_moved,
            handoff,
        },
    })
}

/// Aggregate key: the committee's aggregate key minus the absent keys; refuses an identity among the participants
/// and an identity result (E03). `recheck_root`: the
/// committee bytes come from a later message (four-step path) and must hash to the root [`prepare`] stored.
pub fn keys(
    job: &mut JobV3,
    committee_ssz: &[u8],
    absent_keys: &[[u8; 96]],
    recheck_root: bool,
) -> Result<(), EsaError> {
    if job.stage != stage::PREPARED {
        return Err(stage_error(job.stage));
    }
    if committee_ssz.len() != COMMITTEE_SSZ_LEN {
        return Err(EsaError::E01Malformed);
    }
    if recheck_root && committee_root_fast(committee_ssz)? != job.committee_root {
        return Err(EsaError::E13StoreMismatch);
    }
    let agg_c = key48(committee_ssz, 512).ok_or(EsaError::E01Malformed)?;
    let agg = bls_blst::G1::from_compressed(&agg_c).ok_or(EsaError::E03BadSignature)?;
    let mut sum = bls_blst::G1Sum::new(&agg);
    let mut absent = absent_keys.iter();
    for i in 0..512 {
        let k = key48(committee_ssz, i).ok_or(EsaError::E01Malformed)?;
        if bit(&job.bits, i) {
            if k == IDENTITY_G1 {
                return Err(EsaError::E03BadSignature);
            }
        } else {
            let u = absent.next().ok_or(EsaError::E03BadSignature)?;
            let p = bls_blst::G1::from_uncompressed_matching(u, &k).ok_or(EsaError::E03BadSignature)?;
            sum.sub(&p);
        }
    }
    if absent.next().is_some() {
        return Err(EsaError::E03BadSignature);
    }
    let pk = sum.to_affine();
    if pk.is_identity() {
        return Err(EsaError::E03BadSignature);
    }
    job.aggregate_key = pk.uncompressed();
    job.stage = stage::BEGUN;
    mark("aggregate");
    Ok(())
}

/// [`prepare`] then [`keys`]: the `Begin` step of the three-step path.
pub fn begin(
    attest_program: AccountId,
    instance: AccountId,
    config: &InstanceConfig,
    w: &StoreWitnessV3,
    update_ssz: &[u8],
    absent_keys: &[[u8; 96]],
) -> Result<JobV3, EsaError> {
    let mut job = prepare(attest_program, instance, config, w, update_ssz)?;
    keys(&mut job, &w.committee_ssz, absent_keys, false)?;
    Ok(job)
}

fn stage_error(stage: u8) -> EsaError {
    if stage == stage::DONE {
        EsaError::E14AlreadyApplied
    } else {
        EsaError::E01Malformed
    }
}

/// Signature decode (subgroup check), hash to G2 and the Miller loop over both pairs `e(-G1, S) * e(PK, H(m))`.
pub fn miller(job: &mut JobV3) -> Result<(), EsaError> {
    if job.stage != stage::BEGUN {
        return Err(stage_error(job.stage));
    }
    let pk = bls_blst::G1::from_uncompressed_stored(&job.aggregate_key).ok_or(EsaError::E01Malformed)?;
    let sig = bls_blst::G2::signature_from_compressed(&job.signature).ok_or(EsaError::E03BadSignature)?;
    mark("sig_decode");
    let hm = bls_blst::G2::hash_to_curve(&job.signing_root);
    mark("hash_to_g2");
    job.miller = bls_blst::miller_loop_2([&bls_blst::G1::neg_generator(), &pk], [&sig, &hm]).to_bytes();
    job.stage = stage::MILLER;
    mark("miller");
    Ok(())
}

/// Final exponentiation; on one, the job is done and its outcome is what `attest.Apply` receives.
pub fn finish(job: &mut JobV3) -> Result<Outcome, EsaError> {
    if job.stage != stage::MILLER {
        return Err(stage_error(job.stage));
    }
    let f = bls_blst::Fp12::from_bytes(&job.miller).ok_or(EsaError::E01Malformed)?;
    if !f.final_exp_is_one() {
        return Err(EsaError::E03BadSignature);
    }
    mark("final_exp");
    job.stage = stage::DONE;
    Ok(job.outcome.clone())
}

/// The whole update in one execution (private path; native checks). Same code as the three steps.
pub fn verify_once(
    config: &InstanceConfig,
    w: &StoreWitnessV3,
    update_ssz: &[u8],
    absent_keys: &[[u8; 96]],
) -> Result<Outcome, EsaError> {
    let none = AccountId::new([0u8; 32]);
    let mut job = begin(none, none, config, w, update_ssz, absent_keys)?;
    miller(&mut job)?;
    finish(&mut job)
}

// ---------------------------------------------------------------------------------------------------------------
// Program logic (pure): what `lc_verify_v3` does with one message. The program is a thin wrapper around this.

/// Effect of one accepted step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEffect {
    /// New job state (borsh `JobV3`); `None`: keep the state (`VerifyOnce`).
    pub state: Option<Vec<u8>>,
    /// Timestamp window to set, milliseconds `[from, to)`.
    pub window_ms: Option<(u64, u64)>,
    /// `Finish` and `VerifyOnce`: send sync `Apply` with these fields to `Actor(instance, attest_program)`.
    pub apply: Option<(AccountId, AccountId, Outcome)>,
    /// The job after the step.
    pub job: JobV3,
}

fn window(job: &JobV3) -> Result<(u64, u64), EsaError> {
    // A store whose finalized head is older than the weak-subjectivity bound cannot take updates (E06).
    if job.not_before_ms >= job.not_after_ms {
        return Err(EsaError::E06StaleCheckpoint);
    }
    Ok((job.not_before_ms, job.not_after_ms))
}

fn encode(job: &JobV3) -> Vec<u8> {
    borsh::to_vec(job).unwrap_or_default()
}

fn load_job(pre_state: &[u8]) -> Result<JobV3, EsaError> {
    if pre_state.is_empty() {
        return Err(EsaError::E01Malformed);
    }
    borsh::from_slice(pre_state).map_err(|_| EsaError::E01Malformed)
}

/// One message to `lc_verify_v3`. `program` and `receiver` are the receiving actor (the job account on the public
/// path, a private account for `VerifyOnce`); `pre_state` is the receiver's state under this program.
///
/// Refusals: a step that does not match the job's stage gives E01 (E14 once the job is done); `Begin` on a job that
/// exists gives E14; `Begin` at an account that is not the job's PDA gives E09; a config that is not the instance's
/// gives E09; every check of [`prepare`], [`keys`], [`miller`], [`finish`] keeps its code.
pub fn handle(
    program: &AccountId,
    receiver: &AccountId,
    pre_state: &[u8],
    message: crate::LcVerifyV3Message,
) -> Result<StepEffect, EsaError> {
    use crate::LcVerifyV3Message as M;
    let start = |attest_program: AccountId, instance: AccountId, config: &InstanceConfig| {
        if crate::instance_id(&attest_program, config) != instance {
            return Err(EsaError::E09ChainMismatch);
        }
        Ok(())
    };
    let at_job_pda = |job: &JobV3, update_ssz: &[u8]| {
        if job_id(program, &job_seed(&job.instance, &job.outcome.pre_store, update_ssz)) != *receiver {
            return Err(EsaError::E09ChainMismatch);
        }
        Ok(())
    };
    match message {
        M::Begin {
            attest_program,
            instance,
            config,
            witness,
            update_ssz,
            absent_keys,
        } => {
            if !pre_state.is_empty() {
                return Err(EsaError::E14AlreadyApplied);
            }
            start(attest_program, instance, &config)?;
            let mut job = prepare(attest_program, instance, &config, &witness, &update_ssz)?;
            at_job_pda(&job, &update_ssz)?;
            let w = window(&job)?;
            keys(&mut job, &witness.committee_ssz, &absent_keys, false)?;
            Ok(StepEffect {
                state: Some(encode(&job)),
                window_ms: Some(w),
                apply: None,
                job,
            })
        }
        M::BeginPrepare {
            attest_program,
            instance,
            config,
            witness,
            update_ssz,
        } => {
            if !pre_state.is_empty() {
                return Err(EsaError::E14AlreadyApplied);
            }
            start(attest_program, instance, &config)?;
            let job = prepare(attest_program, instance, &config, &witness, &update_ssz)?;
            at_job_pda(&job, &update_ssz)?;
            let w = window(&job)?;
            Ok(StepEffect {
                state: Some(encode(&job)),
                window_ms: Some(w),
                apply: None,
                job,
            })
        }
        M::BeginKeys {
            committee_ssz,
            absent_keys,
        } => {
            let mut job = load_job(pre_state)?;
            keys(&mut job, &committee_ssz, &absent_keys, true)?;
            Ok(StepEffect {
                state: Some(encode(&job)),
                window_ms: None,
                apply: None,
                job,
            })
        }
        M::Miller => {
            let mut job = load_job(pre_state)?;
            miller(&mut job)?;
            Ok(StepEffect {
                state: Some(encode(&job)),
                window_ms: None,
                apply: None,
                job,
            })
        }
        M::Finish => {
            let mut job = load_job(pre_state)?;
            let w = window(&job)?;
            let out = finish(&mut job)?;
            Ok(StepEffect {
                state: Some(encode(&job)),
                window_ms: Some(w),
                apply: Some((job.instance, job.attest_program, out)),
                job,
            })
        }
        M::VerifyOnce {
            attest_program,
            instance,
            config,
            witness,
            update_ssz,
            absent_keys,
        } => {
            start(attest_program, instance, &config)?;
            let mut job = begin(attest_program, instance, &config, &witness, &update_ssz, &absent_keys)?;
            let w = window(&job)?;
            miller(&mut job)?;
            let out = finish(&mut job)?;
            Ok(StepEffect {
                state: None,
                window_ms: Some(w),
                apply: Some((instance, attest_program, out)),
                job,
            })
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Host helpers

/// Uncompressed keys of the cleared participation bits of `update_ssz`, in bit order, taken from `committee_ssz` (the
/// signing committee): what a submitter sends as `absent_keys`. Empty when the update does not decode; a key that
/// does not decompress is sent as 96 zero bytes (the verifier then refuses with E03, as V1 does).
#[must_use]
pub fn absent_keys(committee_ssz: &[u8], update_ssz: &[u8]) -> Vec<[u8; 96]> {
    let bits = if update_ssz.len() > COMMITTEE_SSZ_LEN {
        Update::<Spec>::from_ssz_bytes(update_ssz).map(|u| u.sync_aggregate().sync_committee_bits.clone())
    } else {
        FinalityUpdate::<Spec>::from_ssz_bytes(update_ssz).map(|u| u.sync_aggregate().sync_committee_bits.clone())
    };
    let Ok(bits) = bits else { return Vec::new() };
    bits.iter()
        .enumerate()
        .filter(|(_, b)| !*b)
        .map(|(i, _)| {
            key48(committee_ssz, i)
                .and_then(|k| bls_blst::G1::from_compressed(&k))
                .map_or([0u8; 96], |p| p.uncompressed())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha64_is_sha256_of_64_bytes() {
        let a = [7u8; 32];
        let b = [9u8; 32];
        let mut x = Sha256::new();
        x.update(a);
        x.update(b);
        let want: Bytes32 = x.finalize().into();
        assert_eq!(sha64(&a, &b), want);
    }

    #[test]
    fn identity_constant_decodes_to_identity() {
        let p = bls_blst::G1::from_compressed(&IDENTITY_G1).expect("identity decodes");
        assert!(p.is_identity());
    }

    #[test]
    fn job_seed_binds_every_input() {
        let i = AccountId::new([1; 32]);
        let base = job_seed(&i, &[2; 32], b"u");
        assert_ne!(base, job_seed(&AccountId::new([3; 32]), &[2; 32], b"u"));
        assert_ne!(base, job_seed(&i, &[4; 32], b"u"));
        assert_ne!(base, job_seed(&i, &[2; 32], b"v"));
    }
}
