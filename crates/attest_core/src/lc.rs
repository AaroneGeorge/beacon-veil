//! Light-client glue around a16z Helios `helios-consensus-core` 0.12.0, used unmodified (docs/DECISIONS.md D-05, D-10, V1).
//!
//! Shared by the `attest` program (`Init`: bootstrap checks), the `lc_verify` program (`Verify`), the host
//! (`attest_host`, store files) and native tests, so the guest and the host compute the same commitments.
//!
//! Nothing here re-implements a consensus rule: every check is a call into Helios (`verify_bootstrap`,
//! `verify_update`, `verify_finality_update`, `apply_*`). This module only decodes SSZ, maps the instance config to
//! Helios `Forks`, rebuilds a `LightClientStore` from a witness and maps Helios errors to [`EsaError`] codes.

use alloy_primitives::{B256, FixedBytes};
use borsh::{BorshDeserialize, BorshSerialize};
use helios_consensus_core::{
    apply_bootstrap, apply_finality_update, apply_update, calc_sync_period,
    consensus_spec::MainnetConsensusSpec,
    errors::ConsensusError,
    get_bits,
    types::{
        BeaconBlockHeader, Bootstrap, FinalityUpdate, Fork, Forks, LightClientHeader, LightClientHeaderBellatrix,
        LightClientStore, OptimisticUpdate, SyncCommittee, Update,
    },
    verify_finality_update, verify_update,
};
use sha2::{Digest, Sha256};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

use crate::{Bytes32, EsaError, Head, HeaderRecord, InstanceConfig, StoreCommitmentInput};

/// Mainnet and Sepolia share the mainnet preset (512 members, 32 slots, 256 epochs per period).
pub type Spec = MainnetConsensusSpec;
pub type Store = LightClientStore<Spec>;
pub type Committee = SyncCommittee<Spec>;

/// Two thirds of the 512-member sync committee, rounded up (342 * 3 = 1026 >= 1024).
pub const QUORUM: u64 = 342;

/// Seconds per slot on mainnet and Sepolia.
pub const SECONDS_PER_SLOT: u64 = 12;

/// Fork names in the order `InstanceConfig::forks` lists them.
pub const FORK_ORDER: [&str; 8] = [
    "genesis",
    "altair",
    "bellatrix",
    "capella",
    "deneb",
    "electra",
    "fulu",
    "gloas",
];

/// Finalized header as kept in the store witness: the beacon block header only. Helios reads only the beacon part
/// of the store's finalized header, and the commitment binds its root.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BeaconHeader {
    pub slot: u64,
    pub proposer_index: u64,
    pub parent_root: Bytes32,
    pub state_root: Bytes32,
    pub body_root: Bytes32,
}

impl BeaconHeader {
    #[must_use]
    pub fn to_helios(&self) -> BeaconBlockHeader {
        BeaconBlockHeader {
            slot: self.slot,
            proposer_index: self.proposer_index,
            parent_root: B256::from(self.parent_root),
            state_root: B256::from(self.state_root),
            body_root: B256::from(self.body_root),
        }
    }

    #[must_use]
    pub fn from_helios(h: &BeaconBlockHeader) -> Self {
        Self {
            slot: h.slot,
            proposer_index: h.proposer_index,
            parent_root: h.parent_root.0,
            state_root: h.state_root.0,
            body_root: h.body_root.0,
        }
    }

    /// `hash_tree_root(BeaconBlockHeader)` = the beacon block root.
    #[must_use]
    pub fn root(&self) -> Bytes32 {
        self.to_helios().tree_hash_root().0
    }
}

/// The light-client store an update is verified against (`LcVerifyMessage::Verify::store_witness`, borsh).
/// The prover supplies it; `lc_verify` hashes it into the pre-store commitment and `attest.Apply` accepts the update
/// only if that commitment equals the instance's on-chain `head.store_commitment`, so the witness is bound.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StoreWitness {
    pub finalized: BeaconHeader,
    /// SSZ `SyncCommittee` (512 x 48 + 48 bytes).
    pub current_committee_ssz: Vec<u8>,
    /// SSZ `SyncCommittee`, when known.
    pub next_committee_ssz: Option<Vec<u8>>,
    pub previous_max_active_participants: u64,
    pub current_max_active_participants: u64,
}

