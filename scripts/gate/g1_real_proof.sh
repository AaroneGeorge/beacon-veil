#!/usr/bin/env bash
# Gate G1, real-proof half (PLAN.md section 1 and 3.4): one light-client update proven for real (not dev mode) and
# accepted by a standalone rc3 sequencer started with RISC0_DEV_MODE unset (seq-real). Writes
# evidence/g1/real-proof.json with dev_mode:false, accepted, tx id, block id, receipt bytes, prove seconds, peak RSS
# and a sequencer log excerpt, plus the raw job directory under evidence/prove/.
#
#   scripts/gate/g1_real_proof.sh --build           # outside a window: build the host binary (no proving)
#   scripts/gate/g1_real_proof.sh --check           # preconditions only (no proving)
#   BV_PROOF_WINDOW_OK=1 scripts/gate/g1_real_proof.sh --instance BASE58 [--step finality_update_15637644]
#                       [--verifier lc_verify|lc_verify_v2] [--name real-proof|real-proof-2]
#   (or --bootstrap bootstrap_15631808.ssz instead of --instance: Init first, in the same job)
#   scripts/gate/g1_real_proof.sh --verify          # re-check an existing evidence/g1/real-proof.json (make gate)
#
# Preconditions (checked): seq-real running and its process environment has no RISC0_DEV_MODE; ids of the programs
# deployed on seq-real in $BV_IDS_REAL (default deploy/ids-real.toml, so deploy/ids.toml of seq-dev is not touched);
# the wallet ($BV_REAL_WALLET, default wallet-real) has bv-payer and a funded private submitter ($BV_SUBMITTER or label
# bv-submitter). Init (public, no proof) can be done before with
#   g1_real_proof --bootstrap bootstrap_15631808.ssz --init-only   (prints G1INIT {instance, init_tx, ...}). Funding a private
# account on seq-real is itself a (small) real proof: do it inside the window, through tools/cost/prove_job.sh.
# Real proving only in a window the coordinator confirmed: BV_PROOF_WINDOW_OK=1.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"

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
orig_args=("$@")
out="$BV_REPO/evidence/g1"
mkdir -p "$out"
bin="$CARGO_TARGET_DIR/debug/g1_real_proof"
mode=run step="finality_update_15637644" instance="" extra=() name="real-proof"
while [ $# -gt 0 ]; do
  case "$1" in
    --build) mode=build; shift ;;
    --check) mode=check; shift ;;
    --verify) mode=verify; shift ;;
    --step) step="$2"; shift 2 ;;
    --instance) instance="$2"; shift 2 ;;
    --bootstrap|--verifier|--min-participants) extra+=("$1" "$2"); shift 2 ;;
    --name) name="$2"; shift 2 ;;   # evidence/g1/<name>.json (default real-proof; real proof 2: real-proof-2)
    *) echo "g1_real_proof: unknown argument $1" >&2; exit 2 ;;
  esac
done

verify_json() {
  python3 -I - "$out/$name.json" <<'EOF'
import json, os, sys
p = sys.argv[1]
if not os.path.exists(p):
    print(json.dumps({"G1_real": "FAIL", "why": f"{p} missing (written after a proof window)"})); sys.exit(1)
d = json.load(open(p))
need = ["tx", "block_id", "receipt_bytes", "prove_seconds", "peak_rss_gib", "sequencer_log_excerpt", "machine"]
ok = d.get("dev_mode") is False and d.get("accepted") is True and all(d.get(k) not in (None, "", []) for k in need)
repo = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(p))))
raw_ok = all(os.path.exists(f if os.path.isabs(f) else os.path.join(repo, f)) for f in d.get("raw", {}).values())
print(json.dumps({"G1_real": "PASS" if ok and raw_ok else "FAIL", "dev_mode": d.get("dev_mode"),
                  "accepted": d.get("accepted"), "tx": d.get("tx"), "raw_files_present": raw_ok}))
sys.exit(0 if ok and raw_ok else 1)
EOF
}

if [ "$mode" = verify ]; then verify_json; exit $?; fi

if [ "$mode" = build ]; then
  ( cd "$BV_REPO" && RISC0_SKIP_BUILD=1 nice -n 15 cargo build -p cost --bin g1_real_proof )
  echo "built $bin"; exit 0
fi

# ---- preconditions ----
fail=0
pidf="$B/run/seq-real.pid"
if "$BV_REPO/scripts/seq.sh" real status >/dev/null 2>&1; then
  if tr '\0' '\n' < "/proc/$(cat "$pidf")/environ" | grep -q '^RISC0_DEV_MODE='; then
    echo "G1 real: seq-real has RISC0_DEV_MODE in its environment: restart it with scripts/seq.sh real start" >&2; fail=1
  fi
else
  echo "G1 real: seq-real is not running (scripts/seq.sh real start)" >&2; fail=1
