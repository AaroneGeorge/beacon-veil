//! The bridge key, from the LEZ wallet mnemonic only (F9). The wallet derives its own keys from
//! `Mnemonic::to_seed("")` (LEZ rc3 `wallet/src/storage/mod.rs:44`); we take the same BIP-39 seed through a separate
//! domain, so no new secret is ever generated or backed up.
//!
//! Source of the mnemonic, in order: `$BV_BRIDGE_MNEMONIC_FILE` (a file), `$BV_BRIDGE_MNEMONIC`. Never printed.

use anyhow::{Context as _, Result, bail};
use bridge_program::{B32, hash::bridge_key};

pub fn load_key() -> Result<B32> {
    let phrase = if let Ok(p) = std::env::var("BV_BRIDGE_MNEMONIC_FILE") {
        std::fs::read_to_string(&p).with_context(|| format!("reading mnemonic file {p}"))?
    } else if let Ok(m) = std::env::var("BV_BRIDGE_MNEMONIC") {
        m
    } else {
        bail!("set BV_BRIDGE_MNEMONIC_FILE (or BV_BRIDGE_MNEMONIC) to the LEZ wallet mnemonic")
    };
    key_from_phrase(phrase.trim())
}

pub fn key_from_phrase(phrase: &str) -> Result<B32> {
    let m = bip39::Mnemonic::parse(phrase).context("mnemonic does not parse as BIP-39")?;
    Ok(bridge_key(&m.to_seed("")))
}
