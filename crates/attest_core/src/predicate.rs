//! Predicates (docs/DECISIONS.md D-11). Evaluation lives in `attest_inclusion::eval`.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::Bytes32;

/// Comparison operand: a big-endian u256 or an opaque bytes32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PredicateValue {
    U256(Bytes32),
    Bytes32(Bytes32),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Predicate {
    Eq(PredicateValue),
    Ne(PredicateValue),
    Lt(PredicateValue),
    Le(PredicateValue),
    Gt(PredicateValue),
    Ge(PredicateValue),
    LogTopicEq { i: u8, v: Bytes32 },
    LogDataWordEq { offset: u32, v: Bytes32 },
    All(Vec<Predicate>),
}

impl Predicate {
    /// `sha256(borsh(self))`, stored in `Statement::predicate_hash`.
    #[must_use]
    pub fn hash(&self) -> Bytes32 {
        use sha2::{Digest, Sha256};
        Sha256::digest(borsh::to_vec(self).expect("borsh encoding of Predicate")).into()
    }
}
