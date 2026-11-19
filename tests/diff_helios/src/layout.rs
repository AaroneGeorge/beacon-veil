//! Byte layout of SSZ `LightClientUpdate` and `LightClientFinalityUpdate` (Electra/Fulu and Gloas containers), so a
//! tamper changes exactly one field of the bytes as received. Helios 0.12.0 derives only `Decode` for these
//! containers, so tampering at the byte level keeps every other byte identical to the fetched update.
//!
//! Containers (consensus-specs, mainnet preset; Helios `types/mod.rs`):
//! - Electra/Fulu `LightClientHeader` (variable): beacon 112, offset of `execution` 4, `execution_branch` 4 x 32, then
//!   the `ExecutionPayloadHeader` (fixed part 584, then `extra_data`).
//! - Gloas `LightClientHeader` (fixed 496): beacon 112, `execution_block_hash` 32, `execution_branch` 11 x 32.
//! - `LightClientUpdate`: attested header, next committee (24,624), next-committee branch (6 or 11 x 32), finalized
//!   header, finality branch (7 or 9 x 32), sync aggregate (64 + 96), signature slot (8).
//! - `LightClientFinalityUpdate`: the same without the committee and its branch.

use std::ops::Range;

pub const COMMITTEE_LEN: usize = 513 * 48;
const BEACON_LEN: usize = 112;
const GLOAS_HEADER_LEN: usize = 496;
/// Offset of `block_number` in the fixed part of an Electra `ExecutionPayloadHeader`.
const EXEC_BLOCK_NUMBER: usize = 32 + 20 + 32 + 32 + 256 + 32;

/// Where each field of one update sits in its SSZ bytes.
#[derive(Debug, Clone)]
pub struct Layout {
    pub full: bool,
    pub gloas: bool,
    pub attested: Range<usize>,
    pub finalized: Range<usize>,
    pub next_committee: Option<Range<usize>>,
    pub next_branch: Option<Range<usize>>,
    pub finality_branch: Range<usize>,
    pub bits: Range<usize>,
    pub signature: Range<usize>,
    pub signature_slot: Range<usize>,
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

impl Layout {
    /// Layout of a full update or a finality update; `None` for anything else (optimistic updates included).
    #[must_use]
    pub fn of(b: &[u8]) -> Option<Self> {
        let n = b.len();
        // Electra full: offset(attested) | committee | branch 6 | offset(finalized) | branch 7 | aggregate | slot.
        const EF_FIXED: usize = 4 + COMMITTEE_LEN + 6 * 32 + 4 + 7 * 32 + 160 + 8;
        const GF_LEN: usize = GLOAS_HEADER_LEN + COMMITTEE_LEN + 11 * 32 + GLOAS_HEADER_LEN + 9 * 32 + 160 + 8;
        const EU_FIXED: usize = 4 + 4 + 7 * 32 + 160 + 8;
        const GU_LEN: usize = 2 * GLOAS_HEADER_LEN + 9 * 32 + 160 + 8;
        if n > COMMITTEE_LEN && u32_at(b, 0) == Some(EF_FIXED) {
            let fin = u32_at(b, 4 + COMMITTEE_LEN + 6 * 32)?;
            if !(EF_FIXED..n).contains(&fin) {
                return None;
            }
            let mut p = 4;
            let next_committee = p..p + COMMITTEE_LEN;
            p += COMMITTEE_LEN;
            let next_branch = p..p + 6 * 32;
            p += 6 * 32 + 4;
            let finality_branch = p..p + 7 * 32;
            p += 7 * 32;
            return Some(Self {
                full: true,
                gloas: false,
                attested: EF_FIXED..fin,
                finalized: fin..n,
                next_committee: Some(next_committee),
                next_branch: Some(next_branch),
                finality_branch,
                bits: p..p + 64,
                signature: p + 64..p + 160,
                signature_slot: p + 160..p + 168,
            });
        }
        if n == GF_LEN {
            let mut p = GLOAS_HEADER_LEN;
            let next_committee = p..p + COMMITTEE_LEN;
            p += COMMITTEE_LEN;
            let next_branch = p..p + 11 * 32;
            p += 11 * 32;
            let finalized = p..p + GLOAS_HEADER_LEN;
            p += GLOAS_HEADER_LEN;
            let finality_branch = p..p + 9 * 32;
            p += 9 * 32;
            return Some(Self {
                full: true,
                gloas: true,
                attested: 0..GLOAS_HEADER_LEN,
                finalized,
                next_committee: Some(next_committee),
                next_branch: Some(next_branch),
                finality_branch,
                bits: p..p + 64,
                signature: p + 64..p + 160,
                signature_slot: p + 160..p + 168,
            });
        }
        if n < COMMITTEE_LEN && u32_at(b, 0) == Some(EU_FIXED) {
            let fin = u32_at(b, 4)?;
            if !(EU_FIXED..n).contains(&fin) {
                return None;
            }
            let p = 8 + 7 * 32;
            return Some(Self {
                full: false,
                gloas: false,
                attested: EU_FIXED..fin,
                finalized: fin..n,
                next_committee: None,
                next_branch: None,
                finality_branch: 8..p,
                bits: p..p + 64,
                signature: p + 64..p + 160,
                signature_slot: p + 160..p + 168,
            });
        }
        if n == GU_LEN {
            let p = 2 * GLOAS_HEADER_LEN + 9 * 32;
            return Some(Self {
                full: false,
                gloas: true,
                attested: 0..GLOAS_HEADER_LEN,
                finalized: GLOAS_HEADER_LEN..2 * GLOAS_HEADER_LEN,
                next_committee: None,
                next_branch: None,
                finality_branch: 2 * GLOAS_HEADER_LEN..p,
                bits: p..p + 64,
                signature: p + 64..p + 160,
                signature_slot: p + 160..p + 168,
            });
        }
        None
    }

