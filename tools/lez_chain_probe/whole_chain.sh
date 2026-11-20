#!/usr/bin/env bash
# Whole-chain parser test against a live sequencer, read only: fetches every block through getBlockRange, then checks
# each one against LEZ's own types and crates/lez_chain (re-encoding, transaction spans, hash) and the whole range as
# one hash chain, and lists every bridge burn (tools/lez_chain_probe). Exits non-zero on any failure.
#
#   tools/lez_chain_probe/whole_chain.sh URL BRIDGE_PROGRAM_ID OUT_JSON [BLOCK_DIR]
#
# BLOCK_DIR keeps the fetched blocks (default: a temporary directory); blocks already there are not fetched again.
# The probe binary: $PROBE, else $CARGO_TARGET_DIR/release/lez_chain_probe, else the probe's own target dir. Build it
# with: cargo build --release --manifest-path tools/lez_chain_probe/Cargo.toml
# CI runs the committed fixture range instead (crates/lez_chain/tests/fixtures.rs).
set -euo pipefail
url="${1:?sequencer RPC URL}"; bridge="${2:?bridge program id (base58)}"; out="${3:?output json}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
blocks="${4:-$(mktemp -d)}"
probe="${PROBE:-${CARGO_TARGET_DIR:-$here/target}/release/lez_chain_probe}"
[ -x "$probe" ] || { echo "probe not built: $probe" >&2; exit 2; }
python3 -I "$here/fetch_blocks.py" "$url" "$blocks" >&2
status=0
"$probe" "$blocks" "$bridge" "$url" > "$out" || status=$?
python3 -I -c '
import json, sys
d = json.load(open(sys.argv[1]))
print(json.dumps({k: d[k] for k in ("blocks", "first", "last", "contiguous", "transactions", "tx_kinds", "failures",
                                    "chain_check_first_to_last")} | {"burns": len(d["burns"])}))' "$out" >&2
exit "$status"
