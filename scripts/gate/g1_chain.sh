#!/usr/bin/env bash
# Gate G1, dev-mode half: on the standalone rc3 sequencer seq-dev, `Init` from a real mainnet
# bootstrap, then consecutive updates including committee handoffs, applied through Path A; a replayed Apply (E14) and
# an unlisted verifier (E12) are refused. The chain itself is the test tests/e2e_chain (it asserts every head slot,
# period and record); this script runs it, then re-checks the printed heads and writes evidence/g1/chain.json.
#
#   scripts/gate/g1_chain.sh
#
# Needs: scripts/seq.sh dev start; scripts/deploy_local.sh dev (deploy/ids.toml); funded bv-payer and bv-submitter
# on wallet-dev (see tests/e2e_chain). Exit 0 only if the test passed AND the log shows Init at slot 15605760
# (period 1905), at least three applied updates, at least one handoff (period >= 1906), and both refusals.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
out="$BV_REPO/evidence/g1"
mkdir -p "$out"
log="$out/chain.log"
seqlog="$B/run/seq-dev.log"

"$BV_REPO/scripts/seq.sh" dev status >/dev/null || { echo "G1 chain: seq-dev is not running (scripts/seq.sh dev start)" >&2; exit 1; }

cmd=(nice -n 15 cargo test -p e2e_chain --test e2e_chain -- --ignored --nocapture --test-threads 1
  mainnet_chain_init_three_updates_handoff_and_rejections)
seq_before=$(wc -l < "$seqlog" 2>/dev/null || echo 0)
started="$(date -u +%FT%TZ)"
set +e
( cd "$BV_REPO" && "${cmd[@]}" ) > "$log" 2>&1
rc=$?
set -e
finished="$(date -u +%FT%TZ)"
tail -n +"$((seq_before + 1))" "$seqlog" 2>/dev/null | grep -E 'ESA:E|revert|panick|rejected' | tail -40 > "$out/chain-seq-excerpt.log" || true

python3 -I - "$log" "$out/chain.json" "$rc" "$started" "$finished" "${cmd[*]}" "$out/chain-seq-excerpt.log" <<'EOF'
import json, re, subprocess, sys
log, dst, rc, started, finished, cmd, excerpt = sys.argv[1:8]
txt = open(log, errors="replace").read()
def sh(c):
    try:
        return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
    except Exception:
        return ""
init = re.search(r"^init: instance (\S+) tx (\S+)", txt, re.M)
ups = [{"step": m[0], "tx": m[1], "head_slot": int(m[2]), "period": int(m[3]), "handoff": m[5] == "true"}
       for m in re.findall(r"^(update\[\d\]): tx (\S+) -> head slot (\d+) period (\d+) next known (\w+) \(handoff (\w+)\)", txt, re.M)]
f11 = re.search(r"^F11: fresh account (\S+) submitted (update\[\d\]): tx (\S+) -> period (\d+)", txt, re.M)
e14 = re.search(r"^replayed Apply: rejected with Some\((\w+)\)", txt, re.M)
e12 = re.search(r"^wrong verifier: rejected with Some\((\w+)\)", txt, re.M)
passed = re.search(r"test result: ok\. 1 passed", txt) is not None
checks = {
    "test_exit_0": rc == "0" and passed,
    "init_logged": init is not None,
    "three_updates": len(ups) >= 3,
    "handoff": any(u["period"] >= 1906 for u in ups),
    "slots_increase": all(b["head_slot"] >= a["head_slot"] for a, b in zip(ups, ups[1:])),
    "replay_E14": bool(e14) and e14[1].startswith("E14"),
    "wrong_verifier_E12": bool(e12) and e12[1].startswith("E12"),
}
ok = all(checks.values())
doc = {
    "gate": "G1 (dev-mode chain half)", "pass": ok, "checks": checks, "test_exit_code": int(rc),
    "started_utc": started, "finished_utc": finished, "command": cmd, "risc0_dev_mode": "1",
    "lez": "v0.4.0-rc3", "sequencer": "seq-dev (standalone, mock Bedrock, 15 s blocks)",
    "fixtures": "fixtures/mainnet: bootstrap_15605760 (period 1905), updates_1905_4",
    "instance": init[1] if init else None, "init_tx": init[2] if init else None, "updates": ups,
    "f11": {"account": f11[1], "step": f11[2], "tx": f11[3], "period": int(f11[4])} if f11 else None,
    "machine": sh("lscpu | sed -n 's/^Model name: *//p'") + ", " + sh("uname -sr"),
    "repo_commit": sh('git -C "$BV_REPO" rev-parse --short HEAD'),
    "raw_log": "evidence/g1/chain.log", "sequencer_log_excerpt": open(excerpt, errors="replace").read().splitlines(),
}
json.dump(doc, open(dst, "w"), indent=2)
print(json.dumps({"G1_chain": "PASS" if ok else "FAIL", "checks": checks}))
sys.exit(0 if ok else 1)
EOF
