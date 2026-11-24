//! Predicate evaluation (docs/DECISIONS.md D-11).
//!
//! | Predicate | Account claim | Storage claim | Log claim |
//! |---|---|---|---|
//! | `Eq`, `Ne` with `U256` or `Bytes32` | on `balance` | on the slot word | E10 |
//! | `Lt`, `Le`, `Gt`, `Ge` with `U256` | on `balance` | on the slot word | E10 |
//! | `Lt`, `Le`, `Gt`, `Ge` with `Bytes32` | E10 (bytes have no order) | E10 | E10 |
//! | `LogTopicEq { i, v }` | E10 | E10 | `topics[i] == v`; a topic the log does not have is E10 |
//! | `LogDataWordEq { offset, v }` | E10 | E10 | `data[offset..offset + 32] == v`; a word beyond `data` is E10 |
//! | `All(ps)` | every `p` holds; first failure is returned; `All([])` is true | same | same |
//!
//! Words compare as big-endian unsigned integers, which is byte-wise lexicographic order on 32 bytes.

use attest_core::{Bytes32, EsaError, Predicate, PredicateValue};

use crate::ClaimValue;

/// Deepest `All` nesting accepted; deeper is `E01_MALFORMED` (bounds guest stack and cycles).
pub const MAX_PREDICATE_DEPTH: usize = 8;
/// Most leaf predicates in one predicate tree; more is `E01_MALFORMED`.
pub const MAX_PREDICATE_TERMS: usize = 64;

/// Evaluates `predicate` on `value`: `Ok(())`, `E08_PREDICATE_FALSE`, `E10_PREDICATE_NOT_COVERED` or `E01_MALFORMED`.
/// The shape (depth, number of terms) is checked before anything is evaluated, so the outcome does not depend on
/// evaluation order.
pub fn eval(predicate: &Predicate, value: &ClaimValue) -> Result<(), EsaError> {
    let mut terms = 0usize;
    check_shape(predicate, 0, &mut terms)?;
    eval_tree(predicate, value)
}

fn check_shape(p: &Predicate, depth: usize, terms: &mut usize) -> Result<(), EsaError> {
    if depth > MAX_PREDICATE_DEPTH {
        return Err(EsaError::E01Malformed);
    }
    match p {
        Predicate::All(ps) => ps.iter().try_for_each(|q| check_shape(q, depth + 1, terms)),
        _ => {
            *terms += 1;
            if *terms > MAX_PREDICATE_TERMS {
                Err(EsaError::E01Malformed)
            } else {
                Ok(())
            }
        }
    }
}

fn eval_tree(p: &Predicate, value: &ClaimValue) -> Result<(), EsaError> {
    match p {
        Predicate::All(ps) => ps.iter().try_for_each(|q| eval_tree(q, value)),
        _ => leaf(p, value),
    }
}

fn holds(b: bool) -> Result<(), EsaError> {
    if b { Ok(()) } else { Err(EsaError::E08PredicateFalse) }
}

fn leaf(p: &Predicate, value: &ClaimValue) -> Result<(), EsaError> {
    let not_covered = Err(EsaError::E10PredicateNotCovered);
    match (p, value) {
        (Predicate::LogTopicEq { i, v }, ClaimValue::Log(log)) => {
            let topic = log
                .topics
                .get(usize::from(*i))
                .ok_or(EsaError::E10PredicateNotCovered)?;
            holds(topic == v)
        }
        (Predicate::LogDataWordEq { offset, v }, ClaimValue::Log(log)) => {
            let word = usize::try_from(*offset)
                .ok()
                .and_then(|start| log.data.get(start..start.checked_add(32)?))
                .ok_or(EsaError::E10PredicateNotCovered)?;
            holds(word == v.as_slice())
        }
        (Predicate::LogTopicEq { .. } | Predicate::LogDataWordEq { .. }, _) => not_covered,
        (_, ClaimValue::Log(_)) => not_covered,
        (cmp, ClaimValue::Word(w)) => compare(cmp, w),
        (cmp, ClaimValue::Account(a)) => compare(cmp, &a.balance),
    }
}

fn compare(p: &Predicate, w: &Bytes32) -> Result<(), EsaError> {
    use core::cmp::Ordering::{Equal, Greater, Less};
    let (operand, ordered) = match p {
        Predicate::Eq(v) | Predicate::Ne(v) => (v, false),
        Predicate::Lt(v) | Predicate::Le(v) | Predicate::Gt(v) | Predicate::Ge(v) => (v, true),
        Predicate::LogTopicEq { .. } | Predicate::LogDataWordEq { .. } | Predicate::All(_) => {
            return Err(EsaError::E10PredicateNotCovered);
        }
    };
    let v = match (operand, ordered) {
        (PredicateValue::Bytes32(_), true) => return Err(EsaError::E10PredicateNotCovered),
        (PredicateValue::U256(v) | PredicateValue::Bytes32(v), _) => v,
    };
    let ord = w.cmp(v);
    holds(match p {
        Predicate::Eq(_) => ord == Equal,
        Predicate::Ne(_) => ord != Equal,
        Predicate::Lt(_) => ord == Less,
        Predicate::Le(_) => ord != Greater,
        Predicate::Gt(_) => ord == Greater,
        Predicate::Ge(_) => ord != Less,
        _ => unreachable!("filtered above"),
    })
}
