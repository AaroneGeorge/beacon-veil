#!/usr/bin/env bash
# Own wallet home for a follower (project rules: one wallet home per writer; bv-payer only
# funds each home's own payer, under the deploy lock).
#
#   crates/follower/scripts/wallet_home.sh <name> [public_amount] [private_amount]
#     name: w4 (dev testing) or w4-runA (run A). Creates $B/run/wallet-<name> with:
#       - wallet_config.json copied from $B/run/wallet-dev (seq-dev RPC)
#       - bv-payer (LEZ local genesis test key, public, not a secret): funding source only
#       - <name>-payer (public), funded from bv-payer under flock: BV_PAYER
#       - bv-submitter (private), funded from <name>-payer: BV_SUBMITTER (Path A in-proof fee)
#   Prints the env lines to use. Idempotent: existing labels are kept.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"
name="${1:?usage: wallet_home.sh <name> [public_amount] [private_amount]}"
pub_amount="${2:-10000000000}"; priv_amount="${3:-1000000000}"  # a private tx reserves about 5e6 in fees (measured 2026-11-12)
export LEE_WALLET_HOME_DIR="$B/run/wallet-$name"
export LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
mkdir -p "$LEE_WALLET_HOME_DIR"
[ -f "$LEE_WALLET_HOME_DIR/wallet_config.json" ] || cp "$B/run/wallet-dev/wallet_config.json" "$LEE_WALLET_HOME_DIR/"
command -v wallet >/dev/null || { echo "wallet binary not on PATH" >&2; exit 1; }

LEZ_TEST_KEY="7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb" # LEZ Justfile, local genesis
has() { wallet account id --account-id "$1" >/dev/null 2>&1; }
id_of() { wallet account id --account-id "$1" 2>/dev/null | tail -1; }
BV_PAYER_ID="CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r" # the imported genesis test account

if ! has bv-payer; then
  # rc3's import has no --label: import, then label the known id.
  wallet account import public --private-key "$LEZ_TEST_KEY" >/dev/null
  wallet account label --account-id "Public/$BV_PAYER_ID" --label bv-payer >/dev/null
fi
payer="$name-payer"
if ! has "$payer"; then
  wallet account new public --label "$payer" >/dev/null
  flock "$R/deploy/ids.toml.lock" wallet auth-transfer send --from bv-payer --to "$payer" --amount "$pub_amount"
fi
if ! has bv-submitter; then
  wallet account new private --label bv-submitter >/dev/null
  wallet auth-transfer send --from "$payer" --to bv-submitter --amount "$priv_amount"
fi
echo "export LEE_WALLET_HOME_DIR=$LEE_WALLET_HOME_DIR"
echo "export BV_PAYER=$(id_of "$payer")"
echo "export BV_SUBMITTER=$(id_of bv-submitter)"
wallet account get --account-id "$payer" || true
