#!/usr/bin/env bash
# The proof window: the bridge round trip with real proofs and one real proof of the staged verifier, in priority
# order, one proof at a time, with RISC0_DEV_MODE unset and every stage under /usr/bin/time -v.
#
#   scripts/proof_window.sh --dry-run    the plan, estimates and every precondition (sends nothing, proves nothing)
#   scripts/proof_window.sh              the window (refuses unless every precondition holds)
#   scripts/proof_window.sh --from STAGE resume at a stage (names as printed by --dry-run)
#
# Order and gates:
#   1. private claim     (needs BV_SEPOLIA_OK=yes: Sepolia vault v4 and a deposit) tools/bridge_cli/e2e_v4.sh
#   2. burn and release  (needs BV_SEPOLIA_OK=yes)                                  tools/bridge_cli/e2e_v4.sh
#   3. staged verifier, one VerifyOnce proven for real on a dedicated V3-only mainnet instance
#                                                                                   scripts/proof/real_proof_v3.sh
# Every run needs BV_PROOF_WINDOW_OK=1 (approval to free memory for the proofs). Without BV_SEPOLIA_OK=yes stages 1 and 2
# are skipped and stage 3 runs alone.
#
# Refuses to start when: memory is short (MemAvailable < 10.5 GiB or swap use >= 2 GiB: a proof peaks at about
# 9.3 GiB), the build lock or the proving lock is held, a run that must stay up is not running (run C and seq-main,
# runs D and E, seq-pub, the two Logos sessions), seq-pub runs with RISC0_DEV_MODE, or the window shares a cgroup
# with seq-pub or a follower. seq-pub, runs D and E and the public Logos session live in the cgroup of
# lechu-tmux.service (OOMPolicy=stop, KillMode=control-group): an out-of-memory kill of a proof started from a tmux
# pane would make systemd stop that whole unit. So the window runs in its own scope; it also marks itself as the first
# process the kernel kills (oom_score_adj 1000), so a shortage ends the proof, and only the proof.
#
# Launch (detached, own scope, pid file $B/run/proof-window.pid, log under $B/run/proof-window/):
#   cd $B/repo && XDG_RUNTIME_DIR=/run/user/$(id -u) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus \
#     setsid nohup systemd-run --user --scope --collect --unit=bv-proof-window -- \
#     env BV_PROOF_WINDOW_OK=1 [BV_SEPOLIA_OK=yes BV_RELEASE_RECIPIENT=0x...] scripts/proof_window.sh \
#     > $B/run/proof-window.out 2>&1 < /dev/null &
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"
unset RISC0_DEV_MODE
DRY=0 FROM=""
STAGES="claim-prep claim-wait claim-fund claim burn release v3-fund v3-init v3-prove"
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1; shift ;;
    --from) FROM="$2"; shift 2 ;;
    *) sed -n '2,32p' "$0"; exit 2 ;;
  esac
done
if [ -n "$FROM" ] && [[ " $STAGES " != *" $FROM "* ]]; then
  echo "unknown stage '$FROM' (stages: $STAGES)" >&2; exit 2
fi
FLOW_ENV="$B/run/w6/v4-sepolia/flow.env"
E2E="$BV_REPO/tools/bridge_cli/e2e_v4.sh"
RP="$BV_REPO/scripts/proof/real_proof_v3.sh"
LOGDIR="$B/run/proof-window/$(date -u +%Y%m%dT%H%M%SZ)"
NEED_AVAIL_KIB=$((10752 * 1024))   # 10.5 GiB
MAX_SWAP_KIB=$((2048 * 1024))

