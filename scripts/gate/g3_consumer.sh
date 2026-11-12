#!/usr/bin/env bash
# Gate G3 (PLAN.md section 1): one consumer attestation lands in the consumer's own transaction and a replay is refused.
#   tx 1: Attest(mainnet USDT Transfer, block 26397891) -> consumer_erc20 accepts, its state changes, nullifier written
#   tx 2: identical -> refused with E11_ALREADY_CONSUMED (simulation), and when sent anyway reverted by the sequencer
# Writes evidence/g3/run.json (+ run.log). Exit 0 only if both hold.
#
# Needs: scripts/seq.sh dev start; attest, lc_verify and consumer_erc20 deployed on seq-dev (consumer_erc20 id in
# $BV_CONSUMER_ERC20 or [programs] consumer_erc20 in the ids file); funded bv-payer and bv-submitter.
# Optional: BV_G3_WALLET (default $B/run/wallet-w3); BV_E2E_INSTANCE=<instance with a record at beacon slot 15637568> to skip building a fresh instance.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
# Own wallet home (W0 rule: wallet storage is not safe for concurrent writers). Needs bv-payer and bv-submitter in it.
export LEE_WALLET_HOME_DIR="${BV_G3_WALLET:-$B/run/wallet-w3}"
export LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
# Own public payer (W0 rule: bv-payer only funds track payers). Default: the id saved when wallet-w3 was set up.
if [ -z "${BV_PAYER:-}" ] && [ -f "$LEE_WALLET_HOME_DIR/w3-payer.id" ]; then
  BV_PAYER="$(cat "$LEE_WALLET_HOME_DIR/w3-payer.id")"; export BV_PAYER
fi
[ -n "${BV_PAYER:-}" ] || { echo "G3: set BV_PAYER (a funded public account in $LEE_WALLET_HOME_DIR)" >&2; exit 1; }
out="$BV_REPO/evidence/g3"
mkdir -p "$out"
log="$out/run.log"
seqlog="$B/run/seq-dev.log"

"$BV_REPO/scripts/seq.sh" dev status >/dev/null || { echo "G3: seq-dev is not running (scripts/seq.sh dev start)" >&2; exit 1; }

cmd=(nice -n 15 cargo test -p e2e_consumer --test e2e_consumer -- --ignored --nocapture --test-threads 1
  g3_consumer_accepts_once_and_refuses_replay)
seq_lines_before=$(wc -l < "$seqlog" 2>/dev/null || echo 0)
started="$(date -u +%FT%TZ)"
set +e
"${cmd[@]}" > "$log" 2>&1
rc=$?
set -e
finished="$(date -u +%FT%TZ)"
excerpt="$(tail -n +"$((seq_lines_before + 1))" "$seqlog" 2>/dev/null | grep -E 'ESA:E|revert|panick' | tail -20 || true)"

python3 -I - "$log" "$out/run.json" "$rc" "$started" "$finished" "${cmd[*]}" "$excerpt" <<'EOF'
import json, platform, subprocess, sys
log, dst, rc, started, finished, cmd, excerpt = sys.argv[1:8]
res = None
for line in open(log, errors="replace"):
    if line.startswith("G3RESULT "):
        res = json.loads(line[len("G3RESULT "):])
def sh(c):
    try:
        return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
    except Exception:
        return ""
ok = (rc == "0" and res is not None and res["tx1"]["accepted"]
      and res["tx1"]["consumer_count_after"] == res["tx1"]["consumer_count_before"] + 1
      and res["tx2_simulated"]["code"] == "E11_ALREADY_CONSUMED" and not res["tx2_sent"]["accepted"])
doc = {
    "gate": "G3", "pass": ok, "test_exit_code": int(rc), "started_utc": started, "finished_utc": finished,
    "command": cmd, "risc0_dev_mode": "1", "lez": "v0.4.0-rc3", "sequencer": "seq-dev (standalone)",
    "machine": sh("uname -srm") + "; " + sh("lscpu | sed -n 's/^Model name: *//p'") + "; " + sh("free -g | awk '/Mem:/{print $2\" GiB RAM\"}'"),
    "repo_commit": sh("git -C \"$BV_REPO\" rev-parse --short HEAD 2>/dev/null"),
    "result": res, "sequencer_log_excerpt": excerpt.splitlines(), "raw_log": "evidence/g3/run.log",
}
json.dump(doc, open(dst, "w"), indent=2)
print(json.dumps({"G3": "PASS" if ok else "FAIL", "run_json": dst}))
sys.exit(0 if ok else 1)
EOF
