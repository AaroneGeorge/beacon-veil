#!/usr/bin/env bash
# One real proof of the staged verifier's program, private mode: a dedicated mainnet instance on seq-pub whose only
# verifier is lc_verify_v3 and that no follower moves, then `attest update --mode v3-private` on a current mainnet
# finality update (lc_verify_v3 VerifyOnce in a private actor, proven by the wallet with RISC0_DEV_MODE unset; the
# sequencer verifies the proof and applies attest.Apply publicly). Writes evidence/public-path/real-proof.json.
#
#   scripts/proof/real_proof_v3.sh --check      preconditions and the plan; sends nothing, proves nothing
#   scripts/proof/real_proof_v3.sh --wallet     wallet home with a funded public payer (public transfer) and a funded
#                                                private submitter (a private transfer: a real proof, window only)
#   scripts/proof/real_proof_v3.sh --init       the dedicated instance (attest init --only-verifier; public, no proof)
#   scripts/proof/real_proof_v3.sh --prove      the proof (window only), then the executor run of the same message and
#                                                the record
#   scripts/proof/real_proof_v3.sh --record JOBDIR   (re)write the record from a finished job directory
#
# Window gates (as tools/cost/prove_job.sh): BV_PROOF_WINDOW_OK=1, RISC0_DEV_MODE unset (BV_REAL_PROOF=1), the proving
# lock. Run from the repo that the host binaries were built from.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
unset RISC0_DEV_MODE
D="${BV_RP_DIR:-$B/run/real-proof-v3}"; mkdir -p "$D"
export LEE_WALLET_HOME_DIR="${BV_RP_WALLET:-$B/run/wallet-real-proof-v3}" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
export BV_IDS="${BV_IDS_PUB:-$BV_REPO/deploy/ids-pub.toml}" BV_CHAIN_ID=1
ATTEST="${BV_ATTEST:-$B/target/host/attest}"
EXEC_ONCE="${BV_EXEC_ONCE:-$B/target/host/exec_once}"
V3_ELF="${BV_V3_ELF:-$(awk -F'"' '/^\[bins\]/{f=1} f && $1 ~ /^lc_verify_v3 /{print $2; exit}' "$BV_IDS" 2>/dev/null || true)/elf/lc_verify_v3}"
OUT="${BV_RP_OUT:-$BV_REPO/evidence/public-path/real-proof.json}"
prog() { awk -v k="$1" '/^\[programs\]/{f=1; next} /^\[/{f=0} f && $1==k {split($0,a,"\""); print a[2]; exit}' "$BV_IDS" 2>/dev/null || true; }
V3="$(prog lc_verify_v3)"
instance() { python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["instance"])' "$D/instance.json" 2>/dev/null; }

check() {
  local fail=0
  "$BV_REPO/scripts/seq.sh" pub status >/dev/null 2>&1 || { echo "seq-pub is not running" >&2; fail=1; }
  local sp; sp="$(cat "$B/run/seq-pub.pid" 2>/dev/null || true)"
  if [ -n "$sp" ] && tr '\0' '\n' < "/proc/$sp/environ" 2>/dev/null | grep -q '^RISC0_DEV_MODE='; then
    echo "seq-pub runs with RISC0_DEV_MODE set" >&2; fail=1; fi
  [ -n "$V3" ] || { echo "no [programs] lc_verify_v3 in $BV_IDS" >&2; fail=1; }
  "$ATTEST" init --help | grep -q -- --only-verifier || { echo "$ATTEST lacks --only-verifier (rebuild host)" >&2; fail=1; }
  "$ATTEST" update --help | grep -q v3-private || { echo "$ATTEST lacks --mode v3-private (rebuild host)" >&2; fail=1; }
  [ -x "$EXEC_ONCE" ] || { echo "$EXEC_ONCE missing: cargo build --profile host -p diff_helios --features executor --bin exec_once" >&2; fail=1; }
  [ -f "$V3_ELF" ] || { echo "lc_verify_v3 ELF $V3_ELF missing" >&2; fail=1; }
  echo "plan: instance file $D/instance.json ($( [ -f "$D/instance.json" ] && instance || echo 'not created yet: --init'))"
  echo "      wallet $LEE_WALLET_HOME_DIR ($( [ -s "$D/wallet.env" ] && echo ready || echo 'not created yet: --wallet'))"
  echo "      proof: attest --chain-id 1 update --mode v3-private (current finality update), executor run of the same"
  echo "      message ($V3_ELF), record $OUT"
  echo "      expected: about 12M user cycles, 12-13 segments, 1.5-2 h, peak RSS about 9.3 GiB"
  return "$fail"
}

