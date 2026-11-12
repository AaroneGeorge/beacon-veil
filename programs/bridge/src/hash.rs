//! Every hash of the bridge. All SHA-256 (accelerated in the RISC Zero guest, a precompile on Ethereum).
//! Integers: Ethereum-facing values (denominations, fees) as 32-byte big-endian words, the way Solidity's
//! `abi.encodePacked(uint256)` writes them; LEZ-only counters as 8-byte little-endian.

use sha2::{Digest, Sha256};

use crate::{Addr, B32};

/// Domain strings. Wire format: never change one without a new version suffix.
pub mod domain {
    /// Vault leaf (Solidity `bytes10("wb/leaf/v1")`).
    pub const LEAF: &[u8] = b"wb/leaf/v1";
    pub const COMMIT: &[u8] = b"wb/c/v1";
    pub const DEST: &[u8] = b"wb/dest/v1";
    pub const KEY: &[u8] = b"wb/key/v1";
    pub const DEPOSIT_SECRET: &[u8] = b"wb/dep/v1";
    pub const DEPOSIT_NULLIFIER: &[u8] = b"wb/n/v1";
    pub const BURN_SECRET: &[u8] = b"wb/burn/v1";
    pub const BURN_NOTE: &[u8] = b"wb/b/v1";
    pub const BURN_LEAF: &[u8] = b"wb/bleaf/v1";
    pub const BURN_NULLIFIER: &[u8] = b"wb/bn/v1";
    pub const MEMO: &[u8] = b"wb/memo/v1";
}

