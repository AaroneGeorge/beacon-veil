//! The JSON-to-SSZ encoder must reproduce, byte for byte, the SSZ bodies that providers serve for the same objects
//! (lodestar, `Accept: application/octet-stream`, fetched 2026-11-12; see tests/data/MANIFEST).

use std::path::PathBuf;

use follower::ssz::{LcKind, encode};
use serde_json::Value;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn check_single(net: &str, kind: LcKind, stem: &str) {
    let v: Value = serde_json::from_slice(&std::fs::read(data(&format!("{net}-{stem}.json"))).unwrap()).unwrap();
    let want = std::fs::read(data(&format!("{net}-{stem}.ssz"))).unwrap();
    let got = encode(kind, v["version"].as_str().unwrap(), &v["data"]).unwrap();
    assert_eq!(got.len(), want.len(), "{net} {stem} length");
    assert!(got == want, "{net} {stem} bytes differ");
}

/// `updates` SSZ responses are chunks: `u64 LE length || 4-byte fork digest || payload`; the length covers the
/// digest and the payload (lodestar, measured).
fn check_updates(net: &str) {
    let v: Value = serde_json::from_slice(&std::fs::read(data(&format!("{net}-updates.json"))).unwrap()).unwrap();
    let raw = std::fs::read(data(&format!("{net}-updates.ssz"))).unwrap();
    let arr = v.as_array().unwrap();
    let mut off = 0usize;
    for e in arr {
        let len = usize::try_from(u64::from_le_bytes(raw[off..off + 8].try_into().unwrap())).unwrap();
        let payload = &raw[off + 12..off + 8 + len];
        let got = encode(LcKind::Update, e["version"].as_str().unwrap(), &e["data"]).unwrap();
        assert!(got == payload, "{net} update bytes differ");
        off += 8 + len;
    }
    assert_eq!(off, raw.len());
}

#[test]
fn sepolia_gloas() {
    check_single("sepolia", LcKind::FinalityUpdate, "finality_update");
    check_single("sepolia", LcKind::Bootstrap, "bootstrap");
    check_updates("sepolia");
}

#[test]
fn mainnet_fulu() {
    check_single("mainnet", LcKind::FinalityUpdate, "finality_update");
    check_single("mainnet", LcKind::Bootstrap, "bootstrap");
    check_updates("mainnet");
}

#[test]
fn wrong_fork_layout_is_refused() {
    let v: Value = serde_json::from_slice(&std::fs::read(data("sepolia-finality_update.json")).unwrap()).unwrap();
    assert!(encode(LcKind::FinalityUpdate, "fulu", &v["data"]).is_err());
    assert!(encode(LcKind::FinalityUpdate, "deneb", &v["data"]).is_err());
}
