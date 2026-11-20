//! The follower on the public path (`[chain] mode = "public"`), against the staged mock chain: an update as three
//! steps, a step that does not land (deferred, then resumed under the same submission number), a second submitter
//! that finishes first (superseded), and a store that moves while a submission is deferred.

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use follower::{
    BeaconSource, BeaconSourceKind, LcObject,
    chain::{MockChain, MockState},
    follower::{Cache, Follower, Params},
    log::Logger,
    ssz::LcKind,
};
use serde_json::Value;

const PERIOD: u64 = 8192;

/// Serves the stored mainnet finality update (finalized slot 15,637,536, period 1908) and nothing else.
struct OneUpdate(LcObject);

impl BeaconSource for OneUpdate {
    fn kind(&self) -> BeaconSourceKind {
        BeaconSourceKind::DirectHttp
    }
    async fn bootstrap(&self, _: &[u8; 32]) -> Result<LcObject> {
        anyhow::bail!("not served")
    }
    async fn updates_by_range(&self, _: u64, _: u64) -> Result<Vec<LcObject>> {
        anyhow::bail!("not served")
    }
    async fn finality_update(&self) -> Result<LcObject> {
        Ok(self.0.clone())
    }
}

fn source() -> OneUpdate {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/mainnet-finality_update.json");
    let v: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
    OneUpdate(LcObject {
        kind: LcKind::FinalityUpdate,
        version: v["version"].as_str().unwrap().into(),
        data: v["data"].clone(),
        provider: Some("test".into()),
    })
}

struct Run {
    dir: PathBuf,
    chain: MockChain,
}

impl Run {
    fn new(name: &str, state: MockState) -> Self {
        let dir = std::env::temp_dir().join(format!("bv-follower-public-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let chain = MockChain::new(dir.join("mock-chain.json")).staged();
        chain.save(&state).unwrap();
        Self { dir, chain }
    }

    fn follower(&self) -> Follower<OneUpdate, MockChain> {
        let params = Params {
            follower_id: "test".into(),
            instance: "mock".into(),
            cadence: Duration::from_secs(6144),
            poll: Duration::from_secs(1),
            jitter_max: Duration::ZERO,
            min_participation: 342,
            state_dir: self.dir.clone(),
        };
        let log = Logger::open(&self.dir.join("follower.jsonl"), BeaconSourceKind::DirectHttp, false).unwrap();
        Follower::new(source(), self.chain.clone(), params, log).unwrap()
    }

    fn lines(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("follower.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn events(&self) -> Vec<String> {
        self.lines()
            .iter()
            .map(|l| l["event"].as_str().unwrap().to_string())
            .filter(|e| e != "tick")
            .collect()
    }

    fn last(&self, event: &str) -> Value {
        self.lines().into_iter().rev().find(|l| l["event"] == event).unwrap()
    }

    fn state(&self) -> MockState {
        self.chain.load().unwrap().unwrap()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn head_1908() -> MockState {
    MockState {
        head_slot: 1908 * PERIOD + 10,
        next_committee_known: true,
        ..MockState::default()
    }
}

async fn step(f: &mut Follower<OneUpdate, MockChain>) {
    let (_tx, mut rx) = tokio::sync::watch::channel(false);
    f.step(&mut rx).await.unwrap();
}

fn step_names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn three_steps_per_update() {
    let r = Run::new("applied", head_1908());
    let mut f = r.follower();
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "applied"]);
    let a = r.last("applied");
    assert_eq!(a["n"], 1);
    assert_eq!(a["extra"]["mode"], "public");
    assert_eq!(a["extra"]["txs"], 3);
    assert_eq!(step_names(&a["extra"]["steps"]), ["Begin", "Miller", "Finish"]);
    assert_eq!(a["tx"], a["extra"]["steps"][2]["tx"], "the applied tx is Finish");
    assert_eq!(r.state().head_slot, 15_637_536);
    // The update is applied: the next poll waits.
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "applied"]);
}

#[tokio::test]
async fn deferred_step_resumes_under_the_same_number() {
    let r = Run::new(
        "deferred",
        MockState {
            fail_next: 2,
            ..head_1908()
        },
    );
    let mut f = r.follower();
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "submit_deferred"]);
    let cache = Cache::load(&r.dir).unwrap().unwrap();
    assert_eq!(cache.pending.as_ref().map(|p| p.n), Some(1));
    // Still not landing at the next poll, then it does.
    step(&mut f).await;
    step(&mut f).await;
    assert_eq!(
        r.events(),
        [
            "submit",
            "submit_deferred",
            "submit_resume",
            "submit_deferred",
            "submit_resume",
            "applied"
        ]
    );
    let a = r.last("applied");
    assert_eq!(a["n"], 1, "the outcome carries the submit's number");
    assert_eq!(a["extra"]["attempts"], 3);
    assert_eq!(step_names(&a["extra"]["steps"]), ["Begin", "Miller", "Finish"]);
    assert_eq!(
        a["extra"]["steps"][0]["tx"],
        Value::Null,
        "Begin landed in a deferred attempt"
    );
    assert!(Cache::load(&r.dir).unwrap().unwrap().pending.is_none());
    assert_eq!(r.state().head_slot, 15_637_536);
}

#[tokio::test]
async fn another_submitter_finishes_first() {
    let r = Run::new(
        "race",
        MockState {
            race_next: true,
            ..head_1908()
        },
    );
    let mut f = r.follower();
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "superseded"]);
    assert_eq!(r.last("superseded")["n"], 1);
    assert_eq!(
        r.state().head_slot,
        15_637_536,
        "the other submitter's update is on chain"
    );
    // Nothing left to do.
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "superseded"]);
}

#[tokio::test]
async fn store_moves_while_deferred() {
    let r = Run::new(
        "moved",
        MockState {
            fail_next: 1,
            ..head_1908()
        },
    );
    let mut f = r.follower();
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "submit_deferred"]);
    // Another submitter applies the update meanwhile.
    let mut s = r.state();
    s.head_slot = 15_637_536;
    r.chain.save(&s).unwrap();
    step(&mut f).await;
    assert_eq!(r.events(), ["submit", "submit_deferred", "superseded"]);
    assert_eq!(r.last("superseded")["n"], 1);
    assert!(Cache::load(&r.dir).unwrap().unwrap().pending.is_none());
}
