#!/usr/bin/env bash
# Gate G5: the measured cost table is reproducible from a clean clone.
#   1. fresh `git clone` of the committed HEAD into $B/.tmp/clean (uncommitted work is not part of the clone);
#   2. `make cost-exec` there, with its own empty target dir ($B/.tmp/g5/target: a cold build, nothing reused);
#   3. exact diff of the cycle and gas columns against the committed evidence/cost/cost.json, read with
#      `git show HEAD:evidence/cost/cost.json` into a file OUTSIDE the clone before cost-exec runs (cost-exec rewrites
#      the clone's copy), plus a self-test: the same comparison on a copy of the reference with one cycle count
#      changed must fail;
#   4. every real-proof record under evidence/prove/ in the clone has its raw files (log, time, RSS samples).
#
#   scripts/gate/g5_repro.sh [--against FILE] [--keep] [--reuse-target]
#
# The clone's build folder ($B/.tmp/g5: target dir, tmp and run dirs) is deleted before every run, so every build is
# cold; --reuse-target keeps it (faster, not a cold build) and repro.json then says "cold_build": false. The cargo
# download cache (~/.cargo registry and git checkouts) is shared: sources, not build outputs.
# --against: reference cost.json (default: the committed evidence/cost/cost.json of the cloned commit).
# Guests: the release set named in the reference (`run.release_commit`), looked up under $B/guests-release; build it
# first with `scripts/guests_release.sh <commit>` if it is missing. A reference without a release set takes the
# guest build mode from `run.guest_build` (docker if the reference used the pinned image).
# Writes evidence/g5/repro.json (+ repro.log). Exit 0 only on an exact match.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
against="" keep=0 reuse=0
while [ $# -gt 0 ]; do
  case "$1" in
    --against) against="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    --reuse-target) reuse=1; shift ;;
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
if [ -n "$against" ]; then
  ref="$against"
else
  ref="$B/.tmp/g5-reference.json"
  git -C "$clean" show HEAD:evidence/cost/cost.json > "$ref" 2>>"$log" \
    || { echo "g5: no committed evidence/cost/cost.json at $commit" | tee -a "$log" >&2; exit 1; }
fi
[ -s "$ref" ] || { echo "g5: reference $ref missing or empty" | tee -a "$log" >&2; exit 1; }
docker=0
python3 -I -c 'import json,sys; sys.exit(0 if "docker" in (json.load(open(sys.argv[1])).get("run") or {}).get("guest_build","") else 1)' "$ref" && docker=1
# A reference made from a release set (COST_BINS) is regenerated from the same set: the guests are then the
# deployed, pinned-image ones (their reproducibility is CI's scripts/check_image_ids.sh), and G5 checks the harness.
release="$(python3 -I -c 'import json,sys; print((json.load(open(sys.argv[1])).get("run") or {}).get("release_commit") or "")' "$ref")"
bins=""
if [ -n "$release" ]; then
  bins="$B/guests-release/$release"
  [ -f "$bins/SHA256SUMS" ] || { echo "g5: release set $release missing: scripts/guests_release.sh $release" | tee -a "$log" >&2; exit 1; }
else
  bins="$(python3 -I -c 'import json,sys; g=(json.load(open(sys.argv[1])).get("run") or {}).get("guest_build",""); print(g[len("release bins "):] if g.startswith("release bins ") else "")' "$ref")"
fi

cold=1
if [ "$reuse" = 1 ] && [ -d "$g5home/target" ]; then
  cold=0
else
  rm -rf "$g5home"
fi
mkdir -p "$g5home"
t0=$(date +%s)
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
build_s=$(( $(date +%s) - t0 ))
echo "g5: cost-exec in the clone exited $rc after ${build_s} s (cold build: $cold)" | tee -a "$log"

set +e
BV_BUILD_ROOT="$B" python3 -I - "$ref" "$clean/evidence/cost/cost.json" "$out/repro.json" "$rc" "$commit" "$started" "$clean" "$docker" "$bins" "$cold" "$build_s" "${CARGO_BUILD_JOBS:-}" <<'EOF'
import json, os, subprocess, sys
ref_p, new_p, dst, rc, commit, started, clean, docker, bins, cold, build_s, jobs = sys.argv[1:13]
COLS = ("user_cycles", "total_cycles_padded", "segments", "gas_exec")
def sh(c):
    return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
