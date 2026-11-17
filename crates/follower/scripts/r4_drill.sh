#!/usr/bin/env bash
# R4 drill: kill -9 the follower, restart it, and check it resumes from the instance's head without re-verifying from
# the checkpoint; then SIGTERM and check the clean stop. Runs against the mock chain (CHAIN=mock, default) or a real
# instance on seq-dev (CHAIN=host INSTANCE=<id>).
#
#   crates/follower/scripts/r4_drill.sh [out_dir]
# Output: <out_dir>/follower.jsonl and <out_dir>/r4-result.json. Exit 0 on PASS.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"
# env.sh resets LEE_WALLET_HOME_DIR to the shared wallet-dev: load the follower wallet env (and BV_IDS) after it.
if [ -n "${WALLET_ENV:-}" ]; then
  # shellcheck source=/dev/null
  source <(grep '^export ' "$WALLET_ENV")
fi
case "$LEE_WALLET_HOME_DIR" in */wallet-dev) [ "${CHAIN:-mock}" = mock ] || { echo "set WALLET_ENV: never use the shared wallet-dev" >&2; exit 1; } ;; esac
OUT="${1:-$B/run/w4/r4-$(date -u +%Y%m%dT%H%M%SZ)}"
CHAIN="${CHAIN:-mock}"
BIN="${FOLLOWERD:-$B/target/host/attest-followerd}"
[ -x "$BIN" ] || BIN="$B/target/debug/attest-followerd"
[ -x "$BIN" ] || { echo "no attest-followerd binary (cargo build --profile host -p follower)" >&2; exit 1; }
SEP="https://lodestar-sepolia.chainsafe.io"
mkdir -p "$OUT"
LOG="$OUT/follower.jsonl"

if [ "$CHAIN" = mock ]; then
  fin=$(curl -sf -m 30 "$SEP/eth/v1/beacon/light_client/finality_update" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["data"]["finalized_header"]["beacon"]["slot"])')
  start=$(( fin / 8192 * 8192 )); head=$(( fin - 4096 )); [ "$head" -gt "$start" ] || head=$(( start + 1 ))
  printf '{"head_slot": %s, "next_committee_known": false, "applied": 0}\n' "$head" > "$OUT/mock-chain.json"
  chain_toml=$'kind = "mock"\n'"mock_file = \"$OUT/mock-chain.json\""
else
  : "${INSTANCE:?set INSTANCE for CHAIN=host}"
  chain_toml=$'kind = "host"\n'"instance = \"$INSTANCE\""
fi
cat > "$OUT/followerd.toml" <<TOML
[follower]
id = "r4-drill"
chain_id = 11155111
state_dir = "$OUT"
cadence_s = 60
poll_s = 10
jitter_max_s = 3
echo = false

[source]
kind = "${SOURCE_KIND:-direct_http}"
endpoints = ["$SEP", "http://unstable.sepolia.beacon-api.nimbus.team"]
logosctl = "$B/run/w4/logosctl/squashfs-root/AppRun"
logosctl_config_dir = "$B/run/w4/logos-session"
timeout_s = 60

[chain]
$chain_toml
TOML

wait_for() { # pattern count timeout_s
  local t=0
  local c
  while :; do
    c=$(grep -c "$1" "$LOG" 2>/dev/null) || c=${c:-0}
    [ "${c:-0}" -lt "$2" ] || break
    sleep 2; t=$((t + 2)); [ "$t" -lt "$3" ] || { echo "timeout waiting for $2 x $1" >&2; return 1; }
  done
}

trap 'kill ${P1:-} ${P2:-} 2>/dev/null || true' EXIT
"$BIN" --config "$OUT/followerd.toml" > "$OUT/run1.out" 2>&1 &
P1=$!
echo "run 1 pid $P1"
wait_for '"event":"applied"' "${APPLIED_BEFORE_KILL:-2}" "${WAIT1:-1500}"
before_lines=$(wc -l < "$LOG")
kill -9 "$P1"; wait "$P1" 2>/dev/null || true
echo "killed -9 $P1 after $(grep -c '"event":"applied"' "$LOG" || true) applied"

"$BIN" --config "$OUT/followerd.toml" > "$OUT/run2.out" 2>&1 &
P2=$!
echo "run 2 pid $P2"
wait_for '"event":"resume"' 1 120
# Run until one more update is applied after the restart (Sepolia finalizes every 6.4 min).
n_applied=$(grep -c '"event":"applied"' "$LOG") || true
wait_for '"event":"applied"' $((n_applied + 1)) 1200
kill -TERM "$P2"
rc=0; wait "$P2" || rc=$?
echo "run 2 exit $rc"

python3 -I - "$LOG" "$before_lines" "$P1" "$P2" "$rc" "$OUT/r4-result.json" <<'PY'
import json, sys
log, before, p1, p2, rc, out = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]), sys.argv[6]
recs = [json.loads(l) for l in open(log) if l.strip()]
r1 = [r for r in recs if r["pid"] == p1]
r2 = [r for r in recs if r["pid"] == p2]
fails = []
head_before = max(r["slot"] for r in r1 if r["event"] == "applied")
resume = [r for r in r2 if r["event"] == "resume"]
if len(resume) != 1:
    fails.append("no single resume line")
elif resume[0].get("head_slot") != head_before:
    fails.append(f"resumed at head {resume[0].get('head_slot')}, expected {head_before}")
if any(r["event"] in ("init",) for r in r2):
    fails.append("re-initialised after restart")
sub2 = [r for r in r2 if r["event"] == "submit"]
if any(r["slot"] < head_before for r in sub2):
    fails.append("submitted an update older than the pre-kill head (replay from checkpoint)")
first_period_r1 = [r for r in r1 if r["event"] == "applied" and r.get("kind") == "period"]
if first_period_r1 and any(r.get("kind") == "period" and r.get("period") == first_period_r1[0].get("period") and r["slot"] <= head_before for r in sub2):
    fails.append("re-submitted the first period update after restart")
app2 = [r for r in r2 if r["event"] == "applied"]
if not app2:
    fails.append("no update applied after the restart")
if any(r["event"] == "submit_failed" for r in recs):
    fails.append("a submission failed")
if rc != 0 or not r2 or r2[-1]["event"] != "stop":
    fails.append(f"SIGTERM did not end with a stop line and exit 0 (rc={rc})")
res = {"result": "PASS" if not fails else "FAIL", "failures": fails, "pid_before": p1, "pid_after": p2,
       "head_before_kill": head_before, "resume_head": resume[0].get("head_slot") if resume else None,
       "applied_before": sum(1 for r in r1 if r["event"] == "applied"), "applied_after": len(app2),
       "submits_after": [(r["slot"], r.get("kind")) for r in sub2], "sigterm_exit": rc, "log": log}
json.dump(res, open(out, "w"), indent=2)
print(json.dumps(res, indent=2))
sys.exit(0 if not fails else 1)
PY
