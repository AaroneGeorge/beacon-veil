//! LEZ v0.3.2 wire types the client reads (`common::block`, `common::transaction`), mirrored field for field so
//! the client does not build `common` (which links the Logos Blockchain node crates). Borsh layouts must equal
//! v0.3.2's `lez/common/src/{block,transaction}.rs`; `tests` in `main.rs` round-trip real testnet bytes.

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum LeeTransaction {
    Public(lee::PublicTransaction),
    PrivacyPreserving(lee::PrivacyPreservingTransaction),
}

impl LeeTransaction {
    /// The hash the sequencer reports: `sha256(borsh(inner transaction))`.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        match self {
            Self::Public(tx) => tx.hash(),
            Self::PrivacyPreserving(tx) => tx.hash(),
        }
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub block_id: u64,
    pub prev_block_hash: [u8; 32],
    pub hash: [u8; 32],
    pub timestamp: u64,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct BlockBody {
    pub transactions: Vec<LeeTransaction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BedrockStatus {
    Pending,
    Safe,
    Finalized,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub body: BlockBody,
    pub bedrock_status: BedrockStatus,
}
