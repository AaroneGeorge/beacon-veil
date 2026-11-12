#!/usr/bin/env bash
# Gate G5 (PLAN.md section 1): the measured cost table is reproducible from a clean clone.
#   1. fresh `git clone` of the committed HEAD into $B/.tmp/clean (uncommitted work is not part of the clone);
#   2. `make cost-exec` there, with its own empty target dir ($B/.tmp/g5/target: a cold build, nothing reused);
#   3. exact diff of the cycle and gas columns against the committed evidence/cost/cost.json;
#   4. every real-proof record under evidence/prove/ in the clone has its raw files (log, time, RSS samples).
#
#   scripts/gate/g5_repro.sh [--against FILE] [--keep]
#
# --against: reference cost.json (default: the clone's own committed evidence/cost/cost.json).
# The guest build mode is taken from the reference (`run.guest_build`): docker if the reference used the pinned image.
# Writes evidence/g5/repro.json (+ repro.log). Exit 0 only on an exact match.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
against="" keep=0
while [ $# -gt 0 ]; do
  case "$1" in
    --against) against="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    *) echo "g5_repro: unknown argument $1" >&2; exit 2 ;;
  esac
done
out="$BV_REPO/evidence/g5"; mkdir -p "$out"
log="$out/repro.log"; : > "$log"
clean="$B/.tmp/clean"
g5home="$B/.tmp/g5"      # B for the clone: CARGO_TARGET_DIR=$g5home/target (cold), TMPDIR under it
started="$(date -u +%FT%TZ)"

rm -rf "$clean"
git clone --quiet "$BV_REPO" "$clean" 2>>"$log"
commit="$(git -C "$clean" rev-parse HEAD)"
echo "g5: clone of $commit in $clean" | tee -a "$log"
ref="${against:-$clean/evidence/cost/cost.json}"
[ -f "$ref" ] || { echo "g5: reference $ref missing (commit evidence/cost/cost.json first)" | tee -a "$log" >&2; exit 1; }
docker=0
python3 -I -c 'import json,sys; sys.exit(0 if "docker" in (json.load(open(sys.argv[1])).get("run") or {}).get("guest_build","") else 1)' "$ref" && docker=1
# A reference made from a release set (COST_BINS) is regenerated from the same set: the guests are then the
# deployed, pinned-image ones (their reproducibility is CI's scripts/check_image_ids.sh), and G5 checks the harness.
bins="$(python3 -I -c 'import json,sys; g=(json.load(open(sys.argv[1])).get("run") or {}).get("guest_build",""); print(g[len("release bins "):] if g.startswith("release bins ") else "")' "$ref")"

mkdir -p "$g5home"
set +e
(
  cd "$clean"
  export B="$g5home"            # env.sh keeps an exported B: separate target, tmp and run dirs for the clone
  export BV_GUEST_DOCKER="$docker"
  [ -n "$bins" ] && export COST_BINS="$bins"
  if grep -q 'cost-exec:.*not implemented' Makefile 2>/dev/null || ! grep -q '^cost-exec:' Makefile; then
    echo "g5: the clone's Makefile has no cost-exec target yet; running tools/cost/cost_exec.sh" >&2
    tools/cost/cost_exec.sh
  else
    make cost-exec
  fi
) >> "$log" 2>&1
rc=$?
set -e
echo "g5: cost-exec in the clone exited $rc" | tee -a "$log"

set +e
python3 -I - "$ref" "$clean/evidence/cost/cost.json" "$out/repro.json" "$rc" "$commit" "$started" "$clean" "$docker" "$bins" <<'EOF'
import json, os, subprocess, sys
ref_p, new_p, dst, rc, commit, started, clean, docker, bins = sys.argv[1:10]
COLS = ("user_cycles", "total_cycles_padded", "segments", "gas_exec")
def sh(c):
    return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
res = {"gate": "G5", "commit": commit, "started_utc": started, "reference": ref_p, "regenerated": new_p,
       "cost_exec_exit": int(rc), "guest_build_docker": docker == "1", "release_bins": bins or None,
       "machine": sh("lscpu | sed -n 's/^Model name: *//p'") + ", " + sh("uname -sr") + ", " + sh("r0vm --version")}
fails = []
if rc != "0" or not os.path.exists(new_p):
    fails.append("cost-exec failed in the clean clone")
else:
    ref, new = json.load(open(ref_p)), json.load(open(new_p))
    r_rows = {r["id"]: r for r in ref["rows"]}
    n_rows = {r["id"]: r for r in new["rows"]}
    if not r_rows:
        fails.append("reference has no rows")
    for i in sorted(set(r_rows) | set(n_rows)):
        if i not in n_rows:
            fails.append(f"row {i} missing after regeneration"); continue
        if i not in r_rows:
            fails.append(f"row {i} not in the reference"); continue
        for c in COLS:
            if r_rows[i].get(c) != n_rows[i].get(c):
                fails.append(f"{i}.{c}: reference {r_rows[i].get(c)} != regenerated {n_rows[i].get(c)}")
    ra = {r["step"]: r for r in ref["derived"]["path_a_gas_per_update"]["rows"]}
    na = {r["step"]: r for r in new["derived"]["path_a_gas_per_update"]["rows"]}
    for s in sorted(set(ra) | set(na)):
        if ra.get(s, {}).get("gas_exec") != na.get(s, {}).get("gas_exec"):
            fails.append(f"path A gas {s}: {ra.get(s, {}).get('gas_exec')} != {na.get(s, {}).get('gas_exec')}")
    res["rows_compared"] = len(r_rows)
    res["image_ids"] = {"reference": {k: v.get("image_id") for k, v in (ref.get("guests") or {}).items()},
                        "regenerated": {k: v.get("image_id") for k, v in (new.get("guests") or {}).items()}}
# Real-proof rows must carry raw logs.
missing = []
for root, _, files in os.walk(os.path.join(clean, "evidence", "prove")):
    if "record.json" in files:
        rec = json.load(open(os.path.join(root, "record.json")))
        for k, p in (rec.get("raw") or {}).items():
            q = p if os.path.isabs(p) else os.path.join(clean, p)
            if not os.path.exists(q):
                missing.append(os.path.relpath(q, clean))
if missing:
    fails.append(f"real-proof raw files missing: {missing[:10]}")
res["pass"] = not fails
res["failures"] = fails
json.dump(res, open(dst, "w"), indent=2)
print(json.dumps({"G5": "PASS" if not fails else "FAIL", "rows_compared": res.get("rows_compared"), "failures": fails[:10]}))
sys.exit(0 if not fails else 1)
EOF
st=$?
set -e
[ "$keep" = 1 ] || rm -rf "$clean"
exit $st