fails=()
check() { # description, command...
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "  ok    $d"; else echo "  FAIL  $d"; fails+=("$d"); fi
}
alive() { [ -n "$1" ] && kill -0 "$1" 2>/dev/null; }
pidfile_alive() { [ -f "$1" ] && alive "$(cat "$1")"; }
mem_ok() {
  local avail swap_total swap_free
  avail=$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)
  swap_total=$(awk '/^SwapTotal:/{print $2}' /proc/meminfo); swap_free=$(awk '/^SwapFree:/{print $2}' /proc/meminfo)
  [ "$avail" -ge "$NEED_AVAIL_KIB" ] && [ $((swap_total - swap_free)) -lt "$MAX_SWAP_KIB" ]
}
lock_free() { flock -n "$1" true; }
seq_pub_real() {
  local p; p=$(cat "$B/run/seq-pub.pid" 2>/dev/null) || return 1
  alive "$p" && ! tr '\0' '\n' < "/proc/$p/environ" | grep -q '^RISC0_DEV_MODE='
}
logos_pub_alive() { pgrep -f "logosctl.*--config-dir $B/run/w4/logos-session-pub" >/dev/null; }
logos_c_alive() { pgrep -f "logosctl --config-dir $B/run/w4/logos-session daemon" >/dev/null; }
own_cgroup_ok() { # not lechu-tmux.service, and not the cgroup of seq-pub or a follower
  local mine p; mine=$(cat /proc/self/cgroup)
  case "$mine" in *lechu-tmux.service*) return 1 ;; esac
  for p in "$(cat "$B/run/seq-pub.pid" 2>/dev/null)" "$(cat "$B/run/w4/run-d/followerd.pid" 2>/dev/null)" \
           "$(cat "$B/run/w4/run-e/followerd.pid" 2>/dev/null)" "$(cat "$B/run/w4/run-c/followerd.pid" 2>/dev/null)" \
           "$(cat "$B/run/seq-main.pid" 2>/dev/null)"; do
    [ -n "$p" ] && [ -r "/proc/$p/cgroup" ] && [ "$(cat "/proc/$p/cgroup")" = "$mine" ] && return 1
  done
  return 0
}
recipient_ok() { [[ "${BV_RELEASE_RECIPIENT:-}" =~ ^0x[0-9a-fA-F]{40}$ ]]; }
no_sepolia_repeat() { # a fresh start (or --from claim-prep) must not find a vault, a deposit or any forge attempt
  case "$FROM" in
    ""|claim-prep)
      ! grep -qE '^(VAULT|DEPOSIT_TX|VAULT_ATTEMPT|DEPOSIT_ATTEMPT)=' "$FLOW_ENV" 2>/dev/null \
        && ! compgen -G "$(dirname "$FLOW_ENV")/forge-*.txt" >/dev/null ;;
    *) return 0 ;;
  esac
}

echo "proof window plan ($(date -u +%FT%TZ)), repo $(git -C "$BV_REPO" rev-parse --short HEAD), dirty $(git -C "$BV_REPO" status --porcelain | wc -l)"
cat <<EOF
  stage            what                                                         estimate (one proof at a time)
  claim-prep       vault v4 on Sepolia, bridge config bound to run E's instance,    public txs only; then wait for
                   wallet on seq-pub, 0.001 ETH deposit                             run E to apply a record past the
                                                                                    deposit block (up to about 50 min)
  claim-fund       fund the private claimer (private transfer)                      real proof, 20-28 min, 9.2 GiB
  claim            attest-root, private claim, E11 repeat, record                   real proof, 35-45 min, 9.3 GiB
  burn             private burn of the claimed 0.001 ETH                            real proof, 35-45 min, 9.3 GiB
  release          STUB anchor root, 600 s wait, Groth16 (3.1.0), release, record   Groth16 about 8 min, 2.4 GiB
  v3-fund          wallet of the dedicated instance, fund its private submitter     real proof, 20-28 min, 9.2 GiB
  v3-init          dedicated mainnet instance, only verifier lc_verify_v3           public tx, seconds
  v3-prove         VerifyOnce on a current finality update, record                  real proof, 1.5-2 h, 9.3 GiB
  total: proving about 4.0-5.0 h, with waits about 5-6 h; peak RSS about 9.3 GiB (a single proof at a time)
