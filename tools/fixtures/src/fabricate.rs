//! The fabricated chain (RFP-021 Verification Security 1, PLAN.md W2 scope).
//!
//! "Given a self-consistent header and inclusion proof over state that was never finalised on the canonical chain,
//! verification must fail" (021:650-653). This module builds exactly that, as an attacker with unlimited compute but
//! no canonical sync-committee keys would:
//!
//! - 512 fresh BLS12-381 keys per sync-committee period, derived from a seed, and their `SyncCommittee`;
//! - a checkpoint header and bootstrap whose state commits to committee P;
//! - a period-P update signed by committee P that finalizes a header and hands off to committee P+1 (valid next
//!   committee branch and finality branch under the attested state root);
//! - a period-P+1 update signed by committee P+1 (the handoff), and a finality update after it;
//! - every finalized header carries an execution payload header (valid execution branch) whose `state_root` is a
//!   real Merkle-Patricia trie containing an attacker-chosen account and storage slot, with `eth_getProof`-style
//!   proofs (`FabState`).
//!
//! By default the chain impersonates a real one: same chain id, genesis validators root, fork schedule and slot
//! numbers. Only the keys differ. The chain verifies under Helios when an instance is anchored to its own
//! checkpoint (positive control) and must fail when anchored to the real one.

use alloy_primitives::{Address, B256, U256};
use attest_core::{AccountId, HeaderRecord, InstanceConfig};
use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Projective, Scalar,
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
};
use helios_consensus_core::types::{BeaconBlockHeader, ExecutionPayloadHeader, ExecutionPayloadHeaderElectra};
use sha2::{Digest, Sha256};
use ssz::Decode;
use tree_hash::TreeHash;

use crate::{
    exec::FabState,
    merkle::{SparseTree, gindex},
    ssz::{BootstrapParts, Committee, Header, UpdateParts},
};

pub const SLOTS_PER_EPOCH: u64 = 32;
pub const SLOTS_PER_PERIOD: u64 = 8192;
pub const COMMITTEE_SIZE: usize = 512;
/// Smallest participation that meets the 2/3 rule: 342 * 3 >= 1024.
pub const QUORUM: usize = 342;
/// BLS ciphersuite of the sync committee (consensus-specs, same as Helios `types/bls.rs`).
pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";
/// `DOMAIN_SYNC_COMMITTEE`.
pub const DOMAIN_SYNC_COMMITTEE: [u8; 4] = [7, 0, 0, 0];

fn h(parts: &[&[u8]]) -> [u8; 32] {
    let mut s = Sha256::new();
    for p in parts {
        s.update(p);
    }
    s.finalize().into()
}

/// The 512 secret keys of one period and the `SyncCommittee` they form.
#[derive(Clone)]
pub struct Keys {
    pub period: u64,
    sks: Vec<Scalar>,
    pub committee: Committee,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys {{ period: {}, root: {} }}", self.period, self.root())
    }
}

impl Keys {
    /// Deterministic keys: `sk_i = from_bytes_wide(sha256(seed||"bls"||period||i||0) || sha256(..||1)) mod r`.
    #[must_use]
    pub fn derive(seed: &[u8], period: u64) -> Self {
        let mut sks = Vec::with_capacity(COMMITTEE_SIZE);
        let mut ssz = Vec::with_capacity(COMMITTEE_SIZE * 48 + 48);
        let mut agg = G1Projective::identity();
        for i in 0..COMMITTEE_SIZE as u64 {
            let a = h(&[seed, b"bls", &period.to_le_bytes(), &i.to_le_bytes(), &[0]]);
            let b = h(&[seed, b"bls", &period.to_le_bytes(), &i.to_le_bytes(), &[1]]);
            let mut wide = [0u8; 64];
            wide[..32].copy_from_slice(&a);
            wide[32..].copy_from_slice(&b);
            let sk = Scalar::from_bytes_wide(&wide);
            let pk = G1Projective::generator() * sk;
            agg += pk;
            ssz.extend_from_slice(&G1Affine::from(pk).to_compressed());
            sks.push(sk);
        }
        ssz.extend_from_slice(&G1Affine::from(agg).to_compressed());
        let committee = Committee::from_ssz_bytes(&ssz).expect("committee SSZ");
        Self { period, sks, committee }
    }

    #[must_use]
    pub fn root(&self) -> B256 {
        self.committee.tree_hash_root()
    }

    /// Aggregate BLS signature of the members whose bit is set, over `signing_root`.
    #[must_use]
    pub fn sign(&self, bits: &[bool], signing_root: &B256) -> [u8; 96] {
        let sk = self
            .sks
            .iter()
            .zip(bits)
            .filter(|(_, b)| **b)
            .fold(Scalar::zero(), |acc, (k, _)| acc + k);
        let hm =
            <G2Projective as HashToCurve<ExpandMsgXmd<sha2_v0_9::Sha256>>>::hash_to_curve(signing_root.as_slice(), DST);
        G2Affine::from(hm * sk).to_compressed()
    }
}

