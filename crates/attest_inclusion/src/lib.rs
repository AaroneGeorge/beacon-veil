//! Inclusion proofs and predicates under a header record's roots (docs/DECISIONS.md D-11).
//!
//! Pure functions, no I/O. The same code runs natively (host dry run, tests, tools) and inside the RISC Zero guest
//! (`attest` program's `Attest` handler, the bridge's claim).
//!
//! # The single entry point
//!
//! [`verify_claim`] is what the `attest` program, `attest_host::dry_run_attest` and the bridge call:
//!
//! ```text
//! verify_claim(instance, record, claim_bytes, predicate) -> Result<Statement, EsaError>
//! ```
//!
//! It decodes `claim_bytes` as a borsh [`Claim`], checks the proof under the record's roots, evaluates the predicate
//! and returns the [`Statement`] to deliver. It does **not** check that `record` is a finalised header of `instance`:
//! the caller does that (the `attest` program sends `CheckHeader{record.hash()}` to `record_id(instance, slot)`).
//!
//! | Error | When |
//! |---|---|
//! | `E01_MALFORMED` | `claim_bytes` is not exactly one borsh `Claim`; predicate deeper than 8 or above 64 terms; record has zero execution roots (gloas) and the claim carries no `exec_header` |
//! | `E07_INCLUSION_INVALID` | a proof does not verify under the root, a value does not decode, `log_index` absent |
//! | `E08_PREDICATE_FALSE` | the predicate is false on the proven value |
//! | `E10_PREDICATE_NOT_COVERED` | the predicate talks about a field the claim kind does not prove |
//! | `E15_HEADER_MISMATCH` | `exec_header` does not hash to `record.exec_hash`, does not decode, or disagrees with a non-zero record root or number |
//!
//! # Claim layout (`AttestMessage::Attest.claim`)
//!
//! `claim_bytes = borsh(Claim { exec_header: Option<rlp header>, proof: ClaimProof })`. Proof nodes are the RLP nodes as `eth_getProof` returns them (root first). The
//! receipt proof is built by `tools/proofgen` from `eth_getBlockReceipts`.
//!
//! | Variant | Proven under | Key |
//! |---|---|---|
//! | `Account { address, account_proof }` | `stateRoot` | `keccak(address)` |
//! | `Storage { address, slot_key, account_proof, storage_proof }` | the account's `storage_root` | `keccak(slot_key)` |
//! | `Log { tx_index, log_index, receipt_proof }` | `receiptsRoot` | `rlp(tx_index)` |
//!
//! `stateRoot`, `receiptsRoot` and the block number come from the record (pre-gloas records carry them) or, when
//! the record's roots are zero, from `exec_header` checked against `record.exec_hash` ([`exec_roots`]). Gloas
//! (EIP-7732) light-client headers prove only `execution_block_hash`, which is the bid's `parent_block_hash`: the
//! latest execution block that was delivered and verified, so a gloas record attests the parent execution block of
//! the finalised beacon block (consensus-specs `gloas/light-client/sync-protocol.md`, `EXECUTION_BLOCK_HASH_GINDEX_GLOAS`).
//!
//! `log_index` is the position of the log **inside its receipt** (not the block-wide `logIndex` of JSON-RPC).
//! Absent accounts and zero storage slots are proven by exclusion proofs and attest the zero value.
//!
//! # Statement layout (what a consumer reads)
//!
//! | Kind | `subject` | `value` | `id` |
//! |---|---|---|---|
//! | Account | `address` (20) | [`AccountValue::encode`]: `nonce` u64 BE (8) ‖ `balance` (32) ‖ `storage_root` (32) ‖ `code_hash` (32) | `statement_id_state(chain_id, address, [], value)` |
//! | Storage | `address` (20) ‖ `slot_key` (32) | the 32-byte big-endian word | `statement_id_state(chain_id, address, slot_key, value)` |
//! | Log | `emitter` (20) ‖ `tx_index` u64 LE (8) ‖ `log_index` u64 LE (8) | `rlp(log)` ([`LogValue::decode`]) | `statement_id_log(chain_id, exec_hash, tx_index, log_index, keccak(rlp(log)))` |
//!
//! `chain_id` and `slot` come from the record, `exec_number` from [`exec_roots`]; `predicate_hash = predicate.hash()`.
//! Account and storage ids do not depend on the block: the same state at two heights has the same id (F6), so a
//! consumer's nullifier on the id refuses a second use of the same fact. A log id is unique per log.
//!
//! # Reuse
//!
//! MPT verification is `alloy_trie::proof::verify_proof` (MIT/Apache-2.0), as Helios 0.12.0 uses it
//! (`core/src/execution/proof.rs`, MIT, including its treatment of exclusion proofs). Keccak is `alloy_primitives`'s
//! `keccak256`; with this workspace's features it runs on `sha3` 0.11 → `keccak` 0.2, which the workspace patches to
//! the LEZ fork that calls the RISC Zero keccak accelerator in the guest. We wrote only the value extraction, the
//! receipt/log decoding, predicates and the statement layout.

