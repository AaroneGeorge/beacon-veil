#!/usr/bin/env bash
# The chain route on Sepolia through CommitteeLezAnchor (3 of 4, stand-in key set) for the real burn on seq-pub that
# chain_anchor_sepolia.sh released through the one-key stub: a third test vault, the same burn, pinned block and tip.
#
#   tools/bridge_cli/committee_anchor_sepolia.sh STAGE
#
# Stages, in order (state in $BV_COMMITTEE_FLOW_DIR/flow.env, default $B/run/w6/committee-sepolia):
#   keys        4 fresh secp256k1 test keys (lechu-secret logos COMMITTEE_TEST_KEY_1..4 and key files, mode 600,
#               sorted by address); only the addresses are saved and printed
#   witness     bridge_cli chain-witness for the same burn, pinned and tip blocks as the chain-route flow (read only:
#               seq-pub getBlockRange); its hashes must equal that flow's
#   deploy      DeployCommittee.s.sol: CommitteeLezAnchor (threshold 3, delay 600) as both anchors of a VaultChain
#               pinned at PINNED, the RISC Zero 3.1.0 emergency stop as verifier; then the witness names this vault
#   deposit     0.001 ETH into the vault (it pays the release)
#   execute     burn_chain_host in the executor (no proof): cycles, segments, journal = native statement
#   prove       prints the Groth16 command (run it alone: own scope, prove.lock, time -v, memory check first)
#   sign        four runs of committee_sign.sh, one per key, each reading seq-pub and checking PINNED..TIP itself
#   refuse-few | refuse-outsider | refuse-twice | refuse-replay
#               attestTip with 2 signatures; 2 members and a fresh key outside the set; one member twice; the tip's
#               signatures submitted for block TIP-1: each simulated (selector checked), then sent with a fixed gas
#               limit, so the refusal is a mined transaction
#   attest      attestTip with 3 of the 4 signatures, sent by the deployer (not a signer: anyone may submit)
#   refuse-again-attest   the same attestTip again: AlreadyAttested
#   early       releaseChain before the delay ends: TipNotFinal
#   release     releaseChain with the real seal after the delay; recipient paid, nullifier spent
#   again       the same release again: AlreadyReleased
#   record      evidence/bridge/committee-anchor/sepolia.json
#
# Sepolia needs BV_OWNER_D3=yes (the owner's yes); the deployer key comes from lechu-secret and is never printed, nor
# are the signer keys. Three vaults on one LEZ instance (vault v4, the chain-route test vault and this one) is a test
# arrangement: each keeps its own nullifier map and pays from its own deposit, so one real burn pays once per vault.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$here/../.." && pwd)"
# shellcheck source=scripts/env.sh
source "$ROOT/scripts/env.sh"
RPC="${BV_SEPOLIA_RPC:-https://ethereum-sepolia-rpc.publicnode.com}"
SEQ="${BV_SEQ_RPC:-http://127.0.0.1:3043}"
V4="${BV_FLOW_DIR:-$B/run/w6/v4-sepolia}"
CF="${BV_CHAIN_FLOW_DIR:-$B/run/w6/chain-sepolia}/flow.env"
S="${BV_COMMITTEE_FLOW_DIR:-$B/run/w6/committee-sepolia}"; F="$S/flow.env"
mkdir -p "$S"; touch "$F"
# shellcheck source=/dev/null
source "$F"
CLI="${BV_BRIDGE_CLI:-$B/target/host/bridge_cli}"
HOST="${BURN_CHAIN_HOST:-$CARGO_TARGET_DIR/release/burn_chain_host}"
export RISC0_HOME="${RISC0_HOME:-$B/run/w5/risc0-31}"
export RISC0_SERVER_PATH="${RISC0_SERVER_PATH:-$RISC0_HOME/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm}"
ESTOP=0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29   # RISC Zero 3.1.0 emergency-stop wrapper (Sepolia)
THRESHOLD=3
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
host_prove() { # mode out [extra...]
  local mode="$1" out="$2"; shift 2
  "$HOST" prove --mode "$mode" --witness-json "$S/witness/witness.json" --dir "$S/witness/blocks" \
    --pinned "$PINNED" --tip "$TIP" --burn-block "$BURN_BLOCK" --tx "$BURN_TX_INDEX" --out "$out" "$@"
}
sig() { cat "$S/sigs/signer-$1.sig"; } # signature of signer 1..4 (ascending address)
arr() { local IFS=,; echo "[$*]"; }
ATTEST="attestTip(uint64,bytes32,bytes[])"
refuse() { # stem flow-key error-signature target sig args...: simulate and check the selector, then send
  local stem="$1" k="$2" err="$3"; shift 3
  [ -z "${!k:-}" ] || { echo "$stem exists (${!k})" >&2; exit 0; }
  sepolia
  local sel reason me
  sel=$(cast sig "$err"); me=$(cast wallet address --private-key "$(key)")
  reason=$(cast call --rpc-url "$RPC" --from "$me" "$@" 2>&1 || true)
  grep -q "$sel" <<<"$reason" || { echo "expected $err ($sel), simulated: $reason" >&2; exit 1; }
  echo "simulated revert: $(grep -m1 -oE 'custom error 0x[0-9a-fA-F]+.*|reverted.*' <<<"$reason")"
  send "$stem" --gas-limit 300000 "$@" || true
  save "$k" "$(jq -r .transactionHash "$S/$stem.json")"
  [ "$(jq -r .status "$S/$stem.json")" = 0x0 ] || { echo "$stem: expected status 0" >&2; exit 1; }
}