/// Fork version active at `slot` under `(activation_epoch, version)` pairs in order.
#[must_use]
pub fn fork_version_at(forks: &[(u64, [u8; 4])], slot: u64) -> [u8; 4] {
    let epoch = slot / SLOTS_PER_EPOCH;
    forks
        .iter()
        .rev()
        .find(|(e, _)| *e <= epoch)
        .map_or([0; 4], |(_, v)| *v)
}

/// consensus-specs `compute_signing_root(header, compute_domain(DOMAIN_SYNC_COMMITTEE, version, gvr))`.
#[must_use]
pub fn sync_committee_signing_root(header_root: B256, fork_version: [u8; 4], genesis_validators_root: B256) -> B256 {
    let mut v = [0u8; 32];
    v[..4].copy_from_slice(&fork_version);
    let fork_data_root = h(&[&v, genesis_validators_root.as_slice()]);
    let mut domain = [0u8; 32];
    domain[..4].copy_from_slice(&DOMAIN_SYNC_COMMITTEE);
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    B256::from(h(&[header_root.as_slice(), &domain]))
}

/// The first `n` members participate.
#[must_use]
pub fn bits_first(n: usize) -> Vec<bool> {
    (0..COMMITTEE_SIZE).map(|i| i < n).collect()
}

/// What the fabricated chain imitates, and what it contains.
#[derive(Debug, Clone)]
pub struct FabParams {
    pub seed: Vec<u8>,
    pub chain_id: u64,
    pub genesis_validators_root: B256,
    pub genesis_time: u64,
    pub forks: Vec<(u64, [u8; 4])>,
    /// Slot of the fabricated checkpoint. Must leave room for two updates in its period.
    pub checkpoint_slot: u64,
    /// Signers of every update (default 512).
    pub participants: usize,
    /// The attacker's account, its balance, and one storage slot ("any deposit they like").
    pub address: Address,
    pub balance: U256,
    pub slot_key: B256,
    pub slot_value: U256,
}

impl FabParams {
    /// Same chain id, genesis validators root, genesis time, forks and checkpoint slot as `real`; only keys differ.
    #[must_use]
    pub fn impersonating(real: &InstanceConfig, seed: &[u8]) -> Self {
        Self {
            seed: seed.to_vec(),
            chain_id: real.chain_id,
            genesis_validators_root: B256::from(real.genesis_validators_root),
            genesis_time: real.genesis_time,
            forks: real.forks.clone(),
            checkpoint_slot: real.checkpoint_slot,
            participants: COMMITTEE_SIZE,
            address: Address::repeat_byte(0xfa),
            balance: U256::from(1_000_000u64) * U256::from(10u64).pow(U256::from(18u64)),
            slot_key: B256::ZERO,
            slot_value: U256::from(0xdead_beefu64),
        }
    }
}

/// The fabricated chain.
#[derive(Debug, Clone)]
pub struct FabChain {
    pub params: FabParams,
    /// Committees of periods P, P+1, P+2 (P = period of the checkpoint).
    pub keys: Vec<Keys>,
    pub bootstrap: BootstrapParts,
    pub checkpoint_root: B256,
    /// `[0]` period-P `LightClientUpdate` (finalizes, carries committee P+1); `[1]` period-P+1 `LightClientUpdate`
    /// signed by P+1 (handoff, carries P+2); `[2]` period-P+1 `LightClientFinalityUpdate`.
    pub updates: Vec<UpdateParts>,
    /// The execution state every finalized header commits to.
    pub state: FabState,
}

