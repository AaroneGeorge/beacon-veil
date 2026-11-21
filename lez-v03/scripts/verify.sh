#!/usr/bin/env bash
# Re-checks evidence/public-testnet against the public LEZ testnet 0.3. Needs only curl-free python3 (stdlib).
#   lez-v03/scripts/verify.sh [--rpc URL] [EVIDENCE_DIR]
# Defaults: --rpc https://testnet.lez.logos.co, evidence/public-testnet. Exit 0 when every check passes.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rpc=https://testnet.lez.logos.co
if [ "${1:-}" = "--rpc" ]; then rpc="$2"; shift 2; fi
ev="${1:-$here/../../evidence/public-testnet}"
exec python3 -I "$here/verify_testnet.py" --rpc "$rpc" "$ev"