case "${1:-}" in
keys)
  [ -z "${SIGNERS:-}" ] || { echo "keys exist: $SIGNERS" >&2; exit 0; }
  mkdir -p "$S/keys"; chmod 700 "$S/keys"
  tmp=$(mktemp -d "$S/keys/new.XXXX")
  for i in 1 2 3 4; do (umask 077; cast wallet new --json | jq -r '.data[0].private_key' > "$tmp/$i"); done
  # Number the keys by ascending address, then store each in lechu-secret and as a key file.
  order=$(for i in 1 2 3 4; do echo "$(cast wallet address --private-key "$(cat "$tmp/$i")") $i"; done \
    | python3 -I -c 'import sys; [print(l.strip()) for l in sorted(sys.stdin, key=lambda l: int(l.split()[0], 16))]')
  n=0; addrs=()
  while read -r a i; do
    n=$((n + 1)); mv "$tmp/$i" "$S/keys/signer-$n.key"; chmod 600 "$S/keys/signer-$n.key"
    # shellcheck disable=SC2024 # the key file is ours to read; lechu-secret reads it from stdin
    sudo -n lechu-secret set logos "COMMITTEE_TEST_KEY_$n" < "$S/keys/signer-$n.key" > /dev/null
    addrs+=("$a"); echo "signer $n: $a"
  done <<<"$order"
  rmdir "$tmp"
  save SIGNERS "$(IFS=,; echo "${addrs[*]}")" ;;
witness)
  # shellcheck source=/dev/null
  source <(grep -E '^(PINNED|TIP|BURN_BLOCK|BURN_TX_INDEX|PINNED_HASH|TIP_HASH|BURN_TXH)=' "$CF" | sed 's/^/OLD_/')
  # shellcheck source=/dev/null
  source <(grep '^export ' "$V4/wallet.env")
  mkdir -p "$S/witness"; chmod 700 "$S/witness"
  BV_BRIDGE_CONFIG="$V4/config.json" BV_BRIDGE_MNEMONIC_FILE="${BV_BRIDGE_MNEMONIC_FILE:-$here/testdata/TEST-ONLY-mnemonic.txt}" \
    "$CLI" chain-witness --index "${BURN_INDEX:-0}" --burn-tx "$OLD_BURN_TXH" --pinned "$OLD_PINNED" --tip "$OLD_TIP" \
    --seq-rpc "$SEQ" --out "$S/witness" > /dev/null
  chmod 600 "$S/witness/witness.json"
  C="$S/witness/chain.json"; jq 'del(.witness_file, .prove)' "$C"
  for k in pinned_hash tip_hash burn_block tx; do
    o=$(case $k in pinned_hash) echo "$OLD_PINNED_HASH" ;; tip_hash) echo "$OLD_TIP_HASH" ;; burn_block) echo "$OLD_BURN_BLOCK" ;;
      tx) echo "$OLD_BURN_TX_INDEX" ;; esac)
    [ "$(jq -r ".$k" "$C")" = "$o" ] || { echo "$k differs from the chain-route flow ($o)" >&2; exit 1; }
  done
  save PINNED "$OLD_PINNED"; save TIP "$OLD_TIP"; save BURN_BLOCK "$OLD_BURN_BLOCK"; save BURN_TX_INDEX "$OLD_BURN_TX_INDEX"
  save PINNED_HASH "$OLD_PINNED_HASH"; save TIP_HASH "$OLD_TIP_HASH"; save BURN_TXH "$OLD_BURN_TXH"
  echo "witness hashes equal the chain-route flow's" ;;
