#!/usr/bin/env bash
# Run A or B (G4): one attest-followerd process following Sepolia for 24 h into an instance on seq-dev.
# Run B: RUN_DIR=$B/run/w4/run-b crates/follower/scripts/run_a.sh start (its own wallet.env, ids.toml, instance.json).
#
#   crates/follower/scripts/run_a.sh start   # preflight, then start detached (setsid); prints the pid
#   crates/follower/scripts/run_a.sh status  # alive?, last line, G4 check so far
#   crates/follower/scripts/run_a.sh stop    # SIGTERM (clean stop line), only after the 24 h
#   crates/follower/scripts/run_a.sh collect # copy the log + G4 summary into evidence/g4/<run>/ (PRIMARY=1: also
#                                            # evidence/g4/follower.jsonl, the file the gate names)
#
# Env: INSTANCE (base58; default: "instance" from $RUN_DIR/instance.json written by `attest init --out`),
#      SOURCE_KIND (logos_module | direct_http; default logos_module), RUN_DIR (default $B/run/w4/run-a),
#      RUN_A_FORCE=1 to move an existing log aside. Never restart run A by hand: G4 needs one PID.
#      CFG_TEMPLATE (default examples/followerd.sepolia.toml; run C on mainnet: examples/followerd.mainnet.toml).
#      SEQ (dev | main | pub; default dev): the sequencer the instance lives on (run C: main, RPC 3042, ids in wallet.env BV_IDS).
#      EVIDENCE_DIR: where `collect` copies the run (default evidence/g4/<run>).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"
RUN_DIR="${RUN_DIR:-$B/run/w4/run-a}"
SOURCE_KIND="${SOURCE_KIND:-logos_module}"
LOG="$RUN_DIR/follower.jsonl"
CFG="$RUN_DIR/followerd.toml"
mkdir -p "$RUN_DIR"
# Own wallet home and payer (one wallet home per writer): wallet_home.sh w4-runA > $RUN_DIR/wallet.env
if [ -f "$RUN_DIR/wallet.env" ]; then
  # shellcheck source=/dev/null
  source <(grep '^export ' "$RUN_DIR/wallet.env")
fi
# env.sh exports RISC0_DEV_MODE=1 for builds and dev chains. A follower of a sequencer with dev mode unset (pub, real)
# must not carry it: its start line records the variable, and the run summary reports it.
case "${SEQ:-dev}" in real|pub) unset RISC0_DEV_MODE ;; esac

pid_alive() { [ -f "$RUN_DIR/followerd.pid" ] && kill -0 "$(cat "$RUN_DIR/followerd.pid")" 2>/dev/null; }

case "${1:-}" in
start)
  pid_alive && { echo "run A already running: pid $(cat "$RUN_DIR/followerd.pid")"; exit 1; }
  if [ -s "$LOG" ]; then
    [ "${RUN_A_FORCE:-0}" = 1 ] || { echo "$LOG exists; RUN_A_FORCE=1 moves it aside" >&2; exit 1; }
    mv "$LOG" "$LOG.old-$(date -u +%Y%m%dT%H%M%SZ)"; rm -f "$RUN_DIR/follower-cache.json"
  fi
  INSTANCE="${INSTANCE:-$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["instance"])' "$RUN_DIR/instance.json" 2>/dev/null || true)}"
  [ -n "$INSTANCE" ] || { echo "no INSTANCE and no $RUN_DIR/instance.json (attest init --out)" >&2; exit 1; }
  case "$LEE_WALLET_HOME_DIR" in */wallet-dev) echo "run A needs its own wallet home: wallet_home.sh w4-runA > $RUN_DIR/wallet.env" >&2; exit 1 ;; esac
  [ -n "${BV_PAYER:-}" ] || { echo "BV_PAYER not set ($RUN_DIR/wallet.env)" >&2; exit 1; }
  case "${SEQ:-dev}" in
    real|pub) [ -z "${RISC0_DEV_MODE+x}" ] || { echo "RISC0_DEV_MODE is set; refusing to start a follower of seq-${SEQ}" >&2; exit 1; } ;;
  esac
  "$R/scripts/seq.sh" "${SEQ:-dev}" status >/dev/null || { echo "seq-${SEQ:-dev} is not running" >&2; exit 1; }
  ATTEST="$B/target/host/attest"; FD="$B/target/host/attest-followerd"
  [ -x "$ATTEST" ] && [ -x "$FD" ] || { echo "build first: nice -n 15 cargo build --profile host -p follower -p attest_cli" >&2; exit 1; }
  echo "preflight: instance status"; "$ATTEST" status --instance "$INSTANCE" | tee "$RUN_DIR/preflight-status.json"
  python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d["store"].get("verified") else 1)' \
    "$RUN_DIR/preflight-status.json" || { echo "store not verified" >&2; exit 1; }
  if [ "$SOURCE_KIND" = logos_module ]; then
    echo "preflight: beacon_api_module probe"; "$here/logos_session.sh" probe | tee "$RUN_DIR/preflight-module.txt"
    grep -q '^probe ok' "$RUN_DIR/preflight-module.txt" || { echo "module probe failed (K8: SOURCE_KIND=direct_http)" >&2; exit 1; }
  fi
  # Stable copy of the binary: later rebuilds in the shared target dir do not touch the running file.
  sha=$(sha256sum "$FD" | cut -c1-64); BIN="$B/run/w4/bin/attest-followerd-${sha:0:12}"
  mkdir -p "$B/run/w4/bin"; cp -f "$FD" "$BIN"
  # The templates name the build folder ${BUILD}; it is filled in here.
  sed -e "s|\${BUILD}|$B|g" -e "s|^state_dir = .*|state_dir = \"$RUN_DIR\"|" -e "s|^instance = .*|instance = \"$INSTANCE\"|" \
      -e "s|^kind = \"logos_module\".*|kind = \"$SOURCE_KIND\"|" -e "s|^echo = .*||" \
      "${CFG_TEMPLATE:-$R/crates/follower/examples/followerd.sepolia.toml}" > "$CFG"
  grep -q "^echo" "$CFG" || sed -i 's|^\[follower\]|[follower]\necho = false|' "$CFG"
  cd "$RUN_DIR"
  setsid nohup nice -n 5 "$BIN" --config "$CFG" > "$RUN_DIR/followerd.stdout" 2>&1 < /dev/null &
  pid=$!
  sleep 5
  kill -0 "$pid" 2>/dev/null || { echo "follower died at start:"; tail -20 "$RUN_DIR/followerd.stdout"; exit 1; }
  case "${SEQ:-dev}" in
    real|pub)
      # The start line must say the follower runs without RISC0_DEV_MODE; stop at once otherwise (no hours lost yet).
      dm="$(python3 -I -c 'import json,sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r.get("event") == "start":
        print(json.dumps((r.get("extra") or {}).get("risc0_dev_mode", "missing")))' "$LOG" | tail -1)"
      if [ "$dm" != "null" ]; then
        kill -TERM "$pid"; echo "start line has risc0_dev_mode=$dm (want null): follower stopped" >&2; exit 1
      fi ;;
  esac
  python3 -I - "$RUN_DIR/run.json" "$pid" "$BIN" "$sha" "$CFG" "$INSTANCE" "$SOURCE_KIND" "$R" <<'PY'