/// A decoded store plus the roots of its committees (hashed once).
pub struct LoadedStore {
    pub store: Store,
    pub current_committee_root: Bytes32,
    pub next_committee_root: Bytes32,
}

fn malformed<T>(_: T) -> EsaError {
    EsaError::E01Malformed
}

/// Maps the config's `(activation_epoch, fork_version)` list (genesis .. fulu, optional gloas) to Helios `Forks`.
pub fn forks(config: &InstanceConfig) -> Result<Forks, EsaError> {
    let f = &config.forks;
    if f.len() < 7 || f.len() > 8 || f.windows(2).any(|w| w[0].0 > w[1].0) {
        return Err(EsaError::E01Malformed);
    }
    let fork = |i: usize| match f.get(i) {
        Some((epoch, version)) => Fork {
            epoch: *epoch,
            fork_version: FixedBytes::<4>::from(*version),
        },
        None => Fork {
            epoch: u64::MAX,
            fork_version: FixedBytes::<4>::from([0u8; 4]),
        },
    };
    Ok(Forks {
        genesis: fork(0),
        altair: fork(1),
        bellatrix: fork(2),
        capella: fork(3),
        deneb: fork(4),
        electra: fork(5),
        fulu: fork(6),
        gloas: fork(7),
    })
}

/// Which light-client container family a slot uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LcFork {
    /// Altair .. Deneb containers.
    Base,
    /// Electra and Fulu containers.
    Electra,
    Gloas,
}

#[must_use]
pub fn lc_fork(forks: &Forks, slot: u64) -> LcFork {
    let epoch = slot / 32;
    if epoch >= forks.gloas.epoch {
        LcFork::Gloas
    } else if epoch >= forks.electra.epoch {
        LcFork::Electra
    } else {
        LcFork::Base
    }
}

/// Decodes an SSZ `LightClientBootstrap` and checks that its container matches the fork of its slot.
pub fn decode_bootstrap(bytes: &[u8], forks: &Forks) -> Result<Bootstrap<Spec>, EsaError> {
    let b = Bootstrap::<Spec>::from_ssz_bytes(bytes).map_err(malformed)?;
    let want = lc_fork(forks, b.header().beacon().slot);
    let got = match b {
        Bootstrap::Base(_) => LcFork::Base,
        Bootstrap::Electra(_) => LcFork::Electra,
        Bootstrap::Gloas(_) => LcFork::Gloas,
    };
    if want != got {
        return Err(EsaError::E01Malformed);
    }
    Ok(b)
}

/// An update as submitted: a full `LightClientUpdate` (may carry the next committee) or a finality update.
#[derive(Debug, Clone)]
pub enum AnyUpdate {
    Full(Update<Spec>),
    Finality(FinalityUpdate<Spec>),
}

impl AnyUpdate {
    #[must_use]
    pub fn attested_header(&self) -> LightClientHeader {
        match self {
            Self::Full(u) => u.attested_header(),
            Self::Finality(u) => u.attested_header(),
        }
    }

    /// Slot of the finalized header; 0 when absent (zero header).
    #[must_use]
    pub fn finalized_slot(&self) -> u64 {
        match self {
            Self::Full(u) => u.finalized_header().beacon().slot,
            Self::Finality(u) => u.finalized_header().beacon().slot,
        }
    }

    #[must_use]
    pub fn signature_slot(&self) -> u64 {
        match self {
            Self::Full(u) => *u.signature_slot(),
            Self::Finality(u) => *u.signature_slot(),
        }
    }

    #[must_use]
    pub fn participation(&self) -> u64 {
        match self {
            Self::Full(u) => get_bits::<Spec>(&u.sync_aggregate().sync_committee_bits),
            Self::Finality(u) => get_bits::<Spec>(&u.sync_aggregate().sync_committee_bits),
        }
    }

