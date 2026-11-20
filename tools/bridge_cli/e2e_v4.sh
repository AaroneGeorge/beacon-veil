#!/usr/bin/env bash
# Bridge round trip with real proofs: vault v4 on Ethereum, LEZ side on seq-pub (RISC0_DEV_MODE unset), inbound
# attestation through the Sepolia instance that run E keeps current. In stages; state in $S/flow.env.
#
#   Ethereum side                                 LEZ side (seq-pub; claim, burn and funding are real proofs)
#   e2e_v4.sh check        preconditions, plan     e2e_v4.sh wallet      wallet home: public payer, private claimer
#   e2e_v4.sh vault        vault v4 + STUB anchor  e2e_v4.sh fund        fund the private claimer (real proof)
#   e2e_v4.sh config       bridge config, bind     e2e_v4.sh claim       attest-root, private claim (real proof),
#   e2e_v4.sh deposit      0.001 ETH deposit                             repeat claim refused (E11, no proof)
#   e2e_v4.sh post-root    burn root to the STUB   e2e_v4.sh burn        private burn (real proof)
#   e2e_v4.sh release      Vault.release (Groth16) e2e_v4.sh prove       burn_proof Groth16 (RISC Zero 3.1.0)
#   e2e_v4.sh wait-anchor  until the STUB delay passed
#   e2e_v4.sh wait-record  until run E's instance holds a record past the deposit block (bridge_cli instance-head)
#   e2e_v4.sh record-claim | record-burn          evidence/bridge/real-claim-v4/record.json, real-burn-v4/record.json
#                                                 (an incomplete record goes to <record>.incomplete, exit non-zero)
# vault and deposit never repeat for a flow (BV_FORCE_NEW_VAULT=1 / BV_FORCE_NEW_DEPOSIT=1 to do it on purpose).
#
# Targets (BV_V4_TARGET):
#   anvil-fork (default)  an anvil fork of Sepolia on 127.0.0.1:8548 (the 3.1.0 emergency-stop wrapper exists there),
#                         anvil dev key 0 (a public test key). LEZ stages are refused: a fork's deposits cannot be
#                         attested by a light client of the real chain.
#   sepolia               Sepolia itself. Needs BV_OWNER_D3=yes (the owner's yes) and the key in lechu-secret
#                         (logos LOGOS_SEPOLIA_KEY), read for the forge calls only, never printed or written.
# Real-proof stages run through tools/cost/prove_job.sh (BV_PROOF_WINDOW_OK=1, proving lock, time -v, RSS samples).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
R="$(cd "$here/../.." && pwd)"
# shellcheck source=scripts/env.sh
source "$R/scripts/env.sh"
unset RISC0_DEV_MODE
TARGET="${BV_V4_TARGET:-anvil-fork}"
SEPOLIA_RPC="${BV_SEPOLIA_RPC:-https://ethereum-sepolia-rpc.publicnode.com}"
case "$TARGET" in
  anvil-fork) RPC="http://127.0.0.1:8548"; S="${BV_FLOW_DIR:-$B/run/w6/v4-anvil-fork}" ;;
  sepolia) RPC="$SEPOLIA_RPC"; S="${BV_FLOW_DIR:-$B/run/w6/v4-sepolia}" ;;
  *) echo "BV_V4_TARGET: anvil-fork or sepolia" >&2; exit 2 ;;
esac
case "$TARGET:${1:-}" in
  sepolia:check | sepolia:record-claim | sepolia:record-burn | sepolia:) ;;
  sepolia:*) [ "${BV_OWNER_D3:-}" = yes ] || { echo "Sepolia stage '$1' needs the owner's yes (BV_OWNER_D3=yes)" >&2; exit 3; } ;;
