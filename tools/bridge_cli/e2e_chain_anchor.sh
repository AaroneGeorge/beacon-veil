#!/usr/bin/env bash
# The chain route end to end on an ANVIL FORK of Sepolia (chain id 31337), never on Sepolia itself.
#
#   tools/bridge_cli/e2e_chain_anchor.sh OUT_DIR
#
# Deploys RiscZeroMockVerifier, both STUB anchors and VaultChain pinned at seq-dev block 2585 (DeployChain.s.sol),
# deposits 0.001 ETH, runs guest burn_chain in the executor (burn_chain_host prove --mode execute) for the synthetic
# private burn of fixtures/lez-chain with this vault and chain in its witness, attests tip 2591, waits the delay, and
# releases through releaseChain with the mock seal. Then checks: recipient and relayer paid, the nullifier spent, a
# second release refused (AlreadyReleased), and a VaultChain on the real Sepolia RISC Zero 3.1.0 emergency stop refusing
# the mock seal. Writes OUT_DIR/record.json; evidence: evidence/bridge/chain-anchor/anvil-fork.json.
#
# Keys: anvil's public dev keys 0 (deployer) and 1 (relayer) only. The fork RPC is checked to be chain 31337 before
# any transaction. Needs: anvil, forge, cast, jq, and burn_chain_host built (guests/burn_chain/README.md).
# Env: SEPOLIA_RPC (fork source, default the public node), BV_CHAIN_FORK_PORT (default 8549), BURN_CHAIN_HOST,
# RISC0_SERVER_PATH (r0vm 3.1.0).
set -euo pipefail
OUT="${1:?output dir}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$here/../.." && pwd)"
FIX="$ROOT/fixtures/lez-chain"
SEPOLIA_RPC="${SEPOLIA_RPC:-https://ethereum-sepolia-rpc.publicnode.com}"
PORT="${BV_CHAIN_FORK_PORT:-8549}"; RPC="http://127.0.0.1:$PORT"
HOST="${BURN_CHAIN_HOST:-${CARGO_TARGET_DIR:-$ROOT/guests/burn_chain/target}/release/burn_chain_host}"
KEY0=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80   # anvil dev key 0 (public)
KEY1=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d   # anvil dev key 1 (public)
RELAYER=$(cast wallet address --private-key "$KEY1")
ESTOP=0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29   # Sepolia: RiscZeroVerifierEmergencyStop around verifier 3.1.0
mkdir -p "$OUT"; unset RISC0_DEV_MODE
[ -x "$HOST" ] || { echo "burn_chain_host not built: $HOST" >&2; exit 2; }

setsid nohup anvil --fork-url "$SEPOLIA_RPC" --chain-id 31337 --port "$PORT" --silent > "$OUT/anvil.log" 2>&1 < /dev/null &
echo $! > "$OUT/anvil.pid"
trap 'kill "$(cat "$OUT/anvil.pid")" 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do cast chain-id --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 1; done
[ "$(cast chain-id --rpc-url "$RPC")" = 31337 ] || { echo "not the anvil fork" >&2; exit 1; }
FORK_BLOCK=$(cast block-number --rpc-url "$RPC")

M="$FIX/manifest.json"
PINNED_HASH=0x$(jq -r '.seq_dev.blocks[] | select(.id == 2585) | .hash' "$M")
TIP_HASH=0x$(jq -r '.synthetic.blocks[] | select(.id == 2591) | .hash' "$M")
CHAIN_IMAGE_ID=$("$HOST" image-id)

deploy() { # VERIFIER -> deploy json
  (cd "$ROOT/contracts" && PRIVATE_KEY=$KEY0 VERIFIER="$1" BURN_IMAGE_ID=0x$(printf '%064x' 1) CHAIN_IMAGE_ID="$CHAIN_IMAGE_ID" \
    LEZ_PROGRAM=0x$(jq -r .bridge_program_hex "$M") LEZ_INSTANCE=0x$(jq -r .instance_hex "$M") \
    PINNED_ID=2585 PINNED_HASH="$PINNED_HASH" ANCHOR_DELAY=600 \
    forge script script/DeployChain.s.sol:DeployChain --rpc-url "$RPC" --broadcast --slow) > "$OUT/deploy-$2.txt" 2>&1
  grep -o '{"admin".*}' "$OUT/deploy-$2.txt" | tail -1 > "$OUT/deploy-$2.json"
  cp "$ROOT/contracts/broadcast/DeployChain.s.sol/31337/run-latest.json" "$OUT/broadcast-deploy-$2.json"
}
witness() { # vault -> witness file for this fork
  jq --arg v "${1#0x}" '.chain_id = "31337" | .vault = ($v | ascii_downcase) | del(.about)' \
    "$FIX/witness-synthetic-private.json" > "$OUT/witness-$2.json"
}
prove() { # name
  "$HOST" prove --mode execute --witness-json "$OUT/witness-$1.json" --dir "$FIX/synthetic" --pinned-dir "$FIX/seq-dev" \
    --pinned 2585 --tip 2591 --burn-block 2586 --tx 0 --out "$OUT/proof-$1" > /dev/null
}

# 1. Vault on the mock verifier: deposit, executor run, attest, wait, release.
deploy 0x0000000000000000000000000000000000000000 mock
VAULT=$(jq -r .vault "$OUT/deploy-mock.json"); TIP_ANCHOR=$(jq -r .tip_anchor_stub "$OUT/deploy-mock.json")
DEP=$(cast send --rpc-url "$RPC" --private-key $KEY0 --value 0.001ether "$VAULT" "deposit(address,uint256,bytes32)" \
  0x0000000000000000000000000000000000000000 0 "$(cast keccak fixture-deposit)" --json)