res = {"gate": "G5", "commit": commit, "started_utc": started, "reference": ref_p,
       "reference_source": f"git show {commit}:evidence/cost/cost.json, saved outside the clone before cost-exec",
       "regenerated": new_p,
       "cost_exec_exit": int(rc), "guest_build_docker": docker == "1", "release_bins": bins or None,
       "cold_build": cold == "1", "build_seconds": int(build_s), "cargo_build_jobs": jobs or None,
       "build_note": "cold: the clone's target dir was empty (scripts/gate/g5_repro.sh deletes it first); the guests of the release set are prebuilt pinned-image ELFs (their reproducibility is CI's image-id check); the lc_verify_v3 measurement build is compiled in the clone",
       "machine": sh("lscpu | sed -n 's/^Model name: *//p'") + ", " + sh("uname -sr") + ", " + sh("r0vm --version")}
fails = []
if rc != "0" or not os.path.exists(new_p):
    fails.append("cost-exec failed in the clean clone")
else:
    import copy
    ref, new = json.load(open(ref_p)), json.load(open(new_p))

    def compare(ref, new):
        out = []
        r_rows = {r["id"]: r for r in ref["rows"]}
        n_rows = {r["id"]: r for r in new["rows"]}
        if not r_rows:
            out.append("reference has no rows")
        for i in sorted(set(r_rows) | set(n_rows)):
            if i not in n_rows:
                out.append(f"row {i} missing after regeneration"); continue
            if i not in r_rows:
                out.append(f"row {i} not in the reference"); continue
            for c in COLS:
                if r_rows[i].get(c) != n_rows[i].get(c):
                    out.append(f"{i}.{c}: reference {r_rows[i].get(c)} != regenerated {n_rows[i].get(c)}")
        ra = {r["step"]: r for r in ref["derived"]["path_a_gas_per_update"]["rows"]}
        na = {r["step"]: r for r in new["derived"]["path_a_gas_per_update"]["rows"]}
        for s in sorted(set(ra) | set(na)):
            if ra.get(s, {}).get("gas_exec") != na.get(s, {}).get("gas_exec"):
                out.append(f"path A gas {s}: {ra.get(s, {}).get('gas_exec')} != {na.get(s, {}).get('gas_exec')}")
        return out

    fails += compare(ref, new)
    # The regenerated table must come from the same guest images, and must be a different file from the reference.
    for k in sorted(set(ref.get("guests") or {}) | set(new.get("guests") or {})):
        a = ((ref.get("guests") or {}).get(k) or {}).get("image_id")
        b = ((new.get("guests") or {}).get(k) or {}).get("image_id")
        if a != b:
            fails.append(f"image id of {k}: reference {a} != regenerated {b}")
    if os.path.realpath(ref_p) == os.path.realpath(new_p):
        fails.append("the reference and the regenerated table are the same file")
    # Self-test: the comparison must see a single changed cycle count.
    probe = copy.deepcopy(ref)
    target = probe["rows"][-1]
    target["user_cycles"] = (target["user_cycles"] or 0) + 1
    caught = compare(probe, new)
    res["self_test"] = {"changed": f"{target['id']}.user_cycles + 1", "detected": bool(caught),
                        "first_failure": caught[0] if caught else None}
    if not caught:
        fails.append("self-test: a changed cycle count was not detected")
    r_rows = {r["id"]: r for r in ref["rows"]}
    res["rows_compared"] = len(r_rows)
    by_prog = {}
    for r in r_rows.values():
        by_prog[r["program"]] = by_prog.get(r["program"], 0) + 1
    res["rows_by_program"] = by_prog
    if "lc_verify_v3" not in by_prog:
        fails.append("no lc_verify_v3 rows in the reference (cost-exec with COST_BINS of a release set that has lc_verify_v3)")
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
# Paths: ${BUILD} for the build folder, ~ for the home directory (no machine path in the evidence).
build, home = os.environ.get("BV_BUILD_ROOT", ""), os.path.expanduser("~")
def clean_paths(v):
    if isinstance(v, str):
        return (v.replace(build + "/", "${BUILD}/") if build else v).replace(home, "~")
    if isinstance(v, dict):
        return {k: clean_paths(x) for k, x in v.items()}
    if isinstance(v, list):
        return [clean_paths(x) for x in v]
    return v
res = clean_paths(res)
json.dump(res, open(dst, "w"), indent=2)
print(json.dumps({"G5": "PASS" if not fails else "FAIL", "rows_compared": res.get("rows_compared"), "rows_by_program": res.get("rows_by_program"), "self_test": res.get("self_test", {}).get("detected"), "failures": fails[:10]}))
sys.exit(0 if not fails else 1)
EOF
st=$?
set -e
sed -i -e "s#$B/#\${BUILD}/#g" -e "s#$HOME#~#g" "$log"
[ "$keep" = 1 ] || rm -rf "$clean"
exit $st