deploy)
  : "${SIGNERS:?keys first}" "${PINNED_HASH:?witness first}"
  [ -z "${VAULT:-}" ] || { echo "vault exists ($VAULT): not deploying another" >&2; exit 0; }
  [ -z "${VAULT_ATTEMPT:-}" ] || { echo "a deploy was attempted at $VAULT_ATTEMPT without a recorded result" >&2; exit 1; }
  sepolia
  save VAULT_ATTEMPT "$(date -u +%FT%TZ)"
  (cd "$ROOT/contracts" && PRIVATE_KEY="$(key)" VERIFIER=$ESTOP SIGNERS="$SIGNERS" THRESHOLD=$THRESHOLD \
    BURN_IMAGE_ID=0x$(printf '%064x' 1) CHAIN_IMAGE_ID="$("$HOST" image-id)" \
    LEZ_PROGRAM="$(jq -r .bridge_program_hex "$V4/config.json")" LEZ_INSTANCE="$(jq -r .instance_hex "$V4/config.json")" \
    PINNED_ID="$PINNED" PINNED_HASH="$PINNED_HASH" ANCHOR_DELAY=600 \
    nice -n 15 forge script script/DeployCommittee.s.sol:DeployCommittee --rpc-url "$RPC" --broadcast --slow) \
    > "$S/forge-deploy.txt" 2>&1
  cp "$ROOT/contracts/broadcast/DeployCommittee.s.sol/11155111/run-latest.json" "$S/broadcast-deploy.json"
  out=$(grep -o '{"admin".*}' "$S/forge-deploy.txt" | tail -1); echo "$out" > "$S/deploy.json"; echo "$out"
  save VAULT "$(jq -r .vault <<<"$out")"; save ANCHOR "$(jq -r .committee_anchor <<<"$out")"
  [ "$(jq -r .verifier <<<"$out")" = "$ESTOP" ] || { echo "verifier is not the 3.1.0 estop wrapper" >&2; exit 1; }
  jq -r '.receipts[] | "\(.transactionHash) gas=\(.gasUsed) block=\(.blockNumber) status=\(.status)"' "$S/broadcast-deploy.json"
  # The journal's vault field must be this vault (releaseChain checks vault == address(this)).
  v=$(jq -r .vault <<<"$out"); jq --arg v "${v#0x}" '.vault = ($v | ascii_downcase)' "$S/witness/witness.json" \
    > "$S/witness/witness.tmp" && chmod 600 "$S/witness/witness.tmp" && mv "$S/witness/witness.tmp" "$S/witness/witness.json" ;;
deposit)
  : "${VAULT:?deploy first}"; [ -z "${DEPOSIT_TX:-}" ] || { echo "deposit exists ($DEPOSIT_TX)" >&2; exit 0; }
  sepolia
  send deposit --value 0.001ether "$VAULT" "deposit(address,uint256,bytes32)" 0x0000000000000000000000000000000000000000 0 \
    "$(cast keccak "committee-route funding $VAULT")"
  save DEPOSIT_TX "$(jq -r .transactionHash "$S/deposit.json")" ;;
execute)
  host_prove execute "$S/execute-vault" | jq -c '{cycles_user, segments, input_bytes, block_bytes, journal_equals_native}' ;;
prove)
  echo "run alone, in a proving window (own scope, prove.lock, /usr/bin/time -v, memory check first):"
  echo "  $HOST prove --mode groth16 --witness-json $S/witness/witness.json --dir $S/witness/blocks --pinned $PINNED" \
    "--tip $TIP --burn-block $BURN_BLOCK --tx $BURN_TX_INDEX --segment-po2 18 --out $S/groth16/proof" ;;
sign)
  : "${ANCHOR:?deploy first}"; mkdir -p "$S/sigs"
  for n in 1 2 3 4; do
    "$here/committee_sign.sh" "$SEQ" "$RPC" "$ANCHOR" "$TIP" "$PINNED" "$PINNED_HASH" "$S/keys/signer-$n.key" \
      > "$S/sigs/signer-$n.sig"
  done
  echo "4 signatures in $S/sigs" ;;
refuse-few)
  refuse refuse-few REFUSE_FEW_TX "TooFewSignatures()" "$ANCHOR" "$ATTEST" "$TIP" "$TIP_HASH" "$(arr "$(sig 1)" "$(sig 2)")" ;;
