#!/usr/bin/env bash
# Status of a running (or finished) follower log: the G4 summary so far, plus whether the process is alive and how
# fresh the log is. Read-only; it never touches the follower.
#
#   scripts/gate/g4_status.sh <follower.jsonl> [--out FILE]
#
# Prints one JSON object: g4_check.py's summary with --min-hours 0 (hours, submits, applied, noops, handoffs,
# period_updates, max_gap_s, max_applied_gap_s, error_lines, head_first, head_last, ...) plus a "status" block:
#   now_utc, pid, pid_alive (from <log dir>/followerd.pid, else the log's pid), last_line_age_s, last_event,
#   last_applied {ts, slot, period, tx}, hours_since_start.
# "result" is the G4 verdict for the run so far ignoring the 24 h minimum (a live run reads FAIL only on a real
# defect: a restart, a failed submit, a gap). Exit 0 always when the log is readable, 2 otherwise.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log="${1:?usage: g4_status.sh <follower.jsonl> [--out FILE]}"
out=""
if [ "${2:-}" = "--out" ]; then out="${3:?--out needs a file}"; fi
[ -r "$log" ] || { echo "g4_status: cannot read $log" >&2; exit 2; }

summary="$(python3 -I "$here/g4_check.py" "$log" --min-hours 0 || true)"
pidfile="$(dirname "$log")/followerd.pid"

G4_SUMMARY="$summary" python3 -I - "$log" "$pidfile" "$out" <<'PY'
import json, os, sys
from datetime import datetime, timezone
log, pidfile, out = sys.argv[1:4]
s = json.loads(os.environ.get("G4_SUMMARY") or "{}")
now = datetime.now(timezone.utc)
def ts(x):
    return datetime.fromisoformat(x.replace("Z", "+00:00"))
recs = []
with open(log, encoding="utf-8") as f:
    for raw in f:
        try:
            recs.append(json.loads(raw))
        except ValueError:
            pass
pid = None
if os.path.exists(pidfile):
    try:
        pid = int(open(pidfile).read().strip())
    except ValueError:
        pid = None
if pid is None and recs:
    pid = recs[-1].get("pid")
alive = False
if pid:
    # Alive only if that pid is still an attest-followerd (a reused pid is not the follower).
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            alive = b"attest-followerd" in f.read()
    except OSError:
        alive = False
last = recs[-1] if recs else {}
applied = [r for r in recs if r.get("event") == "applied"]
la = applied[-1] if applied else None
st = {
    "now_utc": now.isoformat(timespec="seconds"),
    "pid": pid,
    "pid_alive": alive,
    "last_event": last.get("event"),
    "last_line_ts": last.get("ts"),
    "last_line_age_s": round((now - ts(last["ts"])).total_seconds(), 1) if last.get("ts") else None,
    "hours_since_start": round((now - ts(recs[0]["ts"])).total_seconds() / 3600, 3) if recs else None,
    "last_applied": ({k: la.get(k) for k in ("ts", "slot", "period", "tx", "kind")} if la else None),
}
s["status"] = st
text = json.dumps(s, indent=2, default=str)
print(text)
if out:
    with open(out, "w", encoding="utf-8") as f:
        f.write(text + "\n")
PY
