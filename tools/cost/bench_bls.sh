#!/usr/bin/env bash
# `make bench-bls`: BLS phase and whole-update cycles, zkcrypto against blst, into evidence/public-path/bench/
# {phases.json,bench.md}. Executor only: no sequencer, no proof, no network.
#
#   make bench-bls                      # guest built with the local rzup toolchain
#   BENCH_ROUTE=docker make bench-bls   # guest built in risczero/risc0-guest-builder:r0.1.91.1 from the committed tree
#   BENCH_NO_GUEST_BUILD=1 ...          # measure the guest already built for that route
#
# BENCH_TARGET_DIR: cargo target dir for the guest and the cost tool (default: scripts/env.sh's CARGO_TARGET_DIR).
# Builds take $B/run/build.lock (one heavy build at a time on this machine) and refuse to start while a proof job
# holds $B/run/prove.lock.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"
cd "$BV_REPO"
route="${BENCH_ROUTE:-local}"
export BENCH_TARGET_DIR="${BENCH_TARGET_DIR:-$CARGO_TARGET_DIR}"
out="$BV_REPO/evidence/public-path/bench"
mkdir -p "$B/run" "$out"
lock="$B/run/build.lock"
if [ -e "$B/run/prove.lock" ] && ! flock -n "$B/run/prove.lock" true; then
  echo "bench-bls: a proof job holds $B/run/prove.lock; no builds now" >&2; exit 1
fi
if [ "${BENCH_NO_GUEST_BUILD:-0}" != 1 ]; then
  flock "$lock" guests/bls_bench/build.sh "$route"
fi
elf="$BENCH_TARGET_DIR/bench-guest/$route/bls_bench"
[ -f "$elf" ] || { echo "bench-bls: missing guest $elf" >&2; exit 1; }
flock "$lock" env CARGO_TARGET_DIR="$BENCH_TARGET_DIR" RISC0_SKIP_BUILD=1 nice -n 15 cargo build -q --locked -p cost --bin cost
nice -n 15 "$BENCH_TARGET_DIR/debug/cost" bench-bls --elf "$elf" --out "$out"
