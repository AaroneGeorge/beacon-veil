//! V1 (Helios unmodified) and V2 (`attest_core::lc_v2`, one function replaced) must agree on every outcome: the full
//! real mainnet chain, and tampered updates (each must be refused by both with the same code).

use attest_core::{AccountId, lc};

#[test]
fn v1_v2_agree() {
    let chain = e2e_chain::mainnet_chain(vec![AccountId::new([1; 32])]).unwrap();
    let (mut w, _) = lc::init_store(&chain.config, &chain.bootstrap).unwrap();
    let mut refused = 0;
    for (label, u) in &chain.updates {
        // Tampered copies: signature, sync bits, branches, headers, signature slot (positions spread over the SSZ).
        for frac in [1u64, 7, 13, 29, 41, 53, 67, 71, 83, 97, 99] {
            let pos = (u.len() as u64 * frac / 100) as usize;
            for bit in [0u8, 3, 7] {
                let mut t = u.clone();
                t[pos] ^= 1 << bit;
                let a = lc::verify_and_apply(&chain.config, &w, &t).map(|o| o.post_store);
                let b = lc::verify_and_apply_v2(&chain.config, &w, &t).map(|o| o.post_store);
                assert_eq!(a, b, "{label} byte {pos} bit {bit}");
                refused += usize::from(a.is_err());
            }
        }
        let a = lc::verify_and_apply(&chain.config, &w, u).unwrap();
        let b = lc::verify_and_apply_v2(&chain.config, &w, u).unwrap();
        assert_eq!(
            (a.pre_store, a.post_store, &a.head, &a.record),
            (b.pre_store, b.post_store, &b.head, &b.record)
        );
        w = a.new_witness();
    }
    println!("tampered updates refused by both: {refused}");
    assert!(refused > 100);
}
