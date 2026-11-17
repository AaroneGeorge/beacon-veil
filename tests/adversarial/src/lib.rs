//! G2 adversarial suite helpers. The tests live in `tests/adversarial.rs`.
//!
//! Native legs call the same pure functions the guests run: `attest_core::lc` (Helios 0.12.0 unmodified plus the
//! approved pre-checks), `attest_core::rules` (`attest` program checks), `attest_inclusion` (claims and predicates)
//! and `consumer_erc20_program::accept`. On-chain legs (env `BV_G2_ONCHAIN=1`, a running `seq-dev` and
//! `deploy/ids.toml`) send the same inputs through `attest_host` and parse the `ESA:Exx` code from the rejection.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

use attest_core::{
    AccountId, EsaError, InstanceConfig,
    lc::{StoreWitness, init_store, verify_and_apply},
};
use fixtures::{
    fabricate::{FabChain, FabParams},
    real::{self, RealChain},
};

/// Placeholder verifier and `attest` program ids for native legs (on chain they come from `deploy/ids.toml`).
pub const NATIVE_VERIFIER: AccountId = AccountId::new([0x11; 32]);
pub const NATIVE_ATTEST: AccountId = AccountId::new([0xa7; 32]);
/// Seed of the fabricated chain used by G2. Any seed works; it is fixed so failures reproduce.
pub const FAB_SEED: &[u8] = b"beacon-veil/g2/fabricated-chain/v1";

/// Real mainnet fixtures (bootstrap of period 1905, updates 1905..1908), `min_participants = 0`.
pub fn mainnet() -> &'static RealChain {
    static C: OnceLock<RealChain> = OnceLock::new();
    C.get_or_init(|| real::load("mainnet", vec![NATIVE_VERIFIER], 0).expect("load fixtures/mainnet"))
}

/// Real Sepolia fixtures (gloas).
pub fn sepolia() -> &'static RealChain {
    static C: OnceLock<RealChain> = OnceLock::new();
    C.get_or_init(|| real::load("sepolia", vec![NATIVE_VERIFIER], 0).expect("load fixtures/sepolia"))
}

/// The fabricated chain impersonating mainnet: same chain id, genesis validators root, forks and checkpoint slot
/// as `mainnet()`; 512 fresh keys per period.
pub fn fab() -> &'static FabChain {
    static C: OnceLock<FabChain> = OnceLock::new();
    C.get_or_init(|| FabChain::build(FabParams::impersonating(&mainnet().config, FAB_SEED)))
}

/// Config of the attacker's own instance (anchored to the fabricated checkpoint).
pub fn fab_config(min_participants: u16) -> InstanceConfig {
    fab().config(vec![NATIVE_VERIFIER], min_participants)
}

/// Store witness after `Init` on `config` with `bootstrap`, then `updates` in order. Panics on any refusal.
pub fn store_after(config: &InstanceConfig, bootstrap: &[u8], updates: &[&[u8]]) -> StoreWitness {
    let (mut w, _) = init_store(config, bootstrap).unwrap_or_else(|e| panic!("init_store refused: {e}"));
    for (i, u) in updates.iter().enumerate() {
        w = verify_and_apply(config, &w, u)
            .unwrap_or_else(|e| panic!("update {i} refused: {e}"))
            .new_witness();
    }
    w
}

/// Asserts that `r` is the error `want`. `case` names the sub-case in the failure message.
#[track_caller]
pub fn expect_code<T>(r: Result<T, EsaError>, want: EsaError, case: &str) {
    match r {
        Ok(_) => panic!("{case}: accepted, expected {want}"),
        Err(got) => assert_eq!(got, want, "{case}: got {got}, expected {want}"),
    }
}

/// Whether the on-chain legs run (`BV_G2_ONCHAIN=1`).
#[must_use]
pub fn onchain() -> bool {
    std::env::var("BV_G2_ONCHAIN").is_ok_and(|v| v == "1")
}
