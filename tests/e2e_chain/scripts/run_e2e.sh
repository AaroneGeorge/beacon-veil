#!/usr/bin/env bash
# Runs the W1 end-to-end chain test against seq-dev (accept (b); called by scripts/gate/g1_chain.sh).
# Needs: seq-dev running, attest + lc_verify deployed (deploy/ids.toml), a track wallet with a funded public payer
# and private submitter (tests/e2e_chain/scripts/setup_wallet.sh).
#
#   LEE_WALLET_HOME_DIR=... BV_PAYER=... BV_SUBMITTER=... tests/e2e_chain/scripts/run_e2e.sh [log]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
home="${LEE_WALLET_HOME_DIR:?set LEE_WALLET_HOME_DIR to the track wallet}"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"
export LEE_WALLET_HOME_DIR="$home" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}" RUST_LOG="${RUST_LOG:-warn}"
log="${1:-$B/.tmp/e2e_chain-$(date -u +%Y%m%dT%H%M%SZ).log}"
cd "$BV_REPO"
# Build once, then run the test executable directly (running through `cargo test` would wait on the shared target
# lock again).
exe="$(RISC0_SKIP_BUILD=1 nice -n 15 cargo test -p e2e_chain --test e2e_chain --no-run --message-format=json 2>/dev/null \
  | jq -r 'select(.executable != null and .target.name == "e2e_chain" and (.target.kind | index("test"))) | .executable' \
  | tail -1)"
[ -x "$exe" ] || { RISC0_SKIP_BUILD=1 cargo test -p e2e_chain --test e2e_chain --no-run; exit 1; }
{ echo "start $(date -u +%FT%TZ) $(uname -srm) exe $exe"; } > "$log"
set +e
nice -n 15 "$exe" --ignored --nocapture --test-threads 1 >> "$log" 2>&1
rc=$?
set -e
echo "end $(date -u +%FT%TZ) rc=$rc" >> "$log"
grep -E "^init|^update|^F11|rejected|test result" "$log" || true
echo "log: $log"
exit $rc
