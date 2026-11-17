#!/usr/bin/env bash
# Regenerates the attest_inclusion test fixtures (crates/attest_inclusion/tests/data) from public RPC.
# Blocks are the execution blocks of the beacon fixtures:
#   mainnet 26397891 = finalized header of fixtures/mainnet/finality_update_15637644 (beacon slot 15637568)
#   sepolia 12117875 = execution_block_hash 0x06ea7e9f... of fixtures/sepolia/finality_update_11561824 (beacon slot 11561760)
# RPCs (no keys): mainnet https://eth.drpc.org (historical eth_getProof), sepolia publicnode.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$here/../../scripts/env.sh"
out="$BV_REPO/crates/attest_inclusion/tests/data"
mkdir -p "$out"
nice -n 15 cargo build -q -p proofgen
pg() { "$CARGO_TARGET_DIR/debug/proofgen" "$@"; }
# Mainnet public RPCs serve eth_getProof for old blocks only intermittently, so by default the fixtures are rebuilt
# from the raw responses captured on 2026-11-12 ($B/run/w3-raw; RAW= to go to the network instead). Every proof is
# re-verified by proofgen either way.
RAW="${RAW-$B/run/w3-raw}"
rawarg=(); [ -n "$RAW" ] && rawarg=(--raw "$RAW")
M="--chain mainnet --block 26397891 ${rawarg[*]}"
S="--chain sepolia --block 12117875 ${rawarg[*]}"
USDT=0xdAC17F958D2ee523a2206206994597C13D831ec7; USDC=0xA0b86991c6218b36c1d19d4a2e9eb0ce3606eB48
SUSDC=0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238; ABSENT=0x5eed00000000000000000000000000000c0ffee1
pg $M --out "$out/mainnet_storage_usdt_slot0.json" storage --address $USDT --slot 0x0
pg $M --out "$out/mainnet_storage_usdt_slot1.json" storage --address $USDT --slot 0x1
pg $M --out "$out/mainnet_storage_usdt_zero.json" storage --address $USDT --slot 0xdeadbeef
pg $M --out "$out/mainnet_account_usdc.json" account --address $USDC
pg $M --out "$out/mainnet_account_absent.json" account --address $ABSENT
pg $M --out "$out/mainnet_log_usdt_tx1_log2.json" log --tx-index 1 --log-index 2
pg $M --out "$out/mainnet_log_usdt_tx1_log4.json" log --tx-index 1 --log-index 4
pg $M --out "$out/mainnet_log_usdc_tx3_log0.json" log --tx-index 3 --log-index 0
pg $M --out "$out/mainnet_log_tx374_log254.json" log --tx-index 374 --log-index 254
pg $S --out "$out/sepolia_storage_usdc_slot0.json" storage --address $SUSDC --slot 0x0
pg $S --out "$out/sepolia_storage_usdc_zero.json" storage --address $SUSDC --slot 0xdeadbeef
pg $S --out "$out/sepolia_account_absent.json" account --address $ABSENT
pg $S --out "$out/sepolia_log_usdc_tx48_log2.json" log --tx-index 48 --log-index 2
pg $S --out "$out/sepolia_log_usdc_tx167_log2.json" log --tx-index 167 --log-index 2
ls -la "$out"
