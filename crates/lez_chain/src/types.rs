//! Mirror of the borsh layout of LEZ v0.4.0-rc3 blocks and transactions (`lez/common/src/{block,transaction}.rs`,
//! `lee/state_machine/src/{transaction_message,public_transaction,privacy_preserving_transaction}`,
//! `lee/state_machine/core/src/{circuit_io,execution_state,program,sealing,recovery,encryption}`).
//!
//! Only the wire layout is mirrored. Ordered sets and maps (`BTreeSet`, `BTreeMap`) are encoded by borsh as a u32
//! length and the elements, the same as `Vec`, so they are `Vec` here: re-encoding gives back the exact bytes.
//! Field order is the declaration order of the LEZ structs; enum tags are the variant positions.

use alloc::vec::Vec;

use borsh::{BorshDeserialize, BorshSerialize};

pub type Hash32 = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountId(pub [u8; 32]);

/// `lee_core::account::Actor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Actor {
    pub account_id: AccountId,
    pub program_account_id: AccountId,
}

/// `lee_core::execution_state::PublicExecutionContext`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicExecutionContext {
    pub actors: Vec<Actor>,
    pub authorized_accounts: Vec<AccountId>,
    pub promotions: Vec<u64>,
}

/// `lee::PublicAccountEvidence`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PublicAccountEvidence {
    Key([u8; 32]),
    Pda { program: AccountId, seed: Hash32 },
}

/// `lee::TransactionMessage<P>`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TransactionMessage<P> {
    pub public_context: PublicExecutionContext,
    pub execution: P,
    /// `BTreeMap<AccountId, Nonce>`; `Nonce` is a u128.
    pub nonces: Vec<(AccountId, u128)>,
    pub admission_evidence: Vec<PublicAccountEvidence>,
}

/// `lee_core::RootCall`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct RootCall {
    pub to: Actor,
    pub message: Vec<u8>,
}

/// `lee::fees::FeeDeclaration`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FeeDeclaration {
    pub payer: AccountId,
    pub gas_limit: u64,
    pub tip: u64,
    pub max_fee: u128,
}

/// `lee::public_transaction::PublicExecution`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicExecution {
    pub root: RootCall,
    pub fee: Option<FeeDeclaration>,
}

/// `(Signature { value: [u8; 64] }, PublicKey([u8; 32]))`.
pub type SignatureAndKey = ([u8; 64], [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicWitnessSet {
    pub signatures_and_public_keys: Vec<SignatureAndKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicTransaction {
    pub message: TransactionMessage<PublicExecution>,
    pub witness_set: PublicWitnessSet,
}

/// `lee_core::program::MessageEnvelope<Actor>`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MessageEnvelope {
    pub from: Actor,
    pub to: Actor,
    pub message: Vec<u8>,
}

/// `lee_core::execution_state::Delivery<Actor>`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Delivery {
    pub envelope: MessageEnvelope,
    pub inherited_authorizations: Vec<AccountId>,
    pub inherits_entry_authorizations: bool,
    pub pda_seeds: Vec<Hash32>,
}

/// `lee_core::execution_state::BoundaryStep`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BoundaryStep {
    PrivateToPublic(Delivery),
    PublicToPrivate(Delivery),
    EndPrivateSubtree,
    EndPublicSubtree,
}

