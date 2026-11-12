//! Logos glue. The builder derives the `.lidl` from the `AttestFollowerModule` trait. Answers are JSON strings:
//! `{ "ok": true, ... }` or `{ "ok": false, "error": "..." }`.

use std::sync::Arc;
use std::time::Duration;

use crate::supervisor::{FollowerEvent, Supervisor};

pub trait AttestFollowerModule: Send + 'static {
    /// `{ bin, config, log?, state_dir?, env? }`: where `attest-followerd` and its config file are.
    fn configure(&mut self, settings_json: String) -> String;
    /// Starts `attest-followerd --config <config>`. `{ ok, pid }`.
    fn start(&mut self) -> String;
    /// SIGTERM, up to 60 s for the step in progress, then SIGKILL. `{ ok, pid, exit, forced }`.
    fn stop(&mut self) -> String;
    /// `{ ok, configured, running, pid, uptime_s, last_exit, last_line, cache }`.
    fn status(&mut self) -> String;

    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

/// Events, one per follower log line of interest; the payload is the log line (JSON text).
pub trait AttestFollowerModuleEvents {
    /// A `submit` line: an update is being submitted.
    fn update_submitted(&self, line: String);
    /// An `applied` line that moved the instance into a new sync-committee period.
    fn handoff_applied(&self, line: String);
    /// `submit_failed`, `*_error` and `no_instance` lines.
    fn follower_error(&self, line: String);
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

#[derive(Default)]
struct AttestFollower {
    sup: Supervisor,
}

fn emit(e: FollowerEvent) {
    match e {
        FollowerEvent::UpdateSubmitted(v) => emit_update_submitted(&v.to_string()),
        FollowerEvent::HandoffApplied(v) => emit_handoff_applied(&v.to_string()),
        FollowerEvent::FollowerError(v) => emit_follower_error(&v.to_string()),
    }
}

impl AttestFollowerModule for AttestFollower {
    fn configure(&mut self, settings_json: String) -> String {
        self.sup.configure(&settings_json).to_string()
    }

    fn start(&mut self) -> String {
        self.sup.start(Arc::new(emit)).to_string()
    }

    fn stop(&mut self) -> String {
        self.sup.stop(Duration::from_secs(60)).to_string()
    }

    fn status(&mut self) -> String {
        self.sup.status().to_string()
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<AttestFollower>();
}
