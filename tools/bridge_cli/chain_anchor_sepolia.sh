#!/usr/bin/env bash
# The chain route on Sepolia for a real burn on seq-pub: a test vault (VaultChain) bound to the burn's LEZ instance,
# the STUB tip anchor, one Groth16 proof of guest burn_chain, releaseChain through the RISC Zero 3.1.0 emergency stop.
#
#   tools/bridge_cli/chain_anchor_sepolia.sh STAGE
#
# Stages, in order (state in $BV_CHAIN_FLOW_DIR/flow.env, default $B/run/w6/chain-sepolia):
#   witness   bridge_cli chain-witness for burn BURN_INDEX of the v4 flow (read only: seq-pub getBlockRange)
#   execute   burn_chain_host in the executor (no proof): cycles, segments, journal = native statement
#   deploy    StubLezAnchor, StubLezTipAnchor and VaultChain pinned at PINNED (DeployChain.s.sol), ETH registered,
#             instance bound; then the witness is pointed at this vault (the journal's vault field)
#   deposit   0.001 ETH into the vault (it pays the release)
#   attest    the STUB tip anchor's owner attests block TIP (its hash read from seq-pub); final after 600 s
#   prove     prints the Groth16 command (run it alone, in a proving window: own scope, prove.lock, time -v)
#   early     releaseChain before the delay ends, sent as a transaction with a fixed gas limit: must revert TipNotFinal
#   release   releaseChain with the real seal; checks the recipient's gain and the spent nullifier
#   again     the same release again, sent with a fixed gas limit: must revert AlreadyReleased
#   record    evidence/bridge/chain-anchor/sepolia.json
#
# Sepolia needs BV_OWNER_D3=yes (the owner's yes); the key comes from lechu-secret and is never printed. Two vaults on
# one LEZ instance (vault v4 and this one) is a test arrangement: it lets one real burn, already released on vault v4,
# be proven again through the new route. Each vault keeps its own nullifier map, so here the burn pays once per
# vault; a production instance has one vault, which holds the funds of that instance's deposits.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$here/../.." && pwd)"
# shellcheck source=scripts/env.sh
source "$ROOT/scripts/env.sh"
RPC="${BV_SEPOLIA_RPC:-https://ethereum-sepolia-rpc.publicnode.com}"
SEQ="${BV_SEQ_RPC:-http://127.0.0.1:3043}"
V4="${BV_FLOW_DIR:-$B/run/w6/v4-sepolia}"
S="${BV_CHAIN_FLOW_DIR:-$B/run/w6/chain-sepolia}"; F="$S/flow.env"
mkdir -p "$S"; touch "$F"
# shellcheck source=/dev/null
source "$F"
CLI="${BV_BRIDGE_CLI:-$B/target/host/bridge_cli}"
HOST="${BURN_CHAIN_HOST:-$CARGO_TARGET_DIR/release/burn_chain_host}"
export RISC0_HOME="${RISC0_HOME:-$B/run/w5/risc0-31}"
export RISC0_SERVER_PATH="${RISC0_SERVER_PATH:-$RISC0_HOME/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm}"
ESTOP=0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29   # RISC Zero 3.1.0 emergency-stop wrapper (Sepolia)
BURN_INDEX="${BURN_INDEX:-0}"
LOG="$S/flow-$(date -u +%Y%m%dT%H%M%SZ)-${1:-none}.log"
exec > >(tee "$LOG") 2>&1

save() { grep -v "^$1=" "$F" > "$F.tmp" || true; echo "$1=$2" >> "$F.tmp"; mv "$F.tmp" "$F"; }
key() {
  [ "${BV_OWNER_D3:-}" = yes ] || { echo "Sepolia needs the owner's yes (BV_OWNER_D3=yes)" >&2; exit 3; }
  sudo -n lechu-secret show logos LOGOS_SEPOLIA_KEY
}
sepolia() { [ "$(cast chain-id --rpc-url "$RPC")" = 11155111 ] || { echo "RPC is not Sepolia" >&2; exit 1; }; }
send() { # file-stem, cast send args...: tx json saved, then printed as hash, gas, status
  local stem="$1"; shift
  cast send --rpc-url "$RPC" --private-key "$(key)" --json "$@" > "$S/$stem.json"
  jq -r '"\(.transactionHash) gas=\(.gasUsed) block=\(.blockNumber) status=\(.status)"' "$S/$stem.json"
}
block_hash() { # LEZ block id -> 0x header hash, read from seq-pub (borsh: id u64, prev hash, hash, ...)
  curl -s -X POST -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlock\",\"params\":[$1]}" "$SEQ" \
    | python3 -I -c 'import base64,json,sys; print("0x"+base64.b64decode(json.load(sys.stdin)["result"])[40:72].hex())'
}
host_prove() { # mode out [extra...]
  local mode="$1" out="$2"; shift 2
  "$HOST" prove --mode "$mode" --witness-json "$S/witness/witness.json" --dir "$S/witness/blocks" \
    --pinned "$PINNED" --tip "$TIP" --burn-block "$BURN_BLOCK" --tx "$BURN_TX_INDEX" --out "$out" "$@"
}

