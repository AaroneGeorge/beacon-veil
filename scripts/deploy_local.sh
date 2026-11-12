#!/usr/bin/env bash
# Deploys `attest` and `lc_verify` (immutable flag set) to a local sequencer and writes deploy/ids.toml.
#
#   scripts/deploy_local.sh [dev|real] [--bins DIR] [--programs "attest lc_verify ..."] [--ids FILE] [--replace]
#
# --ids FILE: ids file to write, relative to the repo root unless absolute (default deploy/ids.toml; seq-real
# deployments use e.g. deploy/ids-real.toml).
# The ids file is MERGED by default: only the lines of the programs deployed now are replaced or added in [programs]
# (name = "header id") and [bins] (name = "dir the .bin came from"); other lines are kept. [deploy] holds the
# sequencer, rpc and LEZ version; its old single `bins` key is kept as written before (readers fall back to it when a
# program has no [bins] line) and is only set on a fresh file or with --replace. `--replace` rewrites [programs] and
# [bins] with only the programs deployed now. The write is atomic (temp file + rename) under flock on FILE.lock, which
# also serialises wallet use between deploys.
#
# Bins: $B/target/guests/<name>.bin, produced by `make guests` (local rzup toolchain) or `make guests-docker`
# (pinned image r0.1.91.1, reproducible image ids; use it for any deployment whose ids are recorded in evidence/).
# Payer: the first public account in the wallet that has a balance; on a fresh wallet the LEZ local genesis test key is
# imported (LEZ Justfile `wallet-import-test-accounts`; a public test key of the local network, not a secret).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"

which_seq="dev"; bins="$B/target/guests"; programs="attest lc_verify"; replace=0; out="$BV_REPO/deploy/ids.toml"
while [ $# -gt 0 ]; do
  case "$1" in
    dev|real) which_seq="$1"; shift ;;
    --bins) bins="$2"; shift 2 ;;
    --programs) programs="$2"; shift 2 ;;
    --replace) replace=1; shift ;;
    --ids) out="$2"; shift 2 ;;
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
  # rc3: `import public` has no --label; import, then label the printed id.
  pid="$(wallet account import public --private-key "$LEZ_TEST_KEY" | sed -n 's#.*Public/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1)"
  wallet account label --account-id "Public/$pid" --label "$payer_label" >/dev/null
fi

u32le() { od -An -tu4 -j "$2" -N4 "$1" | tr -d ' '; }
# ProgramBinary (risc0-binfmt): "R0BF" | u32 version | u32 header_len | header | u32 user_len | user ELF | kernel.
# program_loader_core::segment_count splits the USER ELF into 96 KiB segments, so count from user_len.
user_elf_len() { local h; h=$(u32le "$1" 8); u32le "$1" $((12 + h)); }

new_public() { # label -> prints account id
  wallet account new public --label "$1" | sed -n 's#.*Public/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1
}

case "$out" in /*) ;; *) out="$BV_REPO/$out" ;; esac  # relative --ids paths are relative to the repo root
bins="$(cd "$bins" && pwd)"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"

# One deploy at a time per repo: serialises wallet storage use and the ids.toml update. The lock is a separate file
# because ids.toml itself is replaced by rename (a lock on the old inode would not exclude the next writer).
mkdir -p "$(dirname "$out")"
exec 9>"$out.lock"
flock -w 900 9 || { echo "could not lock $out.lock within 15 min" >&2; exit 1; }

declare -A new_ids=()
for p in $programs; do
  bin="$bins/$p.bin"
  [ -f "$bin" ] || { echo "missing $bin (run make guests)" >&2; exit 1; }
  size=$(stat -c %s "$bin"); user=$(user_elf_len "$bin"); n=$(( (user + 98303) / 98304 ))
  [ "$n" -le 20 ] || { echo "$p user ELF is $user bytes: $n segments, above the 20-segment cap" >&2; exit 1; }
  header="$(new_public "bv-$p-header-$stamp")"
  segs=()
  for i in $(seq 1 "$n"); do segs+=("$(new_public "bv-$p-seg$i-$stamp")"); done
  echo "deploying $p ($size bytes, user ELF $user bytes, $n segments) header $header"
  wallet program-loader deploy --elf "$bin" --header "Public/$header" --segments "${segs[@]/#/Public/}" \
    --immutable --payer "$payer_label"
  new_ids[$p]="$p = \"$header\"   # sha256 $(sha256sum "$bin" | cut -c1-64), $size bytes, $stamp"
done

# Merge (default): keep every [programs]/[bins] line not redeployed now, replace or add the ones that were.
# --replace: both tables hold only what was deployed now.
section_lines() { # file section -> "key = value" lines of that table
  awk -v sec="[$2]" '$0==sec{f=1; next} /^\[/{f=0} f && /^[A-Za-z0-9_]+ *=/' "$1"
}
kept_lines() { # file section: lines whose key was not deployed now
  [ "$replace" = 0 ] && [ -f "$out" ] || return 0
  section_lines "$out" "$1" | while IFS= read -r line; do
    key="${line%%[ =]*}"
    [ -n "${new_ids[$key]+x}" ] || printf '%s\n' "$line"
  done
}
old_bins=""
if [ "$replace" = 0 ] && [ -f "$out" ]; then
  old_bins="$(section_lines "$out" deploy | sed -n 's/^bins *= *"\(.*\)".*/\1/p' | head -1)"
fi
tmp="$(mktemp "$out.XXXXXX")"
{
  echo "# Written by scripts/deploy_local.sh (merge mode unless --replace). Last update $(date -u +%FT%TZ). Do not edit by hand."
  echo "[deploy]"
  echo "sequencer = \"seq-$which_seq\""
  echo "rpc = \"$(jq -r .sequencer.sequencer_addr "$LEE_WALLET_HOME_DIR/wallet_config.json")\""
  echo "lez = \"v0.4.0-rc3\""
  echo "# Fallback only; per-program dirs are in [bins]."
  echo "bins = \"${old_bins:-$bins}\""
  echo
  echo "[programs]"
  kept_lines programs
  for p in $programs; do printf '%s\n' "${new_ids[$p]}"; done
  echo
  echo "[bins]"
  kept_lines bins
  for p in $programs; do printf '%s = "%s"\n' "$p" "$bins"; done
} > "$tmp"
chmod 644 "$tmp"
mv -f "$tmp" "$out"
echo "wrote $out"