esac
mkdir -p "$S"; F="$S/flow.env"; touch "$F"
# shellcheck source=/dev/null
source "$F"
export BV_ETH_RPC="$RPC"
export LEE_WALLET_HOME_DIR="${BV_V4_WALLET:-$B/run/wallet-w6-bridge}" LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"
export BV_IDS="${BV_IDS_PUB:-$R/deploy/ids-pub.toml}"
export BV_BRIDGE_MNEMONIC_FILE="${BV_BRIDGE_MNEMONIC_FILE:-$here/testdata/TEST-ONLY-mnemonic.txt}"
CLI="${BV_BRIDGE_CLI:-$B/target/host/bridge_cli}"
ATTEST="${BV_ATTEST:-$B/target/host/attest}"
ATTEST_INSTANCE="${ATTEST_INSTANCE:-5ydZVRKHVECxKtXBwHnW9N6bndDADwZoGSx5tDsp4UXN}"   # run E (Sepolia, V3 only)
R31_ENV=(RISC0_HOME="$B/run/w5/risc0-31" CARGO_TARGET_DIR="$B/target-w5-r31"
  RISC0_SERVER_PATH="$B/run/w5/risc0-31/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm")
R31_BIN="$B/target-w5-r31/release/burn_prover_r31"
VERIFIER_31_ESTOP=0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29   # RISC Zero 3.1.0 emergency-stop wrapper (Sepolia)
ANVIL_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80   # anvil dev key 0 (public)
PROVE_JOB="$R/tools/cost/prove_job.sh"
LOG="$S/flow-$(date -u +%Y%m%dT%H%M%SZ)-${1:-none}.log"
exec > >(tee "$LOG") 2>&1

save() { grep -v "^$1=" "$F" > "$F.tmp" || true; echo "$1=$2" >> "$F.tmp"; mv "$F.tmp" "$F"; }
prog_id() { awk -v k="$1" '/^\[programs\]/{f=1; next} /^\[/{f=0} f && $1==k {split($0,a,"\""); print a[2]; exit}' "$BV_IDS"; }
b58hex() { python3 -I -c 'import sys
a="123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";n=0
for c in sys.argv[1]: n=n*58+a.index(c)
print("0x%064x"%n)' "$1"; }
label_id() { wallet account list 2>/dev/null | grep "\[$1\]" | grep -o '\(Public\|Private\)/[A-Za-z0-9]*' | cut -d/ -f2; }
BRIDGE=$(prog_id bridge); ATTEST_PROG=$(prog_id attest)

key() {
  case "$TARGET" in
    anvil-fork) echo "$ANVIL_KEY" ;;
    sepolia)
      [ "${BV_OWNER_D3:-}" = yes ] || { echo "Sepolia needs the owner's yes (BV_OWNER_D3=yes)" >&2; exit 3; }
      sudo -n lechu-secret show logos LOGOS_SEPOLIA_KEY ;;
  esac
}
chain_id() { cast chain-id --rpc-url "$RPC"; }
ensure_anvil() {
  [ "$TARGET" = anvil-fork ] || return 0
  cast chain-id --rpc-url "$RPC" >/dev/null 2>&1 && return 0
  # chain id 31337: forge writes its broadcast records under broadcast/*/31337 (git-ignored), not over the Sepolia ones
  setsid nohup anvil --fork-url "$SEPOLIA_RPC" --chain-id 31337 --port 8548 --silent > "$S/anvil.log" 2>&1 < /dev/null &
  echo $! > "$S/anvil.pid"
  for _ in $(seq 1 30); do cast chain-id --rpc-url "$RPC" >/dev/null 2>&1 && return 0; sleep 1; done
  echo "anvil fork did not start ($S/anvil.log)" >&2; exit 1
}
forge_s() { # contract -> broadcast copy; prints tx, gas, block, status
  local c="$1" stamp; stamp=$(date -u +%H%M%S)
  (cd "$R/contracts" && PRIVATE_KEY="$(key)" nice -n 15 forge script "script/Deploy.s.sol:$c" \
    --rpc-url "$RPC" --broadcast --slow) > "$S/forge-$c-$stamp.txt" 2>&1
  cp "$R/contracts/broadcast/Deploy.s.sol/$(chain_id)/run-latest.json" "$S/broadcast-$c-$stamp.json"
  cp "$S/broadcast-$c-$stamp.json" "$S/broadcast-$c-latest.json"
  jq -r '.receipts[] | "\(.transactionHash) gas=\(.gasUsed) block=\(.blockNumber) status=\(.status)"' "$S/broadcast-$c-$stamp.json"
}
lez_only_on_sepolia() {
  [ "$TARGET" = sepolia ] || { echo "LEZ stage '$1' needs BV_V4_TARGET=sepolia (deposits on a fork cannot be attested)" >&2; exit 3; }
}
proof_job() { # name note -- command...: a real proof under the proving lock; prints the job dir
  local name="$1" note="$2"; shift 3
  local job; job="$S/prove-$name-$(date -u +%Y%m%dT%H%M%SZ)"
  BV_REAL_PROOF=1 "$PROVE_JOB" --wait --foreground --name "$name" --out "$job" --note "$note" -- \
    "$R/scripts/proof/no_dev_mode.sh" "$@" >&2 || return 1
  echo "$job"
}

