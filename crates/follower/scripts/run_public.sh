#!/usr/bin/env bash
# Runs D and E: one attest-followerd per network following its beacon chain into an instance on seq-pub through the
# public path (`[chain] mode = "public"`: each update as Begin, Miller, Finish public transactions to lc_verify_v3).
# A thin layer over run_a.sh (same preflight, detached start, status, collect) with the public-run defaults.
#
#   crates/follower/scripts/run_public.sh mainnet|sepolia session  # start the public runs' own Logos session
#                                                                  # (not run C's), install, load, probe
#   crates/follower/scripts/run_public.sh mainnet|sepolia wallet   # own wallet home on seq-pub -> RUN_DIR/wallet.env
#   crates/follower/scripts/run_public.sh mainnet|sepolia start|status|stop|collect
#
# mainnet = run D ($B/run/w4/run-d, cadence 6,144 s), sepolia = run E ($B/run/w4/run-e, cadence 1,536 s).
# Before `start`: the instance exists on seq-pub with lc_verify_v3 as its only verifier (`attest init
# --only-verifier <lc_verify_v3 id> --out RUN_DIR/instance.json`) and RUN_DIR/instance.json
# names it (`attest init --out`); RUN_DIR/wallet.env exports LEE_WALLET_HOME_DIR, BV_PAYER and BV_IDS
# (deploy/ids-pub.toml of this checkout, or BV_IDS_PUB). The wallet home is $B/run/wallet-w4-<basename RUN_DIR>,
# public accounts only. SOURCE_KIND=direct_http uses the providers in the config instead of the module.
# collect writes evidence/g4-public/<network>/. Never restart a run by hand: G4 needs one PID.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/../../../scripts/env.sh"
net="${1:?usage: run_public.sh mainnet|sepolia session|wallet|start|status|stop|collect}"
cmd="${2:?usage: run_public.sh mainnet|sepolia session|wallet|start|status|stop|collect}"
case "$net" in
  mainnet) run=run-d; chain_id=1 ;;
  sepolia) run=run-e; chain_id=11155111 ;;
  *) echo "network: mainnet or sepolia" >&2; exit 2 ;;
esac
export RUN_DIR="${RUN_DIR:-$B/run/w4/$run}"
export SEQ=pub
export CFG_TEMPLATE="${CFG_TEMPLATE:-$R/crates/follower/examples/followerd.$net.public.toml}"
export EVIDENCE_DIR="${EVIDENCE_DIR:-$R/evidence/g4-public/$net}"
# The public runs' Logos session: its own config dir, temp dir and home, so run C's session is never touched.
export W4_LOGOS_SESSION="$B/run/w4/logos-session-pub" W4_LOGOS_TMPDIR="$B/.tmp-pub" W4_LOGOS_HOME="$B/run/w4/logos-home-pub"
export BV_CHAIN_ID="$chain_id"
# seq-pub runs with dev mode unset: so does everything started from here (env.sh sets it for builds).
unset RISC0_DEV_MODE
# Payer funding: fee is 8 per cycle on seq-pub and an update costs about 12.6M cycles (about 1e8), so run E (56
# updates a day) needs about 5.7e9 a day and run D (14 a day) about 1.4e9 a day, until filing.
case "$net" in
  mainnet) FUND="${PUB_FUND:-30000000000}" ;;
  sepolia) FUND="${PUB_FUND:-100000000000}" ;;
esac
mkdir -p "$RUN_DIR"
lock="$B/repo/deploy/ids-pub.toml.lock"   # the lock seq-pub deployments hold while they send from bv-payer
payer_balance() { # label -> balance
  wallet account get --account-id "$1" 2>/dev/null | sed -n 's/.*Balance \([0-9]*\).*/\1/p' | head -1
}
nobody_else_sends() {
  if pgrep -x v3_onchain >/dev/null; then echo "a v3_onchain process sends from bv-payer on seq-pub; try later" >&2; exit 1; fi
}
case "$cmd" in
  session)
    for c in start install load probe; do "$here/logos_session.sh" "$c"; done
    ;;
  wallet)
    # Own wallet home and payer funded with $FUND; an existing payer is topped up to $FUND. Both send from bv-payer
    # under the deployment lock, and only when no measurement driver sends from it.
    nobody_else_sends
    name="w4-$(basename "$RUN_DIR")"
    if [ ! -s "$RUN_DIR/wallet.env" ]; then
      # Public transactions only: no private account (amount 0).
      WALLET_CONFIG_FROM="$B/run/wallet-pub" flock -w 600 "$lock" "$here/wallet_home.sh" "$name" "$FUND" 0 \
        > "$RUN_DIR/wallet.env.tmp"
      { grep '^export ' "$RUN_DIR/wallet.env.tmp"; echo "export BV_IDS=${BV_IDS_PUB:-$R/deploy/ids-pub.toml}"; } > "$RUN_DIR/wallet.env"
      rm -f "$RUN_DIR/wallet.env.tmp"
    fi
    # shellcheck source=/dev/null
    source <(grep '^export ' "$RUN_DIR/wallet.env")
    have="$(payer_balance "$name-payer")"; have="${have:-0}"
    if [ "$have" -lt "$FUND" ]; then
      echo "payer $name-payer holds $have; topping up to $FUND"
      flock -w 600 "$lock" wallet auth-transfer send --from bv-payer --to "$name-payer" --amount "$((FUND - have))"
      have="$(payer_balance "$name-payer")"
    fi
    cat "$RUN_DIR/wallet.env"; echo "payer $name-payer balance ${have:-unknown} (want >= $FUND)"
    [ "${have:-0}" -ge "$FUND" ] || { echo "payer balance below $FUND" >&2; exit 1; }
    ;;
  start|status|stop|collect)
    exec "$here/run_a.sh" "$cmd"
    ;;
  *) echo "usage: run_public.sh mainnet|sepolia session|wallet|start|status|stop|collect" >&2; exit 2 ;;
esac
