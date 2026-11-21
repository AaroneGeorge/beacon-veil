//! JSON-RPC 2.0 to the LEZ sequencer: at most one request a second, retries with backoff on transport errors.

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use lee_core::account::{Account, AccountId};
use serde_json::{Value, json};

use crate::chain::{Block, LeeTransaction};

pub struct Rpc {
    url: String,
    http: reqwest::blocking::Client,
    last: Cell<Option<Instant>>,
    /// Every call, for the log: `(method, ok)`.
    pub calls: Cell<u64>,
}

/// The fee market at the head (`getFeeState`).
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
pub struct FeeState {
    pub height: u64,
    pub base_fee_exec: u64,
    pub base_fee_stor: u64,
    pub next_base_fee_exec_ceiling: u64,
    pub next_base_fee_stor_ceiling: u64,
    pub max_gas_exec: u64,
    pub max_gas_stor: u64,
}

impl Rpc {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            url: url.to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            last: Cell::new(None),
            calls: Cell::new(0),
        })
    }

    fn pace(&self) {
        if let Some(t) = self.last.get() {
            let e = t.elapsed();
            if e < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_secs(1) - e);
            }
        }
        self.last.set(Some(Instant::now()));
    }

    /// One call; transport failures are retried (5 tries, 2, 4, 8, 16 s apart). An RPC error is returned as is.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut wait = 2;
        for attempt in 1..=5 {
            self.pace();
            self.calls.set(self.calls.get() + 1);
            match self
                .http
                .post(&self.url)
                .json(&body)
                .send()
                .and_then(|r| r.error_for_status())
            {
                Ok(r) => {
                    let v: Value = r.json().context("RPC answer is not JSON")?;
                    if let Some(e) = v.get("error") {
                        bail!("{method}: {e}");
                    }
                    return v.get("result").cloned().ok_or_else(|| anyhow!("{method}: no result"));
                }
                Err(e) if attempt < 5 => {
                    eprintln!("{method}: {e}; retry in {wait} s");
                    std::thread::sleep(Duration::from_secs(wait));
                    wait *= 2;
                }
                Err(e) => return Err(e).context(method.to_string()),
            }
        }
        unreachable!()
    }

    pub fn last_block_id(&self) -> Result<u64> {
        self.call("getLastBlockId", json!({}))?
            .as_u64()
            .context("getLastBlockId")
    }

    pub fn fee_state(&self) -> Result<FeeState> {
        Ok(serde_json::from_value(self.call("getFeeState", json!({}))?)?)
    }

    pub fn account(&self, id: &AccountId) -> Result<Account> {
        Ok(serde_json::from_value(
            self.call("getAccount", json!({"account_id": id.to_string()}))?,
        )?)
    }

    pub fn balance(&self, id: &AccountId) -> Result<u128> {
        let v = self.call("getAccountBalance", json!({"account_id": id.to_string()}))?;
        Ok(serde_json::from_value(v)?)
    }

    pub fn channel_id(&self) -> Result<String> {
        Ok(self
            .call("getChannelId", json!({}))?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// `getBlock`: the decoded block and its raw base64 answer.
    pub fn block(&self, id: u64) -> Result<Option<(Block, String)>> {
        let v = self.call("getBlock", json!({"block_id": id}))?;
        let Some(s) = v.as_str() else { return Ok(None) };
        let b: Block = borsh::from_slice(&B64.decode(s)?).context("block bytes")?;
        Ok(Some((b, s.to_string())))
    }

    /// `getTransaction`: `(raw base64 transaction, block id)` when the sequencer knows the hash.
    pub fn transaction(&self, hash: &[u8; 32]) -> Result<Option<(String, u64)>> {
        let v = self.call("getTransaction", json!({"tx_hash": hex::encode(hash)}))?;
        if v.is_null() {
            return Ok(None);
        }
        let arr = v.as_array().context("getTransaction answer")?;
        Ok(Some((
            arr.first().and_then(Value::as_str).unwrap_or_default().to_string(),
            arr.get(1).and_then(Value::as_u64).unwrap_or_default(),
        )))
    }

    /// `sendTransaction`: the hash the sequencer returns.
    pub fn send(&self, tx: &LeeTransaction) -> Result<String> {
        let b64 = B64.encode(borsh::to_vec(tx)?);
        let v = self.call("sendTransaction", json!({"tx": b64}))?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }
}