case "${1:-}" in
check)
  fail=0
  echo "target $TARGET, rpc $RPC, flow dir $S"
  [ -n "$BRIDGE" ] && [ -n "$ATTEST_PROG" ] || { echo "bridge or attest missing from $BV_IDS" >&2; fail=1; }
  [ -x "$CLI" ] || { echo "$CLI missing (cargo build --profile host -p bridge_cli)" >&2; fail=1; }
  [ -x "$R31_BIN" ] || { echo "$R31_BIN missing" >&2; fail=1; }
  [ -x "$ATTEST" ] || { echo "$ATTEST missing" >&2; fail=1; }
  "$R/scripts/seq.sh" pub status >/dev/null 2>&1 || { echo "seq-pub not running" >&2; fail=1; }
  if [ -x "$R31_BIN" ]; then
    img=$(env "${R31_ENV[@]}" "$R31_BIN" image-id)
    [ "$img" = 0xf929f9f441d9aa652b9914b4ce6c6a7b445aa90d189c075af1e1e9fe3b1497cf ] || { echo "burn image $img" >&2; fail=1; }
    echo "burn_proof image id $img (unchanged)"
  fi
  ( source <(grep '^export ' "$B/run/w4/run-e/wallet.env" 2>/dev/null)
    "$ATTEST" --chain-id 11155111 status --instance "$ATTEST_INSTANCE" 2>/dev/null | jq -c '{instance, head: .head.slot}' ) \
    || { echo "run E instance $ATTEST_INSTANCE not readable" >&2; fail=1; }
  [ -f "$LEE_WALLET_HOME_DIR/wallet_config.json" ] && echo "wallet $LEE_WALLET_HOME_DIR: payer $(label_id w6-payer) claimer $(label_id w6-claimer)" \
    || echo "wallet $LEE_WALLET_HOME_DIR not created yet (stage wallet)"
  echo "bridge $BRIDGE, attest $ATTEST_PROG, lez_program $( [ -n "$BRIDGE" ] && b58hex "$BRIDGE")"
  exit "$fail" ;;
vault)
  # Never a second vault (and anchor and token) for the same flow; BV_FORCE_NEW_VAULT=1 to start over on purpose.
  [ -z "${VAULT:-}" ] || [ "${BV_FORCE_NEW_VAULT:-0}" = 1 ] || { echo "vault exists ($VAULT): not deploying another" >&2; exit 0; }
  ensure_anvil
  IMAGE=$(env "${R31_ENV[@]}" "$R31_BIN" image-id)
  export VERIFIER=$VERIFIER_31_ESTOP BURN_IMAGE_ID=$IMAGE LEZ_PROGRAM=$(b58hex "$BRIDGE") ANCHOR_DELAY=600
  forge_s Deploy > /dev/null
  out=$(grep -o '{"admin".*}' "$S"/forge-Deploy-*.txt | tail -1 | sed 's/^[^{]*//')
  echo "$out" | tee "$S/deploy-v4.json"
  [ "$(jq -r .verifier <<<"$out")" = "$VERIFIER_31_ESTOP" ] || { echo "verifier is not the 3.1.0 estop wrapper" >&2; exit 1; }
  [ "$(jq -r .burn_image_id <<<"$out")" = "$IMAGE" ] || { echo "burn image id differs" >&2; exit 1; }
  save VAULT "$(jq -r .vault <<<"$out")"; save ANCHOR "$(jq -r .anchor_stub <<<"$out")"
  save USDT "$(jq -r .test_usdt <<<"$out")"; save VAULT_BLOCK "$(jq -r .block <<<"$out")"; save CHAIN_ID "$(chain_id)" ;;
