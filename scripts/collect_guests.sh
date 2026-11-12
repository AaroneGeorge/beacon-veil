#!/usr/bin/env bash
# Copies the newest <name>.bin of each guest built by crates/guest_methods into $B/target/guests and prints
# size, segment count (96 KiB each, cap 20 = 1,966,080 bytes) and sha256.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"
dest="$CARGO_TARGET_DIR/guests"; mkdir -p "$dest"
for g in "$@"; do
  src="$(find "$CARGO_TARGET_DIR" -path "$dest" -prune -o -type f -name "$g.bin" -path '*riscv32im-risc0-zkvm-elf*' -printf '%T@ %p\n' \
    | sort -nr | head -1 | cut -d' ' -f2-)"
  [ -n "$src" ] || { echo "no $g.bin found under $CARGO_TARGET_DIR" >&2; exit 1; }
  cp "$src" "$dest/$g.bin"
  size=$(stat -c %s "$dest/$g.bin")
  printf '%-16s %9d bytes  %2d segments  %s\n' "$g" "$size" $(( (size + 98303) / 98304 )) "$(sha256sum "$dest/$g.bin" | cut -c1-16)"
done