import json, subprocess, sys, datetime, hashlib, platform, os
out, pid, binp, sha, cfg, inst, src, repo = sys.argv[1:9]
def sh(*a):
    try: return subprocess.run(a, capture_output=True, text=True, check=False).stdout.strip()
    except OSError: return None
json.dump({"run": os.path.basename(os.path.dirname(out)), "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(), "pid": int(pid),
           "binary": binp, "binary_sha256": sha, "config": cfg,
           "config_sha256": hashlib.sha256(open(cfg, "rb").read()).hexdigest(), "instance": inst, "beacon_source": src,
           "repo_commit": sh("git", "-C", repo, "rev-parse", "HEAD"), "repo_dirty": bool(sh("git", "-C", repo, "status", "--porcelain")),
           "machine": platform.platform(), "cpu": sh("sh", "-c", "grep -m1 'model name' /proc/cpuinfo | cut -d: -f2")},
          open(out, "w"), indent=2)
PY
  echo "$(basename "$RUN_DIR") started: pid $pid, log $LOG"; cat "$RUN_DIR/run.json"
  ;;
status)
  if pid_alive; then echo "running: pid $(cat "$RUN_DIR/followerd.pid")"; else echo "NOT running"; fi
  tail -1 "$LOG" 2>/dev/null || true
  python3 -I "$R/scripts/gate/g4_check.py" "$LOG" --min-hours 0 | head -40 || true
  ;;
stop)
  pid_alive || { echo "not running"; exit 0; }
  kill -TERM "$(cat "$RUN_DIR/followerd.pid")"; echo "SIGTERM sent"
  ;;
collect)
  # evidence/g4/<run>/ for every run; PRIMARY=1 also makes it evidence/g4/follower.jsonl, the file G4 names.
  run="$(basename "$RUN_DIR")"; E="${EVIDENCE_DIR:-$R/evidence/g4/$run}"
  mkdir -p "$E"
  cp -f "$LOG" "$E/follower.jsonl"; cp -f "$RUN_DIR/run.json" "$E/run.json"; cp -f "$CFG" "$E/followerd.toml"
  cp -f "$RUN_DIR/instance.json" "$E/instance.json" 2>/dev/null || true
  rc=0; python3 -I "$R/scripts/gate/g4_check.py" "$E/follower.jsonl" --summary-out "$E/g4-summary.json" >/dev/null || rc=$?
  # Paths in the copies: ${BUILD} for the build folder, ~ for the home directory (no machine path in the evidence).
  for f in follower.jsonl run.json followerd.toml instance.json g4-summary.json; do
    if [ -f "$E/$f" ]; then sed -i -e "s#$B/#\${BUILD}/#g" -e "s#$HOME#~#g" "$E/$f"; fi
  done
  if [ "${PRIMARY:-0}" = 1 ]; then
    cp -f "$E/follower.jsonl" "$R/evidence/g4/follower.jsonl"; cp -f "$E/g4-summary.json" "$R/evidence/g4/g4-summary.json"
  fi
  echo "G4 check exit $rc; summary in $E/g4-summary.json"; exit $rc
  ;;
*) echo "usage: $0 start|status|stop|collect" >&2; exit 2 ;;
esac
