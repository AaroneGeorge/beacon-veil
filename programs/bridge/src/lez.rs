//! Program `bridge` on LEZ: instance config, account derivations, messages, state, errors.
//!
//! Accounts (all public PDAs of the bridge program, except holdings):
//! - instance id = `PDA(bridge, sha256("bridge/instance/v1" || borsh(config)))`. A namespace only: the config is
//!   carried in each message and re-hashed, so it cannot change (F16 separation, F15 pairing).
//! - root receipt = `PDA(bridge, sha256("bridge/root/v1" || sender_program || attest_instance || stmt_id))`. Written
//!   only by a `Statement` delivery whose sender program and contents give exactly this seed, so nobody can squat it.
//! - claim nullifier = `PDA(bridge, sha256("bridge/null/v1" || bridge_instance || deposit_nullifier))`. Empty means
//!   unclaimed. Keyed by the bridge instance so a fake instance cannot burn a real one's nullifier.
//! - burn accumulator = `PDA(bridge, sha256("bridge/acc/v1" || bridge_instance))`.
//! - burn leaf record = `PDA(bridge, sha256("bridge/bleaf/v1" || bridge_instance || index_le))`, write once.
//! - holding: the bridge actor state on the holder's own account (private or public): wrapped balances per
//!   (instance, token).

use attest_core::{AccountId, ClaimKind, HeaderRecord, Statement, statement_id_state};
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::program::PdaSeed;

use crate::{Addr, B32, DEPOSIT_ROOT_SLOT, hash::sha, tree::DEPTH};

pub mod domain {
    pub const INSTANCE: &[u8] = b"bridge/instance/v1";
    pub const ROOT: &[u8] = b"bridge/root/v1";
    pub const NULLIFIER: &[u8] = b"bridge/null/v1";
    pub const ACCUMULATOR: &[u8] = b"bridge/acc/v1";
    pub const LEAF_RECORD: &[u8] = b"bridge/bleaf/v1";
}

// ---------------------------------------------------------------- config

/// One registered token (F10 at spike depth: fixed in the config).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenEntry {
    /// ERC-20 address, or [`crate::NATIVE`] for ETH.
    pub token: Addr,
    /// Permitted denominations in the token's base units. The wrapped token uses the same base unit (identity
    /// conversion, exact by construction).
    pub denoms: Vec<u128>,
    /// Protocol fee per mint and per burn, in wrapped base units, the same for every user (F13).
    pub mint_fee: u128,
    pub burn_fee: u128,
}

/// Immutable configuration of one bridge instance (one entity's deployment, F16).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BridgeConfig {
    /// Ethereum chain id and vault address (F15).
    pub chain_id: u64,
    pub vault: Addr,
    /// RFP-021: the `attest` program and the light-client instance whose headers this bridge trusts.
    pub attest_program: AccountId,
    pub attest_instance: AccountId,
    pub tokens: Vec<TokenEntry>,
    /// Receives protocol fees, if any.
    pub treasury: Option<AccountId>,
}

impl BridgeConfig {
    #[must_use]
    pub fn token(&self, token: &Addr) -> Option<&TokenEntry> {
        self.tokens.iter().find(|t| &t.token == token)
    }

    /// Statement id of "vault slot `DEPOSIT_ROOT_SLOT` holds `root`" on this chain (W3's `statement_id_state`).
    #[must_use]
    pub fn root_statement_id(&self, root: &B32) -> B32 {
        statement_id_state(self.chain_id, &self.vault, &DEPOSIT_ROOT_SLOT, root)
    }

    /// Expected statement subject: `vault || slot_key`.
    #[must_use]
    pub fn root_subject(&self) -> Vec<u8> {
        let mut s = self.vault.to_vec();
        s.extend_from_slice(&DEPOSIT_ROOT_SLOT);
        s
    }
}

// ---------------------------------------------------------------- ids

#[must_use]
pub fn instance_seed(config: &BridgeConfig) -> B32 {
    sha(&[domain::INSTANCE, &borsh::to_vec(config).expect("borsh of BridgeConfig")])
}

#[must_use]
pub fn instance_id(bridge: &AccountId, config: &BridgeConfig) -> AccountId {
    AccountId::for_public_pda(bridge, &PdaSeed::new(instance_seed(config)))
}

#[must_use]
pub fn root_seed(sender_program: &AccountId, attest_instance: &AccountId, stmt_id: &B32) -> PdaSeed {
    PdaSeed::new(sha(&[
        domain::ROOT,
        sender_program.value(),
        attest_instance.value(),
        stmt_id,
    ]))
}

