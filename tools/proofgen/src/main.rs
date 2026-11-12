//! `proofgen` (PLAN.md section 4, owner W3): builds `attest_inclusion::Claim`s from public Ethereum JSON-RPC.
//!
//! ```text
//! proofgen [--chain mainnet|sepolia] [--rpc URL] --block N|latest|finalized <account|storage|log|header> ... [--out F]
//!   account --address 0x..
//!   storage --address 0x.. --slot 0x..
//!   log     (--tx-index I | --tx-hash 0x..) --log-index L     # log index inside the receipt
//!   header                                                     # RLP execution header only
//! ```
//!
//! Sources: `eth_getBlockByNumber` (header, rebuilt to RLP and checked against `hash`), `eth_getProof` (account and
//! storage), `eth_getBlockReceipts` (receipts trie rebuilt and checked against `receiptsRoot`). Every claim is
//! verified with `attest_inclusion` before it is written. No API keys: public endpoints only (PLAN.md 3.3).
//!
//! Output: one JSON fixture with the block fields, `exec_header_rlp`, the expected value, `claim` (borsh hex, no
//! header: for records that carry roots) and `claim_with_header` (borsh hex, header included: required for gloas
//! records whose roots are zero).

use std::path::PathBuf;

use alloy_primitives::{B256, keccak256};
use alloy_rlp::{Encodable, Header};
use alloy_trie::{HashBuilder, Nibbles, proof::ProofRetainer, root::adjust_index_for_rlp};
use anyhow::{Context, Result, anyhow, bail, ensure};
use attest_core::{HeaderRecord, Predicate};
use attest_inclusion::{Claim, ClaimProof, ClaimValue};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(about = "Build account, storage and receipt-log proofs from public Ethereum JSON-RPC")]
struct Cli {
    /// Chain: picks the default RPC and the chain id.
    #[arg(long, default_value = "sepolia")]
    chain: String,
    /// JSON-RPC endpoint. Default per chain, tried in order: mainnet https://eth.drpc.org then
    /// https://mainnet.gateway.tenderly.co; sepolia https://ethereum-sepolia-rpc.publicnode.com then
    /// https://sepolia.drpc.org. All serve historical eth_getProof (checked 2026-11-12).
    #[arg(long)]
    rpc: Option<String>,
    /// Block number (decimal) or tag.
    #[arg(long, default_value = "finalized")]
    block: String,
    /// Answer RPC calls from saved raw responses instead of the network: files `<chain>_*_<block>.json` holding a
    /// JSON-RPC response (`eth_getBlockByNumber`, `eth_getBlockReceipts`, `eth_getProof` for an address with one or
    /// more slots). Proofs are still verified, so a wrong file cannot produce a fixture.
    #[arg(long)]
    raw: Option<PathBuf>,
    /// Write the fixture here instead of stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    #[command(subcommand)]
    what: What,
}

#[derive(Subcommand)]
enum What {
    Account {
        #[arg(long)]
        address: String,
    },
    Storage {
        #[arg(long)]
        address: String,
        #[arg(long)]
        slot: String,
    },
    Log {
        #[arg(long)]
        tx_index: Option<u64>,
        #[arg(long)]
        tx_hash: Option<String>,
        /// Index of the log inside its receipt.
        #[arg(long)]
        log_index: u64,
    },
    Header,
}

struct Rpc {
    /// Endpoints tried in order; the first that answers a call wins for that call.
    urls: Vec<String>,
    http: reqwest::Client,
    raw: Option<(PathBuf, String, String)>,
}

