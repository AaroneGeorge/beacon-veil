//! Bridge library (RFP-022 prototype). Shared by program `bridge`, guest `burn_proof` and `bridge_cli`.
//!
//! - [`hash`]: every hash of the bridge, with its domain string. These are wire format: the vault (Solidity) and the
//!   guests compute the same bytes. See docs/bridge/INBOUND.md and OUTBOUND.md.
//! - [`tree`]: the incremental SHA-256 Merkle tree of the Ethereum deposit contract (CC0), depth 32, used for the
//!   vault's deposit tree and the LEZ burn accumulator.
//! - [`journal`]: the 352-byte public output of guest `burn_proof`, ABI-encoded for `Vault.release`.
//! - [`lez`] (feature `lez`): instance config, account derivations, messages and state of program `bridge`.

#![forbid(unsafe_code)]

pub mod hash;
pub mod journal;
pub mod tree;

#[cfg(feature = "lez")]
pub mod lez;
#[cfg(feature = "lez")]
pub mod program;

/// 32-byte hash.
pub type B32 = [u8; 32];
/// 20-byte Ethereum address.
pub type Addr = [u8; 20];

/// Token address used for native ETH (the vault's `NATIVE`).
pub const NATIVE: Addr = [0; 20];

/// Storage slot of `Vault.depositRoot`: `keccak256("beacon-veil.vault.depositRoot") - 1` (Vault.sol,
/// `DEPOSIT_ROOT_SLOT`). The LEZ claim proves this slot through RFP-021.
pub const DEPOSIT_ROOT_SLOT: B32 = [
    0xc4, 0x8c, 0xe8, 0xc9, 0xc6, 0x9c, 0x08, 0x9f, 0x8e, 0x40, 0x3b, 0xb8, 0x2c, 0x6c, 0x6a, 0x3b, 0x0b, 0x08, 0x2f,
    0x19, 0x7e, 0x29, 0xf6, 0x3d, 0x58, 0x43, 0xb5, 0xb3, 0x7b, 0x73, 0xa2, 0x00,
];