    fn fork(&self) -> LcFork {
        match self {
            Self::Full(Update::Base(_)) | Self::Finality(FinalityUpdate::Base(_)) => LcFork::Base,
            Self::Full(Update::Electra(_)) | Self::Finality(FinalityUpdate::Electra(_)) => LcFork::Electra,
            Self::Full(Update::Gloas(_)) | Self::Finality(FinalityUpdate::Gloas(_)) => LcFork::Gloas,
        }
    }
}

/// Decodes an SSZ `LightClientUpdate` or `LightClientFinalityUpdate` (by size: a full update carries a 24,624-byte
/// committee) and checks that its container matches the fork of its attested slot.
pub fn decode_update(bytes: &[u8], forks: &Forks) -> Result<AnyUpdate, EsaError> {
    let u = if bytes.len() > 24_624 {
        AnyUpdate::Full(Update::<Spec>::from_ssz_bytes(bytes).map_err(malformed)?)
    } else {
        match FinalityUpdate::<Spec>::from_ssz_bytes(bytes) {
            Ok(u) => AnyUpdate::Finality(u),
            // An optimistic update carries no finalized header.
            Err(_) if OptimisticUpdate::<Spec>::from_ssz_bytes(bytes).is_ok() => return Err(EsaError::E02NotFinalised),
            Err(_) => return Err(EsaError::E01Malformed),
        }
    };
    if u.fork() != lc_fork(forks, u.attested_header().beacon().slot) {
        return Err(EsaError::E01Malformed);
    }
    Ok(u)
}

/// Maps a Helios error from `verify_update` / `verify_finality_update` to a code (mapping approved 2026-11-12, G2).
/// Helios returns `eyre::Report`s that wrap [`ConsensusError`]; the only other error on that path is a sync
/// committee public key that is not a valid point while aggregating, which is a signature failure (E03).
#[must_use]
pub fn map_update_error(e: &eyre::Report) -> EsaError {
    match e.downcast_ref::<ConsensusError>() {
        Some(ConsensusError::InsufficientParticipation) => EsaError::E04LowParticipation,
        Some(ConsensusError::InvalidPeriod | ConsensusError::InvalidNextSyncCommitteeProof) => {
            EsaError::E05HandoffNotVerified
        }
        Some(ConsensusError::NotRelevant) => EsaError::E14AlreadyApplied,
        Some(ConsensusError::InvalidFinalityProof) => EsaError::E02NotFinalised,
        Some(ConsensusError::InvalidExecutionPayloadProof) => EsaError::E15HeaderMismatch,
        Some(ConsensusError::InvalidSignature) | None => EsaError::E03BadSignature,
        Some(_) => EsaError::E01Malformed,
    }
}

/// Maps a Helios `verify_bootstrap` error (`Init`): every rejection of a decodable bootstrap (wrong checkpoint
/// root, bad current-committee branch, bad execution branch) is E06 (decision 2026-11-12, asserted by G2).
#[must_use]
pub fn map_bootstrap_error(_e: &eyre::Report) -> EsaError {
    EsaError::E06StaleCheckpoint
}

/// `hash_tree_root(SyncCommittee)` from its SSZ bytes (513 x 48), computed directly with SHA-256 (the RISC Zero
/// accelerated `sha2` in the guest).
///
/// Same value as `tree_hash::TreeHash::tree_hash_root` (checked by the `committee_root_matches_tree_hash` test), but
/// `tree_hash` packs each byte of a 48-byte key separately, which costs about 11M guest cycles per committee
/// (measured with the `cycle-trace` build). Layout (SSZ): container `{ pubkeys: Vector[BLSPubkey, 512],
/// aggregate_pubkey: BLSPubkey }`; a 48-byte key is two chunks (`key[0..32]`, `key[32..48] || 0^16`), so its root is
/// `sha256(key || 0^16)`.
pub fn committee_root_ssz(ssz: &[u8]) -> Result<Bytes32, EsaError> {
    fn h(a: &[u8], b: &[u8]) -> Bytes32 {
        let mut x = Sha256::new();
        x.update(a);
        x.update(b);
        x.finalize().into()
    }
    fn key_root(k: &[u8]) -> Bytes32 {
        h(k, &[0u8; 16])
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
        layer = layer.as_chunks::<2>().0.iter().map(|p| h(&p[0], &p[1])).collect();
    }
    Ok(h(&layer[0], &key_root(&ssz[512 * 48..])))
}

