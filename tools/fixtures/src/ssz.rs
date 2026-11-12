//! SSZ encoding of the Electra/Fulu light-client containers (consensus-specs Electra `LightClientHeader`,
//! `LightClientBootstrap`, `LightClientUpdate`, `LightClientFinalityUpdate`). Helios 0.12.0 derives only `Decode`
//! for these, so the fabricated chain and the tamper tests need an encoder. The leaf parts (`BeaconBlockHeader`,
//! `ExecutionPayloadHeader`, `SyncCommittee`) are encoded by Helios's own `Encode`; this module only lays out the
//! offsets. `tests::reencode_real_*` checks it byte for byte against the fetched mainnet fixtures.

use alloy_primitives::B256;
use helios_consensus_core::{
    consensus_spec::MainnetConsensusSpec,
    types::{
        BeaconBlockHeader, Bootstrap, ExecutionPayloadHeader, ExecutionPayloadHeaderElectra, FinalityUpdate,
        LightClientHeader, SyncCommittee, Update,
    },
};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

pub type Spec = MainnetConsensusSpec;
pub type Committee = SyncCommittee<Spec>;

/// Sizes of fixed parts (mainnet preset).
pub const COMMITTEE_BYTES: usize = 512 * 48 + 48;
pub const BEACON_HEADER_BYTES: usize = 112;
const OFFSET: usize = 4;
const EXEC_BRANCH: usize = 4;
const COMMITTEE_BRANCH: usize = 6;
const FINALITY_BRANCH: usize = 7;
const SYNC_AGGREGATE_BYTES: usize = 64 + 96;

/// An Electra/Fulu `LightClientHeader`.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub beacon: BeaconBlockHeader,
    pub execution: ExecutionPayloadHeader,
    pub execution_branch: Vec<B256>,
}

impl Header {
    /// The all-zero header the protocol uses for "no finalized header".
    #[must_use]
    pub fn empty() -> Self {
        Self {
            beacon: BeaconBlockHeader::default(),
            execution: ExecutionPayloadHeader::Electra(ExecutionPayloadHeaderElectra::default()),
            execution_branch: vec![B256::ZERO; EXEC_BRANCH],
        }
    }

    /// Beacon block root.
    #[must_use]
    pub fn root(&self) -> B256 {
        self.beacon.tree_hash_root()
    }

    #[must_use]
    pub fn execution_root(&self) -> B256 {
        self.execution.tree_hash_root()
    }

    #[must_use]
    pub fn ssz(&self) -> Vec<u8> {
        assert_eq!(self.execution_branch.len(), EXEC_BRANCH);
        let fixed = BEACON_HEADER_BYTES + OFFSET + EXEC_BRANCH * 32;
        let mut out = self.beacon.as_ssz_bytes();
        out.extend_from_slice(&u32::try_from(fixed).expect("offset").to_le_bytes());
        for b in &self.execution_branch {
            out.extend_from_slice(b.as_slice());
        }
        out.extend_from_slice(&self.execution.as_ssz_bytes());
        out
    }

    /// From a decoded Helios header with an execution payload header (Capella..Fulu). `None` for Bellatrix and Gloas.
    #[must_use]
    pub fn from_helios(h: &LightClientHeader) -> Option<Self> {
        let execution = h.execution().ok()?.clone();
        let execution_branch = h.execution_branch().ok()?.iter().copied().collect();
        Some(Self {
            beacon: h.beacon().clone(),
            execution,
            execution_branch,
        })
    }

    /// Mutable access to the execution payload header fields shared by every variant used here.
    pub fn execution_mut(&mut self) -> &mut ExecutionPayloadHeaderElectra {
        if !matches!(self.execution, ExecutionPayloadHeader::Electra(_)) {
            // A Deneb-shaped variant re-encoded as Electra: identical fields, SSZ and tree hash.
            let bytes = self.execution.as_ssz_bytes();
            self.execution = ExecutionPayloadHeader::Electra(
                ExecutionPayloadHeaderElectra::from_ssz_bytes(&bytes).expect("Deneb-shaped execution header"),
            );
        }
        match &mut self.execution {
            ExecutionPayloadHeader::Electra(e) => e,
            _ => unreachable!("converted above"),
        }
    }
}

