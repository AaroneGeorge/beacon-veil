#!/usr/bin/env bash
# Re-reads every transaction recorded in evidence/public-path/onchain/*/run.json from the sequencer RPC and checks it
# (block, position, not reverted, receiver, step, charged gas of the block) and each instance's final head. Prints PASS
# or the first mismatch.
#
#   scripts/gate/gp_onchain.sh [run.json ...]     # default: every evidence/public-path/onchain/*/run.json
#
# Env: BV_IDS (default deploy/ids-pub.toml), LEE_WALLET_HOME_DIR (default $B/run/wallet-pub); seq-pub must be running.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../env.sh"
export BV_IDS="${BV_IDS:-$BV_REPO/deploy/ids-pub.toml}"
export LEE_WALLET_HOME_DIR="${BV_PUB_WALLET:-$B/run/wallet-pub}" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
"$BV_REPO/scripts/seq.sh" pub status > /dev/null || { echo "seq-pub is not running" >&2; exit 1; }
bin="${BENCH_TARGET_DIR:-$CARGO_TARGET_DIR}/debug/v3_onchain"
[ -x "$bin" ] || { echo "$bin missing: cargo build -p cost --bin v3_onchain" >&2; exit 1; }
if [ $# -eq 0 ]; then set -- "$BV_REPO"/evidence/public-path/onchain/*/run.json; fi
"$bin" check "$@"
