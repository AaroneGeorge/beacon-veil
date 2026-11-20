//! Usage: lez_chain_probe BLOCK_DIR BRIDGE_PROGRAM_ID [LABEL]
//!
//! BLOCK_DIR holds `<id>.borsh` files (served block bytes, from `fetch_blocks.py` or `lez_dbdump`). For every block:
//! LEZ's own `common::block::Block` decodes it, re-encodes it to the same bytes and recomputes the stored hash; the
//! mirror `lez_chain` parses it, re-encodes it to the same bytes, hashes it without a parse to the same hash, and cuts
//! the same transaction spans. The whole range is checked as one hash chain. Every bridge message that shows in block
//! bytes (public root calls and privacy-preserving boundary steps) is listed with LEZ's own decoding, and every burn
//! is located again with `lez_chain::find_burn`. Prints one JSON object; exits 1 on any failure.

use std::{collections::BTreeMap, fs, path::Path, str::FromStr as _};

use bridge_program::lez::BridgeMessage;
use common::{block::Block, transaction::LeeTransaction};
use lee_core::{account::AccountId, execution_state::BoundaryStep};
use serde_json::{Value, json};

fn hex32(b: &[u8]) -> String {
    hex::encode(b)
}

fn variant(m: &BridgeMessage) -> &'static str {
    match m {
        BridgeMessage::Claim(_) => "Claim",
        BridgeMessage::CheckRoot { .. } => "CheckRoot",
        BridgeMessage::Nullify { .. } => "Nullify",
        BridgeMessage::Credit { .. } => "Credit",
        BridgeMessage::Burn { .. } => "Burn",
        BridgeMessage::Append { .. } => "Append",
        BridgeMessage::RecordLeaf(_) => "RecordLeaf",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() >= 3,
        "usage: lez_chain_probe BLOCK_DIR BRIDGE_PROGRAM_ID [LABEL]"
    );
    let dir = Path::new(&args[1]);
    let bridge = AccountId::from_str(&args[2]).expect("bridge program id (base58)");
    let bridge_m = lez_chain::AccountId(*bridge.value());
    let label = args.get(3).cloned().unwrap_or_default();

    let mut ids: Vec<u64> = fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".borsh")?.parse().ok())
        .collect();
    ids.sort_unstable();
    let raws: Vec<Vec<u8>> = ids
        .iter()
        .map(|i| fs::read(dir.join(format!("{i}.borsh"))).expect("read"))
        .collect();

    let mut fail: Vec<String> = Vec::new();
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    let mut bridge_msgs: Vec<Value> = Vec::new();
    let mut burns: Vec<Value> = Vec::new();
    let (mut bytes_total, mut max_block, mut tx_total) = (0_usize, 0_usize, 0_usize);

    for (id, raw) in ids.iter().zip(&raws) {
        bytes_total += raw.len();
        max_block = max_block.max(raw.len());
        // LEZ's own types.
        let block: Block = match borsh::from_slice(raw) {
            Ok(b) => b,
            Err(e) => {
                fail.push(format!("{id}: LEZ decode: {e}"));
                continue;
            }
        };
        if borsh::to_vec(&block).unwrap() != *raw {
            fail.push(format!("{id}: LEZ re-encode differs"));
        }
        let lez_hash = block.recompute_hash();
        if lez_hash != block.header.hash {
            fail.push(format!("{id}: LEZ recompute_hash differs from stored hash"));
        }
        // Mirror.
        match lez_chain::parse_block(raw) {
            Ok(m) => {
                if borsh::to_vec(&m).unwrap() != *raw {
                    fail.push(format!("{id}: mirror re-encode differs"));
                }
            }
            Err(e) => fail.push(format!("{id}: mirror parse: {e:?}")),
        }
        match lez_chain::block_hash(raw) {
            Ok(h) if h == lez_hash.0 => {}
            other => fail.push(format!("{id}: mirror hash {other:?}")),
        }
        match lez_chain::transaction_spans(raw) {
            Ok(spans) => {
                if spans.len() != block.body.transactions.len()
                    || spans
                        .iter()
                        .zip(&block.body.transactions)
                        .any(|(&(a, b), tx)| raw[a..b] != borsh::to_vec(tx).unwrap()[..])
                {
                    fail.push(format!("{id}: transaction spans differ"));
                }
            }
            Err(e) => fail.push(format!("{id}: spans: {e:?}")),
        }

        // Where bridge messages show in block bytes.
        let parsed = lez_chain::parse_block(raw).ok();
        tx_total += block.body.transactions.len();
        for (t, tx) in block.body.transactions.iter().enumerate() {
            let reverted = block.body.is_reverted(t);
            match tx {
                LeeTransaction::Public(p) => {
                    *counts.entry("public").or_default() += 1;
                    let root = &p.message().execution.root;
                    if root.to.program_account_id != bridge {
                        continue;
                    }
                    let Ok(m) = borsh::from_slice::<BridgeMessage>(&root.message) else {
                        fail.push(format!("{id}/{t}: public bridge call does not decode"));
                        continue;
                    };
                    bridge_msgs.push(
                        json!({"block": id, "tx": t, "tx_hash": hex32(&tx.hash().0), "where": "public root call",
                        "variant": variant(&m), "reverted": reverted, "to_account": root.to.account_id.to_string()}),
                    );
                    if let BridgeMessage::Burn {
                        config,
                        note,
                        token,
                        denom,
                        ..
                    } = &m
                    {
                        let instance = bridge_program::lez::instance_id(&bridge, config);
                        let found = parsed.as_ref().map(|b| {
                            lez_chain::find_burn(b, t, &bridge_m, &lez_chain::AccountId(*instance.value()), note)
                        });
                        burns.push(
                            json!({"block": id, "tx": t, "tx_hash": hex32(&tx.hash().0), "route": "public",
                            "instance": instance.to_string(), "token": hex32(token), "denom": denom.to_string(),
                            "note": hex32(note), "reverted": reverted, "lez_chain_find_burn": format!("{found:?}")}),
                        );
                    }
                }
                LeeTransaction::PrivacyPreserving(pp) => {
                    *counts.entry("privacy_preserving").or_default() += 1;
                    let exe = &pp.message().execution;
                    if let Some(root) = &exe.public_root {
                        if root.to.program_account_id == bridge {
                            let v = borsh::from_slice::<BridgeMessage>(&root.message)
                                .map(|m| variant(&m))
                                .unwrap_or("?");
                            bridge_msgs.push(json!({"block": id, "tx": t, "where": "pp public_root", "variant": v}));
                        }
                    }
                    for (s, step) in exe.boundary.iter().enumerate() {
                        let (dir_name, d) = match step {
                            BoundaryStep::PrivateToPublic(d) => ("PrivateToPublic", d),
                            BoundaryStep::PublicToPrivate(d) => ("PublicToPrivate", d),
                            _ => continue,
                        };
                        let env = &d.envelope;
                        if env.to.program_account_id != bridge && env.from.program_account_id != bridge {
                            continue;
                        }
                        let m = borsh::from_slice::<BridgeMessage>(&env.message).ok();
                        bridge_msgs.push(json!({"block": id, "tx": t, "tx_hash": hex32(&tx.hash().0), "where": "pp boundary",
                            "step": s, "direction": dir_name, "variant": m.as_ref().map(variant),
                            "from_account": env.from.account_id.to_string(), "from_program": env.from.program_account_id.to_string(),
                            "to_account": env.to.account_id.to_string(), "to_program": env.to.program_account_id.to_string(),
                            "inherited_authorizations": d.inherited_authorizations.len(),
                            "inherits_entry_authorizations": d.inherits_entry_authorizations,
                            "pda_seeds": d.pda_seeds.len(), "reverted": reverted}));
                        if let Some(BridgeMessage::Append {
                            instance,
                            token,
                            denom,
                            note,
                            ..
                        }) = &m
                        {
                            let acc =
                                AccountId::for_public_pda(&bridge, &bridge_program::lez::accumulator_seed(instance));
                            let found = parsed.as_ref().map(|b| {
                                lez_chain::find_burn(b, t, &bridge_m, &lez_chain::AccountId(*instance.value()), note)
                            });
                            burns.push(json!({"block": id, "tx": t, "tx_hash": hex32(&tx.hash().0), "route": "private (boundary Append)",
                                "step": s, "boundary_len": exe.boundary.len(), "instance": instance.to_string(),
                                "accumulator_expected": acc.to_string(), "to_is_accumulator": env.to.account_id == acc,
                                "from_program_is_bridge": env.from.program_account_id == bridge,
                                "from_account": env.from.account_id.to_string(),
                                "public_root": exe.public_root.is_some(), "private_actions": exe.private_actions.len(),
                                "token": hex32(token), "denom": denom.to_string(), "note": hex32(note), "reverted": reverted,
                                "lez_chain_find_burn": format!("{found:?}"),
                                "block_bytes": raw.len(), "tx_bytes": borsh::to_vec(tx).unwrap().len(),
                                "proof_bytes": borsh::to_vec(pp.witness_set()).unwrap().len()}));
                        }
                    }
                }
            }
        }
    }

    // The whole range as one chain from its first block.
    let chain = if raws.is_empty() {
        "empty".to_owned()
    } else {
        let first = lez_chain::header(&raws[0]).expect("header");
        let pinned = lez_chain::Tip {
            block_id: first.block_id,
            hash: lez_chain::block_hash(&raws[0]).unwrap(),
        };
        format!(
            "{:?}",
            lez_chain::verify_chain(pinned, raws[1..].iter().map(Vec::as_slice))
        )
    };

    let out = json!({
        "label": label, "dir": dir.display().to_string(), "bridge_program": bridge.to_string(),
        "blocks": ids.len(), "first": ids.first(), "last": ids.last(), "contiguous": ids.windows(2).all(|w| w[1] == w[0] + 1),
        "bytes_total": bytes_total, "max_block_bytes": max_block, "transactions": tx_total, "tx_kinds": counts,
        "failures": fail.len(), "failure_list": fail.iter().take(20).collect::<Vec<_>>(),
        "chain_check_first_to_last": chain,
        "bridge_messages": bridge_msgs, "burns": burns,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    if !fail.is_empty() || !chain.starts_with("Ok(") {
        std::process::exit(1);
    }
}
