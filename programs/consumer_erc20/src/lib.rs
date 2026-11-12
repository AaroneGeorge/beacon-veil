//! `consumer_erc20`: the reference consumer of an attested ERC-20 `Transfer` (021 U6, first case; PLAN.md 2.3 consumer
//! rule). Shared types for the guest (`src/bin/consumer_erc20.rs`) and host code.
//!
//! # Pattern (what any consumer of `attest` does)
//!
//! 1. **Configuration is the address.** A consumer instance lives at
//!    `PDA(consumer, sha256("c20/config/v1" || borsh(config)))`; `Init` writes the config there once. The config
//!    names the `attest` program, the light-client instance and chain id it trusts, the token and the recipient.
//! 2. **Sender check.** A `Statement` is accepted only if the runtime says it came from `config.attest_program`
//!    (`input.from.program_account_id`), and only for `config.instance` and `config.chain_id`.
//! 3. **Read the value, not the predicate.** The consumer decodes the proven log from `Statement::value` and checks
//!    emitter, `Transfer` signature and recipient itself. The predicate the caller chose cannot widen what is accepted.
//! 4. **Nullifier.** Before crediting, a sync `MarkConsumed` goes to
//!    `PDA(consumer, sha256("c20/null/v1" || consumer_instance || statement.id))`, whose state must be EMPTY
//!    (`E11_ALREADY_CONSUMED` otherwise; `write_once()` is not used because it accepts an identical rewrite). A panic
//!    there rejects the whole transaction, credit included.
//!
//! Why the nullifier includes the consumer instance: anyone can create a consumer instance with any config,
//! including one that trusts a fake `attest` program. If the nullifier were `PDA(consumer, statement.id)` alone, such
//! an instance could mark a real statement's id as used before the honest instance sees it (a griefing front-run).
//! Scoping it to the instance makes that impossible; each instance refuses replays of its own statements.
//!
//! | Check | Error |
//! |---|---|
//! | instance state missing or not decodable; message not decodable | `E01_MALFORMED` |
//! | sender program is not `config.attest_program` | `E12_UNKNOWN_VERIFIER` |
//! | `statement.instance` or `statement.chain_id` differs from config | `E09_CHAIN_MISMATCH` |
//! | statement is not a log statement | `E10_PREDICATE_NOT_COVERED` |
//! | log is not `Transfer(from, config.recipient, amount)` emitted by `config.token` | `E08_PREDICATE_FALSE` |
//! | statement id already consumed by this instance | `E11_ALREADY_CONSUMED` |

#![forbid(unsafe_code)]

use attest_core::{AccountId, Bytes32, EsaError, Statement};
use attest_inclusion::LogValue;
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::program::PdaSeed;
use sha2::{Digest, Sha256};

/// Domain strings of this program.
pub mod domain {
    pub const CONFIG: &[u8] = b"c20/config/v1";
    pub const NULLIFIER: &[u8] = b"c20/null/v1";
}

/// `keccak256("Transfer(address,address,uint256)")`
pub const TRANSFER_TOPIC: Bytes32 = [
    0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b, 0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d, 0xaa, 0x95, 0x2b, 0xa7,
    0xf1, 0x63, 0xc4, 0xa1, 0x16, 0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23, 0xb3, 0xef,
];

/// Immutable configuration of a consumer instance.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ConsumerConfig {
    /// Program id of `attest` (the only accepted statement sender).
    pub attest_program: AccountId,
    /// Light-client instance whose statements are accepted.
    pub instance: AccountId,
    pub chain_id: u64,
    /// ERC-20 contract that must emit the `Transfer`.
    pub token: [u8; 20],
    /// The specified recipient contract (indexed `to`).
    pub recipient: [u8; 20],
    /// Free value that separates instances with the same trust settings (two apps, test reruns).
    pub salt: u64,
}

/// One accepted transfer.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Credit {
    pub statement_id: Bytes32,
    pub from: [u8; 20],
    /// Big-endian u256.
    pub amount: Bytes32,
    pub exec_number: u64,
    /// Subject of the statement: emitter ‖ tx_index LE ‖ log_index LE.
    pub subject: Vec<u8>,
}

/// State of a consumer instance.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ConsumerState {
    pub config: ConsumerConfig,
    /// Number of transfers accepted.
    pub count: u64,
    /// Sum of accepted amounts, big-endian u256 (saturating).
    pub total: Bytes32,
    pub last: Option<Credit>,
}

/// Messages a consumer instance or nullifier accepts from a transaction or from its own program. A `Statement`
/// from `attest` arrives as a bare borsh `Statement` (the sender decides how the bytes are read).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ConsumerMessage {
    /// Top-level; receiver must be `consumer_instance_id(program, config)` and empty.
    Init { config: ConsumerConfig },
    /// Own program only; receiver must be `nullifier_id(program, consumer_instance, statement_id)` and empty.
    MarkConsumed {
        consumer_instance: AccountId,
        statement_id: Bytes32,
    },
}

