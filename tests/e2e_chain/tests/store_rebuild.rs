//! `attest_host::rebuild_witness` (restart fallback (c), decision 3) on real mainnet fixtures, natively.

use attest_core::{AccountId, EsaError, lc};

#[test]
fn rebuilds_the_store_from_a_bootstrap_and_the_period_update() {
    let chain = e2e_chain::mainnet_chain(vec![AccountId::new([1; 32])]).unwrap();
    let (w0, head0) = lc::init_store(&chain.config, &chain.bootstrap).unwrap();

    // Right after Init: nothing to search beyond (0, 0) and no next committee.
    let r = attest_host::rebuild_witness(&chain.config, &head0, &chain.bootstrap, &[]).unwrap();
    assert_eq!(r, w0);

    // A store at the same head that has learned the next committee and has non-zero participation counters (what
    // update[0] produces, placed on the bootstrap's header so the bootstrap is "at the head").
    let learned = lc::verify_and_apply(&chain.config, &w0, &chain.updates[0].1)
        .unwrap()
        .new_witness();
    assert!(learned.next_committee_ssz.is_some());
    let target = lc::StoreWitness {
        next_committee_ssz: learned.next_committee_ssz.clone(),
        previous_max_active_participants: 7,
        current_max_active_participants: learned.current_max_active_participants,
        ..w0.clone()
    };
    let mut head = head0.clone();
    head.store_commitment = target.load().unwrap().commitment();
    let candidates = vec![chain.updates[1].1.clone(), chain.updates[0].1.clone()];
    let r = attest_host::rebuild_witness(&chain.config, &head, &chain.bootstrap, &candidates).unwrap();
    assert_eq!(r, target);

    // Without the period update the next committee cannot be found: E13.
    assert_eq!(
        attest_host::rebuild_witness(&chain.config, &head, &chain.bootstrap, &[]),
        Err(EsaError::E13StoreMismatch)
    );
}