impl Rpc {
    /// Replay from saved responses (`--raw`). `block` is the decimal block number the files are named after.
    fn replay(&self, method: &str, params: &Value) -> Result<Value> {
        let (dir, chain, block) = self.raw.as_ref().expect("raw mode");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                n.starts_with(&format!("{chain}_")) && n.ends_with(&format!("_{block}.json"))
            })
            .collect();
        files.sort();
        let load = |p: &PathBuf| -> Result<Value> {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
            v.get("result")
                .cloned()
                .filter(|r| !r.is_null())
                .ok_or_else(|| anyhow!("{}: no result", p.display()))
        };
        let pick = |kind: &str| -> Result<Value> {
            let p = files
                .iter()
                .find(|p| p.to_string_lossy().contains(&format!("_{kind}_")))
                .context(kind.to_string())?;
            load(p)
        };
        match method {
            "eth_getBlockByNumber" => pick("block"),
            "eth_getBlockReceipts" => pick("receipts"),
            "eth_getProof" => {
                let addr = params[0].as_str().unwrap_or_default().to_lowercase();
                let want: Vec<String> = params[1]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|k| {
                        format!(
                            "{:0>64}",
                            k.as_str().unwrap_or_default().trim_start_matches("0x").to_lowercase()
                        )
                    })
                    .collect();
                for p in files.iter().filter(|p| p.to_string_lossy().contains("_proof_")) {
                    let Ok(mut r) = load(p) else { continue };
                    if r["address"].as_str().map(str::to_lowercase) != Some(addr.clone()) {
                        continue;
                    }
                    let sp: Vec<Value> = r["storageProof"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|s| {
                            want.contains(&format!(
                                "{:0>64}",
                                s["key"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .trim_start_matches("0x")
                                    .to_lowercase()
                            ))
                        })
                        .collect();
                    if sp.len() == want.len() {
                        r["storageProof"] = Value::Array(sp);
                        return Ok(r);
                    }
                }
                bail!("no saved eth_getProof for {addr} slots {want:?} at {chain} {block}")
            }
            _ => bail!("{method}: not replayable"),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        if self.raw.is_some() {
            return self.replay(method, &params);
        }
        let mut errors = Vec::new();
        for url in &self.urls {
            for attempt in 0..3u64 {
                match self.call_one(url, method, &params).await {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        errors.push(format!("{url} (try {attempt}): {e:#}"));
                        tokio::time::sleep(std::time::Duration::from_millis(1500 * (attempt + 1))).await;
                    }
                }
            }
        }
        bail!("{method} failed on every endpoint:\n  {}", errors.join("\n  "))
    }

    async fn call_one(&self, url: &str, method: &str, params: &Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let resp = self.http.post(url).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        ensure!(
            status.is_success(),
            "HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        );
        let resp: Value =
            serde_json::from_str(&text).with_context(|| format!("not JSON: {}", &text[..text.len().min(200)]))?;
        if let Some(e) = resp.get("error") {
            bail!("rpc error {e}");
        }
        resp.get("result")
            .cloned()
            .filter(|r| !r.is_null())
            .ok_or_else(|| anyhow!("empty result"))
    }
}

fn hexs(v: &Value) -> Result<Vec<u8>> {
    let s = v.as_str().ok_or_else(|| anyhow!("expected hex string, got {v}"))?;
    let s = s.strip_prefix("0x").unwrap_or(s);
    let s = if s.len() % 2 == 1 {
        format!("0{s}")
    } else {
        s.to_string()
    };
    Ok(hex::decode(s)?)
}

fn fixed<const N: usize>(v: &Value) -> Result<[u8; N]> {
    let b = hexs(v)?;
    b.as_slice()
        .try_into()
        .map_err(|_| anyhow!("expected {N} bytes, got {}", b.len()))
}

/// Quantity as minimal big-endian bytes (RLP integer payload).
fn quantity(v: &Value) -> Result<Vec<u8>> {
    let b = hexs(v)?;
    Ok(b.into_iter().skip_while(|x| *x == 0).collect())
}

fn h0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let len = items.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(len + 9);
    Header {
        list: true,
        payload_length: len,
    }
    .encode(&mut out);
    for i in items {
        out.extend_from_slice(i);
    }
    out
}

fn rlp_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    b.encode(&mut out);
    out
}

