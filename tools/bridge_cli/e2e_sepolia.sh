#!/usr/bin/env bash
# Sepolia flow of the bridge spike (W5), in stages. State is kept in $B/run/w5/sepolia/flow.env.
#
#   e2e_sepolia.sh vault                                 deploy vault v2 + STUB anchor + test token
#   ATTEST_INSTANCE=<base58> e2e_sepolia.sh config       final bridge config (real attest program from deploy/ids.toml);
#                                                        binds the bridge instance in the vault (once)
#   e2e_sepolia.sh deposit                               0.001 ETH deposit bound to w5-claimer (next deposit index)
#   e2e_sepolia.sh claim                                 attest-root (head record, must be after the deposit), private
#                                                        claim in dev mode, repeat claim refused (K9 part 2)
#   e2e_sepolia.sh burn                                  private burn to the deployer address, post root to the STUB anchor
#   e2e_sepolia.sh prove                                 burn_proof: MODE=execute (default) or MODE=groth16 (window only)
#   e2e_sepolia.sh release                               Vault.release on Sepolia with the Groth16 seal, gas recorded
#
# Key: read from lechu-secret into PRIVATE_KEY for the forge calls only; never printed or written.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
R="$(cd "$here/../.." && pwd)"
source "$R/scripts/env.sh"
export LEE_WALLET_HOME_DIR="$B/run/wallet-w5" LEE_WALLET_PASSWORD=""
export BV_GUESTS="${BV_GUESTS:-$B/run/w5/guests}"
export BV_BRIDGE_MNEMONIC_FILE="${BV_BRIDGE_MNEMONIC_FILE:-$here/testdata/TEST-ONLY-mnemonic.txt}"
export BV_ETH_RPC="${BV_ETH_RPC:-https://ethereum-sepolia-rpc.publicnode.com}"
S="$B/run/w5/sepolia"; mkdir -p "$S"; F="$S/flow.env"; touch "$F"; source "$F"
CLI="${BV_BRIDGE_CLI:-$CARGO_TARGET_DIR/debug/bridge_cli}"
LOG="$S/flow-$(date -u +%Y%m%dT%H%M%SZ)-${1:-none}.log"
exec > >(tee "$LOG") 2>&1
label_id() { wallet account list 2>/dev/null | grep "\[$1\]" | grep -o '\(Public\|Private\)/[A-Za-z0-9]*' | cut -d/ -f2; }
export BV_PAYER="${BV_PAYER:-$(label_id w5-payer)}"
CLAIMER="$(label_id w5-claimer)"
DEPLOYER=0x523072A2aC5e5A3E51c107bc17995A3fa03dBD78
save() { grep -v "^$1=" "$F" > "$F.tmp" || true; echo "$1=$2" >> "$F.tmp"; mv "$F.tmp" "$F"; }
key() { sudo -n lechu-secret show logos LOGOS_SEPOLIA_KEY; }
forge_s() { (cd "$R/contracts" && PRIVATE_KEY="$(key)" nice -n 15 forge script "script/Deploy.s.sol:$1" \
  --rpc-url "$BV_ETH_RPC" --broadcast --slow) > "$S/forge-$1-$(date -u +%H%M%S).txt" 2>&1
  cp "$R/contracts/broadcast/Deploy.s.sol/11155111/run-latest.json" "$S/broadcast-$1-$(date -u +%H%M%S).json"
  jq -r '.receipts[] | "\(.transactionHash) gas=\(.gasUsed) block=\(.blockNumber) status=\(.status)"' \
    "$R/contracts/broadcast/Deploy.s.sol/11155111/run-latest.json"; }
b58hex() { python3 -c 'import sys
a="123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";n=0
for c in sys.argv[1]: n=n*58+a.index(c)
print("0x%064x"%n)' "$1"; }
IDS="${BV_IDS:-$R/deploy/ids.toml}"; export BV_IDS="$IDS"
BRIDGE=$(grep -E '^bridge *=' "$IDS" | cut -d'"' -f2)
ATTEST=$(grep -E '^attest *=' "$IDS" | cut -d'"' -f2)

case "${1:-}" in
vault)
  IMAGE=$("$CLI" image-id)
  out=$(cd "$R/contracts" && PRIVATE_KEY="$(key)" VERIFIER=0x2a098988600d87650Fb061FfAff08B97149Fa84D \
    BURN_IMAGE_ID=$IMAGE LEZ_PROGRAM=$(b58hex "$BRIDGE") ANCHOR_DELAY=600 nice -n 15 forge script \
    script/Deploy.s.sol:Deploy --rpc-url "$BV_ETH_RPC" --broadcast --slow 2>&1 | grep -o '{"admin".*}')
  echo "$out" | tee "$S/deploy-v2.json"
  cp "$R/contracts/broadcast/Deploy.s.sol/11155111/run-latest.json" "$S/broadcast-deploy-v2.json"
  save VAULT "$(jq -r .vault <<<"$out")"; save ANCHOR "$(jq -r .anchor_stub <<<"$out")"
  save USDT "$(jq -r .test_usdt <<<"$out")"; save VAULT_BLOCK "$(jq -r .block <<<"$out")"
  echo "next: e2e_sepolia.sh config ATTEST_INSTANCE=<base58>" ;;
