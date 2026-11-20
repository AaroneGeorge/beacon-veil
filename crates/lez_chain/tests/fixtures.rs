//! The committed fixture range `fixtures/lez-chain/` (README there): real seq-dev blocks 2581..2591 with private burns
//! 0 and 1, and a synthetic chain forked after block 2585 with a private and a public burn of known test secrets.
//! Every block parses, re-encodes to its own bytes and hashes to its stored hash and the manifest's; both chains link;
//! every burn is found, and each listed tampering is refused. The whole-chain run against a live sequencer is
//! `tools/lez_chain_probe/whole_chain.sh`.

use std::path::PathBuf;

use lez_chain::*;
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lez-chain")
}

fn manifest() -> Value {
    serde_json::from_slice(&std::fs::read(dir().join("manifest.json")).unwrap()).unwrap()
}

fn read(set: &str, id: u64) -> Vec<u8> {
    std::fs::read(dir().join(format!("{set}/{id}.borsh"))).unwrap()
}

fn h32(v: &Value) -> [u8; 32] {
    let s = v.as_str().unwrap();
    let b: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    b.try_into().unwrap()
}

fn ids(m: &Value, set: &str) -> Vec<(u64, [u8; 32], usize)> {
    m[set]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["id"].as_u64().unwrap(),
                h32(&b["hash"]),
                b["transactions"].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

fn bridge(m: &Value) -> AccountId {
    AccountId(h32(&m["bridge_program_hex"]))
}

fn instance(m: &Value) -> AccountId {
    AccountId(h32(&m["instance_hex"]))
}

#[test]
fn every_fixture_block_parses_reencodes_and_hashes() {
    let m = manifest();
    for (set, key) in [("seq-dev", "seq_dev"), ("synthetic", "synthetic")] {
        let blocks = ids(&m, key);
        assert!(!blocks.is_empty());
        for (id, hash, txs) in blocks {
            let raw = read(set, id);
            let b = parse_block(&raw).unwrap_or_else(|e| panic!("{set}/{id}: {e:?}"));
            assert_eq!(borsh::to_vec(&b).unwrap(), raw, "{set}/{id}: re-encoding");
            assert_eq!(b.header.block_id, id);
            assert_eq!(block_hash(&raw).unwrap(), b.header.hash, "{set}/{id}: stored hash");
            assert_eq!(b.header.hash, hash, "{set}/{id}: manifest hash");
            let spans = transaction_spans(&raw).unwrap();
            assert_eq!((spans.len(), b.body.transactions.len()), (txs, txs));
            for (&(a, z), tx) in spans.iter().zip(&b.body.transactions) {
                assert_eq!(raw[a..z], borsh::to_vec(tx).unwrap()[..], "{set}/{id}: span");
            }
        }
    }
}

#[test]
fn fixture_chains_link_and_refuse_a_mixed_chain() {
    let m = manifest();
    let real: Vec<Vec<u8>> = (2582..=2591).map(|i| read("seq-dev", i)).collect();
    let p = Tip {
        block_id: 2581,
        hash: block_hash(&read("seq-dev", 2581)).unwrap(),
    };
    let tip = verify_chain(p, real.iter().map(Vec::as_slice)).unwrap();
    assert_eq!(tip.hash, ids(&m, "seq_dev").last().unwrap().1);

    let synth: Vec<Vec<u8>> = (2586..=2591).map(|i| read("synthetic", i)).collect();
    let p = Tip {
        block_id: 2585,
        hash: block_hash(&read("seq-dev", 2585)).unwrap(),
    };
    assert_eq!(
        verify_chain(p, synth.iter().map(Vec::as_slice)).unwrap().hash,
        ids(&m, "synthetic").last().unwrap().1
    );

    // Real 2586, then synthetic 2587: the link breaks.
    let mixed = [read("seq-dev", 2586), read("synthetic", 2587)];
    assert_eq!(
        verify_chain(p, mixed.iter().map(Vec::as_slice)),
        Err(ChainError::BrokenLink { block_id: 2587 })
    );
    // Gap and repeat.
    let gap: Vec<&[u8]> = real
        .iter()
        .filter(|b| header(b).unwrap().block_id != 2584)
        .map(Vec::as_slice)
        .collect();
    let p0 = Tip {
        block_id: 2581,
        hash: block_hash(&read("seq-dev", 2581)).unwrap(),
    };
    assert_eq!(
        verify_chain(p0, gap),
        Err(ChainError::IdGap {
            expected: 2584,
            got: 2585
        })
    );
    assert_eq!(
        verify_chain(p0, [real[0].as_slice(), real[0].as_slice()]),
        Err(ChainError::IdGap {
            expected: 2583,
            got: 2582
        })
    );
    // Wrong pinned hash.
    assert_eq!(
        verify_chain(
            Tip {
                block_id: 2581,
                hash: [0; 32]
            },
            real.iter().map(Vec::as_slice)
        ),
        Err(ChainError::BrokenLink { block_id: 2582 })
    );
}

#[test]
fn fixture_burns_are_found_and_tampering_is_refused() {
    let m = manifest();
    let (br, inst) = (bridge(&m), instance(&m));
    for (set, key) in [("seq-dev", "seq_dev"), ("synthetic", "synthetic")] {
        for burn in m[key]["burns"].as_array().unwrap() {
            let id = burn["block"].as_u64().unwrap();
            let tx = burn["tx"].as_u64().unwrap() as usize;
            let note = h32(&burn["note"]);
            let b = parse_block(&read(set, id)).unwrap();
            let found = find_burn(&b, tx, &br, &inst, &note).unwrap_or_else(|e| panic!("{set}/{id}: {e:?}"));
            let route = burn["route"].as_str().unwrap();
            assert_eq!(matches!(found.route, BurnRoute::Public), route == "public");
            // Listed as reverted.
            let mut rev = b.clone();
            rev.body.reverted.push(tx as u64);
            assert_eq!(find_burn(&rev, tx, &br, &inst, &note), Err(ChainError::Reverted));
            // Another note, instance, bridge program; another transaction of the block.
            assert_eq!(find_burn(&b, tx, &br, &inst, &[1; 32]), Err(ChainError::NotABurn));
            assert_eq!(
                find_burn(&b, tx, &br, &AccountId([1; 32]), &note),
                Err(ChainError::NotABurn)
            );
            assert_eq!(
                find_burn(&b, tx, &AccountId([1; 32]), &inst, &note),
                Err(ChainError::NotABurn)
            );
            for other in (0..b.body.transactions.len()).filter(|&t| t != tx) {
                assert_eq!(find_burn(&b, other, &br, &inst, &note), Err(ChainError::NotABurn));
            }
            assert_eq!(
                find_burn(&b, b.body.transactions.len(), &br, &inst, &note),
                Err(ChainError::NoSuchTransaction)
            );
        }
    }
    // Real burn 0's note is not in the synthetic block 2586 (replaced), and a claim block holds no burn.
    let note0 = h32(&m["seq_dev"]["burns"][0]["note"]);
    assert_eq!(
        find_burn(&parse_block(&read("synthetic", 2586)).unwrap(), 0, &br, &inst, &note0),
        Err(ChainError::NotABurn)
    );
    assert_eq!(
        find_burn(&parse_block(&read("seq-dev", 2583)).unwrap(), 0, &br, &inst, &note0),
        Err(ChainError::NotABurn)
    );
    // A changed body byte in the burn block: the stored hash no longer matches.
    let mut t = read("seq-dev", 2586);
    let k = t.len() - 100;
    t[k] ^= 1;
    let p = Tip {
        block_id: 2585,
        hash: block_hash(&read("seq-dev", 2585)).unwrap(),
    };
    assert_eq!(
        verify_chain(p, [t.as_slice()]),
        Err(ChainError::StoredHash { block_id: 2586 })
    );
}
