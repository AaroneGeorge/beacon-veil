#!/usr/bin/env bash
# The chain route through CommitteeLezAnchor (3 of 4, stand-in key set) on an ANVIL FORK of Sepolia (chain id 31337),
# never on Sepolia itself.
#
#   tools/bridge_cli/e2e_committee_anchor.sh OUT_DIR
#
# Deploys RiscZeroMockVerifier, one CommitteeLezAnchor (anvil dev keys 2 to 5, threshold 3) and a VaultChain that uses
# it for both routes, pinned at seq-dev block 2585 (DeployCommittee.s.sol); deposits 0.001 ETH; runs guest burn_chain
# in the executor for the synthetic private burn of fixtures/lez-chain with this vault in its witness. Each of the four
# signers runs committee_sign.sh over the fixture blocks 2585..2591. Then, each refusal simulated (its error selector
# checked) and sent as a transaction with a fixed gas limit: 2 signatures; a key outside the set; one signer twice;
# signatures made for another anchor; signatures for the tip submitted under another block id. Then 3 of 4 accepted;
# the same call again refused; release before the delay refused; release after the delay pays recipient and relayer;
# a second release refused. Writes OUT_DIR/record.json; evidence: evidence/bridge/committee-anchor/anvil-fork.json.
#
# Keys: anvil's public dev keys only (0 deployer, 1 relayer, 2-5 signers, 6 outsider). The fork RPC is checked to be
# chain 31337 before any transaction. Needs: anvil, forge, cast, jq, python3, burn_chain_host built.
# Env: SEPOLIA_RPC (fork source), BV_COMMITTEE_FORK_PORT (default 8551), BURN_CHAIN_HOST, RISC0_SERVER_PATH (r0vm 3.1.0).
set -euo pipefail
OUT="${1:?output dir}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$here/../.." && pwd)"
FIX="$ROOT/fixtures/lez-chain"
SEPOLIA_RPC="${SEPOLIA_RPC:-https://ethereum-sepolia-rpc.publicnode.com}"
PORT="${BV_COMMITTEE_FORK_PORT:-8551}"; RPC="http://127.0.0.1:$PORT"
HOST="${BURN_CHAIN_HOST:-${CARGO_TARGET_DIR:-$ROOT/guests/burn_chain/target}/release/burn_chain_host}"
# anvil dev keys 0-6 (public, from anvil's default mnemonic)
DEV=(0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
  0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
  0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
  0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
  0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a
  0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba
  0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e)
KEY0=${DEV[0]}; KEY1=${DEV[1]}
RELAYER=$(cast wallet address --private-key "$KEY1")
SIG_SCRIPT="$here/committee_sign.sh"
mkdir -p "$OUT"; unset RISC0_DEV_MODE
[ -x "$HOST" ] || { echo "burn_chain_host not built: $HOST" >&2; exit 2; }

setsid nohup anvil --fork-url "$SEPOLIA_RPC" --chain-id 31337 --port "$PORT" --silent > "$OUT/anvil.log" 2>&1 < /dev/null &
echo $! > "$OUT/anvil.pid"
trap 'kill "$(cat "$OUT/anvil.pid")" 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do cast chain-id --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 1; done
[ "$(cast chain-id --rpc-url "$RPC")" = 31337 ] || { echo "not the anvil fork" >&2; exit 1; }
FORK_BLOCK=$(cast block-number --rpc-url "$RPC")

M="$FIX/manifest.json"
PINNED=2585; TIP=2591
PINNED_HASH=0x$(jq -r ".seq_dev.blocks[] | select(.id == $PINNED) | .hash" "$M")
TIP_HASH=0x$(jq -r ".synthetic.blocks[] | select(.id == $TIP) | .hash" "$M")
BLOCKS="$FIX/synthetic,$FIX/seq-dev" # the synthetic fork 2586..2591 over real 2585
CHAIN_IMAGE_ID=$("$HOST" image-id)