config)
  : "${VAULT:?run the vault stage first}"
  "$CLI" config --out "$S/config.json" --bridge-program "$BRIDGE" --chain-id 11155111 --vault "$VAULT" \
    --vault-block "$VAULT_BLOCK" --anchor "$ANCHOR" --attest-program "$ATTEST_PROG" --attest-instance "$ATTEST_INSTANCE" \
    --token eth:1000000000000000,10000000000000000 --token "$USDT:100000000,1000000000"
  save INSTANCE_HEX "$(jq -r .instance_hex "$S/config.json")"
  ensure_anvil
  export VAULT LEZ_INSTANCE=$(jq -r .instance_hex "$S/config.json")
  bound=$(cast call "$VAULT" 'lezInstance()(bytes32)' --rpc-url "$RPC")
  if [ "$bound" = "$LEZ_INSTANCE" ]; then echo "vault already bound to $LEZ_INSTANCE"
  elif [ "$bound" != 0x0000000000000000000000000000000000000000000000000000000000000000 ]; then
    echo "vault bound to another instance ($bound)" >&2; exit 1
  else forge_s BindInstance; fi
  [ "$(cast call "$VAULT" 'lezInstance()(bytes32)' --rpc-url "$RPC")" = "$(jq -r .instance_hex "$S/config.json")" ] \
    && echo "vault bound to bridge instance $(jq -r .instance_hex "$S/config.json")" ;;
deposit)
  # One deposit per flow: a second one would send another 0.001 ETH. BV_FORCE_NEW_DEPOSIT=1 to send one on purpose.
  [ -z "${DEPOSIT_TX:-}" ] || [ "${BV_FORCE_NEW_DEPOSIT:-0}" = 1 ] || { echo "deposit exists ($DEPOSIT_TX): not sending another" >&2; exit 0; }
  ensure_anvil
  export BV_BRIDGE_CONFIG="$S/config.json"
  CLAIMER="${BV_CLAIMER:-$(label_id w6-claimer)}"
  [ -n "$CLAIMER" ] || { echo "no claimer account (stage wallet; or BV_CLAIMER=<base58> to test)" >&2; exit 1; }
  IDX="${DEPOSIT_INDEX_NEXT:-0}"
  C=$("$CLI" commitment --index "$IDX" --dest "$CLAIMER" --token eth --denom 1000000000000000 | tee "$S/commit-$IDX.json")
  export VAULT TOKEN=0x0000000000000000000000000000000000000000 DENOM_INDEX=0 COMMITMENT=$(jq -r .commitment <<<"$C")
  forge_s DepositOne | tee "$S/deposit-$IDX.txt"
  save DEPOSIT_INDEX "$IDX"; save DEPOSIT_INDEX_NEXT "$((IDX + 1))"
  save DEPOSIT_TX "$(jq -r '.receipts[-1].transactionHash' "$S/broadcast-DepositOne-latest.json")"
  save DEPOSIT_BLOCK "$(jq -r '.receipts[-1].blockNumber' "$S/broadcast-DepositOne-latest.json" | xargs printf '%d')" ;;
