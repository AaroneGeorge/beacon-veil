#!/usr/bin/env bash
# `make cost-exec`: deterministic cycles and gas, from the RISC Zero 3.0.5 executor on real fixtures, into
# evidence/cost/{cost.json,cost.md} with every raw output in evidence/cost/raw/. No sequencer, no proof, no network.
#
#   tools/cost/cost_exec.sh                 # build the guests (crates/guest_methods), then measure
#   COST_NO_GUEST_BUILD=1 tools/cost/cost_exec.sh   # measure the guest ELFs already in the per-guest target dirs
#   BV_GUEST_DOCKER=1 tools/cost/cost_exec.sh       # guests from the pinned image r0.1.91.1 (reproducible image ids)
#
# Harnesses (each writes raw JSON + a log):
#   cost exec-lc     (tools/cost)         lc_verify Verify V1 per mainnet step; attest Init, Apply, WriteRecord, CheckHeader
#   cost sizes       (tools/cost)         borsh sizes of stored states and inputs
#   attest_exec      (tests/e2e_consumer)      attest Attest (log, storage, account), consumer_erc20 Statement, MarkConsumed
#   cost exec-v3     (tools/cost/exec_v3.sh)   lc_verify_v3 per step (Begin, Miller, Finish; with COST_BINS only: the
#                                              release set's lc_verify_v3 and attest, plus a local measurement build)
# Optional raw files dropped into evidence/cost/raw/ by hand are kept and merged (b0-phases.json, ...).
# Cycles depend only on the guest image and the input; the image ids are recorded in cost.json.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"
cd "$BV_REPO"
raw="$BV_REPO/evidence/cost/raw"
mkdir -p "$raw"
started="$(date -u +%FT%TZ)"

# Guests: a release set (scripts/guests_release.sh output: DIR/elf/<name> user ELFs, IMAGE_IDS), so the image ids are
# the deployed ones. Default: the set of the commit that guests/IMAGE_IDS.expected names for lc_verify_v3, looked up
# under COST_RELEASE_DIR (default $B/guests-release); build it first with `scripts/guests_release.sh <commit>`.
#   COST_RELEASE=<commit>   another set under COST_RELEASE_DIR
#   COST_BINS=DIR           a set at any path
#   COST_LOCAL_GUESTS=1     the old route: build the guests with crates/guest_methods (no lc_verify_v3 rows)
if [ -z "${COST_BINS:-}" ] && [ "${COST_LOCAL_GUESTS:-0}" != 1 ]; then
  release="${COST_RELEASE:-$(sed -n 's/^# lc_verify_v3 .*release set of commit \([0-9a-f]\{40\}\).*/\1/p' guests/IMAGE_IDS.expected | head -1)}"
  [ -n "$release" ] || { echo "cost-exec: no release commit in guests/IMAGE_IDS.expected (set COST_RELEASE)" >&2; exit 1; }
  COST_BINS="${COST_RELEASE_DIR:-$B/guests-release}/$release"
  [ -f "$COST_BINS/SHA256SUMS" ] || { echo "cost-exec: release set $release not built: scripts/guests_release.sh $release (or COST_BINS=DIR, or COST_LOCAL_GUESTS=1)" >&2; exit 1; }
fi
if [ -n "${COST_BINS:-}" ]; then
  # The set must be the one the repo expects.
  scripts/check_image_ids.sh "$COST_BINS" > /dev/null || { scripts/check_image_ids.sh "$COST_BINS" >&2; echo "cost-exec: $COST_BINS does not match guests/IMAGE_IDS.expected" >&2; exit 1; }
  release_commit="$(sed -n 's/^commit=//p' "$COST_BINS/BUILD_INFO" | head -1)"
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
  echo "cost-exec: attest_exec did not build; its rows are skipped (see raw/harness-build.log)" >&2
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

# lc_verify_v3 (the staged public path): from the release set only, so its image id is the deployed one.
if [ -n "${COST_BINS:-}" ] && [ -f "$COST_BINS/elf/lc_verify_v3" ]; then
  echo "cost-exec: exec-v3"
  BV_LOCK_HELD="${BV_LOCK_HELD:-0}" BENCH_TARGET_DIR="$CARGO_TARGET_DIR" EXEC_V3_OUT="$raw/exec-v3.json" \
    "$here/exec_v3.sh" > "$raw/exec-v3.log" 2>&1 || { tail -20 "$raw/exec-v3.log" >&2; exit 1; }
fi

# Paths in what this run wrote: repo-relative, ${BUILD} for the build folder, ~ for the home directory.
scrub() {
  for f in "$@"; do
    [ -f "$f" ] && sed -i -e "s#$BV_REPO/##g" -e "s#$B/#\${BUILD}/#g" -e "s#$HOME#~#g" "$f"
  done
  return 0
}
release_label=""
if [ -n "${release_commit:-}" ]; then
  release_label="release set $release_commit (scripts/guests_release.sh $release_commit, \${BUILD}/guests-release/$release_commit)"
fi
python3 -I - "$raw/run-meta.json" "$started" "$release_label" <<EOF
import json, subprocess, sys
def sh(c):
    return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
release = sys.argv[3]
json.dump({
    "command": "make cost-exec (tools/cost/cost_exec.sh)", "started_utc": sys.argv[2],
    "guest_build": release if release else ("release bins ${COST_BINS:-}" if "${COST_BINS:-}" else "docker r0.1.91.1" if "${BV_GUEST_DOCKER:-0}" == "1" else ("prebuilt (COST_NO_GUEST_BUILD)" if "${COST_NO_GUEST_BUILD:-0}" == "1" else "local rzup toolchain")),
    "release_commit": "${release_commit:-}" or None,
    "guests": {"lc_verify": "$lc", "attest": "$at", "consumer_erc20": "$co"},
    # Dirty = source changes; the evidence this run writes does not count.
    "repo_commit": sh("git -C $BV_REPO rev-parse --short HEAD") + (" (dirty)" if sh("git -C $BV_REPO status --porcelain -- . ':!evidence'") else ""),
    "risc0_dev_mode": "${RISC0_DEV_MODE:-unset}", "r0vm": sh("r0vm --version"),
}, open(sys.argv[1], "w"), indent=2)
EOF
scrub "$raw/run-meta.json" "$raw/exec-lc.json" "$raw/exec-lc.log" "$raw/sizes.json" "$raw/sizes.log" "$raw/attest-exec.json" \
  "$raw/attest-exec.log" "$raw/harness-build.log" "$raw/guest-build.log" "$raw/exec-v3.json" "$raw/exec-v3.log"
# The cycle-trace log (committed raw file; a separate measurement build of lc_verify) -> per-phase table.
if [ -f "$raw/w1-v1-trace.log" ]; then
  python3 -I "$here/cost.py" import-trace --log "$raw/w1-v1-trace.log" --out "$raw/b0-phases.json"
fi
python3 -I "$here/cost.py" build --raw "$raw" --out "$BV_REPO/evidence/cost"