#[must_use]
pub fn nullifier_seed(instance: &AccountId, deposit_nullifier: &B32) -> PdaSeed {
    PdaSeed::new(sha(&[domain::NULLIFIER, instance.value(), deposit_nullifier]))
}

#[must_use]
pub fn accumulator_seed(instance: &AccountId) -> PdaSeed {
    PdaSeed::new(sha(&[domain::ACCUMULATOR, instance.value()]))
}

/// Write-once record of burn leaf `index` (spike stand-in for an indexer over LP-0012 events).
#[must_use]
pub fn leaf_record_seed(instance: &AccountId, index: u64) -> PdaSeed {
    PdaSeed::new(sha(&[domain::LEAF_RECORD, instance.value(), &index.to_le_bytes()]))
}

#[must_use]
pub fn pda(bridge: &AccountId, seed: &PdaSeed) -> AccountId {
    AccountId::for_public_pda(bridge, seed)
}

// ---------------------------------------------------------------- messages

/// The attestation leg of a claim: `Attest` runs on `on` (normally the claimer's own private account) under the
/// attest program and delivers the statement to the root receipt. Omitted when the root is already attested.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AttestCall {
    pub on: AccountId,
    pub record: HeaderRecord,
    /// `attest_inclusion::Claim::Storage` for the vault's root slot, borsh (W3), opaque here.
    pub claim: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ClaimArgs {
    pub config: BridgeConfig,
    pub token: Addr,
    pub denom: u128,
    /// Deposit secret (private input; the claim must run privately).
    pub secret: B32,
    /// Account that receives the wrapped tokens; bound in the commitment.
    pub dest: AccountId,
    pub leaf_index: u64,
    pub path: [B32; DEPTH],
    pub root: B32,
    pub attest: Option<AttestCall>,
}

/// Messages of program `bridge`. A `Statement` from another program arrives as raw `borsh(Statement)` instead.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BridgeMessage {
    /// Top level, on the claimer's actor. Proves the deposit opening and Merkle path, then: attest (optional),
    /// `CheckRoot`, `Nullify`, credit.
    Claim(Box<ClaimArgs>),
    /// Own program only, to the root receipt: panics unless it holds the matching statement.
    CheckRoot {
        attest_program: AccountId,
        attest_instance: AccountId,
        chain_id: u64,
        vault: Addr,
        root: B32,
    },
    /// Own program only, to the claim nullifier PDA: panics `E11` unless empty, then marks it.
    Nullify { instance: AccountId, nullifier: B32 },
    /// Own program only: adds `amount` of `(instance, token)` to the receiver's holding.
    Credit {
        instance: AccountId,
        token: Addr,
        amount: u128,
    },
    /// Holder-authorized, on the holder's actor (private or public): debits `denom + burn_fee` and appends the note.
    Burn {
        config: BridgeConfig,
        token: Addr,
        denom: u128,
        note: B32,
        memo: [u8; 36],
    },
    /// Own program only, to the accumulator PDA. Appends, then writes the leaf record of the new index.
    Append {
        instance: AccountId,
        token: Addr,
        denom: u128,
        note: B32,
        memo: [u8; 36],
    },
    /// Own program only, to the leaf record PDA of `event.index`: write once.
    RecordLeaf(BurnEvent),
}

// ---------------------------------------------------------------- state

/// Wrapped balances held by one account.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Holding {
    pub balances: Vec<(AccountId, Addr, u128)>,
}

impl Holding {
    #[must_use]
    pub fn decode(state: &[u8]) -> Option<Self> {
        if state.is_empty() {
            Some(Self::default())
        } else {
            borsh::from_slice(state).ok()
        }
    }

    #[must_use]
    pub fn balance(&self, instance: &AccountId, token: &Addr) -> u128 {
        self.balances
            .iter()
            .find(|(i, t, _)| i == instance && t == token)
            .map_or(0, |(_, _, a)| *a)
    }

    pub fn set(&mut self, instance: AccountId, token: Addr, amount: u128) {
        self.balances.retain(|(i, t, _)| !(i == &instance && t == &token));
        if amount > 0 {
            self.balances.push((instance, token, amount));
        }
    }
}

/// Burn accumulator state of one instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Accumulator {
    pub tree: crate::tree::IncrementalTree,
    /// Root after the last append; what the anchor posts.
    pub root: B32,
}