wallet)
  lez_only_on_sepolia wallet
  pgrep -x v3_onchain >/dev/null && { echo "a v3_onchain process sends from bv-payer; try later" >&2; exit 1; }
  name="$(basename "$LEE_WALLET_HOME_DIR" | sed 's/^wallet-//')"
  WALLET_CONFIG_FROM="$B/run/wallet-pub" flock -w 600 "$R/deploy/ids-pub.toml.lock" \
    "$R/crates/follower/scripts/wallet_home.sh" "$name" 10000000000 0 | grep '^export ' > "$S/wallet.env"
  label_id w6-claimer >/dev/null || true
  [ -n "$(label_id w6-claimer)" ] || wallet account new private --label w6-claimer
  cat "$S/wallet.env" ;;
wait-record)
  # Until the attest instance holds a record whose execution block is at or after the deposit block. A Gloas head
  # carries no block number, so the block is resolved from the record's execution hash (bridge_cli instance-head).
  lez_only_on_sepolia wait-record
  # shellcheck source=/dev/null
  source <(grep '^export ' "$S/wallet.env")
  export BV_BRIDGE_CONFIG="$S/config.json"
  : "${DEPOSIT_BLOCK:?deposit first}"
  for i in $(seq 1 "${BV_WAIT_RECORD_TRIES:-120}"); do
    h=$("$CLI" instance-head 2>/dev/null || true)
    blk=$(printf '%s' "$h" | jq -r '.eth_block // 0' 2>/dev/null); blk=${blk:-0}
    if [ "$blk" -ge "$DEPOSIT_BLOCK" ]; then echo "record at slot $(jq -r .slot <<<"$h") proves block $blk >= $DEPOSIT_BLOCK"; exit 0; fi
    echo "$(date -u +%T) head proves block $blk, waiting for >= $DEPOSIT_BLOCK ($i)"; sleep 60
  done
  echo "no record past the deposit block after ${BV_WAIT_RECORD_TRIES:-120} min" >&2; exit 1 ;;
fund)
  lez_only_on_sepolia fund
  # shellcheck source=/dev/null
  source <(grep '^export ' "$S/wallet.env")
  job=$(proof_job bridge-fund-claimer "private transfer to the claimer on seq-pub" -- \
    wallet auth-transfer send --from "$(basename "$LEE_WALLET_HOME_DIR" | sed 's/^wallet-//')-payer" --to w6-claimer \
    --amount 1000000000)
  save FUND_JOB "$job" ;;
claim)
  lez_only_on_sepolia claim
  # shellcheck source=/dev/null
  source <(grep '^export ' "$S/wallet.env")
  export BV_BRIDGE_CONFIG="$S/config.json"
  CLAIMER="$(label_id w6-claimer)"
  echo "claim stage start UTC $(date -u +%FT%TZ)"
  A=$("$CLI" attest-root --min-eth-block "$DEPOSIT_BLOCK" | tee "$S/attest-root.json")
  ROOT=$(jq -r .root <<<"$A"); save ROOT "$ROOT"
  "$CLI" balance --account "$CLAIMER" > "$S/balance-before-claim.json"
  job=$(proof_job bridge-claim-v4 "private claim of deposit $DEPOSIT_INDEX on seq-pub" -- \
    "$CLI" claim --index "$DEPOSIT_INDEX" --dest "$CLAIMER" --root "$ROOT")
  save CLAIM_JOB "$job"
  python3 -I -c 'import json,re,sys
t=open(sys.argv[1],errors="replace").read(); d=json.JSONDecoder()
for m in re.finditer(r"^\{", t, re.M):
    try: o,_=d.raw_decode(t[m.start():]); print(json.dumps(o)); break
    except ValueError: pass' "$job/output.log" > "$S/claim1.json"
  "$ATTEST" tx --tx "$(jq -r .tx "$S/claim1.json")" > "$S/claim1-tx.json"
  "$CLI" balance --account "$CLAIMER" > "$S/balance-after-claim.json"
  # The repeat is refused while the program runs, before any proof is made (E11); its seconds show that.
  t0=$(date +%s.%N)
  if "$CLI" claim --index "$DEPOSIT_INDEX" --dest "$CLAIMER" --root "$ROOT" --force > "$S/claim2.txt" 2>&1; then
    echo "FAIL: second claim accepted"; exit 1; fi
  python3 -I -c 'import sys; print(round(float(sys.argv[2]) - float(sys.argv[1]), 2))' "$t0" "$(date +%s.%N)" > "$S/claim2-seconds.txt"
  grep -q 'E11' "$S/claim2.txt" && echo "second claim refused with E11"
  echo "claim stage end UTC $(date -u +%FT%TZ)" ;;