config)
  : "${ATTEST_INSTANCE:?ATTEST_INSTANCE=<followed Sepolia instance, base58>}"; : "${VAULT:?run the vault stage first}"
  "$CLI" config --out "$S/config.json" --bridge-program "$BRIDGE" --chain-id 11155111 --vault "$VAULT" \
    --vault-block "$VAULT_BLOCK" --anchor "$ANCHOR" --attest-program "$ATTEST" --attest-instance "$ATTEST_INSTANCE" \
    --token eth:1000000000000000,10000000000000000 --token "$USDT:100000000,1000000000"
  save ATTEST_INSTANCE "$ATTEST_INSTANCE"; save INSTANCE_HEX "$(jq -r .instance_hex "$S/config.json")"
  (cd "$R/contracts" && PRIVATE_KEY="$(key)" VAULT=$VAULT LEZ_INSTANCE=$(jq -r .instance_hex "$S/config.json") \
    nice -n 15 forge script script/Deploy.s.sol:BindInstance --rpc-url "$BV_ETH_RPC" --broadcast --slow | grep -E 'SUCCESS|Error') ;;
deposit)
  export BV_BRIDGE_CONFIG="$S/config.json"
  IDX="${DEPOSIT_INDEX:-${NEXT_DEPOSIT:-0}}"
  C=$("$CLI" commitment --index "$IDX" --dest "$CLAIMER" --token eth --denom 1000000000000000 | tee "$S/commit-$IDX.json")
  export VAULT TOKEN=0x0000000000000000000000000000000000000000 DENOM_INDEX=0 COMMITMENT=$(jq -r .commitment <<<"$C")
  forge_s DepositOne
  save DEPOSIT_INDEX "$IDX"; save NEXT_DEPOSIT "$((IDX + 1))"
  save DEPOSIT_BLOCK "$(jq -r '.receipts[-1].blockNumber' "$R/contracts/broadcast/Deploy.s.sol/11155111/run-latest.json" | xargs printf '%d')" ;;
claim)
  export BV_BRIDGE_CONFIG="$S/config.json"
  A=$("$CLI" attest-root --min-eth-block "$DEPOSIT_BLOCK" | tee "$S/attest-root.json")
  ROOT=$(jq -r .root <<<"$A"); save ROOT "$ROOT"
  "$CLI" claim --index "$DEPOSIT_INDEX" --dest "$CLAIMER" --root "$ROOT" | tee "$S/claim1.json"
  if "$CLI" claim --index "$DEPOSIT_INDEX" --dest "$CLAIMER" --root "$ROOT" --force > "$S/claim2.txt" 2>&1; then
    echo "FAIL: second claim accepted"; exit 1; fi
  grep -E 'E11' "$S/claim2.txt" && echo "K9 part 2: second claim refused OK" ;;
burn)
  export BV_BRIDGE_CONFIG="$S/config.json"
  J="${BURN_INDEX:-0}"
  "$CLI" burn --index "$J" --holder "$CLAIMER" --token eth --denom 1000000000000000 --recipient "$DEPLOYER" \
    --fee 10000000000000 | tee "$S/burn-$J.json"
  ACC=$("$CLI" accumulator-root | tee "$S/accumulator.json")
  export ANCHOR LEZ_INSTANCE=$INSTANCE_HEX ROOT=$(jq -r .root <<<"$ACC")
  forge_s PostRoot
  save BURN_INDEX "$J"; echo "STUB anchor delay: release possible 600 s after the post" ;;
prove)
  export BV_BRIDGE_CONFIG="$S/config.json"
  M="${MODE:-execute}"
  if [ "$M" = groth16 ]; then unset RISC0_DEV_MODE; fi
  /usr/bin/time -v nice -n 19 ionice -c3 "$CLI" prove --index "$BURN_INDEX" --out "$S/burn-$BURN_INDEX-$M" --mode "$M" \
    2> "$S/burn-$BURN_INDEX-$M.time.txt"
  grep -E 'Maximum resident|Elapsed' "$S/burn-$BURN_INDEX-$M.time.txt" ;;
release)
  D="$S/burn-$BURN_INDEX-groth16"
  export VAULT SEAL="$(cat "$D/seal.hex")" JOURNAL="$(cat "$D/journal.hex")"
  forge_s ReleaseOne | tee "$S/release.txt" ;;
*) sed -n '2,15p' "$0"; exit 2 ;;
esac
