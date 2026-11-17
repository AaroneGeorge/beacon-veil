#!/usr/bin/env bash
# Creates a fresh private account in the dev wallet, funds it from the public payer (bv-payer) and prints its id.
# This is the F11 path: an account created seconds earlier submits a light-client update.
#
#   tests/e2e_chain/scripts/new_submitter.sh [label] [amount]     (default label bv-submitter-<utc stamp>, 1000000)
#   export BV_SUBMITTER=<printed id>
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
home="${LEE_WALLET_HOME_DIR:?set LEE_WALLET_HOME_DIR to your own wallet home}"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"   # sets LEE_WALLET_HOME_DIR to the shared wallet: restore ours
export LEE_WALLET_HOME_DIR="$home" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
label="${1:-bv-submitter-$(date -u +%Y%m%dT%H%M%SZ)}"; amount="${2:-1000000}"
out="$(wallet account new private --label "$label")"
id="$(printf '%s\n' "$out" | sed -n 's#.*Private/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1)"
[ -n "$id" ] || { echo "could not parse the new account id from: $out" >&2; exit 1; }
wallet auth-transfer send --from "Public/${BV_PAYER:?set BV_PAYER (the public payer)}" --to "Private/$id" --amount "$amount" >&2
echo "$id"
