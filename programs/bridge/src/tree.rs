//! Incremental SHA-256 Merkle tree, depth 32: the algorithm of the Ethereum deposit contract (CC0), without the
//! length mix-in. Empty leaves are zero; `zero[h+1] = sha256(zero[h] || zero[h])`. Same roots as `Vault.sol`.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{B32, hash::sha};

pub const DEPTH: usize = 32;

/// `zero[h]`: root of an empty subtree of height `h`.
#[must_use]
pub fn zero_hashes() -> [B32; DEPTH] {
    let mut z = [[0_u8; 32]; DEPTH];
    for h in 0..DEPTH - 1 {
        z[h + 1] = sha(&[&z[h], &z[h]]);
    }
    z
}

/// Append-only tree state: leaf count and the left frontier. 8 + 32 * 32 bytes.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct IncrementalTree {
    pub count: u64,
    pub branch: [B32; DEPTH],
}

impl Default for IncrementalTree {
    fn default() -> Self {
        Self {
            count: 0,
            branch: [[0; 32]; DEPTH],
        }
    }
}

impl IncrementalTree {
    /// Appends `leaf`; returns its index. Panics when full (2^32 - 1 leaves), as the vault reverts.
    pub fn insert(&mut self, leaf: B32) -> u64 {
        let index = self.count;
        assert!(index < (1_u64 << DEPTH) - 1, "tree full");
        self.count += 1;
        let mut node = leaf;
        let mut size = self.count;
        for h in 0..DEPTH {
            if size & 1 == 1 {
                self.branch[h] = node;
                return index;
            }
            node = sha(&[&self.branch[h], &node]);
            size >>= 1;
        }
        unreachable!("insert terminates within DEPTH levels")
    }

    #[must_use]
    pub fn root(&self) -> B32 {
        let zero = zero_hashes();
        let mut node = [0_u8; 32];
        let mut size = self.count;
        for (h, z) in zero.iter().enumerate() {
            node = if size & 1 == 1 {
                sha(&[&self.branch[h], &node])
            } else {
                sha(&[&node, z])
            };
            size >>= 1;
        }
        node
    }
}

/// Root implied by `leaf` at `index` with sibling `path` (leaf level first).
#[must_use]
pub fn root_from_path(leaf: &B32, index: u64, path: &[B32; DEPTH]) -> B32 {
    let mut node = *leaf;
    for (h, sib) in path.iter().enumerate() {
        node = if (index >> h) & 1 == 1 {
            sha(&[sib, &node])
        } else {
            sha(&[&node, sib])
        };
    }
    node
}

/// Host helper: root of `leaves` and the path of leaf `index`. O(n) per call.
#[must_use]
pub fn path_for(leaves: &[B32], index: usize) -> (B32, [B32; DEPTH]) {
    assert!(index < leaves.len(), "index out of range");
    let zero = zero_hashes();
    let mut layer: Vec<B32> = leaves.to_vec();
    let mut path = [[0_u8; 32]; DEPTH];
    let mut i = index;
    for h in 0..DEPTH {
        let sib = i ^ 1;
        path[h] = layer.get(sib).copied().unwrap_or(zero[h]);
        let mut next = Vec::with_capacity(layer.len().div_ceil(2));
        for pair in layer.chunks(2) {
            let r = pair.get(1).copied().unwrap_or(zero[h]);
            next.push(sha(&[&pair[0], &r]));
        }
        if next.is_empty() {
            next.push(sha(&[&zero[h], &zero[h]]));
        }
        layer = next;
        i >>= 1;
    }
    (layer[0], path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(n: u8) -> Vec<B32> {
        (0..n).map(|i| sha(&[&[i]])).collect()
    }

    #[test]
    fn empty_root_is_zero_tree() {
        let z = zero_hashes();
        assert_eq!(IncrementalTree::default().root(), sha(&[&z[DEPTH - 1], &z[DEPTH - 1]]));
    }

    #[test]
    fn incremental_matches_full_and_paths_verify() {
        let ls = leaves(11);
        let mut t = IncrementalTree::default();
        for (n, l) in ls.iter().enumerate() {
            assert_eq!(t.insert(*l), n as u64);
            let prefix = &ls[..=n];
            for (i, leaf) in prefix.iter().enumerate() {
                let (root, path) = path_for(prefix, i);
                assert_eq!(root, t.root());
                assert_eq!(root_from_path(leaf, i as u64, &path), root);
            }
        }
    }

    #[test]
    fn wrong_index_or_leaf_fails() {
        let ls = leaves(5);
        let (root, path) = path_for(&ls, 2);
        assert_ne!(root_from_path(&ls[2], 3, &path), root);
        assert_ne!(root_from_path(&ls[3], 2, &path), root);
    }
}