fi
export BV_IDS="${BV_IDS_REAL:-$BV_REPO/deploy/ids-real.toml}"
[ -f "$BV_IDS" ] || { echo "G1 real: $BV_IDS missing (deploy attest + lc_verify to seq-real, ids into this file)" >&2; fail=1; }
[ -x "$bin" ] || { echo "G1 real: $bin missing (run --build outside the window)" >&2; fail=1; }
export LEE_WALLET_HOME_DIR="${BV_REAL_WALLET:-$B/run/wallet-real}" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
[ -f "$LEE_WALLET_HOME_DIR/wallet_config.json" ] || { echo "G1 real: no wallet-real config" >&2; fail=1; }
[ "$fail" = 0 ] || exit 1
if [ "$mode" = check ]; then echo "G1 real: preconditions OK (ids $BV_IDS, step $step)"; exit 0; fi
if [ "${BV_DETACHED:-0}" != 1 ] && [ "${BV_FOREGROUND:-0}" != 1 ]; then bv_detach "g1-$name" "${orig_args[@]}"; fi

# ---- the proof (one job, nice 19, ionice 3, under the proving lock) ----
seqlog="$B/run/seq-real.log"
seq_before=$(wc -l < "$seqlog" 2>/dev/null || echo 0)
jobdir="$BV_REPO/evidence/prove/g1-$name/$(date -u +%Y%m%dT%H%M%SZ)"
args=(--step "$step" "${extra[@]}"); [ -n "$instance" ] && args+=(--instance "$instance")
set +e
BV_REAL_PROOF=1 "$BV_REPO/tools/cost/prove_job.sh" --wait --name "g1-$name" --out "$jobdir" \
  --note "G1 real proof, lc_verify V1 $step, Path A on seq-real" -- "$bin" "${args[@]}"
rc=$?
set -e
tail -n +"$((seq_before + 1))" "$seqlog" 2>/dev/null > "$jobdir/seq-real.log" || true

python3 -I - "$jobdir" "$out/$name.json" "$rc" "$step" "$BV_REPO" <<'EOF'
import json, os, re, sys
jobdir, dst, rc, step, repo = sys.argv[1:6]
jobrel = os.path.relpath(jobdir, repo)
rec = json.load(open(f"{jobdir}/record.json"))
log = open(f"{jobdir}/output.log", errors="replace").read()
m = re.search(r"^G1REAL (\{.*\})$", log, re.M)
res = json.loads(m.group(1)) if m else {}
seq = open(f"{jobdir}/seq-real.log", errors="replace").read().splitlines()
tx = res.get("tx") or ""
blk = res.get("block_id")
idx = [i for i, l in enumerate(seq) if blk is not None and re.search(rf"\bblock {blk}\b|id {blk} ", l)]
excerpt = seq[max(0, idx[0] - 4):idx[-1] + 3] if idx else []
excerpt += [l for l in seq if (tx and tx[:16] in l) or re.search(r"ESA:E|revert|reject|panick", l, re.I)][-20:]
doc = {
    "gate": "G1 (real-proof half)", "dev_mode": rec.get("dev_mode"), "accepted": bool(res.get("accepted")) and rc == "0",
    "step": step, "verifier": res.get("verifier"), "verifier_id": res.get("verifier_id"), "instance": res.get("instance"), "init_tx": res.get("init_tx"), "tx": res.get("tx"),
    "block_id": res.get("block_id"),
    "receipt_bytes": res.get("witness_set_bytes"),
    "receipt_bytes_note": "borsh size of the transaction's witness set (succinct receipt + signatures); LEZ prices the proof at PROOF_BYTES = 223,551",
    "tx_wire_bytes": res.get("tx_wire_bytes"),
    "prove_seconds": res.get("prove_and_submit_s"), "job_wall_seconds": rec.get("wall_s"),
    "peak_rss_gib": rec.get("peak_rss_gib"), "swap_used_kib_max": rec.get("sampler", {}).get("swap_used_by_run_kib_max"),
    "load1_mean": rec.get("sampler", {}).get("load1_mean"),
    "head_before": res.get("head_before"), "head_after": res.get("head_after"),
    "machine": rec.get("machine"), "command": rec.get("command"), "start_utc": rec.get("start_utc"),
    "end_utc": rec.get("end_utc"), "repo_commit": rec.get("repo_commit"), "label": "measured",
    "sequencer": "seq-real (standalone rc3, RISC0_DEV_MODE unset in its environment)",
    "sequencer_log_excerpt": excerpt,
    "raw": {"job_record": f"{jobrel}/record.json", "output": f"{jobrel}/output.log", "time": f"{jobrel}/time.txt",
            "rss_samples": f"{jobrel}/samples.csv", "sequencer_log": f"{jobrel}/seq-real.log"},
}
json.dump(doc, open(dst, "w"), indent=2)
print(json.dumps({k: doc[k] for k in ("dev_mode", "accepted", "tx", "block_id", "prove_seconds", "peak_rss_gib")}))
EOF
verify_json
