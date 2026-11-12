#!/usr/bin/env bash
# `make cost-exec` (W6): deterministic cycles and gas, from the RISC Zero 3.0.5 executor on real fixtures, into
# evidence/cost/{cost.json,cost.md} with every raw output in evidence/cost/raw/. No sequencer, no proof, no network.
#
#   tools/cost/cost_exec.sh                 # build the guests (crates/guest_methods), then measure
#   COST_NO_GUEST_BUILD=1 tools/cost/cost_exec.sh   # measure the guest ELFs already in the per-guest target dirs
#   BV_GUEST_DOCKER=1 tools/cost/cost_exec.sh       # guests from the pinned image r0.1.91.1 (reproducible image ids)
#
# Harnesses (each writes raw JSON + a log):
#   cost exec-lc     (tools/cost, W6)   lc_verify Verify V1 per mainnet step; attest Init, Apply, WriteRecord, CheckHeader
#   cost sizes       (tools/cost, W6)   borsh sizes of stored states and inputs
#   attest_exec      (tests/e2e_consumer, W3)  attest Attest (log, storage, account), consumer_erc20 Statement, MarkConsumed
# Optional raw files other tracks drop into evidence/cost/raw/ by hand are kept and merged (b0-phases.json, ...).
# Cycles depend only on the guest image and the input; the image ids are recorded in cost.json.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"
cd "$BV_REPO"
raw="$BV_REPO/evidence/cost/raw"
mkdir -p "$raw"
started="$(date -u +%FT%TZ)"

# COST_BINS=DIR: measure a release set (scripts/guests_release.sh output: DIR/elf/<name> user ELFs, IMAGE_IDS) instead
# of building; the image ids then equal the deployed ones.
if [ -n "${COST_BINS:-}" ]; then
  COST_NO_GUEST_BUILD=1
  lc="$COST_BINS/elf/lc_verify"; at="$COST_BINS/elf/attest"; co="$COST_BINS/elf/consumer_erc20"; lc2="$COST_BINS/elf/lc_verify_v2"
fi
gdir() { echo "$CARGO_TARGET_DIR/riscv-guest/guest_methods/$1/riscv32im-risc0-zkvm-elf/release/$2"; }
if [ "${COST_NO_GUEST_BUILD:-0}" != 1 ]; then
  echo "cost-exec: building guests (BV_GUEST_DOCKER=${BV_GUEST_DOCKER:-0})"
  nice -n 15 cargo build -p guest_methods > "$raw/guest-build.log" 2>&1 || { tail -30 "$raw/guest-build.log" >&2; exit 1; }
  if [ "${BV_GUEST_DOCKER:-0}" = 1 ]; then
    scripts/collect_guests.sh attest lc_verify consumer_erc20 > /dev/null
    lc="$CARGO_TARGET_DIR/guests/lc_verify.bin"; at="$CARGO_TARGET_DIR/guests/attest.bin"; co="$CARGO_TARGET_DIR/guests/consumer_erc20.bin"
  fi
fi
lc="${lc:-$(gdir lc_verify_program lc_verify)}"
at="${at:-$(gdir attest_program attest)}"
co="${co:-$(gdir consumer_erc20_program consumer_erc20)}"
for f in "$lc" "$at"; do [ -f "$f" ] || { echo "cost-exec: missing guest $f" >&2; exit 1; }; done

echo "cost-exec: building harnesses"
RISC0_SKIP_BUILD=1 nice -n 15 cargo build -p cost --bin cost > "$raw/harness-build.log" 2>&1 \
  || { tail -30 "$raw/harness-build.log" >&2; exit 1; }
have_attest_exec=0
if RISC0_SKIP_BUILD=1 nice -n 15 cargo build -p e2e_consumer --bin attest_exec >> "$raw/harness-build.log" 2>&1; then
  have_attest_exec=1
else
  echo "cost-exec: attest_exec (W3) did not build; its rows are skipped (see raw/harness-build.log)" >&2
fi

bin="$CARGO_TARGET_DIR/debug"
echo "cost-exec: exec-lc"
nice -n 15 "$bin/cost" exec-lc --lc-elf "$lc" --attest-elf "$at" ${lc2:+--lc2-elf "$lc2"} --json "$raw/exec-lc.json" > "$raw/exec-lc.log" 2>&1 \
  || { tail -20 "$raw/exec-lc.log" >&2; exit 1; }
nice -n 15 "$bin/cost" sizes --json "$raw/sizes.json" > "$raw/sizes.log" 2>&1
if [ "$have_attest_exec" = 1 ] && [ -f "$co" ]; then
  echo "cost-exec: attest_exec"
  nice -n 15 "$bin/attest_exec" --attest-elf "$at" --consumer-elf "$co" --json "$raw/attest-exec.json" \
    > "$raw/attest-exec.log" 2>&1 || { echo "cost-exec: attest_exec failed (rows skipped)" >&2; rm -f "$raw/attest-exec.json"; }
fi

python3 -I - "$raw/run-meta.json" "$started" <<EOF
import json, subprocess, sys
def sh(c):
    return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
json.dump({
    "command": "make cost-exec (tools/cost/cost_exec.sh)", "started_utc": sys.argv[2],
    "guest_build": "release bins ${COST_BINS:-}" if "${COST_BINS:-}" else "docker r0.1.91.1" if "${BV_GUEST_DOCKER:-0}" == "1" else ("prebuilt (COST_NO_GUEST_BUILD)" if "${COST_NO_GUEST_BUILD:-0}" == "1" else "local rzup toolchain"),
    "guests": {"lc_verify": "$lc", "attest": "$at", "consumer_erc20": "$co"},
    "repo_commit": sh("git -C $BV_REPO rev-parse --short HEAD") + (" (dirty)" if sh("git -C $BV_REPO status --porcelain") else ""),
    "risc0_dev_mode": "${RISC0_DEV_MODE:-unset}", "r0vm": sh("r0vm --version"),
}, open(sys.argv[1], "w"), indent=2)
EOF
# W1's cycle-trace log (committed raw file; a separate measurement build of lc_verify) -> per-phase table.
if [ -f "$raw/w1-v1-trace.log" ]; then
  python3 -I "$here/cost.py" import-trace --log "$raw/w1-v1-trace.log" --out "$raw/b0-phases.json"
fi
python3 -I "$here/cost.py" build --raw "$raw" --out "$BV_REPO/evidence/cost"
