#!/usr/bin/env bash
# One-time setup of a track's own dev wallet (coordinator rule 2026-11-12: one wallet home per track; bv-payer only
# funds each track's own public payer, under the ids.toml lock).
#
#   LEE_WALLET_HOME_DIR=$B/run/wallet-w1 tests/e2e_chain/scripts/setup_wallet.sh [prefix] [amount]
#   then: export BV_PAYER=<printed public id> BV_SUBMITTER=<printed private id>
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
home="${LEE_WALLET_HOME_DIR:?set LEE_WALLET_HOME_DIR to the track wallet}"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"
export LEE_WALLET_HOME_DIR="$home" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
prefix="${1:-w1}"; amount="${2:-1000000000000}"
mkdir -p "$home"
[ -f "$home/wallet_config.json" ] || cp "$B/run/wallet-dev/wallet_config.json" "$home/"
LEZ_TEST_KEY="7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb" # LEZ Justfile, local genesis; not a secret
if ! wallet account get --account-id bv-payer >/dev/null 2>&1; then
  imported="$(wallet account import public --private-key "$LEZ_TEST_KEY")"
  bvp="$(printf '%s\n' "$imported" | sed -n 's#.*Public/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1)"
  [ -n "$bvp" ] || { echo "could not parse the imported account: $imported" >&2; exit 1; }
  wallet account label --account-id "Public/$bvp" --label bv-payer >/dev/null   # rc3 import has no --label
fi
id_of() { sed -n "s#.*$1/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p" | head -1; }
payer="$(wallet account new public --label "$prefix-payer-$(date -u +%H%M%S)" | id_of Public)"
submitter="$(wallet account new private --label "$prefix-submitter-$(date -u +%H%M%S)" | id_of Private)"
[ -n "$payer" ] && [ -n "$submitter" ] || { echo "could not parse new account ids" >&2; exit 1; }
(
  flock 9
  wallet auth-transfer send --from bv-payer --to "Public/$payer" --amount "$amount" >&2
) 9>"$BV_REPO/deploy/ids.toml.lock"
wallet auth-transfer send --from "Public/$payer" --to "Private/$submitter" --amount "$((amount / 10))" >&2
echo "BV_PAYER=$payer"
echo "BV_SUBMITTER=$submitter"