record() { # jobdir
  local job="$1" ex=""
  [ -d "$job/inputs" ] && [ -x "$EXEC_ONCE" ] && ex="$("$EXEC_ONCE" --inputs "$job/inputs" --elf "$V3_ELF" 2>/dev/null || true)"
  python3 -I - "$job" "$OUT" "$BV_REPO" "$ex" <<'PY'
import json, os, re, sys
job, dst, repo, ex = sys.argv[1:5]
rec = json.load(open(f"{job}/record.json"))
log = open(f"{job}/output.log", errors="replace").read()
res = {}
dec = json.JSONDecoder()
for m in re.finditer(r"^\{", log, re.M):
    try:
        d, _ = dec.raw_decode(log[m.start():])
    except ValueError:
        continue
    if isinstance(d, dict) and d.get("mode") == "v3-private":
        res = d
exr = json.loads(ex) if ex.strip().startswith("{") else {}
rel = os.path.relpath(job, repo)
doc = {
    "what": "One update through lc_verify_v3 VerifyOnce in a private actor, proven for real (RISC0_DEV_MODE unset), "
            "on seq-pub; the sequencer verifies the proof and applies attest.Apply. Instance verifiers: lc_verify_v3 only.",
    "label": "measured, real proof",
    "instance": res.get("instance"), "verifier": res.get("verifier"),
    "update": {k: res.get(k) for k in ("kind", "finalized_slot", "attested_slot", "signature_slot", "participation",
                                       "update_ssz_bytes")},
    "user_cycles": exr.get("user_cycles"), "segments": exr.get("segments"),
    "cycles_source": "the same VerifyOnce message re-run in the RISC Zero executor with the deployed ELF",
    "elf_sha256": exr.get("elf_sha256"), "image_id": exr.get("image_id"), "executor_verdict": exr.get("verdict"),
    "wall_seconds": rec.get("wall_s"), "prove_and_submit_s": res.get("prove_and_submit_s"),
    "peak_rss_gib": rec.get("peak_rss_gib"),
    "tx": res.get("tx"), "block_id": res.get("block_id"),
    "risc0_dev_mode": "unset" if not rec.get("dev_mode") and res.get("risc0_dev_mode") is None else "SET",
    "head_before": res.get("head_before"), "head_after": res.get("head_after"), "expected_head": res.get("expected_head"),
    "head_equals_native": res.get("head_equals_native"),
    "machine": rec.get("machine"), "start_utc": rec.get("start_utc"), "end_utc": rec.get("end_utc"),
    "repo_commit": rec.get("repo_commit"),
    "raw": {"job_record": f"{rel}/record.json", "output": f"{rel}/output.log", "time": f"{rel}/time.txt"},
}
need = ["instance", "user_cycles", "segments", "wall_seconds", "peak_rss_gib", "tx", "block_id"]
missing = [k for k in need if doc.get(k) in (None, "")] + ([] if doc["update"].get("finalized_slot") else ["update"])
doc["complete"] = not missing and doc["risc0_dev_mode"] == "unset" and doc["head_equals_native"] is True
doc["missing"] = missing
os.makedirs(os.path.dirname(dst), exist_ok=True)
json.dump(doc, open(dst, "w"), indent=2)
print(json.dumps({k: doc[k] for k in ("complete", "missing", "tx", "block_id", "wall_seconds", "peak_rss_gib",
                                      "user_cycles", "segments", "head_equals_native")}))
sys.exit(0 if doc["complete"] else 1)
PY
}

case "${1:-}" in
  --check) check ;;
  --wallet)
    [ "${BV_PROOF_WINDOW_OK:-0}" = 1 ] || { echo "the private submitter's funding is a real proof: window only (BV_PROOF_WINDOW_OK=1)" >&2; exit 3; }
    pgrep -x v3_onchain >/dev/null && { echo "a v3_onchain process sends from bv-payer; try later" >&2; exit 1; }
    # Public payer from bv-payer (under the deployment lock), then the private submitter: a real proof, so the whole
    # call runs as a proving job (time -v, RSS samples, the proving lock).
    WALLET_CONFIG_FROM="$B/run/wallet-pub" BV_REAL_PROOF=1 "$BV_REPO/tools/cost/prove_job.sh" --wait --foreground \
      --name real-proof-v3-fund --note "fund the private submitter on seq-pub" -- \
      flock -w 600 "$BV_REPO/deploy/ids-pub.toml.lock" "$BV_REPO/crates/follower/scripts/wallet_home.sh" \
      "$(basename "$LEE_WALLET_HOME_DIR" | sed 's/^wallet-//')" 10000000000 1000000000
    job="$(ls -d "$BV_REPO"/evidence/prove/real-proof-v3-fund/*/ | tail -1)"
    grep '^export ' "$job/output.log" > "$D/wallet.env.tmp"
    grep '^export ' "$D/wallet.env.tmp" > "$D/wallet.env"; rm -f "$D/wallet.env.tmp"; cat "$D/wallet.env" ;;
  --init)
    [ -f "$D/instance.json" ] && { echo "$D/instance.json exists: $(instance)"; exit 0; }
    [ -s "$D/wallet.env" ] || { echo "no wallet: --wallet first" >&2; exit 1; }
    # shellcheck source=/dev/null
    source <(grep '^export ' "$D/wallet.env")
    "$ATTEST" --chain-id 1 init --only-verifier "$V3" --out "$D/instance.json"
    echo "instance $(instance): no follower may use it" ;;
  --prove)
    check || exit 1
    [ -f "$D/instance.json" ] || { echo "no instance: --init first" >&2; exit 1; }
    # shellcheck source=/dev/null
    source <(grep '^export ' "$D/wallet.env")
    job="$BV_REPO/evidence/prove/real-proof-v3/$(date -u +%Y%m%dT%H%M%SZ)"
    set +e
    BV_REAL_PROOF=1 BV_FOREGROUND=1 "$BV_REPO/tools/cost/prove_job.sh" --wait --foreground --name real-proof-v3 \
      --out "$job" --note "lc_verify_v3 VerifyOnce, private actor, mainnet finality update, seq-pub" -- \
      "$ATTEST" --chain-id 1 update --instance "$(instance)" --mode v3-private --v3 "$V3" --save-inputs "$job/inputs"
    rc=$?
    set -e
    record "$job" || true
    exit "$rc" ;;
  --record) record "${2:?--record JOBDIR}" ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
