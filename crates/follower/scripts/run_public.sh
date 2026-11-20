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
# Before `start`: the instance exists on seq-pub with lc_verify_v3 among its verifiers and RUN_DIR/instance.json
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
mkdir -p "$RUN_DIR"
case "$cmd" in
  session)
    for c in start install load probe; do "$here/logos_session.sh" "$c"; done
    ;;
  wallet)
    [ -s "$RUN_DIR/wallet.env" ] && { echo "$RUN_DIR/wallet.env exists"; cat "$RUN_DIR/wallet.env"; exit 0; }
    # Public transactions only: no private account (amount 0).
    WALLET_CONFIG_FROM="$B/run/wallet-pub" "$here/wallet_home.sh" "w4-$(basename "$RUN_DIR")" 10000000000 0 \
      > "$RUN_DIR/wallet.env.tmp"
    { grep '^export ' "$RUN_DIR/wallet.env.tmp"; echo "export BV_IDS=${BV_IDS_PUB:-$R/deploy/ids-pub.toml}"; } > "$RUN_DIR/wallet.env"
    rm -f "$RUN_DIR/wallet.env.tmp"; cat "$RUN_DIR/wallet.env"
    ;;
  start|status|stop|collect)
    exec "$here/run_a.sh" "$cmd"
    ;;
  *) echo "usage: run_public.sh mainnet|sepolia session|wallet|start|status|stop|collect" >&2; exit 2 ;;
esac