/// `sha256(parts[0] || parts[1] || ...)`
#[must_use]
pub fn sha(parts: &[&[u8]]) -> B32 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// `u128` as a 32-byte big-endian word (Solidity `uint256`).
#[must_use]
pub fn word(v: u128) -> B32 {
    let mut w = [0_u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// Parses a 32-byte big-endian word that must fit `u128`.
#[must_use]
pub fn word_to_u128(w: &B32) -> Option<u128> {
    if w[..16].iter().any(|b| *b != 0) {
        return None;
    }
    Some(u128::from_be_bytes(w[16..].try_into().ok()?))
}

// ---------------------------------------------------------------- keys (F9: from the wallet seed only)

/// Bridge master key: `sha256("wb/key/v1" || bip39_seed)`. `bip39_seed` is the 64-byte BIP-39 seed of the LEZ wallet
/// mnemonic, the credential the user already holds.
#[must_use]
pub fn bridge_key(bip39_seed: &[u8]) -> B32 {
    sha(&[domain::KEY, bip39_seed])
}

/// Deposit secret number `index` for one vault: `sha256("wb/dep/v1" || key || chain_id_le || vault || index_le)`.
#[must_use]
pub fn deposit_secret(key: &B32, chain_id: u64, vault: &Addr, index: u64) -> B32 {
    sha(&[
        domain::DEPOSIT_SECRET,
        key,
        &chain_id.to_le_bytes(),
        vault,
        &index.to_le_bytes(),
    ])
}

/// Burn secret number `index` for one bridge instance: `sha256("wb/burn/v1" || key || instance || index_le)`.
#[must_use]
pub fn burn_secret(key: &B32, instance: &B32, index: u64) -> B32 {
    sha(&[domain::BURN_SECRET, key, instance, &index.to_le_bytes()])
}

// ---------------------------------------------------------------- inbound

/// Binds the LEZ destination: `sha256("wb/dest/v1" || bridge_instance || dest_account)`.
#[must_use]
pub fn dest_binding(instance: &B32, dest_account: &B32) -> B32 {
    sha(&[domain::DEST, instance, dest_account])
}

/// Deposit commitment: `sha256("wb/c/v1" || secret || dest_binding || token || word(denom))`. Posted to the vault;
/// hides the destination (F2) and binds it (Security #3).
#[must_use]
pub fn commitment(secret: &B32, dest_binding: &B32, token: &Addr, denom: u128) -> B32 {
    sha(&[domain::COMMIT, secret, dest_binding, token, &word(denom)])
}

/// Vault leaf: `sha256("wb/leaf/v1" || token || word(denom) || commitment)`. Computed by the vault itself, so the
/// token and amount in the leaf are what was really escrowed.
#[must_use]
pub fn leaf(token: &Addr, denom: u128, commitment: &B32) -> B32 {
    sha(&[domain::LEAF, token, &word(denom), commitment])
}

/// Deposit nullifier: `sha256("wb/n/v1" || secret)`. Revealed by the claim; one per deposit.
#[must_use]
pub fn deposit_nullifier(secret: &B32) -> B32 {
    sha(&[domain::DEPOSIT_NULLIFIER, secret])
}

// ---------------------------------------------------------------- outbound

/// Burn note: `sha256("wb/b/v1" || burn_secret || recipient || word(fee))`. Appended on LEZ; hides the Ethereum
/// recipient (F5) and binds it with the relayer fee (Security #3).
#[must_use]
pub fn burn_note(burn_secret: &B32, recipient: &Addr, fee: u128) -> B32 {
    sha(&[domain::BURN_NOTE, burn_secret, recipient, &word(fee)])
}

/// Burn-accumulator leaf: `sha256("wb/bleaf/v1" || token || word(denom) || note)`. Computed by program `bridge`, so
/// token and amount are what was really burned.
#[must_use]
pub fn burn_leaf(token: &Addr, denom: u128, note: &B32) -> B32 {
    sha(&[domain::BURN_LEAF, token, &word(denom), note])
}

/// Release nullifier: `sha256("wb/bn/v1" || burn_secret)`.
#[must_use]
pub fn burn_nullifier(burn_secret: &B32) -> B32 {
    sha(&[domain::BURN_NULLIFIER, burn_secret])
}

/// Recovery memo of a burn (F9): `(recipient || fee_be16) XOR keystream(burn_secret)`. Published with the burn so the
/// owner can rebuild the release from the seed alone; to anyone else it is 36 pseudorandom bytes.
#[must_use]
pub fn memo_seal(burn_secret: &B32, recipient: &Addr, fee: u128) -> [u8; 36] {
    let mut m = [0_u8; 36];
    m[..20].copy_from_slice(recipient);
    m[20..].copy_from_slice(&fee.to_be_bytes());
    xor_keystream(burn_secret, &mut m);
    m
}

/// Inverse of [`memo_seal`].
#[must_use]
pub fn memo_open(burn_secret: &B32, memo: &[u8; 36]) -> (Addr, u128) {
    let mut m = *memo;
    xor_keystream(burn_secret, &mut m);
    let mut recipient = [0_u8; 20];
    recipient.copy_from_slice(&m[..20]);
    let fee = u128::from_be_bytes(m[20..].try_into().unwrap_or([0; 16]));
    (recipient, fee)
}

fn xor_keystream(burn_secret: &B32, m: &mut [u8; 36]) {
    let k0 = sha(&[domain::MEMO, burn_secret, &[0]]);
    let k1 = sha(&[domain::MEMO, burn_secret, &[1]]);
    for (i, b) in m.iter_mut().enumerate() {
        *b ^= if i < 32 { k0[i] } else { k1[i - 32] };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_round_trip() {
        assert_eq!(word_to_u128(&word(10_u128.pow(18))), Some(10_u128.pow(18)));
        let mut big = [0_u8; 32];
        big[0] = 1;
        assert_eq!(word_to_u128(&big), None);
    }

    #[test]
    fn memo_round_trip() {
        let s = [7_u8; 32];
        let r = [9_u8; 20];
        let m = memo_seal(&s, &r, 123);
        assert_ne!(&m[..20], &r);
        assert_eq!(memo_open(&s, &m), (r, 123));
        assert_ne!(memo_open(&[8_u8; 32], &m), (r, 123));
    }

    #[test]
    fn commitment_binds_destination() {
        let s = [1_u8; 32];
        let a = commitment(&s, &dest_binding(&[2; 32], &[3; 32]), &[0; 20], 1);
        let b = commitment(&s, &dest_binding(&[2; 32], &[4; 32]), &[0; 20], 1);
        let c = commitment(&s, &dest_binding(&[5; 32], &[3; 32]), &[0; 20], 1);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