    /// Byte range of `slot` in a header that starts at `h.start`.
    #[must_use]
    pub fn slot_of(h: &Range<usize>) -> Range<usize> {
        h.start..h.start + 8
    }

    /// Byte range of `proposer_index` in a header.
    #[must_use]
    pub fn proposer_of(h: &Range<usize>) -> Range<usize> {
        h.start + 8..h.start + 16
    }

    /// The execution field a tamper changes in a header: `block_number` of the Electra execution payload header, or
    /// the Gloas `execution_block_hash`. Both are covered by the header's execution branch (Helios
    /// `is_valid_header`).
    #[must_use]
    pub fn execution_field(&self, b: &[u8], h: &Range<usize>) -> Option<Range<usize>> {
        if self.gloas {
            return Some(h.start + BEACON_LEN..h.start + BEACON_LEN + 32);
        }
        let exec = h.start + u32_at(b, h.start + BEACON_LEN)?;
        (exec + EXEC_BLOCK_NUMBER + 8 <= h.end).then(|| exec + EXEC_BLOCK_NUMBER..exec + EXEC_BLOCK_NUMBER + 8)
    }

    /// Bytes of a header that may be zeroed while the container still decodes: everything except the offset fields
    /// (Electra: the header's `execution` offset and the payload's `extra_data` offset).
    #[must_use]
    pub fn zeroable(&self, b: &[u8], h: &Range<usize>) -> Vec<Range<usize>> {
        if self.gloas {
            return vec![h.clone()];
        }
        let Some(exec) = u32_at(b, h.start + BEACON_LEN).map(|o| h.start + o) else {
            return vec![];
        };
        // extra_data offset sits after block_number, gas_limit, gas_used, timestamp.
        let extra_off = exec + EXEC_BLOCK_NUMBER + 32;
        vec![
            h.start..h.start + BEACON_LEN,
            h.start + BEACON_LEN + 4..extra_off,
            extra_off + 4..h.end,
        ]
    }

    #[must_use]
    pub fn signature_slot(&self, b: &[u8]) -> u64 {
        u64::from_le_bytes(b[self.signature_slot.clone()].try_into().expect("8 bytes"))
    }

    #[must_use]
    pub fn attested_slot(&self, b: &[u8]) -> u64 {
        u64::from_le_bytes(b[Self::slot_of(&self.attested)].try_into().expect("8 bytes"))
    }

    /// Participation bit `i` (SSZ bitvector, little-endian bit order).
    #[must_use]
    pub fn bit(&self, b: &[u8], i: usize) -> bool {
        b[self.bits.start + i / 8] >> (i % 8) & 1 == 1
    }

    pub fn set_bit(&self, b: &mut [u8], i: usize, v: bool) {
        let at = self.bits.start + i / 8;
        if v {
            b[at] |= 1 << (i % 8);
        } else {
            b[at] &= !(1 << (i % 8));
        }
    }
}

#[cfg(test)]
mod tests {
    use attest_core::lc::{AnyUpdate, decode_update};

    use super::*;
    use crate::load::{Network, load_network};

    /// The layout agrees with Helios's own decoder on every fetched update: header slots, signature slot and the
    /// participation count read from the bytes equal the decoded values.
    #[test]
    fn layout_matches_helios_decode() {
        for name in ["mainnet", "sepolia", "hoodi"] {
            let net: Network = load_network(name).expect("fixtures");
            let forks = attest_core::lc::forks(&net.config).expect("forks");
            let mut seen = 0;
            for u in net
                .ranges
                .iter()
                .flat_map(|r| r.updates.iter())
                .chain(net.finality.iter())
            {
                let l = Layout::of(&u.ssz).expect("layout");
                let d = decode_update(&u.ssz, &forks).expect("decode");
                assert_eq!(l.full, matches!(d, AnyUpdate::Full(_)));
                assert_eq!(l.attested_slot(&u.ssz), d.attested_header().beacon().slot);
                assert_eq!(l.signature_slot(&u.ssz), d.signature_slot());
                let fin = u64::from_le_bytes(u.ssz[Layout::slot_of(&l.finalized)].try_into().unwrap());
                assert_eq!(fin, d.finalized_slot());
                let bits = (0..512).filter(|i| l.bit(&u.ssz, *i)).count() as u64;
                assert_eq!(bits, d.participation());
                seen += 1;
            }
            assert!(seen >= 6, "{name}: {seen}");
        }
    }
}