refuse-outsider)
  [ -n "${OUTSIDER:-}" ] || {
    # A fresh key outside the set signs the same digest; the key is not kept, only its address.
    d=$(cast call --rpc-url "$RPC" "$ANCHOR" "tipDigest(uint64,bytes32)(bytes32)" "$TIP" "$TIP_HASH")
    k=$(cast wallet new --json | jq -r '.data[0].private_key')
    save OUTSIDER "$(cast wallet address --private-key "$k")"
    cast wallet sign --no-hash --private-key "$k" "$d" > "$S/sigs/outsider.sig"; unset k
  }
  refuse refuse-outsider REFUSE_OUTSIDER_TX "NotASigner(address)" "$ANCHOR" "$ATTEST" "$TIP" "$TIP_HASH" \
    "$(arr "$(sig 1)" "$(sig 2)" "$(cat "$S/sigs/outsider.sig")")" ;;
refuse-twice)
  refuse refuse-twice REFUSE_TWICE_TX "SignersNotAscending()" "$ANCHOR" "$ATTEST" "$TIP" "$TIP_HASH" \
    "$(arr "$(sig 1)" "$(sig 1)" "$(sig 2)")" ;;
refuse-replay)
  refuse refuse-replay REFUSE_REPLAY_TX "NotASigner(address)" "$ANCHOR" "$ATTEST" "$((TIP - 1))" "$TIP_HASH" \
    "$(arr "$(sig 1)" "$(sig 2)" "$(sig 3)")" ;;
attest)
  : "${ANCHOR:?deploy first}"; [ -z "${ATTEST_TX:-}" ] || { echo "tip attested ($ATTEST_TX)" >&2; exit 0; }
  sepolia
  send attest "$ANCHOR" "$ATTEST" "$TIP" "$TIP_HASH" "$(arr "$(sig 2)" "$(sig 3)" "$(sig 4)")"
  save ATTEST_TX "$(jq -r .transactionHash "$S/attest.json")"
  [ "$(jq -r .status "$S/attest.json")" = 0x1 ] || { echo "attest failed" >&2; exit 1; } ;;
refuse-again-attest)
  refuse refuse-again-attest REFUSE_AGAIN_ATTEST_TX "AlreadyAttested()" "$ANCHOR" "$ATTEST" "$TIP" "$TIP_HASH" \
    "$(arr "$(sig 1)" "$(sig 2)" "$(sig 3)")" ;;
early | again)
  D="$S/groth16/proof"; SEAL=$(tr -d '\n' < "$D/seal.hex"); JOURNAL=$(tr -d '\n' < "$D/journal.hex")
  if [ "$1" = early ]; then
    refuse refuse-early REFUSE_EARLY_TX "TipNotFinal()" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL"
  else
    refuse refuse-again REFUSE_AGAIN_TX "AlreadyReleased()" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL"
  fi ;;
release)
  : "${VAULT:?}"; [ -z "${RELEASE_TX:-}" ] || { echo "released ($RELEASE_TX)" >&2; exit 0; }
  sepolia
  [ "$(cast call --rpc-url "$RPC" "$ANCHOR" "isFinalTip(uint64,bytes32)(bool)" "$TIP" "$TIP_HASH")" = true ] \
    || { echo "tip not final yet" >&2; exit 1; }
  D="$S/groth16/proof"; SEAL=$(tr -d '\n' < "$D/seal.hex"); JOURNAL=$(tr -d '\n' < "$D/journal.hex")
  ME=$(cast wallet address --private-key "$(key)")
  R=0x$(jq -r .recipient "$S/witness/witness.json"); r0=$(cast balance --rpc-url "$RPC" "$R")
  send release "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL"
  save RELEASE_TX "$(jq -r .transactionHash "$S/release.json")"
  blk=$(jq -r .blockNumber "$S/release.json"); prev=$(( $(cast to-dec "$blk") - 1 ))
  echo "recipient $R, relayer $ME (the same address here: its gain is the payout minus the gas)"
  echo "vault balance before $(cast balance --rpc-url "$RPC" --block "$prev" "$VAULT"), after $(cast balance --rpc-url "$RPC" --block "$blk" "$VAULT")"
  echo "recipient balance before $r0, after $(cast balance --rpc-url "$RPC" "$R")"
  echo "nullifier spent: $(cast call --rpc-url "$RPC" "$VAULT" "burnNullifierSpent(bytes32)(bool)" 0x"$(jq -r .nullifier "$D/meta.json")")" ;;
record)
  out="${2:-$ROOT/evidence/bridge/committee-anchor/sepolia.json}"
  python3 -I "$here/committee_anchor_record.py" "$S" "$RPC" > "$out"; cat "$out" ;;
*) sed -n '2,32p' "$0"; exit 2 ;;
esac
