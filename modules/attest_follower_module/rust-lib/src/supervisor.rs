//! Process supervisor for `attest-followerd` (no Logos code here).

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// What `configure` accepts (JSON):
/// `{ "bin": "/path/attest-followerd", "config": "/path/followerd.toml", "log": "/path/follower.jsonl",
///    "state_dir": "/path/state", "env": { "LEE_WALLET_HOME_DIR": "...", ... } }`
/// `log` and `state_dir` let `status` read the follower's files; `env` is passed to the child.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    pub bin: PathBuf,
    pub config: PathBuf,
    pub log: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Settings {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(s).map_err(|e| format!("configure: not JSON: {e}"))?;
        let path = |k: &str| v.get(k).and_then(Value::as_str).map(PathBuf::from);
        let bin = path("bin").ok_or("configure: 'bin' is required")?;
        let config = path("config").ok_or("configure: 'config' is required")?;
        let mut env = Vec::new();
        if let Some(o) = v.get("env").and_then(Value::as_object) {
            for (k, x) in o {
                let x = x.as_str().ok_or_else(|| format!("configure: env.{k} must be a string"))?;
                env.push((k.clone(), x.to_string()));
            }
        }
        Ok(Self { bin, config, log: path("log"), state_dir: path("state_dir"), env })
    }
}

/// Event the module re-emits, from one follower log line.
#[derive(Debug, Clone, PartialEq)]
pub enum FollowerEvent {
    /// `submit` line.
    UpdateSubmitted(Value),
    /// `applied` line whose `extra.handoff` is true.
    HandoffApplied(Value),
    /// `submit_failed`, `*_error`, `no_instance` lines.
    FollowerError(Value),
}

/// Maps a log line to an event, if it is one the module reports.
pub fn classify(line: &str) -> Option<FollowerEvent> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let ev = v.get("event")?.as_str()?.to_string();
    match ev.as_str() {
        "submit" => Some(FollowerEvent::UpdateSubmitted(v)),
        "applied" if v.pointer("/extra/handoff").and_then(Value::as_bool) == Some(true) => {
            Some(FollowerEvent::HandoffApplied(v))
        }
        "submit_failed" | "no_instance" => Some(FollowerEvent::FollowerError(v)),
        e if e.ends_with("_error") => Some(FollowerEvent::FollowerError(v)),
        _ => None,
    }
}

/// Last non-empty line of a file (reads at most the final 64 KiB).
pub fn last_line(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(65_536))).ok()?;
    BufReader::new(f).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).last()
}

pub type Sink = Arc<dyn Fn(FollowerEvent) + Send + Sync>;

/// Follows `path` from its current end and passes new event lines to `sink` until `stop` is set.
pub fn spawn_tail(path: PathBuf, stop: Arc<AtomicBool>, sink: Sink) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut pos = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut partial = String::new();
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(500));
            let Ok(mut f) = File::open(&path) else { continue };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < pos {
                pos = 0; // truncated or replaced
            }
            if len == pos || f.seek(SeekFrom::Start(pos)).is_err() {
                continue;
            }
            let mut r = BufReader::new(f);
            let mut buf = String::new();
            while let Ok(n) = r.read_line(&mut buf) {
                if n == 0 {
                    break;
                }
                pos += n as u64;
                if buf.ends_with('\n') {
                    let line = std::mem::take(&mut partial) + &buf;
                    if let Some(e) = classify(&line) {
                        sink(e);
                    }
                } else {
                    partial.push_str(&buf);
                }
                buf.clear();
            }
        }
    })
}

/// The supervised follower process.
#[derive(Default)]
pub struct Supervisor {
    pub settings: Option<Settings>,
    child: Option<Child>,
    started: Option<Instant>,
    tail_stop: Option<Arc<AtomicBool>>,
    tail: Option<JoinHandle<()>>,
    last_exit: Arc<Mutex<Option<String>>>,
}

impl Supervisor {
    pub fn configure(&mut self, json: &str) -> Value {
        match Settings::from_json(json) {
            Ok(s) => {
                if !s.bin.exists() {
                    return json!({"ok": false, "error": format!("bin {} does not exist", s.bin.display())});
                }
                if !s.config.exists() {
                    return json!({"ok": false, "error": format!("config {} does not exist", s.config.display())});
                }
                self.settings = Some(s);
                json!({"ok": true})
            }
            Err(e) => json!({"ok": false, "error": e}),
        }
    }

    pub fn running_pid(&mut self) -> Option<u32> {
        let c = self.child.as_mut()?;
        match c.try_wait() {
            Ok(None) => Some(c.id()),
            Ok(Some(st)) => {
                *self.last_exit.lock().unwrap_or_else(|p| p.into_inner()) = Some(format!("{st}"));
                self.child = None;
                None
            }
            Err(_) => None,
        }
    }