#![forbid(unsafe_code)]

mod mpt;
mod predicate;
mod rlp;

use alloy_primitives::{B256, keccak256};
use alloy_trie::{EMPTY_ROOT_HASH, KECCAK_EMPTY, Nibbles};
use attest_core::{AccountId, Bytes32, ClaimKind, EsaError, HeaderRecord, Predicate, Statement};
pub use attest_core::{statement_id_log, statement_id_state};
use borsh::{BorshDeserialize, BorshSerialize};

pub use mpt::MAX_PROOF_NODES;
pub use predicate::{MAX_PREDICATE_DEPTH, MAX_PREDICATE_TERMS, eval};

/// The inclusion claim carried in `AttestMessage::Attest.claim` (borsh-encoded). See the crate docs for keys and roots.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Claim {
    /// RLP execution block header whose keccak is `record.exec_hash`. Required when the record's execution roots are
    /// zero (gloas: the light-client header carries only the execution block hash); optional otherwise, and then it
    /// must agree with the record.
    pub exec_header: Option<Vec<u8>>,
    pub proof: ClaimProof,
}

/// What is proven, and the proof nodes.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ClaimProof {
    Account {
        address: [u8; 20],
        account_proof: Vec<Vec<u8>>,
    },
    Storage {
        address: [u8; 20],
        slot_key: Bytes32,
        account_proof: Vec<Vec<u8>>,
        storage_proof: Vec<Vec<u8>>,
    },
    Log {
        tx_index: u64,
        log_index: u64,
        receipt_proof: Vec<Vec<u8>>,
    },
}

impl Claim {
    /// `borsh(self)`: the bytes to put in `AttestMessage::Attest.claim`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("borsh encoding of Claim")
    }

    /// Strict decode: exactly one `Claim`, no trailing bytes. `E01_MALFORMED` otherwise.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EsaError> {
        borsh::from_slice(bytes).map_err(|_| EsaError::E01Malformed)
    }

    #[must_use]
    pub const fn kind(&self) -> ClaimKind {
        match self.proof {
            ClaimProof::Account { .. } => ClaimKind::Account,
            ClaimProof::Storage { .. } => ClaimKind::Storage,
            ClaimProof::Log { .. } => ClaimKind::Log,
        }
    }
}

/// Execution-block fields a claim is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecRoots {
    pub number: u64,
    pub state_root: Bytes32,
    pub receipts_root: Bytes32,
}

/// Resolves the execution roots of `record`, using `exec_header` when given (see [`Claim::exec_header`]).
/// `E01_MALFORMED`: header missing while the record has no roots. `E15_HEADER_MISMATCH`: header does not hash to
/// `record.exec_hash`, does not decode, or disagrees with a non-zero record field.
pub fn exec_roots(record: &HeaderRecord, exec_header: Option<&[u8]>) -> Result<ExecRoots, EsaError> {
    let from_record = ExecRoots {
        number: record.exec_number,
        state_root: record.exec_state_root,
        receipts_root: record.exec_receipts_root,
    };
    let Some(h) = exec_header else {
        if record.exec_state_root == [0; 32] && record.exec_receipts_root == [0; 32] {
            return Err(EsaError::E01Malformed);
        }
        return Ok(from_record);
    };
    if record.exec_hash == [0; 32] || keccak256(h).0 != record.exec_hash {
        return Err(EsaError::E15HeaderMismatch);
    }
    let (state_root, receipts_root, number) = rlp::decode_exec_header(h)?;
    let agrees = |r: &[u8; 32], v: &[u8; 32]| *r == [0; 32] || r == v;
    if !agrees(&record.exec_state_root, &state_root)
        || !agrees(&record.exec_receipts_root, &receipts_root)
        || (record.exec_number != 0 && record.exec_number != number)
    {
        return Err(EsaError::E15HeaderMismatch);
    }
    Ok(ExecRoots {
        number,
        state_root,
        receipts_root,
    })
}