case "${1:-}" in
witness)
  BURN_TXH=$(cat "$V4/burn-tx.txt"); K=$(jq -r .block_id "$V4/burn-tx.json")
  P="${PINNED:-$((K - 5))}"; T="${TIP:-$((K + 5))}"
  # shellcheck source=/dev/null
  source <(grep '^export ' "$V4/wallet.env")
  BV_BRIDGE_CONFIG="$V4/config.json" BV_BRIDGE_MNEMONIC_FILE="${BV_BRIDGE_MNEMONIC_FILE:-$here/testdata/TEST-ONLY-mnemonic.txt}" \
    "$CLI" chain-witness --index "$BURN_INDEX" --burn-tx "$BURN_TXH" --pinned "$P" --tip "$T" --seq-rpc "$SEQ" \
    --out "$S/witness" > /dev/null
  C="$S/witness/chain.json"; jq 'del(.witness_file, .prove)' "$C"
  save PINNED "$P"; save TIP "$T"; save BURN_BLOCK "$(jq -r .burn_block "$C")"; save BURN_TX_INDEX "$(jq -r .tx "$C")"
  save PINNED_HASH "$(jq -r .pinned_hash "$C")"; save TIP_HASH "$(jq -r .tip_hash "$C")"; save BURN_TXH "$BURN_TXH" ;;
execute)
  host_prove execute "$S/execute-${2:-v4vault}" | jq -c '{cycles_user, segments, input_bytes, block_bytes, journal_equals_native}' ;;
deploy)
  [ -z "${VAULT:-}" ] || { echo "vault exists ($VAULT): not deploying another" >&2; exit 0; }
  [ -z "${VAULT_ATTEMPT:-}" ] || { echo "a deploy was attempted at $VAULT_ATTEMPT without a recorded result" >&2; exit 1; }
  sepolia
  save VAULT_ATTEMPT "$(date -u +%FT%TZ)"
  (cd "$ROOT/contracts" && PRIVATE_KEY="$(key)" VERIFIER=$ESTOP BURN_IMAGE_ID=0x$(printf '%064x' 1) \
    CHAIN_IMAGE_ID="$("$HOST" image-id)" LEZ_PROGRAM="$(jq -r .bridge_program_hex "$V4/config.json")" \
    LEZ_INSTANCE="$(jq -r .instance_hex "$V4/config.json")" PINNED_ID="$PINNED" PINNED_HASH="$PINNED_HASH" \
    ANCHOR_DELAY=600 nice -n 15 forge script script/DeployChain.s.sol:DeployChain --rpc-url "$RPC" --broadcast --slow) \
    > "$S/forge-deploy.txt" 2>&1
  cp "$ROOT/contracts/broadcast/DeployChain.s.sol/11155111/run-latest.json" "$S/broadcast-deploy.json"
  out=$(grep -o '{"admin".*}' "$S/forge-deploy.txt" | tail -1); echo "$out" > "$S/deploy.json"; echo "$out"
  save VAULT "$(jq -r .vault <<<"$out")"; save TIP_ANCHOR "$(jq -r .tip_anchor_stub <<<"$out")"
  [ "$(jq -r .verifier <<<"$out")" = "$ESTOP" ] || { echo "verifier is not the 3.1.0 estop wrapper" >&2; exit 1; }
  jq -r '.receipts[] | "\(.transactionHash) gas=\(.gasUsed) block=\(.blockNumber) status=\(.status)"' "$S/broadcast-deploy.json"
  # The journal's vault field must be this vault (releaseChain checks vault == address(this)).
  v=$(jq -r .vault <<<"$out"); jq --arg v "${v#0x}" '.vault = ($v | ascii_downcase)' "$S/witness/witness.json" \
    > "$S/witness/witness.tmp" && chmod 600 "$S/witness/witness.tmp" && mv "$S/witness/witness.tmp" "$S/witness/witness.json" ;;
