#!/usr/bin/env bash
# `make cost-prove JOB=<job>` (W6): the proof-window jobs of PLAN.md 3.4, one at a time, each through
# tools/cost/prove_job.sh (nice 19, ionice 3, proving lock, wall seconds, peak RSS, receipt bytes, machine line).
# Afterwards (and with no JOB) it rebuilds evidence/cost/prove.{json,md} from every evidence/prove/*/record.json.
#
#   BV_PROOF_WINDOW_OK=1 tools/cost/cost_prove.sh calib-seg20   # LEZ rc3 cycle_bench --prove: per-po2 prove time,
#                                                               #   one 1,048,576-cycle (po2=20) segment per run, RUNS=3
#   BV_PROOF_WINDOW_OK=1 tools/cost/cost_prove.sh calib-ppe     # LEZ rc3 cycle_bench --ppe: privacy-circuit wrap
#   BV_PROOF_WINDOW_OK=1 tools/cost/cost_prove.sh calib-recursion # r0vm CLI, po2 15: lift + join seconds
#   BV_PROOF_WINDOW_OK=1 tools/cost/cost_prove.sh g1-real [--step update[0]] [--instance ID]   # G1 real proof
#   tools/cost/cost_prove.sh                                    # summary only
#
# cycle_bench is LEZ's own bench at the pinned tag (no edits), built outside the window into $B/.tmp/w6/lez-target:
#   cd $B/vendor/lez && CARGO_TARGET_DIR=$B/.tmp/w6/lez-target cargo build --release --locked -p cycle_bench --features ppe
# RISC0_PROVER=ipc makes it prove through the same external r0vm 3.0.5 binary the LEZ wallet uses.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"
# Detach by default (coordinator 2026-11-12): a proof outlives the shell, SSH session or agent that started it. The job
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
if [ -n "${1:-}" ] && [ "${BV_DETACHED:-0}" != 1 ] && [ "${BV_FOREGROUND:-0}" != 1 ]; then bv_detach "cost-prove-$1" "$@"; fi
job="${1:-}"; [ $# -gt 0 ] && shift
cb="${CYCLE_BENCH_BIN:-$B/.tmp/w6/lez-target/release/cycle_bench}"
cb_json="$B/vendor/lez/target/cycle_bench.json"   # cycle_bench writes here (path fixed in its main.rs)

cycle_bench_job() { # name, cycle_bench args...
  local name="$1"; shift
  [ -x "$cb" ] || { echo "cost-prove: $cb missing (build it outside the window, see header)" >&2; exit 1; }
  local dir; dir="$BV_REPO/evidence/prove/$name/$(date -u +%Y%m%dT%H%M%SZ)"
  local rc=0
  RISC0_PROVER=ipc BV_REAL_PROOF=1 "$here/prove_job.sh" --name "$name" --out "$dir" \
    --note "LEZ v0.4.0-rc3 tools/cycle_bench (pinned, unmodified), RISC0_PROVER=ipc (r0vm 3.0.5)" \
    -- "$cb" "$@" || rc=$?
  [ -f "$cb_json" ] && cp "$cb_json" "$dir/cycle_bench.json"
  return $rc
}

case "$job" in
  "") ;;
  calib-seg20)
    # rc3 cycle_bench has one po2=20 case per run (scripted "Write 64 KiB", 550,776 user cycles, padded to 2^20) and
    # five small ones; RUNS separate jobs (default 3) give 3 po2-20 segment proofs.
    for _ in $(seq 1 "${RUNS:-3}"); do cycle_bench_job calib-seg20 --prove --exec-iters 1; done ;;
  calib-ppe) cycle_bench_job calib-ppe --ppe --exec-iters 2 ;;
  calib-recursion)
    # Lift and join cost on this machine: the same small run (attest Apply, update[0] input from `cost exec-lc` with
    # COST_DUMP_DIR) proven by the r0vm 3.0.5 CLI with segment limit po2 15 (several segments), once composite and once
    # succinct; succinct minus composite = n lifts + (n - 1) joins. Inputs: $B/.tmp/w6/rec/{attest.bin,apply.input}.
    rec="${REC_DIR:-$B/.tmp/w6/rec}"
    for kind in composite succinct; do
      RUST_LOG=info BV_REAL_PROOF=1 "$here/prove_job.sh" --name "calib-recursion-$kind" --receipt "$rec/$kind.receipt" \
        --note "r0vm --po2 15 --receipt-kind $kind on attest Apply (image $(sha256sum "$rec/attest.bin" | cut -c1-16) sha256 of .bin)" \
        -- r0vm --elf "$rec/attest.bin" --initial-input "$rec/apply.input" --receipt "$rec/$kind.receipt" \
           --receipt-kind "$kind" --po2 15 -v
    done ;;
  g1-real) "$BV_REPO/scripts/gate/g1_real_proof.sh" "$@" ;;
  *) echo "cost-prove: unknown job '$job' (calib-seg20 | calib-recursion | calib-ppe | g1-real)" >&2; exit 2 ;;
esac
python3 -I "$here/cost.py" prove-summary --prove-dir "$BV_REPO/evidence/prove" --out "$BV_REPO/evidence/cost"