impl FabChain {
    /// Builds the chain. Panics if the forks put the checkpoint in gloas (only Electra/Fulu containers are built).
    #[must_use]
    pub fn build(p: FabParams) -> Self {
        let cp = p.checkpoint_slot;
        let period = cp / SLOTS_PER_PERIOD;
        assert!(
            cp % SLOTS_PER_PERIOD + 4 * SLOTS_PER_EPOCH < SLOTS_PER_PERIOD,
            "checkpoint too late in its period"
        );
        let electra = p.forks.get(5).map_or(u64::MAX, |f| f.0);
        let gloas = p.forks.get(7).map_or(u64::MAX, |f| f.0);
        assert!(
            cp / SLOTS_PER_EPOCH >= electra,
            "fabricated chain builds Electra/Fulu containers only"
        );
        assert!(
            (period + 2) * SLOTS_PER_PERIOD / SLOTS_PER_EPOCH < gloas,
            "fabricated chain must stay before gloas"
        );

        let keys: Vec<Keys> = (0..3).map(|i| Keys::derive(&p.seed, period + i)).collect();
        let state = FabState::build(&p.seed, p.address, p.balance, p.slot_key, p.slot_value, 24);

        let parent = B256::from(h(&[&p.seed, b"genesis-parent"]));
        // Checkpoint and bootstrap (state commits to committee P).
        let (cp_header, cp_state) = make_header(
            &p,
            &state,
            cp,
            parent,
            &[(gindex::CURRENT_SYNC_COMMITTEE, keys[0].root())],
        );
        let bootstrap = BootstrapParts {
            committee: keys[0].committee.clone(),
            committee_branch: cp_state.branch(gindex::CURRENT_SYNC_COMMITTEE),
            header: cp_header.clone(),
        };
        let checkpoint_root = cp_header.root();

        let mut chain = Self {
            params: p,
            keys,
            bootstrap,
            checkpoint_root,
            updates: Vec::new(),
            state,
        };
        let p1 = (period + 1) * SLOTS_PER_PERIOD;
        let u0 = chain.update_at(cp + 2 * SLOTS_PER_EPOCH, 0, Some(1));
        let u1 = chain.update_at(p1 + 2 * SLOTS_PER_EPOCH, 1, Some(2));
        let u2 = chain.update_at(p1 + 4 * SLOTS_PER_EPOCH, 1, None);
        chain.updates = vec![u0, u1, u2];
        chain
    }

    /// A new update finalizing a header at `finalized_slot`, attested one epoch later, signed by `self.keys[signer]`
    /// (all `params.participants`), optionally carrying `self.keys[next]` as the next committee.
    #[must_use]
    pub fn update_at(&self, finalized_slot: u64, signer: usize, next: Option<usize>) -> UpdateParts {
        self.forge_update(finalized_slot, &self.keys[signer], next.map(|n| &self.keys[n]))
    }

    /// As [`Self::update_at`] with any signing committee and any next committee (forgeries: a conflicting handoff).
    #[must_use]
    pub fn forge_update(&self, finalized_slot: u64, signer: &Keys, next: Option<&Keys>) -> UpdateParts {
        let p = &self.params;
        let parent = B256::from(h(&[&p.seed, b"parent", &finalized_slot.to_le_bytes()]));
        let (fin, _) = make_header(
            p,
            &self.state,
            finalized_slot,
            parent,
            &[(gindex::CURRENT_SYNC_COMMITTEE, signer.root())],
        );
        let mut leaves = vec![
            (gindex::FINALIZED_ROOT, fin.root()),
            (gindex::CURRENT_SYNC_COMMITTEE, signer.root()),
        ];
        if let Some(n) = next {
            leaves.push((gindex::NEXT_SYNC_COMMITTEE, n.root()));
        }
        let (att, ast) = make_header(p, &self.state, finalized_slot + SLOTS_PER_EPOCH, fin.root(), &leaves);
        let mut u = UpdateParts {
            signature_slot: att.beacon.slot + 1,
            next: next.map(|n| (n.committee.clone(), ast.branch(gindex::NEXT_SYNC_COMMITTEE))),
            attested: att,
            finalized: fin,
            finality_branch: ast.branch(gindex::FINALIZED_ROOT),
            bits: bits_first(p.participants),
            signature: [0; 96],
        };
        sign_with(&mut u, signer, &p.forks, p.genesis_validators_root);
        u
    }
}

/// A beacon header at `slot` whose state tree holds `state_leaves` and whose body commits to an execution payload
/// header over the fabricated execution state.
fn make_header(
    p: &FabParams,
    state: &FabState,
    slot: u64,
    parent_root: B256,
    state_leaves: &[(u64, B256)],
) -> (Header, SparseTree) {
    let cp = p.checkpoint_slot;
    let exec = ExecutionPayloadHeader::Electra(ExecutionPayloadHeaderElectra {
        parent_hash: B256::from(h(&[&p.seed, b"exec-parent", &slot.to_le_bytes()])),
        state_root: state.state_root,
        receipts_root: state.receipts_root,
        block_number: 20_000_000 + slot.saturating_sub(cp),
        gas_limit: 36_000_000,
        gas_used: 12_000_000,
        timestamp: p.genesis_time + 12 * slot,
        block_hash: B256::from(h(&[&p.seed, b"exec-hash", &slot.to_le_bytes()])),
        transactions_root: B256::from(h(&[&p.seed, b"txs", &slot.to_le_bytes()])),
        withdrawals_root: B256::from(h(&[&p.seed, b"wds", &slot.to_le_bytes()])),
        base_fee_per_gas: U256::from(1_000_000_000u64),
        ..Default::default()
    });
    let body = SparseTree::new(
        B256::from(h(&[&p.seed, b"body", &slot.to_le_bytes()])),
        &[(gindex::EXECUTION_PAYLOAD, exec.tree_hash_root())],
    );
    let st = SparseTree::new(B256::from(h(&[&p.seed, b"state", &slot.to_le_bytes()])), state_leaves);
    let beacon = BeaconBlockHeader {
        slot,
        proposer_index: slot % 997,
        parent_root,
        state_root: st.root(),
        body_root: body.root(),
    };
    (
        Header {
            beacon,
            execution: exec,
            execution_branch: body.branch(gindex::EXECUTION_PAYLOAD),
        },
        st,
    )
}

