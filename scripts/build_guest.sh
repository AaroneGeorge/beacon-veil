#!/usr/bin/env bash
# Builds ONE guest package for riscv32im-risc0-zkvm-elf outside crates/guest_methods (measurement builds, quick
# local iteration) and prints its image id. Same environment as risc0-build 3.0.5 sets for a guest (rustflags,
# RISC0_FEATURE_bigint2, the RISC Zero C compiler); see guests/bls_bench/build.sh for the docker route.
#
#   scripts/build_guest.sh <manifest> <bin> <out_dir> [cargo features]
#
# Output: <out_dir>/<bin> (user ELF), <out_dir>/<bin>.bin (v1compat ProgramBinary), <out_dir>/<bin>.IMAGE_ID.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"
manifest="${1:?manifest}"; bin="${2:?bin}"; out="${3:?out_dir}"; features="${4:-}"
T=riscv32im-risc0-zkvm-elf
us=$'\x1f'
flags="-C${us}passes=lower-atomic${us}-C${us}link-arg=-Ttext=0x00200800${us}-C${us}link-arg=--fatal-warnings${us}-C${us}panic=abort${us}--cfg${us}getrandom_backend=\"custom\""
tc="$HOME/.risc0/toolchains/v1.91.1-rust-x86_64-unknown-linux-gnu"
tdir="${GUEST_TARGET_DIR:-$CARGO_TARGET_DIR/riscv-guest-one}/${features:-default}"
mkdir -p "$out"
(cd "$BV_REPO" && env -u RUSTFLAGS RUSTC="$tc/bin/rustc" CARGO_ENCODED_RUSTFLAGS="$flags" RISC0_FEATURE_bigint2="" \
  CC="$HOME/.risc0/cpp/bin/riscv32-unknown-elf-gcc" CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" \
  CARGO_TARGET_DIR="$tdir" nice -n 15 "$tc/bin/cargo" build --release --locked --target "$T" \
  --manifest-path "$manifest" --bin "$bin" ${features:+--features "$features"})
cp "$tdir/$T/release/$bin" "$out/$bin"
(cd "$BV_REPO" && RISC0_SKIP_BUILD=1 nice -n 15 cargo build -q --locked -p guest_ids)
line="$("$CARGO_TARGET_DIR/debug/guest-ids" "$out/$bin" "$out/$bin.bin")"
echo "${line##* }" > "$out/$bin.IMAGE_ID"
echo "$line"