/// SSZ size of a mainnet-preset `SyncCommittee`.
pub const COMMITTEE_SSZ_LEN: usize = 513 * 48;

/// [`committee_root_ssz`] of a decoded committee (re-encodes it).
#[must_use]
pub fn committee_root(c: &Committee) -> Bytes32 {
    committee_root_ssz(&c.as_ssz_bytes()).expect("a committee encodes to 513 x 48 bytes")
}

fn decode_committee(bytes: &[u8]) -> Result<Committee, EsaError> {
    Committee::from_ssz_bytes(bytes).map_err(malformed)
}

/// Encodes a committee as SSZ (for store witnesses and files).
#[must_use]
pub fn encode_committee(c: &Committee) -> Vec<u8> {
    c.as_ssz_bytes()
}

impl StoreWitness {
    /// Rebuilds the Helios store. `optimistic_header` is set to the finalized header and `best_valid_update` to
    /// `None`: neither is part of the commitment, and neither affects `verify_*` or the finality path of `apply_*`.
    pub fn load(&self) -> Result<LoadedStore, EsaError> {
        let current = decode_committee(&self.current_committee_ssz)?;
        let next = self.next_committee_ssz.as_deref().map(decode_committee).transpose()?;
        let header = LightClientHeader::Bellatrix(LightClientHeaderBellatrix {
            beacon: self.finalized.to_helios(),
        });
        let current_committee_root = committee_root_ssz(&self.current_committee_ssz)?;
        let next_committee_root = match &self.next_committee_ssz {
            Some(b) => committee_root_ssz(b)?,
            None => [0u8; 32],
        };
        Ok(LoadedStore {
            store: Store {
                finalized_header: header.clone(),
                current_sync_committee: current,
                next_sync_committee: next,
                optimistic_header: header,
                previous_max_active_participants: self.previous_max_active_participants,
                current_max_active_participants: self.current_max_active_participants,
                best_valid_update: None,
            },
            current_committee_root,
            next_committee_root,
        })
    }

    /// Witness of a Helios store.
    #[must_use]
    pub fn from_store(store: &Store) -> Self {
        Self {
            finalized: BeaconHeader::from_helios(store.finalized_header.beacon()),
            current_committee_ssz: encode_committee(&store.current_sync_committee),
            next_committee_ssz: store.next_sync_committee.as_ref().map(encode_committee),
            previous_max_active_participants: store.previous_max_active_participants,
            current_max_active_participants: store.current_max_active_participants,
        }
    }
}

impl LoadedStore {
    #[must_use]
    pub fn commitment_input(&self) -> StoreCommitmentInput {
        StoreCommitmentInput {
            finalized_header_root: self.store.finalized_header.beacon().tree_hash_root().0,
            current_committee_root: self.current_committee_root,
            next_committee_root: self.next_committee_root,
            previous_max_active_participants: self.store.previous_max_active_participants,
            current_max_active_participants: self.store.current_max_active_participants,
        }
    }

    #[must_use]
    pub fn commitment(&self) -> Bytes32 {
        self.commitment_input().commitment()
    }

    /// Recomputes the cached committee roots after `apply_*`. On a handoff Helios sets current := old next, so the
    /// old next root is reused; the next committee (new or unchanged) is hashed only if present.
    fn refresh_roots(&mut self, handoff: bool, learned_next: bool, old_next_root: Bytes32) {
        if handoff {
            self.current_committee_root = old_next_root;
        }
        if handoff || learned_next {
            self.next_committee_root = self
                .store
                .next_sync_committee
                .as_ref()
                .map_or([0u8; 32], committee_root);
        }
        // Otherwise the next committee is unchanged (Helios only sets it when it was unknown, or on a handoff).
    }
}

