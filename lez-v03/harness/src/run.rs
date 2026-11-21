//! Runs a whole chain (`Init`, then every update as `Begin`, `Miller`, `Finish`) through the in-process LEZ v0.3.2
//! state, checking after every transaction that the shards equal what the native core computes.

use anyhow::{Result, anyhow, ensure};
use attest_core::{Head, InstanceState, lc};
use lee_core::account::AccountId;
use serde_json::{Value, json};

use crate::{
    fixtures::FixtureChain,
    guests, steps,
    steps::Ids,
    world::{STEP_LIMIT, World},
};

/// Program ids used by the in-process runs (any 32 bytes; on the testnet they are the program-loader headers).
pub const IDS: Ids = Ids {
    attest: AccountId::new([0xA7; 32]),
    v3: AccountId::new([0x33; 32]),
};

pub struct ChainRun {
    pub world: World,
    /// One row per transaction: network, update, step, cycles, instruction bytes.
    pub rows: Vec<Value>,
    /// The instance head after each update.
    pub heads: Vec<(String, Head)>,
}

pub fn world() -> Result<World> {
    World::new(IDS, &guests::bin("attest_v03")?, &guests::bin("lc_verify_v3_v03")?)
}

pub fn run_chain(chain: &FixtureChain) -> Result<ChainRun> {
    let mut world = world()?;
    let net = chain.network;
    let (init, mut inst_state, rec) = steps::init(&IDS, &chain.config, &chain.bootstrap)?;
    let inst = steps::instance(&IDS, &chain.config);
    let o = world.exec(&init, 0, STEP_LIMIT);
    ensure!(o.result == Ok(()), "{net} Init: {:?}", o.result);
    ensure!(o.cycles <= STEP_LIMIT, "{net} Init: {} cycles", o.cycles);
    ensure!(world.shard(inst, IDS.attest) == inst_state, "{net} Init: instance");
    ensure!(
        world.shard(steps::record(&IDS, &inst, rec.slot), IDS.attest) == borsh::to_vec(&rec)?,
        "{net} Init: record"
    );
    let mut rows = vec![
        json!({"network": net, "update": "init", "step": "Init", "cycles": o.cycles,
        "instruction_bytes": init.instruction.len()}),
    ];
    let mut heads = Vec::new();
    let (mut w, _) = lc::init_store(&chain.config, &chain.bootstrap).map_err(|e| anyhow!("{e}"))?;
    for (label, u) in &chain.updates {
        let up = steps::update(&IDS, &chain.config, &w, &inst_state, u)?;
        for (i, tx) in up.txs.iter().enumerate() {
            let ts = tx.window_ms.map_or(0, |(from, _)| from);
            let o = world.exec(tx, ts, STEP_LIMIT);
            ensure!(o.result == Ok(()), "{net} {label} {}: {:?}", tx.name, o.result);
            ensure!(o.cycles <= STEP_LIMIT, "{net} {label} {}: {} cycles", tx.name, o.cycles);
            ensure!(
                world.shard(up.job, IDS.v3) == up.job_states[i],
                "{net} {label} {}: job",
                tx.name
            );
            println!(
                "{net} {label} {}: {} cycles ({} bytes)",
                tx.name,
                o.cycles,
                tx.instruction.len()
            );
            rows.push(
                json!({"network": net, "update": label, "step": tx.name, "cycles": o.cycles,
                "instruction_bytes": tx.instruction.len(), "participation": up.native.participation}),
            );
        }
        inst_state = up.instance_state.clone();
        ensure!(world.shard(inst, IDS.attest) == inst_state, "{net} {label}: instance");
        let st: InstanceState = borsh::from_slice(&inst_state)?;
        ensure!(
            st.head.slot == up.native.head.slot && st.head.store_commitment == up.native.post_store,
            "{net} {label}: head"
        );
        if up.native.new_slot {
            let r = world.shard(steps::record(&IDS, &inst, up.native.record.slot), IDS.attest);
            ensure!(r == borsh::to_vec(&up.native.record)?, "{net} {label}: record");
        }
        heads.push((label.clone(), st.head));
        w = up.native.new_witness();
    }
    Ok(ChainRun { world, rows, heads })
}