/// Execution header fields in canonical order with their kinds. Encoding stops at the first absent field (older
/// forks have fewer fields). Order as in go-ethereum `core/types/block.go` through Prague, then `blockAccessListHash`
/// (EIP-7928) and `slotNumber` (gloas); the rebuilt hash must equal the block's `hash`, which checks the order.
const HEADER_FIELDS: &[(&str, char)] = &[
    ("parentHash", 'b'),
    ("sha3Uncles", 'b'),
    ("miner", 'b'),
    ("stateRoot", 'b'),
    ("transactionsRoot", 'b'),
    ("receiptsRoot", 'b'),
    ("logsBloom", 'b'),
    ("difficulty", 'q'),
    ("number", 'q'),
    ("gasLimit", 'q'),
    ("gasUsed", 'q'),
    ("timestamp", 'q'),
    ("extraData", 'b'),
    ("mixHash", 'b'),
    ("nonce", 'b'),
    ("baseFeePerGas", 'q'),
    ("withdrawalsRoot", 'b'),
    ("blobGasUsed", 'q'),
    ("excessBlobGas", 'q'),
    ("parentBeaconBlockRoot", 'b'),
    ("requestsHash", 'b'),
    ("blockAccessListHash", 'b'),
    ("slotNumber", 'q'),
];

fn header_rlp(block: &Value) -> Result<Vec<u8>> {
    let mut items = Vec::new();
    for (name, kind) in HEADER_FIELDS {
        let Some(v) = block.get(*name).filter(|v| !v.is_null()) else {
            break;
        };
        items.push(match kind {
            'q' => rlp_bytes(&quantity(v)?),
            _ => rlp_bytes(&hexs(v)?),
        });
    }
    let rlp = rlp_list(&items);
    let want: [u8; 32] = fixed(&block["hash"])?;
    ensure!(
        keccak256(&rlp).0 == want,
        "rebuilt header does not hash to the block hash (unknown header fields?)"
    );
    Ok(rlp)
}

/// EIP-2718 receipt encoding: `rlp([status_or_root, cumulative_gas_used, logs_bloom, logs])`, prefixed with the type
/// byte unless legacy.
fn receipt_rlp(r: &Value) -> Result<Vec<u8>> {
    let status = match r.get("status").filter(|v| !v.is_null()) {
        Some(s) => rlp_bytes(&quantity(s)?),
        None => rlp_bytes(&hexs(&r["root"])?),
    };
    let mut logs = Vec::new();
    for l in r["logs"].as_array().context("receipt logs")? {
        let topics: Vec<Vec<u8>> = l["topics"]
            .as_array()
            .context("log topics")?
            .iter()
            .map(|t| hexs(t).map(|b| rlp_bytes(&b)))
            .collect::<Result<_>>()?;
        logs.push(rlp_list(&[
            rlp_bytes(&hexs(&l["address"])?),
            rlp_list(&topics),
            rlp_bytes(&hexs(&l["data"])?),
        ]));
    }
    let body = rlp_list(&[
        status,
        rlp_bytes(&quantity(&r["cumulativeGasUsed"])?),
        rlp_bytes(&hexs(&r["logsBloom"])?),
        rlp_list(&logs),
    ]);
    let ty = quantity(&r["type"])?;
    Ok(match ty.as_slice() {
        [] => body,
        [t] => [vec![*t], body].concat(),
        _ => bail!("receipt type out of range"),
    })
}

/// Receipts trie root and the proof for `target`. Same construction as Helios `create_receipt_proof` (MIT).
fn receipt_proof(encoded: &[Vec<u8>], target: usize) -> (B256, Vec<Vec<u8>>) {
    let key = |i: usize| {
        let mut b = Vec::new();
        i.encode(&mut b);
        Nibbles::unpack(&b)
    };
    let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::new(vec![key(target)]));
    for i in 0..encoded.len() {
        let index = adjust_index_for_rlp(i, encoded.len());
        hb.add_leaf(key(index), &encoded[index]);
    }
    let root = hb.root();
    let nodes = hb
        .take_proof_nodes()
        .into_nodes_sorted()
        .into_iter()
        .map(|(_, n)| n.to_vec())
        .collect();
    (root, nodes)
}

