//! Statement of guest `burn_chain`: a burn of this note is in a LEZ block on a hash chain from a pinned block.
//!
//! From the private witness the statement recomputes the note `sha256("wb/b/v1" || burn_secret || recipient ||
//! word(fee))` and the release nullifier `sha256("wb/bn/v1" || burn_secret)` (the bridge's own `hash.rs`), checks
//! blocks `pinned_id + 1 ..= tip` as one hash chain from `pinned_hash` ([`crate::verify_chain`]), parses the burn
//! block in full and finds the burn of the note for `(lez_program, lez_instance)` at transaction `burn_tx`, not
//! listed as reverted ([`crate::find_burn`]), with the witness's token and denomination.
//!
//! The journal (version 2, 15 static ABI words, 480 bytes) is `Vault.BurnJournal` with the accumulator root replaced by
//! `pinnedId, pinnedHash, tipId, tipHash, burnBlockId`. What it does not show: that block `tipId` with hash `tipHash`
//! is on the real, final LEZ chain (the anchor attests that), and that LEZ executed the chain correctly.

use alloc::vec::Vec;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    AccountId, Addr, B32, ChainError, Tip,
    bridge_hash::{burn_note, burn_nullifier, word, word_to_u128},
    find_burn, parse_block, verify_chain,
};

pub const CHAIN_JOURNAL_VERSION: u128 = 2;
pub const CHAIN_JOURNAL_LEN: usize = 15 * 32;

/// Private input of guest `burn_chain` (borsh).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChainWitness {
    pub burn_secret: B32,
    pub recipient: Addr,
    pub fee: u128,
    pub token: Addr,
    pub denom: u128,
    /// Ethereum side of the pairing.
    pub chain_id: u64,
    pub vault: Addr,
    /// LEZ side of the pairing: program `bridge` account id and bridge instance id.
    pub lez_program: B32,
    pub lez_instance: B32,
    /// The pinned block: its id and hash (the vault holds both as immutables).
    pub pinned_id: u64,
    pub pinned_hash: B32,
    /// Served bytes (`borsh(Block)`) of blocks `pinned_id + 1 ..= tip`, in order.
    pub blocks: Vec<Vec<u8>>,
    pub burn_block_id: u64,
    pub burn_tx: u32,
}

/// Field order is the ABI word order of the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainJournal {
    pub chain_id: u64,
    pub vault: Addr,
    pub lez_program: B32,
    pub lez_instance: B32,
    pub pinned_id: u64,
    pub pinned_hash: B32,
    pub tip_id: u64,
    pub tip_hash: B32,
    pub burn_block_id: u64,
    pub nullifier: B32,
    pub recipient: Addr,
    pub token: Addr,
    pub denom: u128,
    pub fee: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementError {
    /// `fee >= denom` (the vault refuses it too).
    FeeNotBelowDenom,
    /// The burn block is not among blocks `pinned_id + 1 ..= tip`.
    BurnBlockOutOfRange,
    /// The burn found has another token or denomination than the witness.
    TokenOrDenom,
    Chain(ChainError),
}

impl From<ChainError> for StatementError {
    fn from(e: ChainError) -> Self {
        Self::Chain(e)
    }
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

impl ChainJournal {
    #[must_use]
    pub fn encode(&self) -> [u8; CHAIN_JOURNAL_LEN] {
        let words: [B32; 15] = [
            word(CHAIN_JOURNAL_VERSION),
            word(u128::from(self.chain_id)),
            addr_word(&self.vault),
            self.lez_program,
            self.lez_instance,
            word(u128::from(self.pinned_id)),
            self.pinned_hash,
            word(u128::from(self.tip_id)),
            self.tip_hash,
            word(u128::from(self.burn_block_id)),
            self.nullifier,
            addr_word(&self.recipient),
            addr_word(&self.token),
            word(self.denom),
            word(self.fee),
        ];
        let mut out = [0_u8; CHAIN_JOURNAL_LEN];
        for (i, w) in words.iter().enumerate() {
            out[i * 32..(i + 1) * 32].copy_from_slice(w);
        }
        out
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CHAIN_JOURNAL_LEN {
            return None;
        }
        let b32 = |i: usize| -> B32 { bytes[i * 32..(i + 1) * 32].try_into().unwrap_or([0; 32]) };
        let u = |i: usize| word_to_u128(&b32(i));
        let u64w = |i: usize| u(i).and_then(|v| u64::try_from(v).ok());
        if u(0)? != CHAIN_JOURNAL_VERSION {
            return None;
        }
        Some(Self {
            chain_id: u64w(1)?,
            vault: word_addr(&b32(2))?,
            lez_program: b32(3),
            lez_instance: b32(4),
            pinned_id: u64w(5)?,
            pinned_hash: b32(6),
            tip_id: u64w(7)?,
            tip_hash: b32(8),
            burn_block_id: u64w(9)?,
            nullifier: b32(10),
            recipient: word_addr(&b32(11))?,
            token: word_addr(&b32(12))?,
            denom: u(13)?,
            fee: u(14)?,
        })
    }
}

/// The whole statement; runs natively (tests, host) and in the guest.
pub fn prove_burn_chain(w: &ChainWitness) -> Result<ChainJournal, StatementError> {
    if w.fee >= w.denom {
        return Err(StatementError::FeeNotBelowDenom);
    }
    let pinned = Tip {
        block_id: w.pinned_id,
        hash: w.pinned_hash,
    };
    let tip = verify_chain(pinned, w.blocks.iter().map(Vec::as_slice))?;
    let index = w
        .burn_block_id
        .checked_sub(w.pinned_id.checked_add(1).ok_or(StatementError::BurnBlockOutOfRange)?)
        .and_then(|i| usize::try_from(i).ok())
        .filter(|&i| i < w.blocks.len())
        .ok_or(StatementError::BurnBlockOutOfRange)?;
    let block = parse_block(&w.blocks[index])?;
    let note = burn_note(&w.burn_secret, &w.recipient, w.fee);
    let burn = find_burn(
        &block,
        w.burn_tx as usize,
        &AccountId(w.lez_program),
        &AccountId(w.lez_instance),
        &note,
    )?;
    if burn.token != w.token || burn.denom != w.denom {
        return Err(StatementError::TokenOrDenom);
    }
    Ok(ChainJournal {
        chain_id: w.chain_id,
        vault: w.vault,
        lez_program: w.lez_program,
        lez_instance: w.lez_instance,
        pinned_id: w.pinned_id,
        pinned_hash: w.pinned_hash,
        tip_id: tip.block_id,
        tip_hash: tip.hash,
        burn_block_id: w.burn_block_id,
        nullifier: burn_nullifier(&w.burn_secret),
        recipient: w.recipient,
        token: w.token,
        denom: w.denom,
        fee: w.fee,
    })
}