/// Ethereum account fields proven under a state root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountValue {
    pub nonce: u64,
    /// Big-endian u256.
    pub balance: Bytes32,
    pub storage_root: Bytes32,
    pub code_hash: Bytes32,
}

impl Default for AccountValue {
    /// The value of an absent account: zero nonce and balance, empty storage, empty code.
    fn default() -> Self {
        Self {
            nonce: 0,
            balance: [0; 32],
            storage_root: EMPTY_ROOT_HASH.0,
            code_hash: KECCAK_EMPTY.0,
        }
    }
}

impl AccountValue {
    /// Length of [`Self::encode`].
    pub const ENCODED_LEN: usize = 104;

    /// Statement value layout: `nonce` u64 BE ‖ `balance` ‖ `storage_root` ‖ `code_hash`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(Self::ENCODED_LEN);
        v.extend_from_slice(&self.nonce.to_be_bytes());
        v.extend_from_slice(&self.balance);
        v.extend_from_slice(&self.storage_root);
        v.extend_from_slice(&self.code_hash);
        v
    }

    /// Inverse of [`Self::encode`].
    #[must_use]
    pub fn decode(v: &[u8]) -> Option<Self> {
        if v.len() != Self::ENCODED_LEN {
            return None;
        }
        Some(Self {
            nonce: u64::from_be_bytes(v[0..8].try_into().ok()?),
            balance: v[8..40].try_into().ok()?,
            storage_root: v[40..72].try_into().ok()?,
            code_hash: v[72..104].try_into().ok()?,
        })
    }
}

/// A receipt log proven under a receipts root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogValue {
    pub address: [u8; 20],
    pub topics: Vec<Bytes32>,
    pub data: Vec<u8>,
}

impl LogValue {
    /// `rlp([address, topics, data])`: the Log statement value.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        rlp::encode_log(self)
    }

    /// Decodes a Log statement value (strict RLP). Consumers use this on `Statement::value`.
    #[must_use]
    pub fn decode(rlp_log: &[u8]) -> Option<Self> {
        rlp::decode_log(rlp_log).ok()
    }

    /// 32-byte word of `data` at byte `offset`, if in range (ABI-encoded non-indexed arguments).
    #[must_use]
    pub fn data_word(&self, offset: usize) -> Option<Bytes32> {
        self.data.get(offset..offset.checked_add(32)?)?.try_into().ok()
    }
}

/// Value a predicate is evaluated against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimValue {
    /// Account claim. Word predicates compare against `balance`.
    Account(AccountValue),
    /// Storage claim: the slot word.
    Word(Bytes32),
    Log(LogValue),
}

/// Keccak-256 (accelerated in the guest, see crate docs).
#[must_use]
pub fn keccak(data: &[u8]) -> Bytes32 {
    keccak256(data).0
}

fn keccak_nibbles(data: &[u8]) -> Nibbles {
    Nibbles::unpack(keccak256(data))
}

/// Verifies `address`'s account under `state_root` (MPT proof as in `eth_getProof`, RLP nodes, root first). An
/// exclusion proof returns [`AccountValue::default`].
pub fn verify_account(state_root: &Bytes32, address: &[u8; 20], proof: &[Vec<u8>]) -> Result<AccountValue, EsaError> {
    match mpt::get(&B256::from(*state_root), &keccak_nibbles(address), proof)? {
        Some(raw) => rlp::decode_account(&raw),
        None => Ok(AccountValue::default()),
    }
}

/// Verifies a storage slot under an account's `storage_root`. Returns the 32-byte big-endian value; an exclusion
/// proof returns zero.
pub fn verify_storage(storage_root: &Bytes32, slot_key: &Bytes32, proof: &[Vec<u8>]) -> Result<Bytes32, EsaError> {
    match mpt::get(&B256::from(*storage_root), &keccak_nibbles(slot_key), proof)? {
        Some(raw) => rlp::decode_storage(&raw),
        None => Ok([0; 32]),
    }
}

/// Trie key of receipt `tx_index`: `rlp(tx_index)` as nibbles.
#[must_use]
pub fn receipt_key(tx_index: u64) -> Nibbles {
    let mut buf = Vec::with_capacity(9);
    alloy_rlp::Encodable::encode(&tx_index, &mut buf);
    Nibbles::unpack(&buf)
}