/// `lee_core::EncryptedNote { ciphertext: Ciphertext(Vec<u8>), epk: EphemeralPublicKey(Vec<u8>) }`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EncryptedNote {
    pub ciphertext: Vec<u8>,
    pub epk: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SealedMessage {
    pub commitment: Hash32,
    pub note: EncryptedNote,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct RecoveryBinding {
    pub address: AccountId,
    pub note: EncryptedNote,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivateAction {
    pub nullifier: Hash32,
    pub root: Hash32,
    pub commitment: Hash32,
    pub encrypted_post_state: EncryptedNote,
}

/// `ValidityWindow<T> { from: Option<T>, to: Option<T> }` for blocks, then for timestamps.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidityWindows {
    pub blocks: (Option<u64>, Option<u64>),
    pub timestamps: (Option<u64>, Option<u64>),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ProgramImageClaim {
    Disclosed { account_id: AccountId, image_id: [u32; 8] },
    Undisclosed { root: Hash32 },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FeeFallback {
    pub action: PrivateAction,
    pub boundary: Vec<BoundaryStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProvenFee {
    pub refund_to: Option<AccountId>,
    pub fallback: Option<FeeFallback>,
}

/// `lee_core::ProvenExecution`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProvenExecution {
    pub boundary: Vec<BoundaryStep>,
    pub async_messages: Vec<SealedMessage>,
    pub recovery_bindings: Vec<RecoveryBinding>,
    pub public_root: Option<RootCall>,
    pub private_actions: Vec<PrivateAction>,
    pub validity: ValidityWindows,
    pub program_image_claims: Vec<ProgramImageClaim>,
    pub fee: Option<ProvenFee>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyWitnessSet {
    pub signatures_and_public_keys: Vec<SignatureAndKey>,
    /// `Proof(Vec<u8>)`: the borsh of the inner receipt.
    pub proof: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyPreservingTransaction {
    pub message: TransactionMessage<ProvenExecution>,
    pub witness_set: PrivacyWitnessSet,
}

/// `common::transaction::LeeTransaction`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LeeTransaction {
    Public(PublicTransaction),
    PrivacyPreserving(PrivacyPreservingTransaction),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub block_id: u64,
    pub prev_block_hash: Hash32,
    pub hash: Hash32,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockBody {
    pub transactions: Vec<LeeTransaction>,
    pub reverted: Vec<u64>,
}

/// `common::block::BedrockStatus`: outside the block hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BedrockStatus {
    Pending,
    Safe,
    Finalized,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub body: BlockBody,
    pub bedrock_status: BedrockStatus,
}

// ---------------------------------------------------------------- bridge program messages (programs/bridge/src/lez.rs)

pub type Addr = [u8; 20];

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenEntry {
    pub token: Addr,
    pub denoms: Vec<u128>,
    pub mint_fee: u128,
    pub burn_fee: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BridgeConfig {
    pub chain_id: u64,
    pub vault: Addr,
    pub attest_program: AccountId,
    pub attest_instance: AccountId,
    pub tokens: Vec<TokenEntry>,
    pub treasury: Option<AccountId>,
}

/// The two variants of `BridgeMessage` a burn shows: tag 4 `Burn` (the holder's call) and tag 5 `Append` (the
/// bridge's own call to the accumulator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurnMessage {
    Burn {
        config: BridgeConfig,
        token: Addr,
        denom: u128,
        note: Hash32,
        memo: [u8; 36],
    },
    Append {
        instance: AccountId,
        token: Addr,
        denom: u128,
        note: Hash32,
        memo: [u8; 36],
    },
}

pub const BRIDGE_TAG_BURN: u8 = 4;
pub const BRIDGE_TAG_APPEND: u8 = 5;

impl BurnMessage {
    /// Decodes a whole bridge message; `None` unless it is exactly a `Burn` or an `Append`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&tag, mut rest) = bytes.split_first()?;
        let msg = match tag {
            BRIDGE_TAG_BURN => {
                let (config, token, denom, note, memo) =
                    <(BridgeConfig, Addr, u128, Hash32, [u8; 36])>::deserialize(&mut rest).ok()?;
                Self::Burn {
                    config,
                    token,
                    denom,
                    note,
                    memo,
                }
            }
            BRIDGE_TAG_APPEND => {
                let (instance, token, denom, note, memo) =
                    <(AccountId, Addr, u128, Hash32, [u8; 36])>::deserialize(&mut rest).ok()?;
                Self::Append {
                    instance,
                    token,
                    denom,
                    note,
                    memo,
                }
            }
            _ => return None,
        };
        rest.is_empty().then_some(msg)
    }
}