/// Head and record fields from a finalized light-client header. For a gloas header only `exec_hash` is known: it is
/// the PARENT execution block hash of the finalized beacon block (`bid.parent_block_hash`, gindex 2856), so it lags
/// by one or more slots; `exec_number`, `exec_state_root` and `exec_receipts_root` are then zero and inclusion takes
/// the RLP execution header (`attest_inclusion`: keccak(rlp) must equal `exec_hash`, else E15).
#[must_use]
pub fn head_from(
    config: &InstanceConfig,
    header: &LightClientHeader,
    store_commitment: Bytes32,
) -> (Head, HeaderRecord) {
    let beacon = header.beacon();
    let beacon_root = beacon.tree_hash_root().0;
    let (exec_number, exec_hash, exec_state_root, exec_receipts_root) = match header {
        LightClientHeader::Gloas(h) => (0, h.execution_block_hash.0, [0u8; 32], [0u8; 32]),
        _ => match header.execution() {
            Ok(e) => (
                *e.block_number(),
                e.block_hash().0,
                e.state_root().0,
                e.receipts_root().0,
            ),
            Err(()) => (0, [0u8; 32], [0u8; 32], [0u8; 32]),
        },
    };
    let head = Head {
        slot: beacon.slot,
        beacon_root,
        exec_number,
        exec_hash,
        exec_state_root,
        exec_receipts_root,
        period: calc_sync_period::<Spec>(beacon.slot),
        store_commitment,
    };
    let record = HeaderRecord {
        chain_id: config.chain_id,
        slot: beacon.slot,
        beacon_root,
        exec_number,
        exec_hash,
        exec_state_root,
        exec_receipts_root,
    };
    (head, record)
}

/// The checks of Helios `verify_bootstrap` (consensus spec `initialize_light_client_store`), with the committee root
/// supplied by [`committee_root_ssz`]: Helios's own path tree-hashes the committee for about 11M cycles, which does
/// not fit `Init`'s 10M public budget. Same rules, same generalised indices and depths as Helios 0.12.0
/// `consensus-core/src/{consensus_core,proof}.rs` (MIT): valid execution branch for the header's fork, current
/// committee branch into `state_root`, header root equal to the checkpoint. The test
/// `bootstrap_check_agrees_with_helios` compares the result with Helios `verify_bootstrap` on real and tampered
/// bootstraps.
#[must_use]
pub fn bootstrap_valid(
    bootstrap: &Bootstrap<Spec>,
    committee_root: Bytes32,
    checkpoint: Bytes32,
    forks: &Forks,
) -> bool {
    let header = bootstrap.header();
    let beacon = header.beacon();
    let epoch = beacon.slot / 32;
    if !header_valid(&header, forks) {
        return false;
    }
    let (index, depth) = if epoch >= forks.gloas.epoch {
        (897, 11)
    } else if epoch >= forks.electra.epoch {
        (22, 6)
    } else {
        (22, 5)
    };
    root_proof_valid(
        beacon.state_root.0,
        committee_root,
        bootstrap.current_sync_committee_branch(),
        depth,
        index,
    ) && beacon.tree_hash_root().0 == checkpoint
}