/// Event data of one burn (selector [`burn_event_selector`]). The memo lets the owner recover from the seed (F9).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BurnEvent {
    pub instance: AccountId,
    pub index: u64,
    pub token: Addr,
    pub denom: u128,
    pub note: B32,
    pub memo: [u8; 36],
    pub root: B32,
}

/// First 8 bytes of `sha256("bridge::BurnAppended")` (LEZ event convention).
#[must_use]
pub fn burn_event_selector() -> [u8; 8] {
    let h = sha(&[b"bridge::BurnAppended"]);
    let mut s = [0_u8; 8];
    s.copy_from_slice(&h[..8]);
    s
}

/// True when `stmt` says: the deposit-root slot of `vault` on `chain_id` holds `root`, under `attest_instance`.
#[must_use]
pub fn statement_proves_root(
    stmt: &Statement,
    attest_instance: &AccountId,
    chain_id: u64,
    vault: &Addr,
    root: &B32,
) -> bool {
    let mut subject = vault.to_vec();
    subject.extend_from_slice(&DEPOSIT_ROOT_SLOT);
    stmt.instance == *attest_instance
        && stmt.chain_id == chain_id
        && stmt.kind == ClaimKind::Storage
        && stmt.subject == subject
        && stmt.value.as_slice() == root.as_slice()
        && stmt.id == statement_id_state(chain_id, vault, &DEPOSIT_ROOT_SLOT, root)
}

// ---------------------------------------------------------------- errors

/// Bridge-specific error codes; panic text `WB:Bxx`. Shared checks use `attest_core::EsaError` (`ESA:E01` malformed,
/// `ESA:E07` bad Merkle path, `ESA:E11` already claimed). No message names a deposit or burn (Privacy #5, U8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BridgeError {
    UnsupportedToken = 1,
    InvalidAmount = 2,
    RootNotAttested = 3,
    InsufficientBalance = 4,
    NotAuthorized = 5,
    WrongAccount = 6,
    FeeTooHigh = 7,
}

impl BridgeError {
    pub const ALL: [Self; 7] = [
        Self::UnsupportedToken,
        Self::InvalidAmount,
        Self::RootNotAttested,
        Self::InsufficientBalance,
        Self::NotAuthorized,
        Self::WrongAccount,
        Self::FeeTooHigh,
    ];

    #[must_use]
    pub fn panic_text(self) -> String {
        format!("WB:B{:02}", self as u8)
    }

    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::UnsupportedToken => "unsupported token",
            Self::InvalidAmount => "invalid amount: not a permitted denomination",
            Self::RootNotAttested => "insufficient finality: the deposit root is not attested by a finalised header",
            Self::InsufficientBalance => "insufficient wrapped balance",
            Self::NotAuthorized => "not authorized",
            Self::WrongAccount => "wrong account for this message",
            Self::FeeTooHigh => "fee must be below the denomination",
        }
    }

    #[must_use]
    pub fn from_panic_text(text: &str) -> Option<Self> {
        let i = text.find("WB:B")?;
        let n: u8 = text.get(i + 4..i + 6)?.parse().ok()?;
        Self::ALL.into_iter().find(|e| *e as u8 == n)
    }

    pub fn abort(self) -> ! {
        panic!("{}", self.panic_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_round_trip() {
        for e in BridgeError::ALL {
            assert_eq!(BridgeError::from_panic_text(&e.panic_text()), Some(e));
        }
    }

    #[test]
    fn nullifier_pda_depends_on_instance() {
        let bridge = AccountId::new([1; 32]);
        let a = pda(&bridge, &nullifier_seed(&AccountId::new([2; 32]), &[9; 32]));
        let b = pda(&bridge, &nullifier_seed(&AccountId::new([3; 32]), &[9; 32]));
        assert_ne!(a, b);
    }

    #[test]
    fn holding_set_and_get() {
        let i = AccountId::new([1; 32]);
        let mut h = Holding::default();
        h.set(i, [0; 20], 5);
        h.set(i, [1; 20], 7);
        h.set(i, [0; 20], 9);
        assert_eq!(h.balance(&i, &[0; 20]), 9);
        assert_eq!(h.balance(&i, &[1; 20]), 7);
        h.set(i, [1; 20], 0);
        assert_eq!(h.balances.len(), 1);
        let enc = borsh::to_vec(&h).unwrap();
        assert_eq!(Holding::decode(&enc), Some(h));
    }
}
