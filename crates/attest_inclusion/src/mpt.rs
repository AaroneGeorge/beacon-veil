//! Merkle-Patricia proof checks.
//!
//! Soundness rests on one call: `alloy_trie::proof::verify_proof`, the same function Helios 0.12.0 uses for account,
//! storage and receipt proofs (`core/src/execution/proof.rs`, MIT). It checks a proof for a key against an *expected*
//! value. A consumer needs the value itself, so [`candidate_value`] first walks the proof (without hashing) to find the
//! value the proof claims for the key, and `verify_proof` then checks that claim against the root. A wrong candidate
//! can only make verification fail, never pass.

use alloy_primitives::{B256, Bytes};
use alloy_rlp::Decodable as _;
use alloy_trie::{
    Nibbles,
    nodes::{RlpNode, TrieNode},
    proof::verify_proof,
};
use attest_core::EsaError;

/// Most nodes a proof may hold: 64 nibbles of key, plus slack for extension nodes.
pub const MAX_PROOF_NODES: usize = 72;

/// Verifies `proof` for `key` under `root` and returns the value at `key`, or `None` if the proof shows the key is
/// absent (exclusion proof). Any other outcome is `E07_INCLUSION_INVALID`.
pub fn get(root: &B256, key: &Nibbles, proof: &[Vec<u8>]) -> Result<Option<Vec<u8>>, EsaError> {
    if proof.len() > MAX_PROOF_NODES {
        return Err(EsaError::E07InclusionInvalid);
    }
    let candidate = candidate_value(key, proof);
    let nodes: Vec<Bytes> = proof.iter().map(|n| Bytes::copy_from_slice(n)).collect();
    verify_proof(*root, *key, candidate.clone(), nodes.iter()).map_err(|_| EsaError::E07InclusionInvalid)?;
    Ok(candidate)
}

/// Walks the proof along `key` without hashing and returns the leaf value it reaches, if any.
fn candidate_value(key: &Nibbles, proof: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut depth = 0usize;
    for node in proof {
        let decoded = TrieNode::decode(&mut node.as_slice()).ok()?;
        match descend(decoded, key, &mut depth)? {
            Step::Value(v) => return Some(v),
            Step::NextNode => {}
        }
    }
    None
}

enum Step {
    Value(Vec<u8>),
    NextNode,
}

/// Follows one node. `None`: the key diverges here (absent) or the node is malformed.
fn descend(node: TrieNode, key: &Nibbles, depth: &mut usize) -> Option<Step> {
    if *depth > key.len() {
        return None;
    }
    match node {
        TrieNode::Branch(branch) => {
            let nibble = key.get(*depth)?;
            let child = branch.as_ref().children().find(|(i, _)| *i == nibble)?.1?.clone();
            *depth += 1;
            follow(&child, key, depth)
        }
        TrieNode::Extension(ext) => {
            if !key.slice(*depth..).starts_with(&ext.key) {
                return None;
            }
            *depth += ext.key.len();
            follow(&ext.child, key, depth)
        }
        TrieNode::Leaf(leaf) => {
            if key.slice(*depth..) != leaf.key {
                return None;
            }
            Some(Step::Value(leaf.value))
        }
        TrieNode::EmptyRoot => None,
    }
}

/// A child reference: a hash means the next proof node; anything shorter is the child node encoded in place.
fn follow(child: &RlpNode, key: &Nibbles, depth: &mut usize) -> Option<Step> {
    if child.is_hash() {
        Some(Step::NextNode)
    } else {
        descend(TrieNode::decode(&mut child.as_slice()).ok()?, key, depth)
    }
}
