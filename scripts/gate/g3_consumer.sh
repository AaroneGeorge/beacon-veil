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

python3 -I - "$log" "$out/run.json" "$rc" "$started" "$finished" "${cmd[*]}" "$excerpt" "${BV_IDS:-$BV_REPO/deploy/ids.toml}" <<'EOF'
import json, os, platform, re, subprocess, sys
log, dst, rc, started, finished, cmd, excerpt, ids_path = sys.argv[1:9]
res = None
for line in open(log, errors="replace"):
    if line.startswith("G3RESULT "):
        res = json.loads(line[len("G3RESULT "):])
def deployed(ids_path):
    """Program ids, binary sha256 (from deploy_local.sh comments), bins dir and image id (IMAGE_IDS in that dir)."""
    progs, bins, section = {}, {}, None
    for line in open(ids_path):
        m = re.match(r"\[(\w+)\]", line)
        if m:
            section = m.group(1)
            continue
        m = re.match(r'(\w+) = "([^"]+)"(.*)', line)
        if not m:
            continue
        name, val, rest = m.groups()
        if section == "programs":
            sha = re.search(r"sha256 ([0-9a-f]{64})", rest)
            progs[name] = {"id": val, "bin_sha256": sha.group(1) if sha else None}
        elif section == "bins":
            bins[name] = val
    out = {}
    for name in ("attest", "lc_verify", "consumer_erc20"):
        if name not in progs:
            continue
        e = dict(progs[name])
        d = bins.get(name)
        e["bins_dir"] = d
        image = None
        if d and os.path.exists(os.path.join(d, "IMAGE_IDS")):
            for l in open(os.path.join(d, "IMAGE_IDS")):
                m = re.match(rf"{name}\.bin: .*image id ([0-9a-f]{{64}})", l)
                if m:
                    image = m.group(1)
        e["image_id"] = image
        if d and os.path.exists(os.path.join(d, "BUILD_INFO")):
            for l in open(os.path.join(d, "BUILD_INFO")):
                if l.startswith("commit=") or l.startswith("image="):
                    k, v = l.strip().split("=", 1)
                    e["release_" + k] = v
        out[name] = e
    return out
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
    "deployed": deployed(ids_path),
    "result": res, "sequencer_log_excerpt": excerpt.splitlines(), "raw_log": "evidence/g3/run.log",
}
json.dump(doc, open(dst, "w"), indent=2)
print(json.dumps({"G3": "PASS" if ok else "FAIL", "run_json": dst}))
sys.exit(0 if ok else 1)
EOF
