//! Follower (PLAN.md 2.6 and W4). Freeze-1 stub: the trait and the log line shape.

#![forbid(unsafe_code)]

/// Where Beacon API light-client data comes from (G4 records which one ran).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeaconSourceKind {
    /// Logos `beacon_api_module` through `lc_request`.
    LogosModule,
    /// Same Beacon API routes over HTTP.
    DirectHttp,
}

/// Source of light-client data. Bodies return SSZ bytes.
pub trait BeaconSource {
    fn kind(&self) -> BeaconSourceKind;
    fn bootstrap(&self, block_root: &[u8; 32]) -> impl std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send;
    fn updates_by_range(&self, start_period: u64, count: u64) -> impl std::future::Future<Output = anyhow::Result<Vec<Vec<u8>>>> + Send;
    fn finality_update(&self) -> impl std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send;
}

/// One JSON log line per step: `ts,pid,event,slot,period,tx,beacon_source,uptime_s` (G4 checker reads these).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogLine {
    pub ts: String,
    pub pid: u32,
    pub event: String,
    pub slot: Option<u64>,
    pub period: Option<u64>,
    pub tx: Option<String>,
    pub beacon_source: BeaconSourceKind,
    pub uptime_s: u64,
}