burn)
  lez_only_on_sepolia burn
  # shellcheck source=/dev/null
  source <(grep '^export ' "$S/wallet.env")
  export BV_BRIDGE_CONFIG="$S/config.json"
  CLAIMER="$(label_id w6-claimer)"; J="${BURN_INDEX:-0}"
  RECIPIENT="${BV_RELEASE_RECIPIENT:?BV_RELEASE_RECIPIENT=<Ethereum address that receives the release>}"
  job=$(proof_job bridge-burn-v4 "private burn of 0.001 ETH on seq-pub" -- \
    "$CLI" burn --index "$J" --holder "$CLAIMER" --token eth --denom 1000000000000000 --recipient "$RECIPIENT" \
    --fee 10000000000000)
  save BURN_JOB "$job"; save BURN_INDEX "$J"
  grep -o 'burn tx [0-9a-f]*' "$job/output.log" | awk '{print $3}' | head -1 > "$S/burn-tx.txt"
  "$ATTEST" tx --tx "$(cat "$S/burn-tx.txt")" > "$S/burn-tx.json"
  "$CLI" balance --account "$CLAIMER" > "$S/balance-after-burn.json" ;;
post-root)
  ensure_anvil
  export BV_BRIDGE_CONFIG="$S/config.json"
  if [ "$TARGET" = anvil-fork ] && [ -n "${BV_TEST_ROOT:-}" ]; then
    ACC="{\"root\": \"$BV_TEST_ROOT\", \"note\": \"test root on the anvil fork\"}"; echo "$ACC" > "$S/accumulator.json"
  else
    ACC=$("$CLI" accumulator-root | tee "$S/accumulator.json")
  fi
  export ANCHOR LEZ_INSTANCE=$INSTANCE_HEX ROOT=$(jq -r .root <<<"$ACC")
  forge_s PostRoot | tee "$S/postroot.txt"
  save POSTROOT_UNIX "$(date +%s)"
  echo "STUB anchor: release possible 600 s after the post" ;;
wait-anchor)
  : "${POSTROOT_UNIX:?post-root first}"
  left=$((POSTROOT_UNIX + 600 + 30 - $(date +%s)))
  [ "$left" -gt 0 ] && { echo "waiting $left s for the STUB anchor delay"; sleep "$left"; }
  echo "anchor delay passed" ;;
prove)
  lez_only_on_sepolia prove
  export BV_BRIDGE_CONFIG="$S/config.json"
  W="$S/burn-$BURN_INDEX-witness"
  "$CLI" prove --index "$BURN_INDEX" --out "$W" --mode witness >/dev/null
  job=$(proof_job burn-groth16-v4 "burn_proof Groth16, RISC Zero 3.1.0" -- \
    env "${R31_ENV[@]}" "$R31_BIN" prove --witness "$W/witness.borsh" --out "$S/burn-$BURN_INDEX-groth16" --mode groth16)
  rm -f "$W/witness.borsh"
  save GROTH16_JOB "$job" ;;
release)
  ensure_anvil
  D="$S/burn-$BURN_INDEX-groth16"
  export VAULT SEAL="$(cat "$D/seal.hex")" JOURNAL="$(cat "$D/journal.hex")"
  forge_s ReleaseOne | tee "$S/release.txt" ;;
record-claim | record-burn)
  python3 -I "$here/record_v4.py" "${1#record-}" --flow "$S" --target "$TARGET" \
    --out "${BV_RECORD_OUT:-$R/evidence/bridge/real-${1#record-}-v4/record.json}" ;;
*) sed -n '2,24p' "$0"; exit 2 ;;
esac
