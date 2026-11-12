#!/usr/bin/env bash
# `make gate` (W6): runs the objective checks of PLAN.md section 1, G1 to G5, and writes evidence/gate/summary.json.
# G6 (signatures in TEAM.md) is the owner's confirmation and is only reported as present or not.
#
#   scripts/gate/gate.sh [--quick]
#
#   G1  scripts/gate/g1_chain.sh (seq-dev, mainnet fixtures) + g1_real_proof.sh --verify (evidence/g1/real-proof.json)
#   G2  evidence/g2/ci-run.txt holds the URL of a green GitHub Actions run on the default branch (checked with gh)
#   G3  scripts/gate/g3_consumer.sh (W3)
#   G4  python3 scripts/gate/g4_check.py evidence/g4/follower.jsonl (W4)
#   G5  scripts/gate/g5_repro.sh (clean clone, cold build: slow)
#   --quick: G1 and G3 re-check their existing evidence JSON instead of re-running on seq-dev; G5 re-checks
#            evidence/g5/repro.json instead of a new clean-clone build.
# Each check's own output goes to evidence/gate/<check>.log. Exit 0 only if G1 to G5 all pass.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
quick=0; [ "${1:-}" = "--quick" ] && quick=1
out="$BV_REPO/evidence/gate"; mkdir -p "$out"
declare -A res

json_pass() { # file key: exit 0 if JSON file has key == true
  python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get(sys.argv[2]) is True else 1)' "$1" "$2" 2>/dev/null
}
run() { # name, command...
  local name="$1"; shift
  if "$@" > "$out/$name.log" 2>&1; then res[$name]=PASS; else res[$name]=FAIL; fi
  echo "$name: ${res[$name]} (log evidence/gate/$name.log)"
}

if [ "$quick" = 1 ]; then
  run G1_chain json_pass "$BV_REPO/evidence/g1/chain.json" pass
else
  run G1_chain "$here/g1_chain.sh"
fi
run G1_real "$here/g1_real_proof.sh" --verify

g2() {
  local f="$BV_REPO/evidence/g2/ci-run.txt"
  [ -s "$f" ] || { echo "missing $f"; return 1; }
  local url; url="$(grep -Eo 'https://github.com/[^ ]+/actions/runs/[0-9]+' "$f" | head -1)"
  [ -n "$url" ] || { echo "no Actions run URL in $f"; return 1; }
  echo "run: $url"
  command -v gh >/dev/null || { echo "gh not available: URL present, status not checked"; return 1; }
  local repo id; repo="$(echo "$url" | sed -E 's#https://github.com/([^/]+/[^/]+)/.*#\1#')"; id="${url##*/}"
  gh run view "$id" -R "$repo" --json conclusion,headBranch,event,url | tee /dev/stderr \
    | python3 -I -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d.get("conclusion")=="success" else 1)'
}
run G2 g2

if [ "$quick" = 1 ]; then
  run G3 json_pass "$BV_REPO/evidence/g3/run.json" pass
else
  run G3 "$here/g3_consumer.sh"
fi
run G4 python3 -I "$here/g4_check.py" "$BV_REPO/evidence/g4/follower.jsonl" --summary-out "$out/g4-summary.json"
if [ "$quick" = 1 ]; then
  run G5 json_pass "$BV_REPO/evidence/g5/repro.json" pass
else
  run G5 "$here/g5_repro.sh"
fi
if grep -qi "I will sign this proposal" "$BV_REPO/TEAM.md" 2>/dev/null; then res[G6]="present (owner confirms)"; else res[G6]="missing"; fi
echo "G6: ${res[G6]}"

python3 -I - "$out/summary.json" "$quick" "${res[G1_chain]}" "${res[G1_real]}" "${res[G2]}" "${res[G3]}" "${res[G4]}" "${res[G5]}" "${res[G6]}" <<'EOF'
import datetime, json, subprocess, sys
dst, quick, g1c, g1r, g2, g3, g4, g5, g6 = sys.argv[1:10]
def sh(c): return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
d = {"generated_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
     "mode": "quick (re-check evidence)" if quick == "1" else "full (re-run)",
     "repo_commit": sh('git -C "$BV_REPO" rev-parse --short HEAD'),
     "G1": "PASS" if g1c == g1r == "PASS" else "FAIL", "G1_chain": g1c, "G1_real_proof": g1r,
     "G2": g2, "G3": g3, "G4": g4, "G5": g5, "G6": g6}
d["G1_to_G5"] = "PASS" if all(d[k] == "PASS" for k in ("G1", "G2", "G3", "G4", "G5")) else "FAIL"
json.dump(d, open(dst, "w"), indent=2)
print(json.dumps(d, indent=1))
sys.exit(0 if d["G1_to_G5"] == "PASS" else 1)
EOF