# Signer key files (mode 600), sorted by address.
mkdir -p "$OUT/keys"; chmod 700 "$OUT/keys"
SIGNERS=$(for i in 2 3 4 5; do echo "$(cast wallet address --private-key "${DEV[$i]}") $i"; done \
  | python3 -I -c 'import sys; [print(l.strip()) for l in sorted(sys.stdin, key=lambda l: int(l.split()[0], 16))]')
KEYF=(); ADDRS=()
while read -r a i; do
  f="$OUT/keys/signer-$i.key"; (umask 077; printf '%s' "${DEV[$i]}" > "$f"); KEYF+=("$f"); ADDRS+=("$a")
done <<<"$SIGNERS"
OUTSIDER_KEY=${DEV[6]}

deploy() { # name -> deploy-<name>.json
  (cd "$ROOT/contracts" && PRIVATE_KEY=$KEY0 VERIFIER=0x0000000000000000000000000000000000000000 \
    SIGNERS="$(IFS=,; echo "${ADDRS[*]}")" THRESHOLD=3 BURN_IMAGE_ID=0x$(printf '%064x' 1) \
    CHAIN_IMAGE_ID="$CHAIN_IMAGE_ID" LEZ_PROGRAM=0x$(jq -r .bridge_program_hex "$M") \
    LEZ_INSTANCE=0x$(jq -r .instance_hex "$M") PINNED_ID=$PINNED PINNED_HASH="$PINNED_HASH" ANCHOR_DELAY=600 \
    forge script script/DeployCommittee.s.sol:DeployCommittee --rpc-url "$RPC" --broadcast --slow) > "$OUT/deploy-$1.txt" 2>&1
  grep -o '{"admin".*}' "$OUT/deploy-$1.txt" | tail -1 > "$OUT/deploy-$1.json"
  cp "$ROOT/contracts/broadcast/DeployCommittee.s.sol/31337/run-latest.json" "$OUT/broadcast-deploy-$1.json"
}
arr() { local IFS=,; echo "[$*]"; }
raw_sign() { cast wallet sign --no-hash --private-key "$1" "$2"; } # key digest (for the outsider and the other anchor)

REFUSALS="$OUT/refusals.jsonl"; : > "$REFUSALS"
refuse() { # name error-signature target sig args...: simulate, check the selector, send with a fixed gas limit
  local name="$1" err="$2"; shift 2
  local sel out tx st ok=false
  sel=$(cast sig "$err")
  out=$(cast call --rpc-url "$RPC" --from "$RELAYER" "$@" 2>&1 || true)
  tx=$(cast send --rpc-url "$RPC" --private-key "$KEY1" --gas-limit 300000 --json "$@" 2>/dev/null || true)
  st=$(jq -r '.status // empty' <<<"$tx" 2>/dev/null || true)
  grep -q "$sel" <<<"$out" && [ "$st" = 0x0 ] && ok=true
  jq -nc --arg name "$name" --arg error "$err" --arg selector "$sel" --arg tx "$(jq -r '.transactionHash // empty' <<<"$tx")" \
    --arg status "$st" --argjson ok "$ok" --arg simulated "$(grep -m1 -oE 'custom error 0x[0-9a-f]+|revert.*' <<<"$out" || true)" \
    '{name: $name, expected: $error, selector: $selector, simulated: $simulated, tx: $tx, status: $status, ok: $ok}' >> "$REFUSALS"
  echo "refusal $name: ok=$ok" >&2
}

# 1. Deploy, deposit, executor run.
deploy main
VAULT=$(jq -r .vault "$OUT/deploy-main.json"); ANCHOR=$(jq -r .committee_anchor "$OUT/deploy-main.json")
[ "$(cast call --rpc-url "$RPC" "$VAULT" "tipAnchor()(address)")" = "$ANCHOR" ] || { echo "tip anchor mismatch" >&2; exit 1; }
[ "$(cast call --rpc-url "$RPC" "$VAULT" "anchor()(address)")" = "$ANCHOR" ] || { echo "root anchor mismatch" >&2; exit 1; }
DEP=$(cast send --rpc-url "$RPC" --private-key "$KEY0" --value 0.001ether "$VAULT" "deposit(address,uint256,bytes32)" \
  0x0000000000000000000000000000000000000000 0 "$(cast keccak fixture-deposit)" --json)
