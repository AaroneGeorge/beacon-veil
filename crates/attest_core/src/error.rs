//! Error codes (docs/DECISIONS.md D-13). On chain a handler panics with the text `ESA:<code>`, for example `ESA:E11`.
//! The host dry run returns the same enum (021 U2, U5).

use borsh::{BorshDeserialize, BorshSerialize};

/// Typed error of every beacon-veil check. The discriminant is the numeric code.
///
/// Light-client mapping (`attest_core::lc`, asserted by the G2 suite):
/// - `Verify`, in this order: SSZ decode fails -> E01; no or default finalized header (optimistic update) -> E02;
///   participants < max(min_participants, 342) -> E04; then Helios: InvalidPeriod / InvalidNextSyncCommitteeProof ->
///   E05, InvalidSignature or a bad committee key -> E03, InvalidFinalityProof -> E02, InvalidExecutionPayloadProof
///   -> E15, NotRelevant -> E14, InvalidTimestamp -> E01; an update that moves nothing -> E14; config not the
///   instance's -> E09.
/// - `Init`: wrong receiver -> E09; instance exists -> E14; any Helios bootstrap rejection (root mismatch, bad
///   committee branch, bad execution branch) or slot != checkpoint_slot -> E06.
/// - `Apply`: sender not a verifier -> E12; pre-store differs -> E14 if post-store is the current one (replay), else
///   E13; no change or older slot -> E14; record.chain_id != config.chain_id -> E09; head/record disagree -> E15.
/// - `CheckHeader`: no record -> E02; hash differs -> E15. `attest_inclusion` also uses E15 for keccak(rlp header) != exec_hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum EsaError {
    E01Malformed = 1,
    E02NotFinalised = 2,
    E03BadSignature = 3,
    E04LowParticipation = 4,
    E05HandoffNotVerified = 5,
    E06StaleCheckpoint = 6,
    E07InclusionInvalid = 7,
    E08PredicateFalse = 8,
    E09ChainMismatch = 9,
    E10PredicateNotCovered = 10,
    E11AlreadyConsumed = 11,
    E12UnknownVerifier = 12,
    E13StoreMismatch = 13,
    E14AlreadyApplied = 14,
    E15HeaderMismatch = 15,
}

impl EsaError {
    /// All codes, in order.
    pub const ALL: [Self; 15] = [
        Self::E01Malformed,
        Self::E02NotFinalised,
        Self::E03BadSignature,
        Self::E04LowParticipation,
        Self::E05HandoffNotVerified,
        Self::E06StaleCheckpoint,
        Self::E07InclusionInvalid,
        Self::E08PredicateFalse,
        Self::E09ChainMismatch,
        Self::E10PredicateNotCovered,
        Self::E11AlreadyConsumed,
        Self::E12UnknownVerifier,
        Self::E13StoreMismatch,
        Self::E14AlreadyApplied,
        Self::E15HeaderMismatch,
    ];

    /// Short code, for example `E11`.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::E01Malformed => "E01",
            Self::E02NotFinalised => "E02",
            Self::E03BadSignature => "E03",
            Self::E04LowParticipation => "E04",
            Self::E05HandoffNotVerified => "E05",
            Self::E06StaleCheckpoint => "E06",
            Self::E07InclusionInvalid => "E07",
            Self::E08PredicateFalse => "E08",
            Self::E09ChainMismatch => "E09",
            Self::E10PredicateNotCovered => "E10",
            Self::E11AlreadyConsumed => "E11",
            Self::E12UnknownVerifier => "E12",
            Self::E13StoreMismatch => "E13",
            Self::E14AlreadyApplied => "E14",
            Self::E15HeaderMismatch => "E15",
        }
    }

    /// Full name as written in docs/DECISIONS.md D-13, for example `E11_ALREADY_CONSUMED`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::E01Malformed => "E01_MALFORMED",
            Self::E02NotFinalised => "E02_NOT_FINALISED",
            Self::E03BadSignature => "E03_BAD_SIGNATURE",
            Self::E04LowParticipation => "E04_LOW_PARTICIPATION",
            Self::E05HandoffNotVerified => "E05_HANDOFF_NOT_VERIFIED",
            Self::E06StaleCheckpoint => "E06_STALE_CHECKPOINT",
            Self::E07InclusionInvalid => "E07_INCLUSION_INVALID",
            Self::E08PredicateFalse => "E08_PREDICATE_FALSE",
            Self::E09ChainMismatch => "E09_CHAIN_MISMATCH",
            Self::E10PredicateNotCovered => "E10_PREDICATE_NOT_COVERED",
            Self::E11AlreadyConsumed => "E11_ALREADY_CONSUMED",
            Self::E12UnknownVerifier => "E12_UNKNOWN_VERIFIER",
            Self::E13StoreMismatch => "E13_STORE_MISMATCH",
            Self::E14AlreadyApplied => "E14_ALREADY_APPLIED",
            Self::E15HeaderMismatch => "E15_HEADER_MISMATCH",
        }
    }

    /// Panic text used on chain: `ESA:<code>`.
    #[must_use]
    pub fn panic_text(self) -> String {
        format!("ESA:{}", self.code())
    }

    /// Parses `ESA:Exx` out of a panic or rejection message (sequencer logs, wallet errors).
    #[must_use]
    pub fn from_panic_text(text: &str) -> Option<Self> {
        let idx = text.find("ESA:E")?;
        let code = text.get(idx + 4..idx + 7)?;
        Self::ALL.into_iter().find(|e| e.code() == code)
    }

    /// Panics with `ESA:<code>`. Use inside guests.
    pub fn abort(self) -> ! {
        panic!("{}", self.panic_text())
    }
}

impl core::fmt::Display for EsaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for EsaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for e in EsaError::ALL {
            assert_eq!(EsaError::from_panic_text(&e.panic_text()), Some(e));
            assert_eq!(e as u8, e.code()[1..].parse::<u8>().unwrap());
        }
    }
}
