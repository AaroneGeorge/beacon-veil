//! A sparse binary Merkle tree over generalized indices (SSZ `hash_tree_root` shape: `sha256(left || right)`).
//!
//! The fabricated chain never builds a full `BeaconState` or `BeaconBlockBody`. It only needs a root under which
//! chosen leaves sit at the generalized indices the light-client protocol proves (current and next sync committee,
//! finalized checkpoint root, execution payload header). Every other subtree is a deterministic filler hash, which
//! is exactly what a verifier sees of a real state: a branch of sibling hashes.

use std::collections::BTreeMap;

use alloy_primitives::B256;
use sha2::{Digest, Sha256};

/// Generalized indices, Electra/Fulu (consensus-specs `light-client/sync-protocol.md`, Electra).
pub mod gindex {
    /// `BeaconState.current_sync_committee`.
    pub const CURRENT_SYNC_COMMITTEE: u64 = 86;
    /// `BeaconState.next_sync_committee`.
    pub const NEXT_SYNC_COMMITTEE: u64 = 87;
    /// `BeaconState.finalized_checkpoint.root`.
    pub const FINALIZED_ROOT: u64 = 169;
    /// `BeaconBlockBody.execution_payload` (Capella..Fulu).
    pub const EXECUTION_PAYLOAD: u64 = 25;
}

/// Depth of a generalized index (number of branch nodes).
#[must_use]
pub fn depth(g: u64) -> usize {
    (63 - g.leading_zeros()) as usize
}

fn hash2(a: &B256, b: &B256) -> B256 {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    B256::from_slice(&h.finalize())
}

/// Sparse tree with fixed leaves; everything else is `sha256(tag || gindex)`.
pub struct SparseTree {
    leaves: BTreeMap<u64, B256>,
    tag: B256,
}

impl SparseTree {
    /// `tag` makes filler hashes differ between trees (two states never share filler nodes by accident).
    #[must_use]
    pub fn new(tag: B256, leaves: &[(u64, B256)]) -> Self {
        let leaves: BTreeMap<u64, B256> = leaves.iter().copied().collect();
        for &g in leaves.keys() {
            assert!(g >= 2, "gindex 1 is the root");
            for &h in leaves.keys() {
                if g != h {
                    assert!(!is_ancestor(g, h), "leaf {g} is an ancestor of leaf {h}");
                }
            }
        }
        Self { leaves, tag }
    }

    fn filler(&self, g: u64) -> B256 {
        let mut h = Sha256::new();
        h.update(self.tag);
        h.update(g.to_le_bytes());
        B256::from_slice(&h.finalize())
    }

    fn node(&self, g: u64) -> B256 {
        if let Some(l) = self.leaves.get(&g) {
            return *l;
        }
        if !self.leaves.keys().any(|&l| is_ancestor(g, l)) {
            return self.filler(g);
        }
        hash2(&self.node(2 * g), &self.node(2 * g + 1))
    }

    #[must_use]
    pub fn root(&self) -> B256 {
        self.node(1)
    }

    /// Siblings from the leaf up to the root (Helios / consensus-specs order).
    #[must_use]
    pub fn branch(&self, leaf: u64) -> Vec<B256> {
        let mut out = Vec::with_capacity(depth(leaf));
        let mut g = leaf;
        while g > 1 {
            out.push(self.node(g ^ 1));
            g >>= 1;
        }
        out
    }
}

/// True if `a` is a strict ancestor of `b`.
#[must_use]
pub fn is_ancestor(a: u64, b: u64) -> bool {
    let mut x = b;
    while x > a {
        x >>= 1;
    }
    x == a && a != b
}

/// Recomputes a root from a leaf and its branch (same rule as consensus-specs `is_valid_merkle_branch`).
#[must_use]
pub fn root_from_branch(leaf: B256, branch: &[B256], gindex: u64) -> B256 {
    let mut node = leaf;
    for (i, sib) in branch.iter().enumerate() {
        node = if (gindex >> i) & 1 == 1 {
            hash2(sib, &node)
        } else {
            hash2(&node, sib)
        };
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_rebuild_the_root() {
        let a = B256::repeat_byte(1);
        let b = B256::repeat_byte(2);
        let c = B256::repeat_byte(3);
        let t = SparseTree::new(B256::repeat_byte(9), &[(86, a), (87, b), (169, c)]);
        let root = t.root();
        for (g, l) in [(86, a), (87, b), (169, c)] {
            let br = t.branch(g);
            assert_eq!(br.len(), depth(g));
            assert_eq!(root_from_branch(l, &br, g), root);
        }
    }
}