fn proof_nodes(v: &Value) -> Result<Vec<Vec<u8>>> {
    v.as_array().context("proof array")?.iter().map(hexs).collect()
}

fn block_param(block: &str) -> Value {
    match block.parse::<u64>() {
        Ok(n) => json!(format!("0x{n:x}")),
        Err(_) => json!(block),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (chain_id, defaults): (u64, &[&str]) = match cli.chain.as_str() {
        "mainnet" => (1, &["https://eth.drpc.org", "https://mainnet.gateway.tenderly.co"]),
        "sepolia" => (
            11_155_111,
            &[
                "https://ethereum-sepolia-rpc.publicnode.com",
                "https://sepolia.drpc.org",
            ],
        ),
        other => bail!("unknown chain {other}"),
    };
    let urls = cli
        .rpc
        .clone()
        .map_or_else(|| defaults.iter().map(|s| (*s).to_string()).collect(), |u| vec![u]);
    let http = reqwest::Client::builder()
        .user_agent("beacon-veil-proofgen/0.1")
        .build()?;
    let raw = cli.raw.clone().map(|d| (d, cli.chain.clone(), cli.block.clone()));
    let rpc = Rpc {
        urls: if raw.is_some() {
            vec![format!("replay:{}", cli.raw.as_ref().unwrap().display())]
        } else {
            urls
        },
        http,
        raw,
    };

    let block = rpc
        .call("eth_getBlockByNumber", json!([block_param(&cli.block), false]))
        .await?;
    let number = u64::from_str_radix(block["number"].as_str().context("number")?.trim_start_matches("0x"), 16)?;
    let nhex = json!(format!("0x{number:x}"));
    let header = header_rlp(&block)?;
    let exec_hash: [u8; 32] = fixed(&block["hash"])?;
    let state_root: [u8; 32] = fixed(&block["stateRoot"])?;
    let receipts_root: [u8; 32] = fixed(&block["receiptsRoot"])?;

    let mut fixture = json!({
        "tool": "proofgen",
        "chain": cli.chain, "chain_id": chain_id, "rpc": rpc.urls,
        "fetched_at_utc": now_utc(),
        "block_number": number, "block_hash": h0x(&exec_hash),
        "state_root": h0x(&state_root), "receipts_root": h0x(&receipts_root),
        "exec_header_rlp": h0x(&header),
    });

    let (proof, expected) = match &cli.what {
        What::Header => (None, Value::Null),
        What::Account { address } => {
            let p = rpc.call("eth_getProof", json!([address, [], nhex])).await?;
            let address: [u8; 20] = fixed(&json!(address))?;
            let expected = json!({"nonce": p["nonce"], "balance": p["balance"], "storage_hash": p["storageHash"], "code_hash": p["codeHash"]});
            (
                Some(ClaimProof::Account {
                    address,
                    account_proof: proof_nodes(&p["accountProof"])?,
                }),
                expected,
            )
        }
        What::Storage { address, slot } => {
            let slot_key = {
                let b = hexs(&json!(slot))?;
                ensure!(b.len() <= 32, "slot above 32 bytes");
                let mut k = [0u8; 32];
                k[32 - b.len()..].copy_from_slice(&b);
                k
            };
            let p = rpc
                .call("eth_getProof", json!([address, [h0x(&slot_key)], nhex]))
                .await?;
            let sp = &p["storageProof"][0];
            let address: [u8; 20] = fixed(&json!(address))?;
            fixture["slot_key"] = json!(h0x(&slot_key));
            (
                Some(ClaimProof::Storage {
                    address,
                    slot_key,
                    account_proof: proof_nodes(&p["accountProof"])?,
                    storage_proof: proof_nodes(&sp["proof"])?,
                }),
                sp["value"].clone(),
            )
        }
        What::Log {
            tx_index,
            tx_hash,
            log_index,
        } => {
            let receipts = rpc.call("eth_getBlockReceipts", json!([nhex])).await?;
            let receipts = receipts.as_array().context("receipts")?;
            let tx_index = match (tx_index, tx_hash) {
                (Some(i), None) => usize::try_from(*i)?,
                (None, Some(h)) => receipts
                    .iter()
                    .position(|r| r["transactionHash"].as_str().map(str::to_lowercase) == Some(h.to_lowercase()))
                    .context("tx hash not in block")?,
                _ => bail!("give exactly one of --tx-index, --tx-hash"),
            };
            let encoded: Vec<Vec<u8>> = receipts.iter().map(receipt_rlp).collect::<Result<_>>()?;
            let (root, nodes) = receipt_proof(&encoded, tx_index);
            ensure!(
                root.0 == receipts_root,
                "rebuilt receipts root {root} != block receiptsRoot"
            );
            let log = receipts[tx_index]["logs"]
                .get(usize::try_from(*log_index)?)
                .cloned()
                .context("log index")?;
            fixture["tx_hash"] = receipts[tx_index]["transactionHash"].clone();
            fixture["receipts_in_block"] = json!(receipts.len());
            (
                Some(ClaimProof::Log {
                    tx_index: tx_index as u64,
                    log_index: *log_index,
                    receipt_proof: nodes,
                }),
                json!({"address": log["address"], "topics": log["topics"], "data": log["data"], "block_log_index": log["logIndex"]}),
            )
        }
    };

    if let Some(proof) = proof {
        let record = HeaderRecord {
            chain_id,
            slot: 0,
            beacon_root: [0; 32],
            exec_number: number,
            exec_hash,
            exec_state_root: state_root,
            exec_receipts_root: receipts_root,
        };
        let claim = Claim {
            exec_header: None,
            proof: proof.clone(),
        };
        let with_header = Claim {
            exec_header: Some(header.clone()),
            proof,
        };
        // Self-check: both forms verify, the second also against a gloas-style record without roots.
        let gloas_record = HeaderRecord {
            exec_number: 0,
            exec_state_root: [0; 32],
            exec_receipts_root: [0; 32],
            ..record.clone()
        };
        let trivially = Predicate::All(vec![]);
        let any = attest_core::AccountId::new([0; 32]);
        let st = attest_inclusion::verify_claim(any, &record, &claim.to_bytes(), &trivially)
            .map_err(|e| anyhow!("self-check failed: {e}"))?;
        let st2 = attest_inclusion::verify_claim(any, &gloas_record, &with_header.to_bytes(), &trivially)
            .map_err(|e| anyhow!("self-check (header form) failed: {e}"))?;
        ensure!(st == st2, "header and record forms disagree");
        let v = attest_inclusion::verify_claim_value(&record, &claim).map_err(|e| anyhow!("{e}"))?;
        fixture["kind"] = json!(format!("{:?}", claim.kind()));
        fixture["claim"] = json!(h0x(&claim.to_bytes()));
        fixture["claim_with_header"] = json!(h0x(&with_header.to_bytes()));
        fixture["statement_id"] = json!(h0x(&st.id));
        fixture["statement_subject"] = json!(h0x(&st.subject));
        fixture["statement_value"] = json!(h0x(&st.value));
        if let ClaimValue::Word(w) = v.value {
            fixture["proven_word"] = json!(h0x(&w));
        }
    }
    fixture["expected"] = expected;

    let text = serde_json::to_string_pretty(&fixture)? + "\n";
    match cli.out {
        Some(p) => std::fs::write(&p, text).with_context(|| format!("write {}", p.display()))?,
        None => print!("{text}"),
    }
    Ok(())
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (d, s) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (H. Hinnant), UTC.
    let z = d as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        (s % 3600) / 60,
        s % 60
    )
}
