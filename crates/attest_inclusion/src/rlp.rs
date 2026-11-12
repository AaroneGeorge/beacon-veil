//! Strict RLP decoding of the three Ethereum values we prove: accounts, receipts and logs (alloy-rlp `Header`).

use alloy_rlp::Header;
use attest_core::{Bytes32, EsaError};

use crate::{AccountValue, LogValue};

type R<T> = Result<T, EsaError>;

const BAD: EsaError = EsaError::E07InclusionInvalid;

/// Next item as a list; returns its payload and advances `buf`.
pub(crate) fn list<'a>(buf: &mut &'a [u8]) -> R<&'a [u8]> {
    let h = Header::decode(buf).map_err(|_| BAD)?;
    if !h.list || buf.len() < h.payload_length {
        return Err(BAD);
    }
    let (payload, rest) = buf.split_at(h.payload_length);
    *buf = rest;
    Ok(payload)
}

/// Next item as a byte string; returns its payload and advances `buf`.
pub(crate) fn string<'a>(buf: &mut &'a [u8]) -> R<&'a [u8]> {
    Header::decode_bytes(buf, false).map_err(|_| BAD)
}

/// Canonical big-endian unsigned integer of at most `max` bytes (no leading zero byte).
fn uint<'a>(buf: &mut &'a [u8], max: usize) -> R<&'a [u8]> {
    let s = string(buf)?;
    if s.len() > max || s.first() == Some(&0) {
        return Err(BAD);
    }
    Ok(s)
}

fn word(be: &[u8]) -> Bytes32 {
    let mut w = [0u8; 32];
    w[32 - be.len()..].copy_from_slice(be);
    w
}

fn fixed<const N: usize>(buf: &mut &[u8]) -> R<[u8; N]> {
    string(buf)?.try_into().map_err(|_| BAD)
}

/// `rlp([nonce, balance, storage_root, code_hash])`, the state-trie value of an account.
pub(crate) fn decode_account(mut buf: &[u8]) -> R<AccountValue> {
    let mut p = list(&mut buf)?;
    if !buf.is_empty() {
        return Err(BAD);
    }
    let nonce_be = uint(&mut p, 8)?;
    let mut nonce = [0u8; 8];
    nonce[8 - nonce_be.len()..].copy_from_slice(nonce_be);
    let balance = word(uint(&mut p, 32)?);
    let storage_root = fixed::<32>(&mut p)?;
    let code_hash = fixed::<32>(&mut p)?;
    if !p.is_empty() {
        return Err(BAD);
    }
    Ok(AccountValue {
        nonce: u64::from_be_bytes(nonce),
        balance,
        storage_root,
        code_hash,
    })
}

/// Storage-trie value: `rlp(value)` with leading zeros stripped. Returns the 32-byte big-endian word.
pub(crate) fn decode_storage(mut buf: &[u8]) -> R<Bytes32> {
    let v = uint(&mut buf, 32)?;
    if !buf.is_empty() || v.is_empty() {
        // An empty value is never stored: a zero slot is absent from the trie.
        return Err(BAD);
    }
    Ok(word(v))
}

/// One log: `rlp([address, [topic...], data])`.
pub(crate) fn decode_log(mut buf: &[u8]) -> R<LogValue> {
    let mut p = list(&mut buf)?;
    if !buf.is_empty() {
        return Err(BAD);
    }
    let address = fixed::<20>(&mut p)?;
    let mut t = list(&mut p)?;
    let mut topics = Vec::new();
    while !t.is_empty() {
        topics.push(fixed::<32>(&mut t)?);
    }
    if topics.len() > 4 {
        return Err(BAD);
    }
    let data = string(&mut p)?.to_vec();
    if !p.is_empty() {
        return Err(BAD);
    }
    Ok(LogValue { address, topics, data })
}

/// Receipt-trie value (EIP-2718: legacy `rlp([...])` or `type || rlp([...])`) to the raw RLP of log `log_index`.
/// Receipt fields: `[status_or_post_state, cumulative_gas_used, logs_bloom, logs]`.
pub(crate) fn receipt_log(receipt: &[u8], log_index: u64) -> R<&[u8]> {
    let mut buf = match receipt.first() {
        Some(&b) if b >= 0xc0 => receipt,
        Some(&b) if b <= 0x7f => &receipt[1..],
        _ => return Err(BAD),
    };
    let mut p = list(&mut buf)?;
    if !buf.is_empty() {
        return Err(BAD);
    }
    let _status = string(&mut p)?;
    let _cumulative_gas = uint(&mut p, 8)?;
    let _bloom = fixed::<256>(&mut p)?;
    let mut logs = list(&mut p)?;
    if !p.is_empty() {
        return Err(BAD);
    }
    let mut i = 0u64;
    while !logs.is_empty() {
        let start = logs;
        list(&mut logs)?;
        if i == log_index {
            return Ok(&start[..start.len() - logs.len()]);
        }
        i += 1;
    }
    Err(BAD)
}

/// `rlp(log)`: the canonical encoding used for the statement value and the statement id.
pub(crate) fn encode_log(log: &LogValue) -> Vec<u8> {
    use alloy_rlp::Encodable;
    let topics_len: usize = log.topics.iter().map(|t| t.as_slice().length()).sum();
    let topics_header = Header {
        list: true,
        payload_length: topics_len,
    };
    let payload_len =
        log.address.as_slice().length() + topics_header.length() + topics_len + log.data.as_slice().length();
    let mut out = Vec::with_capacity(payload_len + 4);
    Header {
        list: true,
        payload_length: payload_len,
    }
    .encode(&mut out);
    log.address.as_slice().encode(&mut out);
    topics_header.encode(&mut out);
    for t in &log.topics {
        t.as_slice().encode(&mut out);
    }
    log.data.as_slice().encode(&mut out);
    out
}

/// Fields we need from an RLP execution block header: `[parent_hash, ommers_hash, beneficiary, state_root,
/// transactions_root, receipts_root, logs_bloom, difficulty, number, ...]`. Later fields (any fork) are not read; the
/// header's hash (keccak of the whole encoding) binds them all.
pub(crate) fn decode_exec_header(mut buf: &[u8]) -> R<(Bytes32, Bytes32, u64)> {
    let bad = EsaError::E15HeaderMismatch;
    let mut p = list(&mut buf).map_err(|_| bad)?;
    if !buf.is_empty() {
        return Err(bad);
    }
    let _parent = fixed::<32>(&mut p).map_err(|_| bad)?;
    let _ommers = fixed::<32>(&mut p).map_err(|_| bad)?;
    let _beneficiary = fixed::<20>(&mut p).map_err(|_| bad)?;
    let state_root = fixed::<32>(&mut p).map_err(|_| bad)?;
    let _tx_root = fixed::<32>(&mut p).map_err(|_| bad)?;
    let receipts_root = fixed::<32>(&mut p).map_err(|_| bad)?;
    let _bloom = fixed::<256>(&mut p).map_err(|_| bad)?;
    let _difficulty = uint(&mut p, 32).map_err(|_| bad)?;
    let number_be = uint(&mut p, 8).map_err(|_| bad)?;
    let mut n = [0u8; 8];
    n[8 - number_be.len()..].copy_from_slice(number_be);
    Ok((state_root, receipts_root, u64::from_be_bytes(n)))
}