/// Verifies receipt `tx_index` under `receipts_root` and returns its trie value (EIP-2718 encoded receipt).
pub fn verify_receipt(receipts_root: &Bytes32, tx_index: u64, proof: &[Vec<u8>]) -> Result<Vec<u8>, EsaError> {
    mpt::get(&B256::from(*receipts_root), &receipt_key(tx_index), proof)?.ok_or(EsaError::E07InclusionInvalid)
}

/// Verifies log `log_index` (position inside the receipt) of receipt `tx_index` under `receipts_root`.
pub fn verify_log(
    receipts_root: &Bytes32,
    tx_index: u64,
    proof: &[Vec<u8>],
    log_index: u64,
) -> Result<LogValue, EsaError> {
    verify_log_raw(receipts_root, tx_index, proof, log_index).map(|(log, _)| log)
}

/// As [`verify_log`], also returning `rlp(log)` exactly as it appears in the receipt.
pub fn verify_log_raw(
    receipts_root: &Bytes32,
    tx_index: u64,
    proof: &[Vec<u8>],
    log_index: u64,
) -> Result<(LogValue, Vec<u8>), EsaError> {
    let receipt = verify_receipt(receipts_root, tx_index, proof)?;
    let raw = rlp::receipt_log(&receipt, log_index)?;
    Ok((rlp::decode_log(raw)?, raw.to_vec()))
}

/// A verified claim: the proven value, the statement fields that depend on it, and the execution block number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub value: ClaimValue,
    pub subject: Vec<u8>,
    pub value_bytes: Vec<u8>,
    pub id: Bytes32,
    pub exec_number: u64,
}

/// Checks `claim` under `record`'s roots (no predicate).
pub fn verify_claim_value(record: &HeaderRecord, claim: &Claim) -> Result<Verified, EsaError> {
    let roots = exec_roots(record, claim.exec_header.as_deref())?;
    let (value, subject, value_bytes, id) = match &claim.proof {
        ClaimProof::Account { address, account_proof } => {
            let acc = verify_account(&roots.state_root, address, account_proof)?;
            let value = acc.encode();
            let id = statement_id_state(record.chain_id, address, &[], &value);
            (ClaimValue::Account(acc), address.to_vec(), value, id)
        }
        ClaimProof::Storage {
            address,
            slot_key,
            account_proof,
            storage_proof,
        } => {
            let acc = verify_account(&roots.state_root, address, account_proof)?;
            let word = verify_storage(&acc.storage_root, slot_key, storage_proof)?;
            let mut subject = address.to_vec();
            subject.extend_from_slice(slot_key);
            let id = statement_id_state(record.chain_id, address, slot_key, &word);
            (ClaimValue::Word(word), subject, word.to_vec(), id)
        }
        ClaimProof::Log {
            tx_index,
            log_index,
            receipt_proof,
        } => {
            let (log, raw) = verify_log_raw(&roots.receipts_root, *tx_index, receipt_proof, *log_index)?;
            let mut subject = log.address.to_vec();
            subject.extend_from_slice(&tx_index.to_le_bytes());
            subject.extend_from_slice(&log_index.to_le_bytes());
            let id = statement_id_log(
                record.chain_id,
                &record.exec_hash,
                *tx_index,
                *log_index,
                &keccak256(&raw).0,
            );
            (ClaimValue::Log(log), subject, raw, id)
        }
    };
    Ok(Verified {
        value,
        subject,
        value_bytes,
        id,
        exec_number: roots.number,
    })
}

/// The single entry point (see crate docs): decode `claim_bytes`, verify under `record`'s roots, evaluate
/// `predicate`, build the [`Statement`] for `instance`. The caller must still prove that `record` is a finalised
/// header of `instance` (`CheckHeader`).
pub fn verify_claim(
    instance: AccountId,
    record: &HeaderRecord,
    claim_bytes: &[u8],
    predicate: &Predicate,
) -> Result<Statement, EsaError> {
    let claim = Claim::from_bytes(claim_bytes)?;
    let v = verify_claim_value(record, &claim)?;
    eval(predicate, &v.value)?;
    Ok(Statement {
        instance,
        chain_id: record.chain_id,
        slot: record.slot,
        exec_number: v.exec_number,
        kind: claim.kind(),
        subject: v.subject,
        value: v.value_bytes,
        predicate_hash: predicate.hash(),
        id: v.id,
    })
}
