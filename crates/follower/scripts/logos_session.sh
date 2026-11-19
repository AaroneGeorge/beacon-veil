#!/usr/bin/env bash
# Logos side of the follower: a logosctl 0.3.2 daemon session with beacon_api_module (v0.2.0) and
# attest_follower_module installed. Runbook: modules/attest_follower_module/RUNBOOK.md.
#
#   crates/follower/scripts/logos_session.sh start|install|load|probe|status|stop
#
# Env overrides: W4_LOGOS_SESSION (session dir), W4_LOGOSCTL (logosctl binary), BEACON_LGX, FOLLOWER_LGX (.lgx files).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"

S="${W4_LOGOS_SESSION:-$B/run/w4/logos-session}"
CTL="${W4_LOGOSCTL:-$B/run/w4/logosctl/squashfs-root/AppRun}"
BEACON_LGX="${BEACON_LGX:-$(ls "$B"/run/w4/nix/beacon-api-lgx-portable/*.lgx 2>/dev/null | head -1)}"
FOLLOWER_LGX="${FOLLOWER_LGX:-$(ls "$B"/run/w4/nix/attest-follower-lgx-portable/*.lgx 2>/dev/null | head -1)}"
# The bundled package downloader starts a storage node in $HOME: keep it out of the real one.
export HOME="$B/run/w4/logos-home"
mkdir -p "$S" "$HOME"
ctl() { "$CTL" --config-dir "$S" "$@"; }

case "${1:-}" in
  start)
    ctl daemon status >/dev/null 2>&1 && { echo "daemon already running ($S)"; exit 0; }
    ctl daemon start --detach
    ;;
  install)
    [ -n "$BEACON_LGX" ] || { echo "no beacon_api_module .lgx (build: see RUNBOOK.md)" >&2; exit 1; }
    ctl install "$BEACON_LGX" -y
    if [ -n "$FOLLOWER_LGX" ]; then ctl install "$FOLLOWER_LGX" -y; else echo "note: attest_follower_module .lgx not built; skipped"; fi
    ctl package ls
    ;;
  load)
    ctl module load beacon_api_module
    ctl call beacon_api_module init_defaults
    ctl call beacon_api_module list_providers 11155111
    ;;
  probe)
    # One live light-client request through the module, the same call the follower makes. Prints one summary line.
    # Chain: $BV_CHAIN_ID (default 11155111 Sepolia; 1 = mainnet).
    ctl --json call beacon_api_module lc_request "${BV_CHAIN_ID:-11155111}" str:getLightClientFinalityUpdate 'str:{}' 'str:' 30000 \
      | python3 -I -c '
import json, sys
o = json.load(sys.stdin)
r = json.loads(o["result"]) if isinstance(o.get("result"), str) else o.get("result", {})
if o.get("status") != "ok" or not r.get("ok"):
    print("probe FAILED", json.dumps(o)[:400]); sys.exit(1)
b = json.loads(r["body"])
print("probe ok provider=%s version=%s finalized_slot=%s" % (r.get("provider"), b.get("version"),
      b["data"]["finalized_header"]["beacon"]["slot"]))'
    ;;
  status) ctl daemon status ;;
  stop) ctl daemon stop ;;
  *) echo "usage: $0 start|install|load|probe|status|stop" >&2; exit 2 ;;
esac
