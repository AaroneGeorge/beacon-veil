//! Public output of guest `burn_proof`: 11 static ABI words (352 bytes), decoded by `Vault.release` with
//! `abi.decode(journal, (BurnJournal))`. The vault verifies `sha256(journal)` against the seal.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    Addr, B32,
    hash::{burn_leaf, burn_note, burn_nullifier, word, word_to_u128},
    tree::{DEPTH, root_from_path},
};

pub const JOURNAL_VERSION: u128 = 1;
pub const JOURNAL_LEN: usize = 11 * 32;

/// Field order is the Solidity struct `Vault.BurnJournal`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BurnJournal {
    pub chain_id: u64,
    pub vault: Addr,
    pub lez_program: B32,
    pub lez_instance: B32,
    pub accumulator_root: B32,
    pub nullifier: B32,
    pub recipient: Addr,
    pub token: Addr,
    pub denom: u128,
    pub fee: u128,
}

fn addr_word(a: &Addr) -> B32 {
    let mut w = [0_u8; 32];
    w[12..].copy_from_slice(a);
    w
}

fn word_addr(w: &[u8]) -> Option<Addr> {
    if w[..12].iter().any(|b| *b != 0) {
        return None;
    }
    w[12..32].try_into().ok()
}

impl BurnJournal {
    #[must_use]
    pub fn encode(&self) -> [u8; JOURNAL_LEN] {
        let words: [B32; 11] = [
            word(JOURNAL_VERSION),
            word(u128::from(self.chain_id)),
            addr_word(&self.vault),
            self.lez_program,
            self.lez_instance,
            self.accumulator_root,
            self.nullifier,
            addr_word(&self.recipient),
            addr_word(&self.token),
            word(self.denom),
            word(self.fee),
        ];
        let mut out = [0_u8; JOURNAL_LEN];
        for (i, w) in words.iter().enumerate() {
            out[i * 32..(i + 1) * 32].copy_from_slice(w);
        }
        out
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != JOURNAL_LEN {
            return None;
        }
        let w = |i: usize| -> &[u8] { &bytes[i * 32..(i + 1) * 32] };
        let b32 = |i: usize| -> B32 { w(i).try_into().unwrap_or([0; 32]) };
        if word_to_u128(&b32(0))? != JOURNAL_VERSION {
            return None;
        }
        Some(Self {
            chain_id: u64::try_from(word_to_u128(&b32(1))?).ok()?,
            vault: word_addr(w(2))?,
            lez_program: b32(3),
            lez_instance: b32(4),
            accumulator_root: b32(5),
            nullifier: b32(6),
            recipient: word_addr(w(7))?,
            token: word_addr(w(8))?,
            denom: word_to_u128(&b32(9))?,
            fee: word_to_u128(&b32(10))?,
        })
    }
}

/// Private input of guest `burn_proof` (borsh).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BurnWitness {
    pub burn_secret: B32,
    pub recipient: Addr,
    pub fee: u128,
    pub token: Addr,
    pub denom: u128,
    pub leaf_index: u64,
    pub path: [B32; DEPTH],
    /// Ethereum side of the pairing (F15).
    pub chain_id: u64,
    pub vault: Addr,
    /// LEZ side of the pairing (F15): program `bridge` account id and bridge instance id.
    pub lez_program: B32,
    pub lez_instance: B32,
}

/// The whole statement of guest `burn_proof`: the note opens to (secret, recipient, fee), its leaf (token, denom,
/// note) is at `leaf_index` under the returned root, and the nullifier comes from the secret. Runs natively (tests,
/// `bridge_cli prove --dry-run`) and in the guest.
///
/// Panics when `fee >= denom` (the vault would refuse it too).
#[must_use]
pub fn prove_burn(w: &BurnWitness) -> BurnJournal {
    assert!(w.fee < w.denom, "WB:B07 fee must be below the denomination");
    let note = burn_note(&w.burn_secret, &w.recipient, w.fee);
    let leaf = burn_leaf(&w.token, w.denom, &note);
    BurnJournal {
        chain_id: w.chain_id,
        vault: w.vault,
        lez_program: w.lez_program,
        lez_instance: w.lez_instance,
        accumulator_root: root_from_path(&leaf, w.leaf_index, &w.path),
        nullifier: burn_nullifier(&w.burn_secret),
        recipient: w.recipient,
        token: w.token,
        denom: w.denom,
        fee: w.fee,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{IncrementalTree, path_for};

    #[test]
    fn journal_round_trip() {
        let j = BurnJournal {
            chain_id: 11_155_111,
            vault: [1; 20],
            lez_program: [2; 32],
            lez_instance: [3; 32],
            accumulator_root: [4; 32],
            nullifier: [5; 32],
            recipient: [6; 20],
            token: [0; 20],
            denom: 10_u128.pow(17),
            fee: 10_u128.pow(15),
        };
        let e = j.encode();
        assert_eq!(BurnJournal::decode(&e), Some(j));
        assert_eq!(BurnJournal::decode(&e[..351]), None);
    }

    #[test]
    fn proof_statement_matches_accumulator() {
        let secret = [9_u8; 32];
        let recipient = [7_u8; 20];
        let note = burn_note(&secret, &recipient, 5);
        let mut leaves = vec![[1_u8; 32], [2_u8; 32]];
        leaves.push(burn_leaf(&[0; 20], 100, &note));
        let mut acc = IncrementalTree::default();
        for l in &leaves {
            acc.insert(*l);
        }
        let (root, path) = path_for(&leaves, 2);
        let w = BurnWitness {
            burn_secret: secret,
            recipient,
            fee: 5,
            token: [0; 20],
            denom: 100,
            leaf_index: 2,
            path,
            chain_id: 1,
            vault: [1; 20],
            lez_program: [2; 32],
            lez_instance: [3; 32],
        };
        let j = prove_burn(&w);
        assert_eq!(j.accumulator_root, root);
        assert_eq!(j.accumulator_root, acc.root());
        // a different recipient changes the note, so the root no longer matches
        let mut bad = w;
        bad.recipient = [8; 20];
        assert_ne!(prove_burn(&bad).accumulator_root, root);
    }
}
