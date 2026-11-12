#!/usr/bin/env bash
# Local round trip of the bridge spike (W5): anvil vault + seq-dev (dev mode).
#   deposit (anvil) -> dev-attest-root (DEV statement source) -> private claim -> repeat claim refused (E11)
#   -> private burn -> STUB anchor post -> burn_proof in the executor -> release on anvil (mock verifier) -> recover
#
# The inbound attestation here is the DEV statement source, not RFP-021: no light client follows anvil. The Sepolia
# run (docs/bridge/INBOUND.md) uses `attest-root` against the followed instance instead.
#
# Needs: seq-dev running; guests built (`make guests`: bridge.bin, bridge_dev_source.bin, burn_proof.bin in
# $B/target/guests); bridge_cli built; anvil on PATH. Wallet home: $B/run/wallet-w5 with a funded public `w5-payer`
# and a funded private `w5-claimer` (see TRACK-W5-NOTES.md). Keys used: anvil dev key 0 and the LEZ local test key,
# both public test keys, not secrets.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
R="$(cd "$here/../.." && pwd)"
source "$R/scripts/env.sh"
export LEE_WALLET_HOME_DIR="$B/run/wallet-w5" LEE_WALLET_PASSWORD=""
RUN="$B/run/w5/e2e-$(date -u +%Y%m%dT%H%M%SZ)"; mkdir -p "$RUN"
exec > >(tee "$RUN/log.txt") 2>&1
CLI="${BV_BRIDGE_CLI:-$CARGO_TARGET_DIR/debug/bridge_cli}"
ANVIL_RPC="http://127.0.0.1:8547"
export PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80   # anvil dev key 0 (public)
export BV_BRIDGE_MNEMONIC_FILE="$here/testdata/TEST-ONLY-mnemonic.txt"
export BV_ETH_RPC="$ANVIL_RPC"
label_id() { wallet account list 2>/dev/null | grep "\[$1\]" | grep -o '\(Public\|Private\)/[A-Za-z0-9]*' | cut -d/ -f2; }
export BV_PAYER="${BV_PAYER:-$(label_id w5-payer)}"
CLAIMER="${BV_CLAIMER:-$(label_id w5-claimer)}"
[ -n "$BV_PAYER" ] && [ -n "$CLAIMER" ] || { echo "need wallet labels w5-payer (public) and w5-claimer (private)"; exit 1; }
step() { echo; echo "== $(date -u +%T) $*"; }
b58hex() { python3 -c 'import sys
a="123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";n=0
for c in sys.argv[1]: n=n*58+a.index(c)
print("0x%064x"%n)' "$1"; }

cast chain-id --rpc-url "$ANVIL_RPC" >/dev/null 2>&1 || { anvil --port 8547 --silent >"$RUN/anvil.log" 2>&1 & sleep 2; }

export BV_GUESTS="${BV_GUESTS:-$B/run/w5/guests}"
if [ "${BV_REDEPLOY:-0}" = 1 ] || ! grep -qE '^bridge_dev_source *=' "$R/deploy/ids.toml"; then
  step "deploy bridge + DEV statement source on seq-dev"
  "$R/scripts/deploy_local.sh" dev --bins "$BV_GUESTS" --programs "bridge bridge_dev_source"
fi
prog_id() { awk -v k="$1" '/^\[programs\]/{f=1; next} /^\[/{f=0} f && $1==k {split($0,a,"\""); print a[2]; exit}' "$R/deploy/ids.toml"; }
BRIDGE=$(prog_id bridge)
DEVSRC=$(prog_id bridge_dev_source)
# burn_proof on RISC Zero 3.1.0 (GHSA-g6qg-9j8p-7j87), separate workspace guests/burn_proof/r31.
R31=(env RISC0_HOME="$B/run/w5/risc0-31"
  RISC0_SERVER_PATH="$B/run/w5/risc0-31/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm"
  "$B/target-w5-r31/release/burn_prover_r31")
IMAGE=$("${R31[@]}" image-id)
echo "bridge=$BRIDGE dev_source=$DEVSRC burn_image_id=$IMAGE"

step "deploy vault on anvil (mock verifier, STUB anchor delay 0)"
cd "$R/contracts"
BURN_IMAGE_ID=$IMAGE LEZ_PROGRAM=$(b58hex "$BRIDGE") ANCHOR_DELAY=0 \
  forge script script/Deploy.s.sol:Deploy --rpc-url "$ANVIL_RPC" --broadcast > "$RUN/deploy.txt" 2>&1
DEPLOY_JSON=$(grep -o '{"admin".*}' "$RUN/deploy.txt"); echo "$DEPLOY_JSON" > "$RUN/deploy.json"
VAULT=$(jq -r .vault <<<"$DEPLOY_JSON"); ANCHOR=$(jq -r .anchor_stub <<<"$DEPLOY_JSON"); BLOCK=$(jq -r .block <<<"$DEPLOY_JSON")
echo "vault=$VAULT anchor=$ANCHOR"

step "bridge config (attest_program = DEV source)"
"$CLI" config --out "$RUN/config.json" --bridge-program "$BRIDGE" --chain-id 31337 --vault "$VAULT" \
  --vault-block "$BLOCK" --anchor "$ANCHOR" --attest-program "$DEVSRC" \
  --attest-instance 11111111111111111111111111111111 --token eth:1000000000000000,10000000000000000
export BV_BRIDGE_CONFIG="$RUN/config.json"
INSTANCE_HEX=$(jq -r .instance_hex "$RUN/config.json")
VAULT=$VAULT LEZ_INSTANCE=$INSTANCE_HEX forge script script/Deploy.s.sol:BindInstance --rpc-url "$ANVIL_RPC" --broadcast >/dev/null

step "deposit 0.01 ETH, commitment bound to the private claimer"
C=$("$CLI" commitment --index 0 --dest "$CLAIMER" --token eth --denom 10000000000000000 | tee "$RUN/commitment.json")
VAULT=$VAULT TOKEN=0x0000000000000000000000000000000000000000 DENOM_INDEX=1 COMMITMENT=$(jq -r .commitment <<<"$C") \
  forge script script/Deploy.s.sol:DepositOne --rpc-url "$ANVIL_RPC" --broadcast | grep -E 'deposit (root|count)'
ROOT=$(cast call "$VAULT" 'depositRoot()(bytes32)' --rpc-url "$ANVIL_RPC")
cd "$R"

step "DEV attest root $ROOT"
"$CLI" dev-attest-root

step "private claim (dev mode)"
"$CLI" claim --index 0 --dest "$CLAIMER" --root "$ROOT" | tee "$RUN/claim1.json"

step "repeat claim must be refused with ESA:E11"
if "$CLI" claim --index 0 --dest "$CLAIMER" --root "$ROOT" --force > "$RUN/claim2.txt" 2>&1; then
  echo "FAIL: second claim accepted"; exit 1
fi
grep -E 'E11|ALREADY_CONSUMED' "$RUN/claim2.txt" && echo "second claim refused: OK"

step "private burn of 0.01 ETH to 0x...beef, fee 0.0001"
"$CLI" burn --index 0 --holder "$CLAIMER" --token eth --denom 10000000000000000 \
  --recipient 0x000000000000000000000000000000000000bEEF --fee 100000000000000 | tee "$RUN/burn.json"
ACC=$("$CLI" accumulator-root | tee "$RUN/accumulator.json")

step "STUB anchor: owner posts the accumulator root"
cd "$R/contracts"
ANCHOR=$ANCHOR LEZ_INSTANCE=$INSTANCE_HEX ROOT=$(jq -r .root <<<"$ACC") \
  forge script script/Deploy.s.sol:PostRoot --rpc-url "$ANVIL_RPC" --broadcast >/dev/null
cd "$R"

step "burn_proof (risc0 3.1.0) in the executor (dev mode, mock seal)"
"$CLI" prove --index 0 --out "$RUN/burn0w" --mode witness > /dev/null
"${R31[@]}" prove --witness "$RUN/burn0w/witness.borsh" --out "$RUN/burn0" --mode execute
rm -f "$RUN/burn0w/witness.borsh"

step "release on anvil"
before=$(cast balance 0x000000000000000000000000000000000000bEEF --rpc-url "$ANVIL_RPC")
cd "$R/contracts"
VAULT=$VAULT SEAL="$(cat "$RUN/burn0/seal.hex")" JOURNAL="$(cat "$RUN/burn0/journal.hex")" \
  forge script script/Deploy.s.sol:ReleaseOne --rpc-url "$ANVIL_RPC" --broadcast > "$RUN/release.txt" 2>&1
cd "$R"
after=$(cast balance 0x000000000000000000000000000000000000bEEF --rpc-url "$ANVIL_RPC")
echo "recipient balance: $before -> $after (expect +9900000000000000)"
python3 -c "import sys; sys.exit(0 if int('$after') - int('$before') == 9900000000000000 else 1)"

step "recover from the seed alone"
"$CLI" recover --dest "$CLAIMER" --lez | tee "$RUN/recover.json"
echo; echo "E2E LOCAL OK: $RUN"
