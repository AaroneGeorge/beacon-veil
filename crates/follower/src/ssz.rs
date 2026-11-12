//! Beacon API light-client JSON to canonical SSZ, for the forks the follower meets: `electra` and `fulu` (Electra
//! light-client layout; mainnet in 2026-11) and `gloas` (Sepolia in 2026-11).
//!
//! Why this exists: Logos `beacon_api_module.lc_request` answers JSON only, and Helios 0.12 decodes SSZ but has no
//! encoder for its update types. `attest_host::submit_update` takes SSZ. The encoder is a small schema walk over the
//! consensus-spec containers. Tests check it byte for byte against the SSZ bodies providers serve for the same objects
//! (`tests/data/*.ssz`).

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

/// Light-client container kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LcKind {
    Bootstrap,
    Update,
    FinalityUpdate,
}

/// Light-client layout family of a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `electra`, `fulu`: header carries the execution payload header.
    Electra,
    /// `gloas`: header carries the execution block hash only.
    Gloas,
}

impl Layout {
    /// From the `version` field (or `Eth-Consensus-Version` header).
    pub fn from_version(version: &str) -> Result<Self> {
        match version.to_ascii_lowercase().as_str() {
            "electra" | "fulu" => Ok(Self::Electra),
            "gloas" => Ok(Self::Gloas),
            other => bail!("light-client fork '{other}' is not supported (electra, fulu, gloas)"),
        }
    }
}

/// One SSZ type in a schema.
#[derive(Debug, Clone)]
enum Ty {
    U64,
    /// uint256, JSON decimal string, little-endian 32 bytes.
    U256,
    /// Fixed-length bytes, JSON `0x` hex.
    Bytes(usize),
    /// Variable-length bytes (`ByteList`), JSON `0x` hex.
    VarBytes(usize),
    /// Fixed-length vector.
    Vector(Box<Ty>, usize),
    Container(Vec<(&'static str, Ty)>),
}

impl Ty {
    fn is_fixed(&self) -> bool {
        match self {
            Ty::U64 | Ty::U256 | Ty::Bytes(_) => true,
            Ty::VarBytes(_) => false,
            Ty::Vector(t, _) => t.is_fixed(),
            Ty::Container(fs) => fs.iter().all(|(_, t)| t.is_fixed()),
        }
    }

    fn fixed_len(&self) -> usize {
        match self {
            Ty::U64 => 8,
            Ty::U256 => 32,
            Ty::Bytes(n) => *n,
            Ty::Vector(t, n) if t.is_fixed() => t.fixed_len() * n,
            Ty::Container(fs) if self.is_fixed() => fs.iter().map(|(_, t)| t.fixed_len()).sum(),
            _ => 4, // offset
        }
    }
}

fn b32() -> Ty {
    Ty::Bytes(32)
}

fn beacon_block_header() -> Ty {
    Ty::Container(vec![
        ("slot", Ty::U64),
        ("proposer_index", Ty::U64),
        ("parent_root", b32()),
        ("state_root", b32()),
        ("body_root", b32()),
    ])
}

fn execution_payload_header_deneb() -> Ty {
    Ty::Container(vec![
        ("parent_hash", b32()),
        ("fee_recipient", Ty::Bytes(20)),
        ("state_root", b32()),
        ("receipts_root", b32()),
        ("logs_bloom", Ty::Bytes(256)),
        ("prev_randao", b32()),
        ("block_number", Ty::U64),
        ("gas_limit", Ty::U64),
        ("gas_used", Ty::U64),
        ("timestamp", Ty::U64),
        ("extra_data", Ty::VarBytes(32)),
        ("base_fee_per_gas", Ty::U256),
        ("block_hash", b32()),
        ("transactions_root", b32()),
        ("withdrawals_root", b32()),
        ("blob_gas_used", Ty::U64),
        ("excess_blob_gas", Ty::U64),
    ])
}

fn lc_header(layout: Layout) -> Ty {
    match layout {
        Layout::Electra => Ty::Container(vec![
            ("beacon", beacon_block_header()),
            ("execution", execution_payload_header_deneb()),
            ("execution_branch", Ty::Vector(Box::new(b32()), 4)),
        ]),
        Layout::Gloas => Ty::Container(vec![
            ("beacon", beacon_block_header()),
            ("execution_block_hash", b32()),
            ("execution_branch", Ty::Vector(Box::new(b32()), 11)),
        ]),
    }
}

fn sync_committee() -> Ty {
    Ty::Container(vec![
        ("pubkeys", Ty::Vector(Box::new(Ty::Bytes(48)), 512)),
        ("aggregate_pubkey", Ty::Bytes(48)),
    ])
}

fn sync_aggregate() -> Ty {
    Ty::Container(vec![
        ("sync_committee_bits", Ty::Bytes(64)),
        ("sync_committee_signature", Ty::Bytes(96)),
    ])
}

/// `(current_sync_committee_branch, next_sync_committee_branch, finality_branch)` depths.
fn depths(layout: Layout) -> (usize, usize, usize) {
    match layout {
        Layout::Electra => (6, 6, 7),
        Layout::Gloas => (11, 11, 9),
    }
}

fn schema(kind: LcKind, layout: Layout) -> Ty {
    let (cur_d, next_d, fin_d) = depths(layout);
    let branch = |n| Ty::Vector(Box::new(b32()), n);
    match kind {
        LcKind::Bootstrap => Ty::Container(vec![
            ("header", lc_header(layout)),
            ("current_sync_committee", sync_committee()),
            ("current_sync_committee_branch", branch(cur_d)),
        ]),
        LcKind::Update => Ty::Container(vec![
            ("attested_header", lc_header(layout)),
            ("next_sync_committee", sync_committee()),
            ("next_sync_committee_branch", branch(next_d)),
            ("finalized_header", lc_header(layout)),
            ("finality_branch", branch(fin_d)),
            ("sync_aggregate", sync_aggregate()),
            ("signature_slot", Ty::U64),
        ]),
        LcKind::FinalityUpdate => Ty::Container(vec![
            ("attested_header", lc_header(layout)),
            ("finalized_header", lc_header(layout)),
            ("finality_branch", branch(fin_d)),
            ("sync_aggregate", sync_aggregate()),
            ("signature_slot", Ty::U64),
        ]),
    }
}

/// Encodes the `data` object of a Beacon API light-client response as SSZ.
pub fn encode(kind: LcKind, version: &str, data: &Value) -> Result<Vec<u8>> {
    let layout = Layout::from_version(version)?;
    let mut out = Vec::with_capacity(32 * 1024);
    enc(&schema(kind, layout), data, &mut out, "data")?;
    Ok(out)
}

fn hex_bytes(v: &Value, path: &str) -> Result<Vec<u8>> {
    let s = v.as_str().ok_or_else(|| anyhow!("{path}: expected a hex string"))?;
    let h = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(h).with_context(|| format!("{path}: bad hex"))
}

fn dec_u64(v: &Value, path: &str) -> Result<u64> {
    match v {
        Value::String(s) => s.parse().with_context(|| format!("{path}: not a u64")),
        Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("{path}: not a u64")),
        _ => bail!("{path}: expected a decimal u64"),
    }
}

/// Decimal string to 32-byte little-endian.
fn dec_u256_le(v: &Value, path: &str) -> Result<[u8; 32]> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => bail!("{path}: expected a decimal uint256"),
    };
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        bail!("{path}: not a decimal uint256");
    }
    let mut le = [0u8; 32];
    for d in s.bytes() {
        // le = le * 10 + d
        let mut carry = u32::from(d - b'0');
        for byte in &mut le {
            let x = u32::from(*byte) * 10 + carry;
            *byte = (x & 0xff) as u8;
            carry = x >> 8;
        }
        if carry != 0 {
            bail!("{path}: uint256 overflow");
        }
    }
    Ok(le)
}