    pub fn start(&mut self, sink: Sink) -> Value {
        let Some(s) = self.settings.clone() else {
            return json!({"ok": false, "error": "not configured: call configure first"});
        };
        if let Some(pid) = self.running_pid() {
            return json!({"ok": true, "pid": pid, "already_running": true});
        }
        let mut cmd = Command::new(&s.bin);
        cmd.arg("--config").arg(&s.config).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in &s.env {
            cmd.env(k, v);
        }
        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                self.child = Some(child);
                self.started = Some(Instant::now());
                if let Some(log) = s.log.clone() {
                    let stop = Arc::new(AtomicBool::new(false));
                    self.tail = Some(spawn_tail(log, stop.clone(), sink));
                    self.tail_stop = Some(stop);
                }
                json!({"ok": true, "pid": pid})
            }
            Err(e) => json!({"ok": false, "error": format!("spawn {}: {e}", s.bin.display())}),
        }
    }

    /// SIGTERM (through `kill`, so no unsafe code), wait up to `grace`, then SIGKILL.
    pub fn stop(&mut self, grace: Duration) -> Value {
        if let Some(st) = self.tail_stop.take() {
            st.store(true, Ordering::Relaxed);
        }
        if let Some(t) = self.tail.take() {
            let _ = t.join();
        }
        let Some(mut c) = self.child.take() else {
            return json!({"ok": true, "running": false});
        };
        let pid = c.id();
        let _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).status();
        let t0 = Instant::now();
        while t0.elapsed() < grace {
            if let Ok(Some(st)) = c.try_wait() {
                return json!({"ok": true, "pid": pid, "exit": format!("{st}"), "forced": false});
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = c.kill();
        let st = c.wait().map(|s| format!("{s}")).unwrap_or_default();
        json!({"ok": true, "pid": pid, "exit": st, "forced": true})
    }

    pub fn status(&mut self) -> Value {
        let pid = self.running_pid();
        let s = self.settings.clone();
        let last = s.as_ref().and_then(|s| s.log.as_ref()).and_then(|p| last_line(p));
        let cache = s
            .as_ref()
            .and_then(|s| s.state_dir.as_ref())
            .and_then(|d| std::fs::read(d.join("follower-cache.json")).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        json!({
            "ok": true,
            "configured": s.is_some(),
            "running": pid.is_some(),
            "pid": pid,
            "uptime_s": self.started.filter(|_| pid.is_some()).map(|t| t.elapsed().as_secs()),
            "last_exit": self.last_exit.lock().map(|g| g.clone()).unwrap_or(None),
            "last_line": last.and_then(|l| serde_json::from_str::<Value>(&l).ok()),
            "cache": cache,
        })
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.stop(Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse() {
        let s = Settings::from_json(r#"{"bin":"/b","config":"/c","log":"/l","env":{"A":"1"}}"#).unwrap();
        assert_eq!(s.bin, PathBuf::from("/b"));
        assert_eq!(s.env, vec![("A".into(), "1".into())]);
        assert!(Settings::from_json(r#"{"bin":"/b"}"#).is_err());
    }

    #[test]
    fn classify_lines() {
        assert!(matches!(classify(r#"{"event":"submit","slot":1}"#), Some(FollowerEvent::UpdateSubmitted(_))));
        assert!(matches!(
            classify(r#"{"event":"applied","extra":{"handoff":true}}"#),
            Some(FollowerEvent::HandoffApplied(_))
        ));
        assert!(classify(r#"{"event":"applied","extra":{"handoff":false}}"#).is_none());
        assert!(matches!(classify(r#"{"event":"source_error"}"#), Some(FollowerEvent::FollowerError(_))));
        assert!(classify(r#"{"event":"tick"}"#).is_none());
        assert!(classify("not json").is_none());
    }

    #[test]
    fn supervise_a_process() {
        let dir = std::env::temp_dir().join(format!("afm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in follower: a shell script that writes a submit line and sleeps.
        let bin = dir.join("fake-followerd");
        let log = dir.join("f.jsonl");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\nsleep 1\necho '{{\"event\":\"submit\",\"slot\":7}}' >> {}\nexec sleep 30\n", log.display()),
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&bin).status().unwrap();
        let cfg = dir.join("c.toml");
        std::fs::write(&cfg, "").unwrap();
        std::fs::write(&log, "").unwrap();
        let mut s = Supervisor::default();
        let r = s.configure(&json!({"bin": bin, "config": cfg, "log": log}).to_string());
        assert_eq!(r["ok"], true, "{r}");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let r = s.start(Arc::new(move |e| seen2.lock().unwrap().push(e)));
        assert_eq!(r["ok"], true, "{r}");
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(s.status()["running"], true);
        assert_eq!(seen.lock().unwrap().len(), 1);
        let r = s.stop(Duration::from_secs(5));
        assert_eq!(r["ok"], true);
        assert_eq!(s.status()["running"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
