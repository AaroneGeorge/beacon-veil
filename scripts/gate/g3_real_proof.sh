#!/usr/bin/env bash
# Gate G3 with a REAL proof (requested 2026-11-12): on seq-real (RISC0_DEV_MODE unset), a private
# `Attest` of the mainnet USDT `Transfer` (block 26397891) proven for real in the user's private account and delivered
# to a public consumer_erc20 instance in the same transaction; then the identical transaction again, refused with
# E11_ALREADY_CONSUMED. Writes evidence/g3/real-proof.json (wall time, peak RSS, tx, block, image ids) and the raw job
# directory under evidence/prove/g3-real-consumer/.
#
#   BV_PROOF_WINDOW_OK=1 scripts/gate/g3_real_proof.sh --instance BASE58     # detaches; PID in $B/run/g3-real-consumer.pid
#   scripts/gate/g3_real_proof.sh --verify                                   # re-check evidence/g3/real-proof.json
#
# Needs: seq-real running without RISC0_DEV_MODE; attest, lc_verify, consumer_erc20 in $BV_IDS_REAL (default
# deploy/ids-real.toml) from the release set; the instance holds the record of beacon slot 15637568; a wallet
# ($BV_REAL_WALLET, default $B/run/wallet-w6-real) with bv-payer and a funded private bv-submitter;
# $CARGO_TARGET_DIR/debug/g3_real built (cargo build -p e2e_consumer --bin g3_real).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
bv_detach() { # pidname, original args...
  local pidname="$1"; shift
  local runlog; runlog="$B/run/$pidname.$(date -u +%Y%m%dT%H%M%SZ).log"
  BV_DETACHED=1 setsid nohup "$0" "$@" > "$runlog" 2>&1 < /dev/null &
  local pid=$!
  echo "$pid" > "$B/run/$pidname.pid"
  echo "detached: pid $pid, log $runlog, pid file $B/run/$pidname.pid"
  exit 0
}
orig_args=("$@")
out="$BV_REPO/evidence/g3"; mkdir -p "$out"
bin="$CARGO_TARGET_DIR/debug/g3_real"
mode=run instance=""
while [ $# -gt 0 ]; do
  case "$1" in
    --instance) instance="$2"; shift 2 ;;
    --verify) mode=verify; shift ;;
    *) echo "g3_real_proof: unknown argument $1" >&2; exit 2 ;;
  esac
done
verify_json() {
  python3 -I - "$out/real-proof.json" <<'EOF'
import json, os, sys
p = sys.argv[1]
if not os.path.exists(p):
    print(json.dumps({"G3_real": "FAIL", "why": f"{p} missing"})); sys.exit(1)
d = json.load(open(p))
ok = (d.get("dev_mode") is False and d.get("accepted") is True and d.get("replay", {}).get("code") == "E11_ALREADY_CONSUMED"
      and not d.get("replay", {}).get("accepted") and all(d.get(k) not in (None, "") for k in ("tx", "block_id", "peak_rss_gib", "job_wall_seconds")))
repo = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(p))))
raw_ok = all(os.path.exists(os.path.join(repo, f)) for f in d.get("raw", {}).values())
print(json.dumps({"G3_real": "PASS" if ok and raw_ok else "FAIL", "tx": d.get("tx"), "raw_files_present": raw_ok}))
sys.exit(0 if ok and raw_ok else 1)
EOF
}
[ "$mode" = verify ] && { verify_json; exit $?; }

# ---- preconditions ----
[ -n "$instance" ] || { echo "g3_real_proof: --instance required" >&2; exit 2; }
pidf="$B/run/seq-real.pid"
"$BV_REPO/scripts/seq.sh" real status >/dev/null 2>&1 || { echo "seq-real is not running" >&2; exit 1; }
if tr '\0' '\n' < "/proc/$(cat "$pidf")/environ" | grep -q '^RISC0_DEV_MODE='; then
  echo "seq-real has RISC0_DEV_MODE in its environment" >&2; exit 1
fi
export BV_IDS="${BV_IDS_REAL:-$BV_REPO/deploy/ids-real.toml}"
export LEE_WALLET_HOME_DIR="${BV_REAL_WALLET:-$B/run/wallet-w6-real}" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
[ -x "$bin" ] || { echo "$bin missing (cargo build -p e2e_consumer --bin g3_real)" >&2; exit 1; }
if [ "${BV_DETACHED:-0}" != 1 ] && [ "${BV_FOREGROUND:-0}" != 1 ]; then bv_detach "g3-real-consumer" "${orig_args[@]}"; fi

