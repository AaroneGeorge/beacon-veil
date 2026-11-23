#!/usr/bin/env bash
# One signer of a CommitteeLezAnchor (stand-in key set): checks LEZ blocks PINNED..TIP itself, then signs the
# anchor's EIP-712 Tip digest for (TIP, hash of TIP).
#
#   tools/bridge_cli/committee_sign.sh SEQ ETH_RPC ANCHOR TIP_ID PINNED_ID PINNED_HASH KEY_FILE
#
# SEQ is a sequencer RPC URL (http://..., read only: getBlock), or a comma list of directories that hold served block
# bytes as <id>.borsh (fixtures/lez-chain). Checks, each refusing with exit 1:
#   1. every block's hash, recomputed from its bytes, equals the hash it carries:
#      SHA-256("/LEE/v0.3/Message/Block/" + 8 zero bytes || b[0:40] || b[72:len-1]) (crates/lez_chain);
#   2. block ids rise by one and each block's prev_block_hash is the hash of the block before, from TIP back to PINNED;
#   3. the recomputed hash of PINNED equals PINNED_HASH (the vault's pinned block);
#   4. the key is in the anchor's signer set.
# Then reads tipDigest(TIP, hash) from the anchor and prints one 65-byte signature (cast wallet sign --no-hash).
# KEY_FILE holds the signer's private key (mode 600); it is never printed. What the signer cannot check: finality
# (a standalone sequencer reports every block as Pending) and whether the sequencer it reads is the real chain.
set -euo pipefail
[ $# -eq 7 ] || { sed -n '2,17p' "$0" >&2; exit 2; }
SEQ="$1"; ETH_RPC="$2"; ANCHOR="$3"; TIP="$4"; PINNED="$5"; PINNED_HASH="$6"; KEY_FILE="$7"
[ -r "$KEY_FILE" ] || { echo "refused: key file $KEY_FILE not readable" >&2; exit 1; }
[ "$TIP" -gt "$PINNED" ] || { echo "refused: tip $TIP is not above pinned $PINNED" >&2; exit 1; }
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT

fetch() { # id -> $work/<id>.borsh
  local id="$1"
  case "$SEQ" in
    http://* | https://*)
      curl -sf -X POST -H 'content-type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlock\",\"params\":[$id]}" "$SEQ" \
        | python3 -I -c 'import base64,json,sys; sys.stdout.buffer.write(base64.b64decode(json.load(sys.stdin)["result"]))' \
          > "$work/$id.borsh" || { echo "refused: getBlock($id) failed" >&2; exit 1; } ;;
    *)
      local d IFS=,
      for d in $SEQ; do [ -f "$d/$id.borsh" ] && { cp "$d/$id.borsh" "$work/"; return 0; }; done
      echo "refused: block $id not found in $SEQ" >&2; exit 1 ;;
  esac
}
for ((i = PINNED; i <= TIP; i++)); do fetch "$i"; done

TIP_HASH=$(python3 -I - "$work" "$PINNED" "$TIP" "$PINNED_HASH" <<'EOF'
import hashlib, sys
d, p, t, pinned_hash = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4].lower().removeprefix("0x")
PREFIX = b"/LEE/v0.3/Message/Block/" + bytes(8)
def fail(msg):
    print("refused: " + msg, file=sys.stderr); sys.exit(1)
hashes, prevs = {}, {}
for i in range(p, t + 1):
    b = open(f"{d}/{i}.borsh", "rb").read()
    if len(b) < 81: fail(f"block {i}: {len(b)} bytes")
    if int.from_bytes(b[0:8], "little") != i: fail(f"block {i}: carries id {int.from_bytes(b[0:8], 'little')}")
    h = hashlib.sha256(PREFIX + b[0:40] + b[72:-1]).digest()
    if h != b[40:72]: fail(f"block {i}: recomputed hash {h.hex()} differs from the carried {b[40:72].hex()}")
    hashes[i], prevs[i] = h, b[8:40]
for i in range(p + 1, t + 1):
    if prevs[i] != hashes[i - 1]: fail(f"block {i}: prev_block_hash is not the hash of block {i - 1}")
if hashes[p].hex() != pinned_hash: fail(f"pinned block {p}: hash {hashes[p].hex()} is not the pinned {pinned_hash}")
print("0x" + hashes[t].hex())
EOF
)

ME=$(cast wallet address --private-key "$(cat "$KEY_FILE")")
[ "$(cast call --rpc-url "$ETH_RPC" "$ANCHOR" "isSigner(address)(bool)" "$ME")" = true ] \
  || { echo "refused: $ME is not in the signer set of $ANCHOR" >&2; exit 1; }
DIGEST=$(cast call --rpc-url "$ETH_RPC" "$ANCHOR" "tipDigest(uint64,bytes32)(bytes32)" "$TIP" "$TIP_HASH")
echo "signer $ME: blocks $PINNED..$TIP checked, tip hash $TIP_HASH, digest $DIGEST" >&2
cast wallet sign --no-hash --private-key "$(cat "$KEY_FILE")" "$DIGEST"