witness "$VAULT" mock; prove mock
JOURNAL=$(tr -d '\n' < "$OUT/proof-mock/journal.hex"); SEAL=$(tr -d '\n' < "$OUT/proof-mock/seal.hex")
ATT=$(cast send --rpc-url "$RPC" --private-key $KEY0 "$TIP_ANCHOR" "attestTip(uint64,bytes32)" 2591 "$TIP_HASH" --json)
EARLY=$(cast call --rpc-url "$RPC" --from "$RELAYER" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL" 2>&1 || true)
cast rpc --rpc-url "$RPC" evm_increaseTime 601 > /dev/null; cast rpc --rpc-url "$RPC" evm_mine > /dev/null
RECIPIENT=0x$(jq -r .recipient "$OUT/witness-mock.json")
R0=$(cast balance --rpc-url "$RPC" "$RECIPIENT"); L0=$(cast balance --rpc-url "$RPC" "$RELAYER")
REL=$(cast send --rpc-url "$RPC" --private-key $KEY1 "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL" --json)
R1=$(cast balance --rpc-url "$RPC" "$RECIPIENT"); L1=$(cast balance --rpc-url "$RPC" "$RELAYER")
NULL=0x$(jq -r .nullifier "$OUT/proof-mock/meta.json")
SPENT=$(cast call --rpc-url "$RPC" "$VAULT" "burnNullifierSpent(bytes32)(bool)" "$NULL")
AGAIN=$(cast call --rpc-url "$RPC" --from "$RELAYER" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL" 2>&1 || true)

# 2. Vault on the real Sepolia 3.1.0 emergency stop: the mock seal must be refused.
deploy "$ESTOP" estop
VAULT2=$(jq -r .vault "$OUT/deploy-estop.json"); TIP2=$(jq -r .tip_anchor_stub "$OUT/deploy-estop.json")
cast send --rpc-url "$RPC" --private-key $KEY0 --value 0.001ether "$VAULT2" "deposit(address,uint256,bytes32)" \
  0x0000000000000000000000000000000000000000 0 "$(cast keccak fixture-deposit-2)" > /dev/null
witness "$VAULT2" estop; prove estop
cast send --rpc-url "$RPC" --private-key $KEY0 "$TIP2" "attestTip(uint64,bytes32)" 2591 "$TIP_HASH" > /dev/null
cast rpc --rpc-url "$RPC" evm_increaseTime 601 > /dev/null; cast rpc --rpc-url "$RPC" evm_mine > /dev/null
ESTOP_REFUSAL=$(cast call --rpc-url "$RPC" --from "$RELAYER" "$VAULT2" "releaseChain(bytes,bytes)" \
  "$(tr -d '\n' < "$OUT/proof-estop/seal.hex")" "$(tr -d '\n' < "$OUT/proof-estop/journal.hex")" 2>&1 || true)

gas() { cast to-dec "$(jq -r .gasUsed <<<"$1")"; }
jq -n --arg fork_block "$FORK_BLOCK" --arg g_dep "$(gas "$DEP")" --arg g_att "$(gas "$ATT")" --arg g_rel "$(gas "$REL")" --slurpfile dep "$OUT/deploy-mock.json" --slurpfile meta "$OUT/proof-mock/meta.json" \
  --argjson deposit "$DEP" --argjson attest "$ATT" --argjson release "$REL" \
  --arg early "$EARLY" --arg again "$AGAIN" --arg spent "$SPENT" --arg estop_refusal "$ESTOP_REFUSAL" \
  --arg r0 "$R0" --arg r1 "$R1" --arg l0 "$L0" --arg l1 "$L1" --arg recipient "$RECIPIENT" --arg relayer "$RELAYER" \
  --slurpfile dep2 "$OUT/deploy-estop.json" '{
  what: "VaultChain.releaseChain on an anvil fork of Sepolia (chain id 31337); nothing sent to Sepolia",
  label: "anvil fork; guest burn_chain in the executor (no proof); mock seal for RiscZeroMockVerifier; synthetic burn",
  fork_of: "Sepolia", fork_block: ($fork_block | tonumber),
  deploy: $dep[0], guest: {image_id: $meta[0].image_id, cycles_user: $meta[0].cycles_user, segments: $meta[0].segments},
  deposit: {tx: $deposit.transactionHash, gas: ($g_dep | tonumber)},
  attest_tip: {tx: $attest.transactionHash, gas: ($g_att | tonumber)},
  release_before_delay: ($early | split("\n") | map(select(test("revert|Error|error"))) | .[0]),
  release_chain: {tx: $release.transactionHash, gas: ($g_rel | tonumber), status: $release.status,
    gas_note: "mock verifier; the real 3.1.0 Groth16 verify through the emergency stop adds about 260,000 (estimate, evidence/bridge/groth16-release-v3)"},
  recipient: $recipient, recipient_gain_wei: (($r1 | tonumber) - ($r0 | tonumber) | tostring),
  relayer: $relayer, relayer_balance_before: $l0, relayer_balance_after: $l1,
  nullifier_spent: $spent,
  second_release: ($again | split("\n") | map(select(test("revert|Error|error"))) | .[0]),
  estop_vault: $dep2[0].vault, estop_verifier: $dep2[0].verifier,
  estop_refuses_mock_seal: ($estop_refusal | split("\n") | map(select(test("revert|Error|error"))) | .[0])
}' > "$OUT/record.json"
cat "$OUT/record.json"
