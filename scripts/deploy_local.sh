#!/usr/bin/env bash
# Deploys `attest` and `lc_verify` (immutable flag set) to a local sequencer and writes deploy/ids.toml.
#
#   scripts/deploy_local.sh [dev|real] [--bins DIR] [--programs "attest lc_verify ..."]
#
# Bins: $B/target/guests/<name>.bin, produced by `make guests` (local rzup toolchain) or `make guests-docker`
# (pinned image r0.1.91.1, reproducible image ids; use it for any deployment whose ids are recorded in evidence/).
# Payer: the first public account in the wallet that has a balance; on a fresh wallet the LEZ local genesis test key is
# imported (LEZ Justfile `wallet-import-test-accounts`; a public test key of the local network, not a secret).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"

which_seq="dev"; bins="$B/target/guests"; programs="attest lc_verify"
while [ $# -gt 0 ]; do
  case "$1" in
    dev|real) which_seq="$1"; shift ;;
    --bins) bins="$2"; shift 2 ;;
    --programs) programs="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

export LEE_WALLET_HOME_DIR="$B/run/wallet-$which_seq"
[ "$which_seq" = real ] && unset RISC0_DEV_MODE
"$here/seq.sh" "$which_seq" status >/dev/null || { echo "seq-$which_seq is not running: scripts/seq.sh $which_seq start" >&2; exit 1; }
command -v wallet >/dev/null || { echo "wallet binary not on PATH (build LEZ: see env-install.log)" >&2; exit 1; }
export LEE_WALLET_PASSWORD="${LEE_WALLET_PASSWORD:-}"

LEZ_TEST_KEY="7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb" # LEZ Justfile, local genesis
payer_label="bv-payer"
if ! wallet account get --account-id "$payer_label" >/dev/null 2>&1; then
  wallet account import public --private-key "$LEZ_TEST_KEY" --label "$payer_label" >/dev/null
fi

new_public() { # label -> prints account id
  wallet account new public --label "$1" | sed -n 's#.*Public/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1
}

out="$BV_REPO/deploy/ids.toml"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
{
  echo "# Written by scripts/deploy_local.sh at $(date -u +%FT%TZ) on seq-$which_seq (do not edit by hand)."
  echo "[deploy]"
  echo "sequencer = \"seq-$which_seq\""
  echo "rpc = \"$(jq -r .sequencer.sequencer_addr "$LEE_WALLET_HOME_DIR/wallet_config.json")\""
  echo "lez = \"v0.4.0-rc3\""
  echo "bins = \"$bins\""
  echo
  echo "[programs]"
} > "$out.tmp"

for p in $programs; do
  bin="$bins/$p.bin"
  [ -f "$bin" ] || { echo "missing $bin (run make guests)" >&2; exit 1; }
  size=$(stat -c %s "$bin"); n=$(( (size + 98303) / 98304 ))
  [ "$n" -le 20 ] || { echo "$p.bin is $size bytes: $n segments, above the 20-segment cap" >&2; exit 1; }
  header="$(new_public "bv-$p-header-$stamp")"
  segs=()
  for i in $(seq 1 "$n"); do segs+=("$(new_public "bv-$p-seg$i-$stamp")"); done
  echo "deploying $p ($size bytes, $n segments) header $header"
  wallet program-loader deploy --elf "$bin" --header "Public/$header" --segments "${segs[@]/#/Public/}" \
    --immutable --payer "$payer_label"
  echo "$p = \"$header\"   # sha256 $(sha256sum "$bin" | cut -c1-64), $size bytes" >> "$out.tmp"
done
mv "$out.tmp" "$out"
echo "wrote $out"