/// `sha256("c20/config/v1" || borsh(config))`
#[must_use]
pub fn consumer_instance_seed(config: &ConsumerConfig) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain::CONFIG);
    h.update(borsh::to_vec(config).expect("borsh encoding of ConsumerConfig"));
    h.finalize().into()
}

#[must_use]
pub fn consumer_instance_id(program: &AccountId, config: &ConsumerConfig) -> AccountId {
    AccountId::for_public_pda(program, &PdaSeed::new(consumer_instance_seed(config)))
}

/// `sha256("c20/null/v1" || consumer_instance || statement_id)`
#[must_use]
pub fn nullifier_seed(consumer_instance: &AccountId, statement_id: &Bytes32) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain::NULLIFIER);
    h.update(consumer_instance.value());
    h.update(statement_id);
    h.finalize().into()
}

#[must_use]
pub fn nullifier_id(program: &AccountId, consumer_instance: &AccountId, statement_id: &Bytes32) -> AccountId {
    AccountId::for_public_pda(program, &PdaSeed::new(nullifier_seed(consumer_instance, statement_id)))
}

/// The address word of an indexed `address` topic: 12 zero bytes then the address.
#[must_use]
pub fn address_topic(a: &[u8; 20]) -> Bytes32 {
    let mut t = [0u8; 32];
    t[12..].copy_from_slice(a);
    t
}

/// Big-endian u256 addition, saturating at 2^256 - 1.
#[must_use]
pub fn add_u256(a: &Bytes32, b: &Bytes32) -> Bytes32 {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in (0..32).rev() {
        let s = u16::from(a[i]) + u16::from(b[i]) + carry;
        out[i] = (s & 0xff) as u8;
        carry = s >> 8;
    }
    if carry != 0 { [0xff; 32] } else { out }
}

/// Every check a consumer instance makes on a delivered statement (the pure part of the handler, also used by host
/// dry runs). `sender_program` is `input.from.program_account_id` as the runtime reports it.
pub fn accept(state: &ConsumerState, sender_program: &AccountId, st: &Statement) -> Result<Credit, EsaError> {
    let cfg = &state.config;
    if *sender_program != cfg.attest_program {
        return Err(EsaError::E12UnknownVerifier);
    }
    if st.instance != cfg.instance || st.chain_id != cfg.chain_id {
        return Err(EsaError::E09ChainMismatch);
    }
    if st.kind != attest_core::ClaimKind::Log {
        return Err(EsaError::E10PredicateNotCovered);
    }
    let log = LogValue::decode(&st.value).ok_or(EsaError::E01Malformed)?;
    if st.subject.get(..20) != Some(log.address.as_slice()) {
        return Err(EsaError::E01Malformed);
    }
    let is_transfer = log.address == cfg.token
        && log.topics.len() == 3
        && log.topics[0] == TRANSFER_TOPIC
        && log.topics[2] == address_topic(&cfg.recipient)
        && log.topics[1][..12] == [0u8; 12]
        && log.data.len() == 32;
    if !is_transfer {
        return Err(EsaError::E08PredicateFalse);
    }
    let from: [u8; 20] = log.topics[1][12..].try_into().expect("20 bytes");
    let amount = log.data_word(0).expect("data is 32 bytes");
    Ok(Credit {
        statement_id: st.id,
        from,
        amount,
        exec_number: st.exec_number,
        subject: st.subject.clone(),
    })
}

impl ConsumerState {
    #[must_use]
    pub fn new(config: ConsumerConfig) -> Self {
        Self {
            config,
            count: 0,
            total: [0; 32],
            last: None,
        }
    }

    /// State after accepting `credit`.
    #[must_use]
    pub fn credited(&self, credit: Credit) -> Self {
        Self {
            config: self.config.clone(),
            count: self.count.saturating_add(1),
            total: add_u256(&self.total, &credit.amount),
            last: Some(credit),
        }
    }
}

/// Event selector `sha256("consumer_erc20::Credited")[..8]`; event data is `borsh(Credit)`.
#[must_use]
pub fn credited_selector() -> [u8; 8] {
    let h: Bytes32 = Sha256::digest(b"consumer_erc20::Credited").into();
    h[..8].try_into().expect("8 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_topic_is_keccak_of_signature() {
        assert_eq!(
            attest_inclusion::keccak(b"Transfer(address,address,uint256)"),
            TRANSFER_TOPIC
        );
    }

    #[test]
    fn u256_add() {
        let mut a = [0u8; 32];
        a[31] = 0xff;
        let mut one = [0u8; 32];
        one[31] = 1;
        let s = add_u256(&a, &one);
        assert_eq!(s[30], 1);
        assert_eq!(s[31], 0);
        assert_eq!(add_u256(&[0xff; 32], &one), [0xff; 32]);
    }
}
