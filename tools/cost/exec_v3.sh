#!/usr/bin/env bash
# `make exec-v3`: every step of lc_verify_v3 (Begin, Miller, Finish; BeginPrepare and BeginKeys for the four-step
# layout) in the RISC Zero 3.0.5 executor on real mainnet and Sepolia updates and fabricated 342-signer updates, with
# the chained attest Apply and WriteRecord, into evidence/public-path/exec-steps.json. No sequencer, no proof.
#
#   COST_BINS=$B/guests-release/<commit> make exec-v3   # deployed ELFs from the release set (scripts/guests_release.sh)
#   make exec-v3                                        # lc_verify_v3 and attest built with the local toolchain
#
# The measurement build of lc_verify_v3 (--features cycle-trace, phase marks) is always built locally; its phases are
# reported next to the deployed build's metered cycles. BENCH_TARGET_DIR: cargo target dir (default CARGO_TARGET_DIR).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"
cd "$BV_REPO"
tdir="${BENCH_TARGET_DIR:-$CARGO_TARGET_DIR}"
export GUEST_TARGET_DIR="$tdir/riscv-guest-one"
lock="$B/run/build.lock"
mkdir -p "$B/run"
if [ -e "$B/run/prove.lock" ] && ! flock -n "$B/run/prove.lock" true; then
  echo "exec-v3: a proof job holds $B/run/prove.lock; no builds now" >&2; exit 1
fi
g="$tdir/v3-guests"
flock "$lock" env GUEST_CARGO_CONFIG="$BV_V3_CARGO_CONFIG" scripts/build_guest.sh programs/lc_verify_v3/Cargo.toml lc_verify_v3 "$g/trace" cycle-trace
if [ -n "${COST_BINS:-}" ]; then
  v3="$COST_BINS/elf/lc_verify_v3"; at="$COST_BINS/elf/attest"
else
  flock "$lock" env GUEST_CARGO_CONFIG="$BV_V3_CARGO_CONFIG" scripts/build_guest.sh programs/lc_verify_v3/Cargo.toml lc_verify_v3 "$g/plain"
  flock "$lock" scripts/build_guest.sh programs/attest/Cargo.toml attest "$g/attest"
  v3="$g/plain/lc_verify_v3"; at="$g/attest/attest"
fi
flock "$lock" env CARGO_TARGET_DIR="$tdir" RISC0_SKIP_BUILD=1 nice -n 15 cargo build -q --locked -p cost --bin cost
nice -n 15 "$tdir/debug/cost" exec-v3 --v3-elf "$v3" --trace-elf "$g/trace/lc_verify_v3" --attest-elf "$at" \
  --json "$BV_REPO/evidence/public-path/exec-steps.json"