impl FabChain {
    /// Instance config anchored to the fabricated checkpoint (the attacker's own instance).
    #[must_use]
    pub fn config(&self, verifiers: Vec<AccountId>, min_participants: u16) -> InstanceConfig {
        InstanceConfig {
            chain_id: self.params.chain_id,
            genesis_validators_root: self.params.genesis_validators_root.0,
            genesis_time: self.params.genesis_time,
            forks: self.params.forks.clone(),
            checkpoint_root: self.checkpoint_root.0,
            checkpoint_slot: self.params.checkpoint_slot,
            verifiers,
            min_participants,
        }
    }

    /// Re-signs `u` with the committee of the period of its signature slot, under this chain's domain.
    pub fn resign(&self, u: &mut UpdateParts) {
        let k = self.keys_for_slot(u.signature_slot);
        sign_with(u, k, &self.params.forks, self.params.genesis_validators_root);
    }

    #[must_use]
    pub fn keys_for_slot(&self, slot: u64) -> &Keys {
        let p = slot / SLOTS_PER_PERIOD;
        self.keys
            .iter()
            .find(|k| k.period == p)
            .expect("no fabricated committee for that period")
    }

    /// The header record a verifier would derive from update `i`'s finalized header.
    #[must_use]
    pub fn record(&self, i: usize) -> HeaderRecord {
        let f = &self.updates[i].finalized;
        let ExecutionPayloadHeader::Electra(e) = &f.execution else {
            unreachable!()
        };
        HeaderRecord {
            chain_id: self.params.chain_id,
            slot: f.beacon.slot,
            beacon_root: f.root().0,
            exec_number: e.block_number,
            exec_hash: e.block_hash.0,
            exec_state_root: e.state_root.0,
            exec_receipts_root: e.receipts_root.0,
        }
    }
}

/// Signs `u` (its attested header at its signature slot) with `keys` under `(forks, gvr)`, using the fork version of
/// `signature_slot - 1` as Helios does (`consensus_core.rs` `verify_generic_update`).
pub fn sign_with(u: &mut UpdateParts, keys: &Keys, forks: &[(u64, [u8; 4])], gvr: B256) {
    let version = fork_version_at(forks, u.signature_slot.saturating_sub(1));
    let root = sync_committee_signing_root(u.attested.root(), version, gvr);
    u.signature = keys.sign(&u.bits, &root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merkle::root_from_branch;

    fn params() -> FabParams {
        let forks = vec![
            (0, [0, 0, 0, 0]),
            (1, [1, 0, 0, 0]),
            (2, [2, 0, 0, 0]),
            (3, [3, 0, 0, 0]),
            (4, [4, 0, 0, 0]),
            (5, [5, 0, 0, 0]),
            (6, [6, 0, 0, 0]),
            (u64::MAX, [7, 0, 0, 0]),
        ];
        let cfg = InstanceConfig {
            chain_id: 1,
            genesis_validators_root: [0x4b; 32],
            genesis_time: 1_606_824_023,
            forks,
            checkpoint_root: [0; 32],
            checkpoint_slot: 1905 * SLOTS_PER_PERIOD,
            verifiers: vec![],
            min_participants: 0,
        };
        FabParams::impersonating(&cfg, b"unit")
    }

    #[test]
    fn chain_is_self_consistent() {
        let c = FabChain::build(params());
        let u = &c.updates[0];
        let (next, nb) = u.next.as_ref().expect("next");
        assert_eq!(
            root_from_branch(next.tree_hash_root(), nb, gindex::NEXT_SYNC_COMMITTEE),
            u.attested.beacon.state_root
        );
        assert_eq!(
            root_from_branch(u.finalized.root(), &u.finality_branch, gindex::FINALIZED_ROOT),
            u.attested.beacon.state_root
        );
        assert_eq!(
            root_from_branch(
                u.finalized.execution_root(),
                &u.finalized.execution_branch,
                gindex::EXECUTION_PAYLOAD
            ),
            u.finalized.beacon.body_root
        );
        assert_eq!(
            root_from_branch(
                c.keys[0].root(),
                &c.bootstrap.committee_branch,
                gindex::CURRENT_SYNC_COMMITTEE
            ),
            c.bootstrap.header.beacon.state_root
        );
    }
}
