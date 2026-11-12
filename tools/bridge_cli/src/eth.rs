//! Minimal Ethereum JSON-RPC: logs of `Vault.Deposit`, blocks, `eth_call`. Read-only; transactions go through
//! `forge script` (contracts/script/Deploy.s.sol) so keys stay in Foundry's env handling.

use alloy_primitives::keccak256;
use anyhow::{Context as _, Result, anyhow, bail};
use bridge_program::{Addr, B32};
use serde_json::{Value, json};

pub struct Rpc {
    pub url: String,
    http: reqwest::Client,
}

/// One `Deposit(address indexed token, uint256 denom, bytes32 commitment, uint256 leafIndex, bytes32 root)` log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositLog {
    pub block: u64,
    pub tx: String,
    pub token: Addr,
    pub denom: u128,
    pub commitment: B32,
    pub leaf_index: u64,
    pub root: B32,
}

pub fn deposit_topic() -> B32 {
    keccak256(b"Deposit(address,uint256,bytes32,uint256,bytes32)").0
}

pub fn hex32(v: &Value) -> Result<B32> {
    let s = v.as_str().context("expected hex string")?;
    let b = hex::decode(s.trim_start_matches("0x"))?;
    let mut out = [0_u8; 32];
    if b.len() > 32 {
        bail!("hex longer than 32 bytes");
    }
    out[32 - b.len()..].copy_from_slice(&b);
    Ok(out)
}

pub fn parse_addr(s: &str) -> Result<Addr> {
    let b = hex::decode(s.trim_start_matches("0x"))?;
    b.try_into().map_err(|_| anyhow!("address must be 20 bytes: {s}"))
}

pub fn parse_b32(s: &str) -> Result<B32> {
    hex32(&json!(s))
}

pub fn quantity(v: &Value) -> Result<u64> {
    let s = v.as_str().context("expected quantity")?;
    Ok(u64::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

impl Rpc {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut last = None;
        for attempt in 0..4_u64 {
            match self.http.post(&self.url).json(&body).send().await {
                Ok(r) => {
                    let v: Value = r.json().await?;
                    if let Some(e) = v.get("error") {
                        bail!("{method}: {e}");
                    }
                    return Ok(v["result"].clone());
                }
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
                }
            }
        }
        Err(anyhow!("{method}: {}", last.map(|e| e.to_string()).unwrap_or_default()))
    }

    pub async fn block_number(&self) -> Result<u64> {
        quantity(&self.call("eth_blockNumber", json!([])).await?)
    }

    pub async fn block_by_hash(&self, hash: &B32) -> Result<Value> {
        let b = self
            .call("eth_getBlockByHash", json!([format!("0x{}", hex::encode(hash)), false]))
            .await?;
        if b.is_null() {
            bail!("block 0x{} not found at {}", hex::encode(hash), self.url);
        }
        Ok(b)
    }

    /// All deposit logs of `vault` in `[from, to]`, in chunks (public RPCs cap the range).
    pub async fn deposit_logs(&self, vault: &Addr, from: u64, to: u64) -> Result<Vec<DepositLog>> {
        let mut out = Vec::new();
        let step = 5_000_u64;
        let mut lo = from;
        while lo <= to {
            let hi = (lo + step - 1).min(to);
            let logs = self
                .call(
                    "eth_getLogs",
                    json!([{
                        "address": format!("0x{}", hex::encode(vault)),
                        "fromBlock": format!("0x{lo:x}"),
                        "toBlock": format!("0x{hi:x}"),
                        "topics": [format!("0x{}", hex::encode(deposit_topic()))],
                    }]),
                )
                .await?;
            for l in logs.as_array().context("logs")? {
                let data = hex::decode(l["data"].as_str().context("data")?.trim_start_matches("0x"))?;
                if data.len() != 128 {
                    bail!("deposit log data must be 4 words");
                }
                let w = |i: usize| -> B32 { data[i * 32..(i + 1) * 32].try_into().unwrap_or([0; 32]) };
                let token_word = hex32(&l["topics"][1])?;
                let mut token = [0_u8; 20];
                token.copy_from_slice(&token_word[12..]);
                out.push(DepositLog {
                    block: quantity(&l["blockNumber"])?,
                    tx: l["transactionHash"].as_str().unwrap_or_default().to_string(),
                    token,
                    denom: bridge_program::hash::word_to_u128(&w(0)).context("denom above u128")?,
                    commitment: w(1),
                    leaf_index: u64::try_from(bridge_program::hash::word_to_u128(&w(2)).context("index")?)?,
                    root: w(3),
                });
            }
            lo = hi + 1;
        }
        out.sort_by_key(|d| d.leaf_index);
        Ok(out)
    }

    /// `Vault.burnNullifierSpent(n)`.
    pub async fn burn_nullifier_spent(&self, vault: &Addr, nullifier: &B32) -> Result<bool> {
        let sel = &keccak256(b"burnNullifierSpent(bytes32)").0[..4];
        let data = format!("0x{}{}", hex::encode(sel), hex::encode(nullifier));
        let r = self
            .call(
                "eth_call",
                json!([{"to": format!("0x{}", hex::encode(vault)), "data": data}, "latest"]),
            )
            .await?;
        Ok(hex32(&r)?[31] == 1)
    }
}
