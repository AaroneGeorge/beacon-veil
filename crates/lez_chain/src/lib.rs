//! LEZ v0.4.0-rc3 block bytes: hash, hash chain, full parse, bridge burn finder.
//!
//! What a block hash commits to (`lez/common/src/block.rs` `HashableBlockData::compute_hash`):
//! `SHA-256(PREFIX || borsh(block_id, prev_block_hash, timestamp, transactions, reverted))`. The served block is
//! `borsh(Block { header { block_id, prev_block_hash, hash, timestamp }, body { transactions, reverted },
//! bedrock_status })`, so the preimage is the served bytes without the stored hash (bytes 40..72) and without the
//! final status byte. No parse is needed to hash a block; [`parse_block`] is needed only where transactions are read.

#![no_std]
extern crate alloc;

pub mod types;

use alloc::vec::Vec;

use borsh::BorshDeserialize;
use sha2::{Digest, Sha256};
pub use types::*;

pub const BLOCK_HASH_PREFIX: &[u8; 32] = b"/LEE/v0.3/Message/Block/\x00\x00\x00\x00\x00\x00\x00\x00";
pub const PDA_PREFIX: &[u8; 32] = b"/LEE/v0.2/AccountId/PDA/\x00\x00\x00\x00\x00\x00\x00\x00";
/// `programs/bridge/src/lez.rs` `domain`.
pub const DOMAIN_INSTANCE: &[u8] = b"bridge/instance/v1";
pub const DOMAIN_ACCUMULATOR: &[u8] = b"bridge/acc/v1";

/// id (8) + previous hash (32) + stored hash (32) + timestamp (8).
pub const HEADER_LEN: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainError {
    /// Shorter than a header and a status byte, or a status byte that is not 0, 1 or 2.
    Malformed,
    /// The stored hash differs from the recomputed one.
    StoredHash { block_id: u64 },
    /// A block id is not the previous id plus one.
    IdGap { expected: u64, got: u64 },
    /// A block's previous hash is not the hash of the block before it.
    BrokenLink { block_id: u64 },
    /// The bytes do not parse as a block, or bytes are left over.
    Parse,
    /// The transaction index is out of range.
    NoSuchTransaction,
    /// The block lists the transaction as reverted.
    Reverted,
    /// The transaction is not a burn of the given instance and note.
    NotABurn,
}

#[must_use]
pub fn sha256(parts: &[&[u8]]) -> Hash32 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Header fields read at fixed offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderView {
    pub block_id: u64,
    pub prev_block_hash: Hash32,
    pub stored_hash: Hash32,
    pub timestamp: u64,
}

pub fn header(raw: &[u8]) -> Result<HeaderView, ChainError> {
    if raw.len() < HEADER_LEN + 1 + 8 || raw[raw.len() - 1] > 2 {
        return Err(ChainError::Malformed);
    }
    let u64_at = |i: usize| u64::from_le_bytes(raw[i..i + 8].try_into().expect("8 bytes"));
    Ok(HeaderView {
        block_id: u64_at(0),
        prev_block_hash: raw[8..40].try_into().expect("32 bytes"),
        stored_hash: raw[40..72].try_into().expect("32 bytes"),
        timestamp: u64_at(72),
    })
}

/// The block hash recomputed from served bytes (no parse). The body is hashed as opaque bytes: by collision
/// resistance, bytes with the right hash are the block's real body.
pub fn block_hash(raw: &[u8]) -> Result<Hash32, ChainError> {
    header(raw)?;
    Ok(sha256(&[
        BLOCK_HASH_PREFIX,
        &raw[0..40],
        &raw[72..80],
        &raw[HEADER_LEN..raw.len() - 1],
    ]))
}

/// Full strict parse: every byte is consumed.
pub fn parse_block(raw: &[u8]) -> Result<Block, ChainError> {
    let mut rest = raw;
    let block = Block::deserialize(&mut rest).map_err(|_| ChainError::Parse)?;
    if rest.is_empty() {
        Ok(block)
    } else {
        Err(ChainError::Parse)
    }
}

/// The last block of a checked chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tip {
    pub block_id: u64,
    pub hash: Hash32,
}

/// Checks that `blocks` are blocks `pinned.block_id + 1, + 2, ...`, each linked to the one before by its previous
/// hash, starting from the pinned hash; each stored hash must equal the recomputed one. Returns the last block.
pub fn verify_chain<'a>(pinned: Tip, blocks: impl IntoIterator<Item = &'a [u8]>) -> Result<Tip, ChainError> {
    let mut tip = pinned;
    for raw in blocks {
        let h = header(raw)?;
        let expected = tip.block_id.checked_add(1).ok_or(ChainError::Malformed)?;
        if h.block_id != expected {
            return Err(ChainError::IdGap {
                expected,
                got: h.block_id,
            });
        }
        if h.prev_block_hash != tip.hash {
            return Err(ChainError::BrokenLink { block_id: h.block_id });
        }
        let hash = block_hash(raw)?;
        if hash != h.stored_hash {
            return Err(ChainError::StoredHash { block_id: h.block_id });
        }
        tip = Tip {
            block_id: h.block_id,
            hash,
        };
    }
    Ok(tip)
}

