//! Execution-side state for the fabricated chain: a state trie with an attacker-chosen account and storage slot,
//! and `eth_getProof`-shaped proofs (RLP nodes, root first). Built with `alloy-trie`'s `HashBuilder` and
//! `ProofRetainer`, the same library that verifies the proofs (`attest_inclusion`).

use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_trie::{EMPTY_ROOT_HASH, HashBuilder, KECCAK_EMPTY, Nibbles, TrieAccount, proof::ProofRetainer};
use sha2::{Digest, Sha256};

/// A trie built from `(key, rlp_value)` pairs, with a proof for each requested key.
fn build_trie(mut leaves: Vec<(B256, Vec<u8>)>, targets: &[B256]) -> (B256, Vec<Vec<Vec<u8>>>) {
    leaves.sort_by_key(|l| l.0);
    let target_nibbles: Vec<Nibbles> = targets.iter().map(|k| Nibbles::unpack(k)).collect();
    let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::new(target_nibbles.clone()));
    for (k, v) in &leaves {
        hb.add_leaf(Nibbles::unpack(k), v);
    }
    let root = hb.root();
    let nodes = hb.take_proof_nodes();
    let proofs = target_nibbles
        .iter()
        .map(|t| {
            nodes
                .matching_nodes_sorted(t)
                .into_iter()
                .map(|(_, b)| b.to_vec())
                .collect()
        })
        .collect();
    (root, proofs)
}

fn rand32(seed: &[u8], tag: &str, i: u64) -> B256 {
    let mut h = Sha256::new();
    h.update(seed);
    h.update(tag.as_bytes());
    h.update(i.to_le_bytes());
    B256::from_slice(&h.finalize())
}

/// The fabricated execution state and proofs of one account and one of its storage slots.
#[derive(Debug, Clone)]
pub struct FabState {
    pub state_root: B256,
    pub receipts_root: B256,
    pub address: Address,
    pub nonce: u64,
    pub balance: U256,
    pub storage_root: B256,
    pub code_hash: B256,
    pub account_proof: Vec<Vec<u8>>,
    pub slot_key: B256,
    pub slot_value: U256,
    pub storage_proof: Vec<Vec<u8>>,
}

impl FabState {
    /// A state with `others` random accounts plus `address`, whose storage holds `slot_key = slot_value` among
    /// `others` random slots. The attacker picks every value: this is "any deposit they like" (021:122).
    #[must_use]
    pub fn build(seed: &[u8], address: Address, balance: U256, slot_key: B256, slot_value: U256, others: u64) -> Self {
        // Storage trie of the target account.
        let mut slots: Vec<(B256, Vec<u8>)> = (0..others)
            .map(|i| {
                (
                    keccak256(rand32(seed, "slot", i)),
                    alloy_rlp::encode(U256::from_be_bytes(rand32(seed, "sval", i).0)),
                )
            })
            .collect();
        let hashed_slot = keccak256(slot_key);
        slots.push((hashed_slot, alloy_rlp::encode(slot_value)));
        let (storage_root, sp) = build_trie(slots, &[hashed_slot]);

        let code_hash = keccak256(b"fabricated vault code");
        let target = TrieAccount {
            nonce: 1,
            balance,
            storage_root,
            code_hash,
        };
        let mut accounts: Vec<(B256, Vec<u8>)> = (0..others)
            .map(|i| {
                let a = Address::from_slice(&rand32(seed, "acct", i)[..20]);
                let acc = TrieAccount {
                    nonce: i,
                    balance: U256::from(i) * U256::from(10u64).pow(U256::from(18u64)),
                    storage_root: EMPTY_ROOT_HASH,
                    code_hash: KECCAK_EMPTY,
                };
                (keccak256(a), alloy_rlp::encode(acc))
            })
            .collect();
        let hashed_addr = keccak256(address);
        accounts.push((hashed_addr, alloy_rlp::encode(target)));
        let (state_root, ap) = build_trie(accounts, &[hashed_addr]);

        Self {
            state_root,
            receipts_root: rand32(seed, "receipts", 0),
            address,
            nonce: 1,
            balance,
            storage_root,
            code_hash,
            account_proof: ap.into_iter().next().unwrap_or_default(),
            slot_key,
            slot_value,
            storage_proof: sp.into_iter().next().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use alloy_trie::proof::verify_proof;

    fn nodes(p: &[Vec<u8>]) -> Vec<Bytes> {
        p.iter().map(|n| Bytes::from(n.clone())).collect()
    }

    #[test]
    fn proofs_verify_with_alloy_trie() {
        let st = FabState::build(
            b"t",
            Address::repeat_byte(0xaa),
            U256::from(7u64),
            B256::ZERO,
            U256::from(1_000_000u64),
            16,
        );
        let acc = TrieAccount {
            nonce: 1,
            balance: st.balance,
            storage_root: st.storage_root,
            code_hash: st.code_hash,
        };
        verify_proof(
            st.state_root,
            Nibbles::unpack(keccak256(st.address)),
            Some(alloy_rlp::encode(acc)),
            nodes(&st.account_proof).iter(),
        )
        .expect("account proof");
        verify_proof(
            st.storage_root,
            Nibbles::unpack(keccak256(st.slot_key)),
            Some(alloy_rlp::encode(st.slot_value)),
            nodes(&st.storage_proof).iter(),
        )
        .expect("storage proof");
    }
}
