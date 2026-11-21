//! The block cap: every transaction (plan, every apply, the chained `Apply`) costs at most 9,000,000 cycles on the
//! committed chains and at the 342-signer floor (a fabricated chain impersonating mainnet, participation bits set
//! to the first 342 and re-signed: the heaviest `Begin`, two committee roots and 170 absent keys). Rows go to
//! `$V03_SIM_DIR/<chain>.json` when set.

use attest_v03::core_id;
use fixtures::fabricate::{FabChain, FabParams, bits_first};
use v03_harness::{
    fixtures::{self as fx, FixtureChain},
    run,
};

fn save(name: &str, rows: &[serde_json::Value]) {
    if let Ok(d) = std::env::var("V03_SIM_DIR") {
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(format!("{d}/{name}.json"), serde_json::to_string_pretty(rows).unwrap()).unwrap();
    }
}

#[test]
fn floor_342_within_the_cap() {
    let v = vec![core_id(&run::IDS.v3)];
    let real = fx::mainnet(v.clone()).unwrap();
    let fab = FabChain::build(FabParams::impersonating(&real.config, b"beacon-veil/v3/floor"));
    let config = fab.config(v, 0);
    // Period update at 342 signers on the bootstrap store.
    let mut u0 = fab.updates[0].clone();
    u0.bits = bits_first(342);
    fab.resign(&mut u0);
    let period = FixtureChain {
        network: "floor",
        config: config.clone(),
        bootstrap: fab.bootstrap.ssz(),
        updates: vec![("period_update_342".into(), u0.ssz())],
    };
    let mut rows = run::run_chain(&period).unwrap().rows;
    // Finality update at 342 signers after the full-participation period and handoff updates.
    let mut u2 = fab.updates[2].clone();
    u2.bits = bits_first(342);
    fab.resign(&mut u2);
    let finality = FixtureChain {
        network: "floor",
        config,
        bootstrap: fab.bootstrap.ssz(),
        updates: vec![
            ("update[0]".into(), fab.updates[0].ssz()),
            ("update[1]".into(), fab.updates[1].ssz()),
            ("finality_update_342".into(), u2.ssz()),
        ],
    };
    rows.extend(run::run_chain(&finality).unwrap().rows);
    save("floor", &rows);
}

#[test]
fn committed_chains_within_the_cap() {
    let v = vec![core_id(&run::IDS.v3)];
    for chain in [
        fx::sepolia(v.clone()).unwrap(),
        fx::mainnet(v.clone()).unwrap(),
        fx::pub_chain("hoodi", v).unwrap(),
    ] {
        let rows = run::run_chain(&chain).unwrap().rows;
        save(&format!("{}-{}", chain.network, chain.updates.len()), &rows);
    }
}
