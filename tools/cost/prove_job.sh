#!/usr/bin/env bash
# Proof-window job runner. One real-proving job at a time, under
# `nice -n 19 ionice -c3`, with wall seconds, peak RSS, receipt bytes and the machine line recorded next to the raw
# output. Used by `make cost-prove` and by scripts/gate/g1_real_proof.sh.
#
#   tools/cost/prove_job.sh --name NAME [--receipt FILE] [--receipt-regex RE] [--cycles N] [--note TEXT]
#                           [--out DIR] [--wait] [--allow-dev] -- COMMAND [ARGS...]
#
#   --name           job name; raw files go to evidence/prove/NAME/<UTC stamp>/
#   --receipt FILE   receipt file the command writes: its size is recorded as receipt_bytes
#   --receipt-regex  or: a Python regex whose group 1 is the receipt byte count, searched in the command's output
#   --cycles N       padded total cycles proven (for seconds per Mcycle); optional
#   --wait           wait for the proving lock instead of failing when another job holds it
#   --foreground     run attached (default: detach with setsid nohup, PID in $B/run/prove-NAME.pid)
#   --allow-dev      allow RISC0_DEV_MODE (only to test this script: the record then says dev_mode true)
#
# Refuses to run unless BV_PROOF_WINDOW_OK=1 (the proof window has been confirmed), and unless
# RISC0_DEV_MODE is unset (real proofs only). Exit code = the command's exit code (or 3 for a refusal).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"

# Detach by default (2026-11-12): a proof outlives the shell, SSH session or process that started it. The job
# re-executes itself under `setsid nohup` (new session, parent init, no tty), writes its PID to $B/run/<pidname>.pid
# and returns at once. --foreground (or BV_FOREGROUND=1) runs attached; nested calls inherit BV_DETACHED=1.
bv_detach() { # pidname, original args...
  local pidname="$1"; shift
  mkdir -p "$B/run"
  local runlog; runlog="$B/run/$pidname.$(date -u +%Y%m%dT%H%M%SZ).log"
  BV_DETACHED=1 setsid nohup "$0" "$@" > "$runlog" 2>&1 < /dev/null &
  local pid=$!
  echo "$pid" > "$B/run/$pidname.pid"
  echo "detached: pid $pid (session leader, parent init), log $runlog, pid file $B/run/$pidname.pid"
  exit 0
}
orig_args=("$@")

name="" receipt="" receipt_re="" cycles="" note="" outbase="" wait=0 allow_dev=0
while [ $# -gt 0 ]; do
  case "$1" in
    --name) name="$2"; shift 2 ;;
    --receipt) receipt="$2"; shift 2 ;;
    --receipt-regex) receipt_re="$2"; shift 2 ;;
    --cycles) cycles="$2"; shift 2 ;;
    --note) note="$2"; shift 2 ;;
    --out) outbase="$2"; shift 2 ;;
    --wait) wait=1; shift ;;
    --allow-dev) allow_dev=1; shift ;;
    --foreground) export BV_FOREGROUND=1; shift ;;
    --) shift; break ;;
    *) echo "prove_job: unknown option $1" >&2; exit 2 ;;
  esac
done
[ -n "$name" ] && [ $# -gt 0 ] || { sed -n '2,20p' "$0" >&2; exit 2; }

# env.sh exports RISC0_DEV_MODE=1 for everything else; a proof job must say explicitly that it wants real proofs.
if [ "${BV_REAL_PROOF:-0}" = 1 ]; then unset RISC0_DEV_MODE; fi
if [ -n "${RISC0_DEV_MODE:-}" ] && [ "${RISC0_DEV_MODE}" != 0 ] && [ "$allow_dev" != 1 ]; then
  echo "prove_job: RISC0_DEV_MODE=$RISC0_DEV_MODE. Real proofs need BV_REAL_PROOF=1 (which unsets it)." >&2; exit 3
fi
if [ "${BV_PROOF_WINDOW_OK:-0}" != 1 ] && [ "$allow_dev" != 1 ]; then
  echo "prove_job: refusing: real proving only in a confirmed proof window (BV_PROOF_WINDOW_OK=1)." >&2
  exit 3
fi
if [ "${BV_DETACHED:-0}" != 1 ] && [ "${BV_FOREGROUND:-0}" != 1 ]; then bv_detach "prove-$name" "${orig_args[@]}"; fi
dev_mode=false; [ -n "${RISC0_DEV_MODE:-}" ] && [ "${RISC0_DEV_MODE}" != 0 ] && dev_mode=true

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="$(realpath -m "${outbase:-$BV_REPO/evidence/prove/$name/$stamp}")"
mkdir -p "$out" "$B/run"
lock="$B/run/prove.lock"
exec 9>"$lock"
if [ "$wait" = 1 ]; then flock 9; else flock -n 9 || { echo "prove_job: another proving job holds $lock" >&2; exit 3; }; fi

machine="$(lscpu | sed -n 's/^Model name: *//p'), $(nproc) threads, $(free -g | awk '/Mem:/{print $2}') GiB RAM, $(free -g | awk '/Swap:/{print $2}') GiB swap, kernel $(uname -r), $(r0vm --version 2>/dev/null || echo 'r0vm ?')"
{
  echo "name=$name"
  echo "start_utc=$(date -u +%FT%TZ)"
  echo "machine=$machine"
  echo "loadavg_start=$(cat /proc/loadavg)"
  echo "mem_start=$(free -m | awk '/Mem:/{print "total "$2" MiB, available "$7" MiB"}'); swap $(free -m | awk '/Swap:/{print "used "$3" MiB of "$2}')"
  echo "risc0_dev_mode=${RISC0_DEV_MODE:-unset} risc0_prover=${RISC0_PROVER:-default}"
  echo "repo_commit=$(git -C "$BV_REPO" rev-parse --short HEAD 2>/dev/null || echo none) dirty=$(git -C "$BV_REPO" status --porcelain 2>/dev/null | wc -l)"
  printf 'cmd=nice -n 19 ionice -c3'; printf ' %q' "$@"; echo
  echo "cwd=$(pwd)"
  echo "note=$note"
} > "$out/meta.txt"
echo "prove_job: $name -> $out"

set +e
/usr/bin/time -v -o "$out/time.txt" nice -n 19 ionice -c3 "$@" > "$out/output.log" 2>&1 &
root=$!
python3 -I "$here/rss_sampler.py" "$root" "$out/samples.csv" "$out/sampler.json" --interval 1 &
sampler=$!
wait "$root"
rc=$?
wait "$sampler"
set -e
{
  echo "end_utc=$(date -u +%FT%TZ)"
  echo "loadavg_end=$(cat /proc/loadavg)"
  echo "exit=$rc"
} >> "$out/meta.txt"

python3 -I "$here/cost.py" prove-record --dir "$out" --name "$name" --exit "$rc" --dev-mode "$dev_mode" \
  ${receipt:+--receipt "$receipt"} ${receipt_re:+--receipt-regex "$receipt_re"} ${cycles:+--cycles "$cycles"} \
  > "$out/record.path"
echo "prove_job: exit $rc; record $(cat "$out/record.path")"
exit "$rc"
