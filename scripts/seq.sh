#!/usr/bin/env bash
# Local LEZ v0.4.0-rc3 standalone sequencers (mock Bedrock, 15 s blocks).
#
#   scripts/seq.sh dev|real|main|pub start|stop|status|reset|logs
#
#   dev  : RISC0_DEV_MODE=1, RPC 127.0.0.1:3040, ops/metrics 127.0.0.1:9040, home $B/run/seq-dev,  wallet $B/run/wallet-dev
#   real : dev mode UNSET,   RPC 127.0.0.1:3041, ops/metrics 127.0.0.1:9041, home $B/run/seq-real, wallet $B/run/wallet-real
#          (proof windows only)
#   main : RISC0_DEV_MODE=1, RPC 127.0.0.1:3042, ops/metrics 127.0.0.1:9042, home $B/run/seq-main, wallet $B/run/wallet-main
#          (2026-11-17: fresh chain for the mainnet follower run C; seq-dev's 37 h chain takes ~2.5 h to replay on start)
#   pub  : dev mode UNSET,   RPC 127.0.0.1:3043, ops/metrics 127.0.0.1:9043, home $B/run/seq-pub,  wallet $B/run/wallet-pub
#          (public-path probes and evidence: public transactions are executed by the sequencer, no proofs involved)
#
# Binary: $B/vendor/lez/target/release/sequencer_service, built with `--features standalone` (see env-install.log).
# The config template and the funding key are LEZ's own (`just run-sequencer-standalone`).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"

which_seq="${1:-}"; cmd="${2:-status}"
case "$which_seq" in
  dev)  port=3040; ops=9040 ;;
  real) port=3041; ops=9041 ;;
  main) port=3042; ops=9042 ;;
  pub)  port=3043; ops=9043 ;;
  *) echo "usage: $0 dev|real|main|pub start|stop|status|reset|logs" >&2; exit 2 ;;
esac

LEZ="$B/vendor/lez"
SEQ_BIN="$LEZ/target/release/sequencer_service"
FUNDING_PK="2e03b2eff5a45478e7e79668d2a146cf2c5c7925bce927f2b1c67f2ab4fc0d26" # LEZ Justfile BEDROCK_FUNDING_PUBLIC_KEY
home="$B/run/seq-$which_seq"
wallet_home="$B/run/wallet-$which_seq"
log="$B/run/seq-$which_seq.log"
pidf="$B/run/seq-$which_seq.pid"

running() { [ -f "$pidf" ] && kill -0 "$(cat "$pidf")" 2>/dev/null; }

ensure_wallet_config() {
  mkdir -p "$wallet_home"
  if [ ! -f "$wallet_home/wallet_config.json" ]; then
    sed "s#http://127.0.0.1:3040#http://127.0.0.1:$port#" "$LEZ/lez/wallet/configs/debug/wallet_config.json" \
      > "$wallet_home/wallet_config.json"
  fi
}

case "$cmd" in
  start)
    if running; then echo "seq-$which_seq already running (pid $(cat "$pidf"))"; exit 0; fi
    [ -x "$SEQ_BIN" ] || { echo "missing $SEQ_BIN (build it: see env-install.log)" >&2; exit 1; }
    mkdir -p "$B/run"
    if [ ! -f "$home/sequencer_config.json" ]; then
      "$SEQ_BIN" setup "$home" --config "$LEZ/lez/sequencer/service/networks/local.json" \
        --funding-public-key "$FUNDING_PK" --create-channel
    fi
    ensure_wallet_config
    case "$which_seq" in real|pub) unset RISC0_DEV_MODE ;; *) export RISC0_DEV_MODE=1 ;; esac
    printf '\n=== %s start seq-%s RISC0_DEV_MODE=%s\n' "$(date -u +%FT%TZ)" "$which_seq" "${RISC0_DEV_MODE:-unset}" >> "$log"
    RUST_LOG="${RUST_LOG:-info,risc0_zkvm=warn,kameo=warn}" setsid nohup nice -n 10 "$SEQ_BIN" start "$home" \
      --rpc-listen-address "127.0.0.1:$port" --metrics-listen-address "127.0.0.1:$ops" >> "$log" 2>&1 < /dev/null &
    echo $! > "$pidf"
    for _ in $(seq 1 60); do
      if curl -fsS "http://127.0.0.1:$ops/health" >/dev/null 2>&1; then
        echo "seq-$which_seq up: pid $(cat "$pidf"), rpc 127.0.0.1:$port, log $log"
        echo "wallet: export LEE_WALLET_HOME_DIR=$wallet_home"
        exit 0
      fi
      running || { echo "seq-$which_seq exited; tail of $log:" >&2; tail -20 "$log" >&2; exit 1; }
      sleep 1
    done
    echo "seq-$which_seq started (pid $(cat "$pidf")) but /health did not answer in 60 s; see $log" >&2
    exit 1
    ;;
  stop)
    if running; then
      pid="$(cat "$pidf")"; kill -TERM "$pid"
      for _ in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
      kill -0 "$pid" 2>/dev/null && kill -KILL "$pid"
      echo "seq-$which_seq stopped"
    else
      echo "seq-$which_seq not running"
    fi
    rm -f "$pidf"
    ;;
  status)
    if running; then
      echo "seq-$which_seq running: pid $(cat "$pidf"), rpc 127.0.0.1:$port"
      curl -fsS "http://127.0.0.1:$ops/health" && echo
    else
      echo "seq-$which_seq stopped"; exit 3
    fi
    ;;
  reset)
    running && { echo "stop seq-$which_seq first" >&2; exit 1; }
    rm -rf "$home" "$wallet_home/storage.json"
    echo "seq-$which_seq state wiped (wallet config kept)"
    ;;
  logs) tail -n 50 -f "$log" ;;
  *) echo "usage: $0 dev|real|main|pub start|stop|status|reset|logs" >&2; exit 2 ;;
esac