jq --arg v "${VAULT#0x}" '.chain_id = "31337" | .vault = ($v | ascii_downcase) | del(.about)' \
  "$FIX/witness-synthetic-private.json" > "$OUT/witness.json"
"$HOST" prove --mode execute --witness-json "$OUT/witness.json" --dir "$FIX/synthetic" --pinned-dir "$FIX/seq-dev" \
  --pinned $PINNED --tip $TIP --burn-block 2586 --tx 0 --out "$OUT/proof" > /dev/null
JOURNAL=$(tr -d '\n' < "$OUT/proof/journal.hex"); SEAL=$(tr -d '\n' < "$OUT/proof/seal.hex")

# 2. Four signer runs, each checking the blocks itself.
SIGS=()
for f in "${KEYF[@]}"; do SIGS+=("$("$SIG_SCRIPT" "$BLOCKS" "$RPC" "$ANCHOR" $TIP $PINNED "$PINNED_HASH" "$f")"); done
[ ${#SIGS[@]} = 4 ] || { echo "expected 4 signatures" >&2; exit 1; }
# A signer refuses a wrong pinned hash (its own check, before any signing).
SIGNER_REFUSAL=$("$SIG_SCRIPT" "$BLOCKS" "$RPC" "$ANCHOR" $TIP $PINNED "$TIP_HASH" "${KEYF[0]}" 2>&1 >/dev/null || true)
grep -q "refused: pinned block" <<<"$SIGNER_REFUSAL" || { echo "signer did not refuse a wrong pinned hash" >&2; exit 1; }

# 3. Refusals, then 3 of 4.
F="attestTip(uint64,bytes32,bytes[])"
DIGEST=$(cast call --rpc-url "$RPC" "$ANCHOR" "tipDigest(uint64,bytes32)(bytes32)" $TIP "$TIP_HASH")
OUTSIDER_SIG=$(raw_sign "$OUTSIDER_KEY" "$DIGEST")
deploy other
OTHER=$(jq -r .committee_anchor "$OUT/deploy-other.json")
ODIGEST=$(cast call --rpc-url "$RPC" "$OTHER" "tipDigest(uint64,bytes32)(bytes32)" $TIP "$TIP_HASH")
OSIGS=(); for i in 0 1 2; do OSIGS+=("$(raw_sign "$(cat "${KEYF[$i]}")" "$ODIGEST")"); done
refuse two_signatures "TooFewSignatures()" "$ANCHOR" "$F" $TIP "$TIP_HASH" "$(arr "${SIGS[0]}" "${SIGS[1]}")"
refuse outside_signer "NotASigner(address)" "$ANCHOR" "$F" $TIP "$TIP_HASH" "$(arr "${SIGS[0]}" "${SIGS[1]}" "$OUTSIDER_SIG")"
refuse same_signer_twice "SignersNotAscending()" "$ANCHOR" "$F" $TIP "$TIP_HASH" "$(arr "${SIGS[0]}" "${SIGS[0]}" "${SIGS[1]}")"
refuse another_anchor "NotASigner(address)" "$ANCHOR" "$F" $TIP "$TIP_HASH" "$(arr "${OSIGS[@]}")"
refuse another_block_id "NotASigner(address)" "$ANCHOR" "$F" $((TIP - 1)) "$TIP_HASH" "$(arr "${SIGS[0]}" "${SIGS[1]}" "${SIGS[2]}")"
ATT=$(cast send --rpc-url "$RPC" --private-key "$KEY1" --json "$ANCHOR" "$F" $TIP "$TIP_HASH" \
  "$(arr "${SIGS[1]}" "${SIGS[2]}" "${SIGS[3]}")")
refuse attest_again "AlreadyAttested()" "$ANCHOR" "$F" $TIP "$TIP_HASH" "$(arr "${SIGS[0]}" "${SIGS[1]}" "${SIGS[2]}")"
refuse release_before_delay "TipNotFinal()" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL"

# 4. After the delay: release, then the same release again.
cast rpc --rpc-url "$RPC" evm_increaseTime 601 > /dev/null; cast rpc --rpc-url "$RPC" evm_mine > /dev/null
FINAL=$(cast call --rpc-url "$RPC" "$ANCHOR" "isFinalTip(uint64,bytes32)(bool)" $TIP "$TIP_HASH")
RECIPIENT=0x$(jq -r .recipient "$OUT/witness.json")
R0=$(cast balance --rpc-url "$RPC" "$RECIPIENT"); L0=$(cast balance --rpc-url "$RPC" "$RELAYER")
REL=$(cast send --rpc-url "$RPC" --private-key "$KEY1" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL" --json)
R1=$(cast balance --rpc-url "$RPC" "$RECIPIENT"); L1=$(cast balance --rpc-url "$RPC" "$RELAYER")
NULL=0x$(jq -r .nullifier "$OUT/proof/meta.json")
SPENT=$(cast call --rpc-url "$RPC" "$VAULT" "burnNullifierSpent(bytes32)(bool)" "$NULL")
refuse second_release "AlreadyReleased()" "$VAULT" "releaseChain(bytes,bytes)" "$SEAL" "$JOURNAL"

gas() { cast to-dec "$(jq -r .gasUsed <<<"$1")"; }
jq -n --arg fork_block "$FORK_BLOCK" --arg g_dep "$(gas "$DEP")" --arg g_att "$(gas "$ATT")" --arg g_rel "$(gas "$REL")" \
  --slurpfile dep "$OUT/deploy-main.json" --slurpfile meta "$OUT/proof/meta.json" --slurpfile refusals "$REFUSALS" \
  --argjson deposit "$DEP" --argjson attest "$ATT" --argjson release "$REL" --arg final "$FINAL" --arg spent "$SPENT" \
  --arg r0 "$R0" --arg r1 "$R1" --arg l0 "$L0" --arg l1 "$L1" --arg recipient "$RECIPIENT" --arg relayer "$RELAYER" \
  --arg tip "$TIP" --arg tip_hash "$TIP_HASH" --arg pinned "$PINNED" --arg pinned_hash "$PINNED_HASH" \
  --arg signer_refusal "$(grep -m1 refused <<<"$SIGNER_REFUSAL")" --arg other "$OTHER" '{
  what: "VaultChain.releaseChain through CommitteeLezAnchor (3 of 4) on an anvil fork of Sepolia (chain id 31337); nothing sent to Sepolia",
  label: "anvil fork; anvil public dev keys as the stand-in signer set; guest burn_chain in the executor (no proof); mock seal for RiscZeroMockVerifier; synthetic burn",
  fork_of: "Sepolia", fork_block: ($fork_block | tonumber),
  deploy: $dep[0], guest: {image_id: $meta[0].image_id, cycles_user: $meta[0].cycles_user, segments: $meta[0].segments},
  blocks: {pinned: ($pinned | tonumber), pinned_hash: $pinned_hash, tip: ($tip | tonumber), tip_hash: $tip_hash,
    source: "fixtures/lez-chain: synthetic 2586..2591 over real seq-dev 2585; each signer recomputed every hash and link"},
  signer_refuses_wrong_pinned_hash: $signer_refusal,
  deposit: {tx: $deposit.transactionHash, gas: ($g_dep | tonumber)},
  attest_tip: {tx: $attest.transactionHash, gas: ($g_att | tonumber), status: $attest.status, signatures: 3,
    submitted_by: $relayer, note: "submitted by the relayer, not a signer: anyone may post the signatures"},
  other_anchor: $other,
  refusals: $refusals,
  tip_final_after_delay: ($final == "true"),
  release_chain: {tx: $release.transactionHash, gas: ($g_rel | tonumber), status: $release.status,
    gas_note: "mock verifier; the real 3.1.0 Groth16 verify through the emergency stop adds about 260,000 (estimate, evidence/bridge/groth16-release-v3)"},
  recipient: $recipient, recipient_gain_wei: (($r1 | tonumber) - ($r0 | tonumber) | tostring),
  relayer: $relayer, relayer_balance_before: $l0, relayer_balance_after: $l1,
  nullifier_spent: ($spent == "true")
}' > "$OUT/record.json"
cat "$OUT/record.json"