/// 512 participation bits as SSZ `Bitvector[512]` (bit i in byte i/8, least significant bit first).
#[must_use]
pub fn bits_ssz(bits: &[bool]) -> Vec<u8> {
    assert_eq!(bits.len(), 512);
    let mut out = vec![0u8; 64];
    for (i, b) in bits.iter().enumerate() {
        if *b {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// An Electra/Fulu `LightClientUpdate` (when `next` is set) or `LightClientFinalityUpdate` (when it is not).
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateParts {
    pub attested: Header,
    /// Next sync committee and its branch (6 nodes). `None` encodes a `LightClientFinalityUpdate`.
    pub next: Option<(Committee, Vec<B256>)>,
    /// `Header::empty()` means "no finality".
    pub finalized: Header,
    pub finality_branch: Vec<B256>,
    pub bits: Vec<bool>,
    pub signature: [u8; 96],
    pub signature_slot: u64,
}

impl UpdateParts {
    #[must_use]
    pub fn participants(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }

    #[must_use]
    pub fn ssz(&self) -> Vec<u8> {
        assert_eq!(self.finality_branch.len(), FINALITY_BRANCH);
        let att = self.attested.ssz();
        let fin = self.finalized.ssz();
        let mut out = Vec::new();
        let tail = |out: &mut Vec<u8>| {
            for b in &self.finality_branch {
                out.extend_from_slice(b.as_slice());
            }
            out.extend_from_slice(&bits_ssz(&self.bits));
            out.extend_from_slice(&self.signature);
            out.extend_from_slice(&self.signature_slot.to_le_bytes());
        };
        if let Some((committee, branch)) = &self.next {
            assert_eq!(branch.len(), COMMITTEE_BRANCH);
            let fixed = OFFSET
                + COMMITTEE_BYTES
                + COMMITTEE_BRANCH * 32
                + OFFSET
                + FINALITY_BRANCH * 32
                + SYNC_AGGREGATE_BYTES
                + 8;
            out.extend_from_slice(&u32::try_from(fixed).expect("offset").to_le_bytes());
            out.extend_from_slice(&committee.as_ssz_bytes());
            for b in branch {
                out.extend_from_slice(b.as_slice());
            }
            out.extend_from_slice(&u32::try_from(fixed + att.len()).expect("offset").to_le_bytes());
            tail(&mut out);
        } else {
            let fixed = OFFSET + OFFSET + FINALITY_BRANCH * 32 + SYNC_AGGREGATE_BYTES + 8;
            out.extend_from_slice(&u32::try_from(fixed).expect("offset").to_le_bytes());
            out.extend_from_slice(&u32::try_from(fixed + att.len()).expect("offset").to_le_bytes());
            tail(&mut out);
        }
        out.extend_from_slice(&att);
        out.extend_from_slice(&fin);
        out
    }

    /// From a decoded Helios `LightClientUpdate` (Electra/Fulu container). `None` for other forks.
    #[must_use]
    pub fn from_update(u: &Update<Spec>) -> Option<Self> {
        let Update::Electra(_) = u else { return None };
        let agg = u.sync_aggregate();
        Some(Self {
            attested: Header::from_helios(&u.attested_header())?,
            next: Some((u.next_sync_committee().clone(), u.next_sync_committee_branch().to_vec())),
            finalized: Header::from_helios(&u.finalized_header())?,
            finality_branch: u.finality_branch().to_vec(),
            bits: agg.sync_committee_bits.iter().collect(),
            signature: agg.sync_committee_signature.as_ssz_bytes().try_into().ok()?,
            signature_slot: *u.signature_slot(),
        })
    }

    /// From a decoded Helios `LightClientFinalityUpdate` (Electra/Fulu container).
    #[must_use]
    pub fn from_finality_update(u: &FinalityUpdate<Spec>) -> Option<Self> {
        let FinalityUpdate::Electra(_) = u else { return None };
        let agg = u.sync_aggregate();
        Some(Self {
            attested: Header::from_helios(&u.attested_header())?,
            next: None,
            finalized: Header::from_helios(&u.finalized_header())?,
            finality_branch: u.finality_branch().to_vec(),
            bits: agg.sync_committee_bits.iter().collect(),
            signature: agg.sync_committee_signature.as_ssz_bytes().try_into().ok()?,
            signature_slot: *u.signature_slot(),
        })
    }

    /// Decodes SSZ bytes (full update if larger than a committee, as `attest_core::lc::decode_update` does).
    #[must_use]
    pub fn from_ssz(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > COMMITTEE_BYTES {
            Self::from_update(&Update::<Spec>::from_ssz_bytes(bytes).ok()?)
        } else {
            Self::from_finality_update(&FinalityUpdate::<Spec>::from_ssz_bytes(bytes).ok()?)
        }
    }
}

/// An Electra/Fulu `LightClientBootstrap`.
#[derive(Debug, Clone, PartialEq)]
pub struct BootstrapParts {
    pub header: Header,
    pub committee: Committee,
    pub committee_branch: Vec<B256>,
}

impl BootstrapParts {
    #[must_use]
    pub fn ssz(&self) -> Vec<u8> {
        assert_eq!(self.committee_branch.len(), COMMITTEE_BRANCH);
        let fixed = OFFSET + COMMITTEE_BYTES + COMMITTEE_BRANCH * 32;
        let mut out = u32::try_from(fixed).expect("offset").to_le_bytes().to_vec();
        out.extend_from_slice(&self.committee.as_ssz_bytes());
        for b in &self.committee_branch {
            out.extend_from_slice(b.as_slice());
        }
        out.extend_from_slice(&self.header.ssz());
        out
    }

    #[must_use]
    pub fn from_ssz(bytes: &[u8]) -> Option<Self> {
        let b = Bootstrap::<Spec>::from_ssz_bytes(bytes).ok()?;
        let Bootstrap::Electra(_) = &b else { return None };
        Some(Self {
            header: Header::from_helios(&b.header())?,
            committee: b.current_sync_committee().clone(),
            committee_branch: b.current_sync_committee_branch().to_vec(),
        })
    }
}

/// Splits a Beacon API SSZ `updates_by_range` body: per item a u64-le length (fork digest + payload), the 4-byte
/// fork digest, then the payload.
pub fn split_range(bytes: &[u8]) -> anyhow::Result<Vec<([u8; 4], Vec<u8>)>> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < bytes.len() {
        anyhow::ensure!(p + 12 <= bytes.len(), "truncated chunk header at {p}");
        let n = usize::try_from(u64::from_le_bytes(bytes[p..p + 8].try_into()?))?;
        anyhow::ensure!(n >= 4 && p + 8 + n <= bytes.len(), "bad chunk length {n} at {p}");
        let digest: [u8; 4] = bytes[p + 8..p + 12].try_into()?;
        out.push((digest, bytes[p + 12..p + 8 + n].to_vec()));
        p += 8 + n;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures_dir;

    fn read(p: &str) -> Vec<u8> {
        std::fs::read(fixtures_dir().join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
    }

    #[test]
    fn reencode_real_mainnet_updates() {
        let items = split_range(&read("mainnet/updates_1905_4.ssz")).expect("split");
        assert_eq!(items.len(), 4);
        for (_, payload) in items {
            let parts = UpdateParts::from_ssz(&payload).expect("decode Electra update");
            assert_eq!(parts.ssz(), payload, "re-encoded update differs");
        }
    }

    #[test]
    fn reencode_real_mainnet_bootstrap_and_finality() {
        let b = read("mainnet/bootstrap_15605760.ssz");
        assert_eq!(BootstrapParts::from_ssz(&b).expect("bootstrap").ssz(), b);
        let f = read("mainnet/finality_update_15637644.ssz");
        assert_eq!(UpdateParts::from_ssz(&f).expect("finality").ssz(), f);
    }
}