/// `AccountId::for_public_pda(program, seed)`.
#[must_use]
pub fn public_pda(program: &AccountId, seed: &Hash32) -> AccountId {
    AccountId(sha256(&[PDA_PREFIX, &program.0, seed]))
}

/// The bridge instance id of a config: `PDA(bridge, sha256("bridge/instance/v1" || borsh(config)))`.
#[must_use]
pub fn instance_id(bridge: &AccountId, config: &BridgeConfig) -> AccountId {
    let bytes = borsh::to_vec(config).expect("borsh of BridgeConfig");
    public_pda(bridge, &sha256(&[DOMAIN_INSTANCE, &bytes]))
}

/// The burn accumulator of an instance: `PDA(bridge, sha256("bridge/acc/v1" || instance))`.
#[must_use]
pub fn accumulator_id(bridge: &AccountId, instance: &AccountId) -> AccountId {
    public_pda(bridge, &sha256(&[DOMAIN_ACCUMULATOR, &instance.0]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnRoute {
    /// A public transaction whose root call is `Burn` on the bridge.
    Public,
    /// A privacy-preserving transaction whose boundary delivers `Append` from the bridge to the accumulator.
    PrivateToPublic { step: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Burn {
    pub route: BurnRoute,
    pub instance: AccountId,
    pub token: Addr,
    pub denom: u128,
    pub note: Hash32,
}

/// Finds the burn of `note` for `instance` in transaction `index` of a parsed block, or refuses.
///
/// Public route: root call to an actor of program `bridge`, message `Burn{config, ...}` with
/// `instance_id(bridge, config) == instance`. Private route: a `PrivateToPublic` boundary step from an actor of
/// program `bridge` to the actor (accumulator of `instance`, program `bridge`), message `Append{instance, ...}`.
/// Either way the transaction must not be listed in `reverted`.
pub fn find_burn(
    block: &Block,
    index: usize,
    bridge: &AccountId,
    instance: &AccountId,
    note: &Hash32,
) -> Result<Burn, ChainError> {
    let tx = block
        .body
        .transactions
        .get(index)
        .ok_or(ChainError::NoSuchTransaction)?;
    if block.body.reverted.iter().any(|&r| r == index as u64) {
        return Err(ChainError::Reverted);
    }
    match tx {
        LeeTransaction::Public(tx) => {
            let root = &tx.message.execution.root;
            if root.to.program_account_id != *bridge {
                return Err(ChainError::NotABurn);
            }
            match BurnMessage::decode(&root.message) {
                Some(BurnMessage::Burn {
                    config,
                    token,
                    denom,
                    note: n,
                    ..
                }) if n == *note && instance_id(bridge, &config) == *instance => Ok(Burn {
                    route: BurnRoute::Public,
                    instance: *instance,
                    token,
                    denom,
                    note: n,
                }),
                _ => Err(ChainError::NotABurn),
            }
        }
        LeeTransaction::PrivacyPreserving(tx) => {
            let acc = Actor {
                account_id: accumulator_id(bridge, instance),
                program_account_id: *bridge,
            };
            tx.message
                .execution
                .boundary
                .iter()
                .enumerate()
                .find_map(|(step, s)| match s {
                    BoundaryStep::PrivateToPublic(d)
                        if d.envelope.to == acc && d.envelope.from.program_account_id == *bridge =>
                    {
                        match BurnMessage::decode(&d.envelope.message) {
                            Some(BurnMessage::Append {
                                instance: i,
                                token,
                                denom,
                                note: n,
                                ..
                            }) if i == *instance && n == *note => Some(Burn {
                                route: BurnRoute::PrivateToPublic { step },
                                instance: i,
                                token,
                                denom,
                                note: n,
                            }),
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .ok_or(ChainError::NotABurn)
        }
    }
}

/// Byte spans of each transaction in a block's served bytes, from the strict parse (for tests and diagnostics).
pub fn transaction_spans(raw: &[u8]) -> Result<Vec<(usize, usize)>, ChainError> {
    let mut rest = &raw[HEADER_LEN..];
    let n = u32::deserialize(&mut rest).map_err(|_| ChainError::Parse)?;
    let mut spans = Vec::new();
    for _ in 0..n {
        let start = raw.len() - rest.len();
        LeeTransaction::deserialize(&mut rest).map_err(|_| ChainError::Parse)?;
        spans.push((start, raw.len() - rest.len()));
    }
    Ok(spans)
}