deposit)
  : "${VAULT:?deploy first}"; [ -z "${DEPOSIT_TX:-}" ] || { echo "deposit exists ($DEPOSIT_TX)" >&2; exit 0; }
  sepolia
  send deposit --value 0.001ether "$VAULT" "deposit(address,uint256,bytes32)" 0x0000000000000000000000000000000000000000 0 \
    "$(cast keccak "chain-route funding $VAULT")"
  save DEPOSIT_TX "$(jq -r .transactionHash "$S/deposit.json")" ;;
attest)
  : "${TIP_ANCHOR:?deploy first}"; [ -z "${ATTEST_TX:-}" ] || { echo "tip attested ($ATTEST_TX)" >&2; exit 0; }
  sepolia
  h=$(block_hash "$TIP"); [ "$h" = "$TIP_HASH" ] || { echo "seq-pub block $TIP hash $h differs from the witness $TIP_HASH" >&2; exit 1; }
  send attest "$TIP_ANCHOR" "attestTip(uint64,bytes32)" "$TIP" "$TIP_HASH"
  save ATTEST_TX "$(jq -r .transactionHash "$S/attest.json")" ;;
prove)
  echo "run alone, in a proving window (own scope, prove.lock, /usr/bin/time -v, memory check first):"
  echo "  $HOST prove --mode groth16 --witness-json $S/witness/witness.json --dir $S/witness/blocks --pinned $PINNED" \
    "--tip $TIP --burn-block $BURN_BLOCK --tx $BURN_TX_INDEX --segment-po2 18 --out $S/groth16/proof" ;;
early|release|again)
  : "${VAULT:?}"; sepolia
  D="$S/groth16/proof"; SEAL=$(tr -d '\n' < "$D/seal.hex"); JOURNAL=$(tr -d '\n' < "$D/journal.hex")
  ME=$(cast wallet address --private-key "$(key)")
  case "$1" in
    release)
      R=0x$(jq -r .recipient "$S/witness/witness.json"); r0=$(cast balance --rpc-url "$RPC" "$R")
      send release "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL"
      save RELEASE_TX "$(jq -r .transactionHash "$S/release.json")"
      blk=$(jq -r .blockNumber "$S/release.json"); prev=$(( $(cast to-dec "$blk") - 1 ))
      echo "recipient $R, relayer $ME (the same address here: its gain is the payout minus the gas)"
      echo "vault balance before $(cast balance --rpc-url "$RPC" --block "$prev" "$VAULT"), after $(cast balance --rpc-url "$RPC" --block "$blk" "$VAULT")"
      echo "recipient balance before $r0, after $(cast balance --rpc-url "$RPC" "$R")"
      echo "nullifier spent: $(cast call --rpc-url "$RPC" "$VAULT" "burnNullifierSpent(bytes32)(bool)" 0x"$(jq -r .nullifier "$D/meta.json")")" ;;
    early|again)
      # Sent as a real transaction with a fixed gas limit (no estimate), so the refusal is on chain.
      want=TipNotFinal; [ "$1" = again ] && want=AlreadyReleased
      sel=$(cast sig "$want()")
      reason=$(cast call --rpc-url "$RPC" --from "$ME" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL" 2>&1 || true)
      grep -q "$sel" <<<"$reason" || { echo "expected $want ($sel), simulated: $reason" >&2; exit 1; }
      send "refuse-$1" --gas-limit 300000 "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL" || true
      save "REFUSE_${1^^}_TX" "$(jq -r .transactionHash "$S/refuse-$1.json")"
      echo "simulated revert: $want ($sel)" ;;
  esac ;;
record)
  out="${2:-$ROOT/evidence/bridge/chain-anchor/sepolia.json}"
  python3 -I "$here/chain_anchor_record.py" "$S" "$RPC" > "$out"; cat "$out" ;;
*) sed -n '2,24p' "$0"; exit 2 ;;
esac
