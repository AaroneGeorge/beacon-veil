//! One JSON line per step (G4 reads these). Required fields are always present (null when unknown):
//! `ts, pid, event, slot, period, tx, beacon_source, uptime_s`. Optional fields follow.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::source::BeaconSourceKind;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub ts: String,
    pub pid: u32,
    pub event: String,
    pub slot: Option<u64>,
    pub period: Option<u64>,
    pub tx: Option<String>,
    pub beacon_source: Option<BeaconSourceKind>,
    pub uptime_s: u64,
    /// Candidate kind: `finality`, `finality_handoff`, `period`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// On-chain head slot when the line was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_slot: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_committee_known: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycles: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gas: Option<u64>,
    /// Submission counter within this process; `submit` and its outcome line share it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// Appends lines to a file (fsync per line) and to stdout.
pub struct Logger {
    file: Mutex<File>,
    started: Instant,
    pid: u32,
    source: BeaconSourceKind,
    echo: bool,
}

impl Logger {
    pub fn open(path: &Path, source: BeaconSourceKind, echo: bool) -> Result<Self> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open log {}", path.display()))?;
        Ok(Self {
            file: Mutex::new(file),
            started: Instant::now(),
            pid: std::process::id(),
            source,
            echo,
        })
    }

    /// A line with the required fields filled.
    pub fn line(&self, event: &str) -> LogLine {
        LogLine {
            ts: rfc3339_now(),
            pid: self.pid,
            event: event.to_string(),
            beacon_source: Some(self.source),
            uptime_s: self.started.elapsed().as_secs(),
            ..Default::default()
        }
    }

    pub fn write(&self, mut l: LogLine) {
        // Refresh time fields at write time.
        l.ts = rfc3339_now();
        l.uptime_s = self.started.elapsed().as_secs();
        let s = match serde_json::to_string(&l) {
            Ok(s) => s,
            Err(e) => format!("{{\"event\":\"log_encode_error\",\"detail\":\"{e}\"}}"),
        };
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{s}");
            let _ = f.sync_data();
        }
        if self.echo {
            println!("{s}");
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC).
pub fn rfc3339_now() -> String {
    rfc3339(SystemTime::now())
}

pub fn rfc3339(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let ms = d.subsec_millis();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let sod = secs % 86_400;
    let (y, m, dd) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{dd:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rfc3339_known() {
        // 2026-11-12T09:30:00.250Z = 1794475800.250
        let t = UNIX_EPOCH + Duration::from_millis(1_794_475_800_250);
        assert_eq!(rfc3339(t), "2026-11-12T09:30:00.250Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn required_fields_always_present() {
        let l = LogLine {
            ts: "t".into(),
            event: "tick".into(),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(&l).unwrap();
        for k in [
            "ts",
            "pid",
            "event",
            "slot",
            "period",
            "tx",
            "beacon_source",
            "uptime_s",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert!(v.get("detail").is_none());
    }
}