fn enc(ty: &Ty, v: &Value, out: &mut Vec<u8>, path: &str) -> Result<()> {
    match ty {
        Ty::U64 => out.extend_from_slice(&dec_u64(v, path)?.to_le_bytes()),
        Ty::U256 => out.extend_from_slice(&dec_u256_le(v, path)?),
        Ty::Bytes(n) => {
            let b = hex_bytes(v, path)?;
            if b.len() != *n {
                bail!("{path}: expected {n} bytes, got {}", b.len());
            }
            out.extend_from_slice(&b);
        }
        Ty::VarBytes(max) => {
            let b = hex_bytes(v, path)?;
            if b.len() > *max {
                bail!("{path}: {} bytes, above the limit {max}", b.len());
            }
            out.extend_from_slice(&b);
        }
        Ty::Vector(t, n) => {
            let arr = v.as_array().ok_or_else(|| anyhow!("{path}: expected an array"))?;
            if arr.len() != *n {
                bail!("{path}: expected {n} elements, got {}", arr.len());
            }
            if !t.is_fixed() {
                bail!("{path}: vectors of variable-size elements are not used by light-client types");
            }
            for (i, e) in arr.iter().enumerate() {
                enc(t, e, out, &format!("{path}[{i}]"))?;
            }
        }
        Ty::Container(fields) => {
            let obj = v.as_object().ok_or_else(|| anyhow!("{path}: expected an object"))?;
            if let Some(extra) = obj.keys().find(|k| !fields.iter().any(|(n, _)| n == k)) {
                bail!("{path}: unexpected field '{extra}' (wrong fork layout?)");
            }
            let fixed_size: usize = fields.iter().map(|(_, t)| t.fixed_len()).sum();
            let mut fixed = Vec::with_capacity(fixed_size);
            let mut var = Vec::new();
            for (name, t) in fields {
                let fv = obj.get(*name).ok_or_else(|| anyhow!("{path}.{name}: missing"))?;
                let p = format!("{path}.{name}");
                if t.is_fixed() {
                    enc(t, fv, &mut fixed, &p)?;
                } else {
                    let off = u32::try_from(fixed_size + var.len()).context("offset overflow")?;
                    fixed.extend_from_slice(&off.to_le_bytes());
                    enc(t, fv, &mut var, &p)?;
                }
            }
            out.extend_from_slice(&fixed);
            out.extend_from_slice(&var);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u256_decimal() {
        let le = dec_u256_le(&Value::String("258".into()), "x").unwrap();
        assert_eq!(&le[..3], &[2, 1, 0]);
        assert!(dec_u256_le(&Value::String("1".repeat(80)), "x").is_err());
    }

    #[test]
    fn fixed_sizes() {
        assert_eq!(lc_header(Layout::Gloas).fixed_len(), 112 + 32 + 11 * 32);
        assert_eq!(sync_committee().fixed_len(), 512 * 48 + 48);
        assert!(!lc_header(Layout::Electra).is_fixed());
    }
}