# ---- the proof: one job under the proving lock (prove_job: nice 19, ionice 3, time -v, RSS sampler) ----
seqlog="$B/run/seq-real.log"
seq_before=$(wc -l < "$seqlog" 2>/dev/null || echo 0)
jobdir="$BV_REPO/evidence/prove/g3-real-consumer/$(date -u +%Y%m%dT%H%M%SZ)"
bins_dir="$(python3 -I -c "import re,sys;t=open(sys.argv[1]).read();m=re.search(r'\[bins\][^\[]*?consumer_erc20 = \"([^\"]+)\"',t);print(m.group(1) if m else '')" "$BV_IDS")"
set +e
BV_REAL_PROOF=1 BV_FOREGROUND=1 "$BV_REPO/tools/cost/prove_job.sh" --wait --name g3-real-consumer --out "$jobdir" \
  --note "G3 real: private Attest (USDT Transfer log, block 26397891) -> consumer_erc20 on seq-real, then replay (E11)" \
  -- "$bin" --instance "$instance"
rc=$?
"$bin" --check-image "$bins_dir/IMAGE_IDS" > "$jobdir/image-check.log" 2>&1 || true
set -e
tail -n +"$((seq_before + 1))" "$seqlog" 2>/dev/null > "$jobdir/seq-real.log" || true
python3 -I - "$jobdir" "$out/real-proof.json" "$rc" "$BV_REPO" "$BV_IDS" "$bins_dir" <<'EOF'
import json, os, re, sys
jobdir, dst, rc, repo, ids_path, bins_dir = sys.argv[1:7]
jobrel = os.path.relpath(jobdir, repo)
rec = json.load(open(f"{jobdir}/record.json"))
log = open(f"{jobdir}/output.log", errors="replace").read()
m = re.search(r"^G3REAL (\{.*\})$", log, re.M)
res = json.loads(m.group(1)) if m else {}
seq = open(f"{jobdir}/seq-real.log", errors="replace").read().splitlines()
tx = res.get("tx") or ""
blk = res.get("block_id")
excerpt = [l for l in seq if (tx and tx[:16] in l) or (blk is not None and re.search(rf"\b(block|id) {blk}\b", l))
           or re.search(r"ESA:E|revert|reject|panick", l, re.I)][-20:]
images = {}
for l in open(os.path.join(bins_dir, "IMAGE_IDS")):
    mm = re.match(r"(\w+)\.bin: .*image id ([0-9a-f]{64})", l)
    if mm and mm.group(1) in ("attest", "consumer_erc20", "lc_verify"):
        images[mm.group(1)] = mm.group(2)
build = dict(l.strip().split("=", 1) for l in open(os.path.join(bins_dir, "BUILD_INFO")) if l.startswith(("commit=", "image=")))
progs = {}
for l in open(ids_path):
    mm = re.match(r'(attest|lc_verify|consumer_erc20) = "([1-9A-Za-z]{32,44})"\s*#\s*sha256 ([0-9a-f]{64})', l)
    if mm:
        progs[mm.group(1)] = {"id": mm.group(2), "bin_sha256": mm.group(3), "image_id": images.get(mm.group(1))}
check = open(f"{jobdir}/image-check.log", errors="replace").read()
doc = {
    "gate": "G3 (real proof, private placement)", "dev_mode": rec.get("dev_mode"),
    "accepted": bool(res.get("accepted")) and rc == "0", "label": "measured",
    "what": "private Attest (mainnet USDT Transfer, block 26397891, tx 1 log 2) proven for real in a private account, Statement delivered to public consumer_erc20 in the same tx; identical tx again refused",
    "tx": res.get("tx"), "block_id": res.get("block_id"), "instance": res.get("instance"),
    "consumer_instance": res.get("consumer_instance"), "consumer_init_tx": res.get("consumer_init_tx"),
    "statement_id": res.get("statement_id"), "nullifier": res.get("nullifier"),
    "consumer_count_before": res.get("consumer_count_before"), "consumer_count_after": res.get("consumer_count_after"),
    "replay": res.get("replay"),
    "prove_and_submit_seconds": res.get("prove_and_submit_s"), "job_wall_seconds": rec.get("wall_s"),
    "peak_rss_gib": rec.get("peak_rss_gib"), "swap_used_kib_max": rec.get("sampler", {}).get("swap_used_by_run_kib_max"),
    "tx_wire_bytes": res.get("tx_wire_bytes"), "witness_set_bytes": res.get("witness_set_bytes"),
    "programs": progs, "release": {"dir": bins_dir, **build},
    "consumer_image_check_on_chain": check.strip().splitlines()[-1:] ,
    "machine": rec.get("machine"), "command": rec.get("command"), "start_utc": rec.get("start_utc"),
    "end_utc": rec.get("end_utc"), "repo_commit": rec.get("repo_commit"),
    "sequencer": "seq-real (standalone rc3, RISC0_DEV_MODE unset in its environment)",
    "sequencer_log_excerpt": excerpt,
    "raw": {"job_record": f"{jobrel}/record.json", "output": f"{jobrel}/output.log", "time": f"{jobrel}/time.txt",
            "rss_samples": f"{jobrel}/samples.csv", "sequencer_log": f"{jobrel}/seq-real.log",
            "image_check": f"{jobrel}/image-check.log"},
}
json.dump(doc, open(dst, "w"), indent=2)
print(json.dumps({k: doc[k] for k in ("dev_mode", "accepted", "tx", "block_id", "prove_and_submit_seconds", "peak_rss_gib")}))
EOF
verify_json
