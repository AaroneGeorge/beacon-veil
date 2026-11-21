//! The port changes the wrappers and nothing else: on the same data as the rc3 on-chain runs
//! (`evidence/public-path/onchain/{mainnet,sepolia,hoodi}/run.json`), the instance head after every update (slot,
//! beacon root, store commitment) equals the one recorded there, through LEZ v0.3.2.

use attest_v03::core_id;
use v03_harness::{fixtures, run};

fn rc3_run(network: &str) -> serde_json::Value {
    let p = fixtures::fixtures_dir().join(format!("../evidence/public-path/onchain/{network}/run.json"));
    serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap()
}

#[test]
fn heads_equal_the_rc3_runs() {
    for network in ["mainnet", "sepolia", "hoodi"] {
        let chain = fixtures::pub_chain(network, vec![core_id(&run::IDS.v3)]).unwrap();
        let r = run::run_chain(&chain).unwrap();
        let rc3 = rc3_run(network);
        let ups = rc3["updates"].as_array().unwrap();
        assert_eq!(ups.len(), r.heads.len(), "{network}: number of updates");
        for ((label, head), u) in r.heads.iter().zip(ups) {
            assert_eq!(*label, u["update"].as_str().unwrap(), "{network}: update order");
            let h = &u["head_after"];
            assert_eq!(head.slot, h["slot"].as_u64().unwrap(), "{network} {label}: slot");
            assert_eq!(
                hex::encode(head.store_commitment),
                h["store_commitment"].as_str().unwrap(),
                "{network} {label}: store commitment"
            );
            assert_eq!(
                hex::encode(head.beacon_root),
                h["beacon_root"].as_str().unwrap(),
                "{network} {label}: beacon root"
            );
        }
        let last = &r.heads.last().unwrap().1;
        assert_eq!(
            hex::encode(last.store_commitment),
            rc3["final_head"]["store_commitment"].as_str().unwrap()
        );
        println!("{network}: {} updates, heads equal to the rc3 run", r.heads.len());
    }
}