EOF
echo "preconditions:"
check "BV_PROOF_WINDOW_OK=1 (approval to free memory for the proofs)" test "${BV_PROOF_WINDOW_OK:-0}" = 1
if [ "${BV_SEPOLIA_OK:-}" = yes ]; then echo "  ok    BV_SEPOLIA_OK=yes: Sepolia stages enabled"
else echo "  skip  BV_SEPOLIA_OK not yes: the claim and burn stages are skipped (Sepolia actions need explicit approval)"; fi
check "memory: MemAvailable >= 10.5 GiB and swap use < 2 GiB (now $(free -g | awk '/Mem:/{print $7}') GiB available, $(free -m | awk '/Swap:/{print $3}') MiB swap)" mem_ok
check "build lock free ($B/run/build.lock)" lock_free "$B/run/build.lock"
check "proving lock free ($B/run/prove.lock)" lock_free "$B/run/prove.lock"
check "seq-main up (pid $(cat "$B/run/seq-main.pid" 2>/dev/null))" pidfile_alive "$B/run/seq-main.pid"
check "run C follower up (pid $(cat "$B/run/w4/run-c/followerd.pid" 2>/dev/null))" pidfile_alive "$B/run/w4/run-c/followerd.pid"
check "run C Logos session up" logos_c_alive
check "run D follower up ($(cat "$B/run/w4/run-d/followerd.pid" 2>/dev/null))" pidfile_alive "$B/run/w4/run-d/followerd.pid"
check "run E follower up ($(cat "$B/run/w4/run-e/followerd.pid" 2>/dev/null))" pidfile_alive "$B/run/w4/run-e/followerd.pid"
check "public runs' Logos session up" logos_pub_alive
check "seq-pub up without RISC0_DEV_MODE (pid $(cat "$B/run/seq-pub.pid" 2>/dev/null))" seq_pub_real
check "RISC0_DEV_MODE unset in this shell" test -z "${RISC0_DEV_MODE+x}"
check "own cgroup ($(cut -d: -f3 /proc/self/cgroup)) is not lechu-tmux.service nor that of seq-pub or a follower (launch with systemd-run --user --scope)" own_cgroup_ok
check "stage names valid${FROM:+ (--from $FROM)}" true
if [ "${BV_SEPOLIA_OK:-}" = yes ]; then
  check "BV_RELEASE_RECIPIENT is an Ethereum address (${BV_RELEASE_RECIPIENT:-unset})" recipient_ok
  check "no vault, deposit or forge attempt of an earlier window in $(dirname "$FLOW_ENV") (else resume with --from claim-wait or later)" no_sepolia_repeat
fi
echo "stage scripts:"
if [ "${BV_SEPOLIA_OK:-}" = yes ]; then e2e_target=sepolia; else e2e_target=anvil-fork; fi
BV_V4_TARGET=$e2e_target "$E2E" check 2>&1 | sed 's/^/  e2e_v4: /' | grep -v '^  e2e_v4: $' ; [ "${PIPESTATUS[0]}" = 0 ] || fails+=("e2e_v4.sh check")
"$RP" --check 2>&1 | sed 's/^/  real_proof_v3: /'; [ "${PIPESTATUS[0]}" = 0 ] || fails+=("real_proof_v3.sh --check")

if [ "$DRY" = 1 ]; then
  echo "dry run: ${#fails[@]} precondition(s) failing${fails[*]:+: ${fails[*]}}"
  exit 0
fi
[ "${#fails[@]}" = 0 ] || { echo "refusing to start: ${fails[*]}" >&2; exit 1; }

mkdir -p "$LOGDIR"
echo $$ > "$B/run/proof-window.pid"
trap 'rm -f "$B/run/proof-window.pid"' EXIT
echo 1000 > /proc/self/oom_score_adj   # inherited by every stage: under memory pressure the proof goes first
export BV_V4_TARGET=sepolia BV_PROOF_WINDOW_OK=1
started=0
stage() { # name, command...
  local name="$1"; shift
  [ -z "$FROM" ] || [ "$FROM" = "$name" ] || [ "$started" = 1 ] || { echo "skip $name (--from $FROM)"; return 0; }
  started=1
  mem_ok || { echo "stage $name: memory short now; stopping (resume with --from $name)" >&2; exit 1; }
  echo "== $(date -u +%FT%TZ) stage $name"
  /usr/bin/time -v -o "$LOGDIR/$name.time.txt" "$@" > "$LOGDIR/$name.log" 2>&1
  local rc=$?
  echo "== $(date -u +%FT%TZ) stage $name exit $rc ($(grep -E 'Elapsed|Maximum resident' "$LOGDIR/$name.time.txt" | tr -s ' ' | paste -sd ';'))"
  [ "$rc" = 0 ] || { echo "stage $name failed: $LOGDIR/$name.log (resume with --from $name)" >&2; exit "$rc"; }
}
if [ "${BV_SEPOLIA_OK:-}" = yes ]; then
  stage claim-prep bash -c "'$E2E' vault && '$E2E' config && '$E2E' wallet && '$E2E' deposit"
  stage claim-wait "$E2E" wait-record
  stage claim-fund "$E2E" fund
  stage claim bash -c "'$E2E' claim && '$E2E' record-claim"
  stage burn "$E2E" burn
  stage release bash -c "'$E2E' post-root && '$E2E' wait-anchor && '$E2E' prove && '$E2E' release && '$E2E' record-burn"
fi
stage v3-fund "$RP" --wallet
stage v3-init "$RP" --init
stage v3-prove "$RP" --prove
echo "proof window done: $LOGDIR"