/// Helios `is_valid_header`.
pub(crate) fn header_valid(header: &LightClientHeader, forks: &Forks) -> bool {
    let beacon = header.beacon();
    let epoch = beacon.slot / 32;
    if let LightClientHeader::Gloas(h) = header {
        let (index, depth) = if epoch >= forks.gloas.epoch {
            (808, 11)
        } else if epoch >= forks.deneb.epoch {
            (300, 9)
        } else if epoch >= forks.capella.epoch {
            (156, 8)
        } else {
            return h.execution_block_hash.is_zero() && h.execution_branch.iter().all(B256::is_zero);
        };
        root_proof_valid(
            beacon.body_root.0,
            h.execution_block_hash.0,
            &h.execution_branch,
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

/// Helios `is_root_proof_valid`: a branch longer than `depth` must be zero-padded at the front.
pub(crate) fn root_proof_valid(root: Bytes32, leaf: Bytes32, branch: &[B256], depth: usize, index: usize) -> bool {
    let Some(extra) = branch.len().checked_sub(depth) else {
        return false;
    };
    if branch[..extra].iter().any(|n| !n.is_zero()) {
        return false;
    }
    let mut node = leaf;
    for (i, sibling) in branch[extra..].iter().enumerate() {
        let mut x = Sha256::new();
        if (index >> i) & 1 == 1 {
            x.update(sibling.0);
            x.update(node);
        } else {
            x.update(node);
            x.update(sibling.0);
        }
        node = x.finalize().into();
    }
    node == root
}

/// Result of `Init` checks on a bootstrap.
pub struct BootstrapOutcome {
    pub store: LoadedStore,
    pub head: Head,
    pub record: HeaderRecord,
}

/// `Init` checks: config shape, Helios `verify_bootstrap` against `config.checkpoint_root`, slot match.
pub fn verify_bootstrap_ssz(config: &InstanceConfig, bootstrap_ssz: &[u8]) -> Result<BootstrapOutcome, EsaError> {
    if config.verifiers.is_empty() || config.verifiers.len() > 2 {
        return Err(EsaError::E01Malformed);
    }
    let forks = forks(config)?;
    let bootstrap = decode_bootstrap(bootstrap_ssz, &forks)?;
    // The committee's SSZ bytes inside the bootstrap: after the header's 4-byte offset (Base, Electra: the header
    // is variable-size), or after the fixed 496-byte gloas header. `decode_bootstrap` succeeded on this layout.
    let at = if matches!(bootstrap, Bootstrap::Gloas(_)) {
        496
    } else {
        4
    };
    let committee_ssz = bootstrap_ssz
        .get(at..at + COMMITTEE_SSZ_LEN)
        .ok_or(EsaError::E01Malformed)?;
    let current_committee_root = committee_root_ssz(committee_ssz)?;
    if !bootstrap_valid(&bootstrap, current_committee_root, config.checkpoint_root, &forks) {
        return Err(EsaError::E06StaleCheckpoint);
    }
    let header = bootstrap.header();
    if header.beacon().slot != config.checkpoint_slot {
        return Err(EsaError::E06StaleCheckpoint);
    }
    let mut store = Store::default();
    apply_bootstrap(&mut store, &bootstrap);
    let loaded = LoadedStore {
        store,
        current_committee_root,
        next_committee_root: [0u8; 32],
    };
    let commitment = loaded.commitment();
    let (head, record) = head_from(config, &header, commitment);
    Ok(BootstrapOutcome {
        store: loaded,
        head,
        record,
    })
}

/// Result of a verified and applied update.
pub struct UpdateOutcome {
    pub pre_store: Bytes32,
    pub post_store: Bytes32,
    pub head: Head,
    pub record: HeaderRecord,
    pub signature_slot: u64,
    /// The finalized slot moved forward (a new header record is due).
    pub new_slot: bool,
    /// A sync-committee handoff happened (current := old next).
    pub handoff: bool,
    pub participation: u64,
    pub store: LoadedStore,
}

/// `Verify` checks, Helios unmodified: decode, verify (with `expected_current_slot = signature_slot`; the chain-time
/// side is enforced by the transaction's timestamp window), check `min_participants`, apply, and require that the
/// store moved (a newer finalized header or a newly known next committee), else E14.
pub fn verify_and_apply(
    config: &InstanceConfig,
    witness: &StoreWitness,
    update_ssz: &[u8],
) -> Result<UpdateOutcome, EsaError> {
    verify_and_apply_variant(config, witness, update_ssz, Variant::V1)
}

/// Which `Verify` variant runs (docs/DECISIONS.md D-10). Same interface and same pre-checks, `apply_*` and commitments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Helios 0.12.0 `verify_update` / `verify_finality_update`, unmodified.
    V1,
    /// [`crate::lc_v2::verify`]: Helios's rules with the signature check replaced (no second subgroup check).
    #[cfg(feature = "v2")]
    V2,
}

/// V2 of [`verify_and_apply`] (feature `v2`).
#[cfg(feature = "v2")]
pub fn verify_and_apply_v2(
    config: &InstanceConfig,
    witness: &StoreWitness,
    update_ssz: &[u8],
) -> Result<UpdateOutcome, EsaError> {
    verify_and_apply_variant(config, witness, update_ssz, Variant::V2)
}

fn verify_and_apply_variant(
    config: &InstanceConfig,
    witness: &StoreWitness,
    update_ssz: &[u8],
    variant: Variant,
) -> Result<UpdateOutcome, EsaError> {
    #[cfg(all(feature = "cycle-trace", target_os = "zkvm"))]
    let mut marks = crate::lc_trace::Marks::start();
    let forks = forks(config)?;
    let mut loaded = witness.load()?;
    let pre_store = loaded.commitment();
    #[cfg(all(feature = "cycle-trace", target_os = "zkvm"))]
    marks.mark("witness load + pre-store commitment (decode and hash the committees)");
    // Pre-checks before Helios, in this order (approved 2026-11-12): decode (E01), finalized header present (E02),
    // participation >= max(min_participants, 342) (E04, before the signature check: Helios's verify does not
    // enforce the 2/3 quorum, only its apply skips silently).
    let update = decode_update(update_ssz, &forks)?;
    if update.finalized_slot() == 0 {
        return Err(EsaError::E02NotFinalised);
    }
    let signature_slot = update.signature_slot();
    let participation = update.participation();
    if participation < u64::from(config.min_participants).max(QUORUM) {
        return Err(EsaError::E04LowParticipation);
    }
    #[cfg(all(feature = "cycle-trace", target_os = "zkvm"))]
    {
        marks.mark("decode update + pre-checks");
        crate::lc_trace::probe(&loaded.store, &update);
        marks.mark("(probe itself, not part of Verify)");
    }
    let genesis_root = B256::from(config.genesis_validators_root);
    let old_slot = loaded.store.finalized_header.beacon().slot;
    let old_next_known = loaded.store.next_sync_committee.is_some();
    let old_next_root = loaded.next_committee_root;
    let old_period = calc_sync_period::<Spec>(old_slot);
    match (&update, variant) {
        (AnyUpdate::Full(u), Variant::V1) => {
            verify_update(u, signature_slot, &loaded.store, genesis_root, &forks).map_err(|e| map_update_error(&e))?;
            apply_update(&mut loaded.store, u);
        }
        (AnyUpdate::Finality(u), Variant::V1) => {
            verify_finality_update(u, signature_slot, &loaded.store, genesis_root, &forks)
                .map_err(|e| map_update_error(&e))?;
            apply_finality_update(&mut loaded.store, u);
        }
        #[cfg(feature = "v2")]
        (AnyUpdate::Full(u), Variant::V2) => {
            let g = helios_consensus_core::types::GenericUpdate::from(u);
            crate::lc_v2::verify(&g, signature_slot, &loaded.store, genesis_root, &forks)?;
            apply_update(&mut loaded.store, u);
        }
        #[cfg(feature = "v2")]
        (AnyUpdate::Finality(u), Variant::V2) => {
            let g = helios_consensus_core::types::GenericUpdate::from(u);
            crate::lc_v2::verify(&g, signature_slot, &loaded.store, genesis_root, &forks)?;
            apply_finality_update(&mut loaded.store, u);
        }
    }
    #[cfg(all(feature = "cycle-trace", target_os = "zkvm"))]
    marks.mark("Helios verify_* + apply_*");
    let new_slot_v = loaded.store.finalized_header.beacon().slot;
    let new_slot = new_slot_v > old_slot;
    let learned_next = !old_next_known && loaded.store.next_sync_committee.is_some();
    if !new_slot && !learned_next {
        return Err(EsaError::E14AlreadyApplied);
    }
    let handoff = calc_sync_period::<Spec>(new_slot_v) > old_period;
    loaded.refresh_roots(handoff, learned_next, old_next_root);
    let post_store = loaded.commitment();
    let finalized = loaded.store.finalized_header.clone();
    let (head, record) = head_from(config, &finalized, post_store);
    #[cfg(all(feature = "cycle-trace", target_os = "zkvm"))]
    marks.mark("post-store commitment + head");
    Ok(UpdateOutcome {
        pre_store,
        post_store,
        head,
        record,
        signature_slot,
        new_slot,
        handoff,
        participation,
        store: loaded,
    })
}

impl UpdateOutcome {
    /// The store after the update, as the next witness.
    #[must_use]
    pub fn new_witness(&self) -> StoreWitness {
        StoreWitness::from_store(&self.store.store)
    }
}

/// `Init` checks as a pure function: the store witness and head of a new instance.
pub fn init_store(config: &InstanceConfig, bootstrap_ssz: &[u8]) -> Result<(StoreWitness, Head), EsaError> {
    let out = verify_bootstrap_ssz(config, bootstrap_ssz)?;
    Ok((StoreWitness::from_store(&out.store.store), out.head))
}

#[cfg(test)]
mod tests {
    use helios_consensus_core::verify_bootstrap;

    use super::*;

    #[test]
    fn committee_root_matches_tree_hash() {
        // A committee with distinct keys (not curve points: hashing only) and a real aggregate-key length.
        let mut ssz = Vec::with_capacity(513 * 48);
        for i in 0..513u32 {
            let mut k = [0u8; 48];
            for (j, b) in k.iter_mut().enumerate() {
                *b = (i as u8)
                    .wrapping_mul(31)
                    .wrapping_add(j as u8)
                    .wrapping_add((i >> 8) as u8);
            }
            ssz.extend_from_slice(&k);
        }
        let c = Committee::from_ssz_bytes(&ssz).unwrap();
        assert_eq!(committee_root(&c), c.tree_hash_root().0);
        assert_eq!(committee_root_ssz(&ssz).unwrap(), c.tree_hash_root().0);
    }

    fn mainnet() -> (InstanceConfig, Vec<u8>) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/mainnet/");
        let boot = std::fs::read(format!("{dir}bootstrap_15605760.ssz")).unwrap();
        let f = |e: u64, v: [u8; 4]| (e, v);
        let config = InstanceConfig {
            chain_id: 1,
            genesis_validators_root: [0; 32],
            genesis_time: 1_606_824_023,
            forks: vec![
                f(0, [0, 0, 0, 0]),
                f(74240, [1, 0, 0, 0]),
                f(144_896, [2, 0, 0, 0]),
                f(194_048, [3, 0, 0, 0]),
                f(269_568, [4, 0, 0, 0]),
                f(364_032, [5, 0, 0, 0]),
                f(411_392, [6, 0, 0, 0]),
            ],
            checkpoint_root: [0; 32],
            checkpoint_slot: 15_605_760,
            verifiers: vec![crate::AccountId::new([1; 32])],
            min_participants: 0,
        };
        (config, boot)
    }

    #[test]
    fn bootstrap_check_agrees_with_helios() {
        let (mut config, boot) = mainnet();
        let forks = forks(&config).unwrap();
        let b = decode_bootstrap(&boot, &forks).unwrap();
        let real_root = b.header().beacon().tree_hash_root().0;
        let real_committee_root = committee_root(b.current_sync_committee());
        // Real bootstrap, right and wrong checkpoint.
        for (checkpoint, want) in [(real_root, true), ([7u8; 32], false)] {
            let ours = bootstrap_valid(&b, real_committee_root, checkpoint, &forks);
            let helios = verify_bootstrap(&b, B256::from(checkpoint), &forks).is_ok();
            assert_eq!((ours, helios), (want, want));
        }
        // Tampered bytes anywhere in the header, committee or branch: both must agree (decode may also fail).
        for pos in [10usize, 300, 600, 5000, 24_000, boot.len() - 40, boot.len() - 1] {
            let mut t = boot.clone();
            t[pos] ^= 1;
            let Ok(tb) = decode_bootstrap(&t, &forks) else { continue };
            let cr = committee_root(tb.current_sync_committee());
            let ours = bootstrap_valid(&tb, cr, real_root, &forks);
            let helios = verify_bootstrap(&tb, B256::from(real_root), &forks).is_ok();
            assert_eq!(ours, helios, "byte {pos}");
            assert!(!ours, "byte {pos} tamper accepted");
        }
        // The full Init path on the real bootstrap.
        config.checkpoint_root = real_root;
        let out = verify_bootstrap_ssz(&config, &boot).unwrap();
        assert_eq!(out.head.beacon_root, real_root);
        assert_eq!(out.store.current_committee_root, real_committee_root);
    }
}
