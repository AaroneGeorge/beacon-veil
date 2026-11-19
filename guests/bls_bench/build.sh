#!/usr/bin/env bash
# Builds the measurement guest `bls_bench` for riscv32im-risc0-zkvm-elf and prints its image id.
#
#   guests/bls_bench/build.sh local    # local rzup toolchain (rust 1.91.1, cpp 2024.1.5)
#   guests/bls_bench/build.sh docker   # pinned image risczero/risc0-guest-builder:r0.1.91.1, from the committed tree
#
# Both routes set the environment risc0-build 3.0.5 sets for a guest (crates/risc0-build src/lib.rs
# `cargo_command_internal` and src/docker.rs): the same CARGO_ENCODED_RUSTFLAGS, RISC0_FEATURE_bigint2, and the RISC
# Zero C compiler for blst's C sources (CC / CFLAGS_riscv32im_risc0_zkvm_elf = "-march=rv32im -nostdlib").
# The guest is not part of crates/guest_methods: it is never deployed, and the C toolchain stays out of the
# normal guest build.
#
# Output: $BENCH_OUT (default $BENCH_TARGET_DIR/bench-guest/<route>/, BENCH_TARGET_DIR default $CARGO_TARGET_DIR) with bls_bench (user ELF), bls_bench.bin
# (v1compat ProgramBinary) and IMAGE_ID. Docker builds use a detached worktree of HEAD, so the tree must be committed;
# BUILD_INFO records commit, image digest and times.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/../../scripts/env.sh"
route="${1:?usage: build.sh local|docker}"
T=riscv32im-risc0-zkvm-elf
tbase="${BENCH_TARGET_DIR:-$CARGO_TARGET_DIR}"
out="${BENCH_OUT:-$tbase/bench-guest/$route}"
mkdir -p "$out"
us=$'\x1f'
flags="-C${us}passes=lower-atomic${us}-C${us}link-arg=-Ttext=0x00200800${us}-C${us}link-arg=--fatal-warnings${us}-C${us}panic=abort${us}--cfg${us}getrandom_backend=\"custom\""
started="$(date -u +%FT%TZ)"
commit="$(git -C "$BV_REPO" rev-parse HEAD)"

case "$route" in
  local)
    tc="$HOME/.risc0/toolchains/v1.91.1-rust-x86_64-unknown-linux-gnu"
    [ -x "$tc/bin/rustc" ] || { echo "rzup rust 1.91.1 toolchain missing ($tc)" >&2; exit 1; }
    [ -x "$HOME/.risc0/cpp/bin/riscv32-unknown-elf-gcc" ] || { echo "rzup cpp toolchain missing" >&2; exit 1; }
    tdir="$tbase/riscv-guest-bench"
    (cd "$BV_REPO" && env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
      RUSTC="$tc/bin/rustc" CARGO_ENCODED_RUSTFLAGS="$flags" RISC0_FEATURE_bigint2="" \
      CC="$HOME/.risc0/cpp/bin/riscv32-unknown-elf-gcc" CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" \
      CARGO_TARGET_DIR="$tdir" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
      nice -n 15 "$tc/bin/cargo" build --release --locked --target "$T" --manifest-path guests/bls_bench/Cargo.toml --bin bls_bench)
    cp "$tdir/$T/release/bls_bench" "$out/bls_bench"
    image="local rzup: rust $("$tc/bin/rustc" --version | cut -d' ' -f2), $("$HOME/.risc0/cpp/bin/riscv32-unknown-elf-gcc" --version | head -1)"
    ;;
  docker)
    tag="${RISC0_DOCKER_CONTAINER_TAG:-r0.1.91.1}"
    src="$B/.tmp/bench-src-${commit:0:12}"
    work="$B/.tmp/bench-docker-${commit:0:12}"
    mkdir -p "$work"
    [ -d "$src" ] || git -C "$BV_REPO" worktree add -q --detach "$src" "$commit"
    [ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || { echo "$src is not at $commit" >&2; exit 1; }
    cat > "$work/Dockerfile" <<DOCKER
FROM risczero/risc0-guest-builder:$tag as build
WORKDIR /src
COPY . .
ENV CARGO_ENCODED_RUSTFLAGS="${flags//\"/\\\"}"
ENV CARGO_TARGET_DIR="target"
ENV RISC0_FEATURE_bigint2=""
ENV CC_riscv32im_risc0_zkvm_elf="/root/.risc0/cpp/bin/riscv32-unknown-elf-gcc"
ENV CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib"
RUN cargo +risc0 fetch --locked --target $T --manifest-path guests/bls_bench/Cargo.toml
RUN mkdir -p /out && CARGO_MANIFEST_PATH=guests/bls_bench/Cargo.toml CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4} \\
    cargo +risc0 build --release --locked --target $T --manifest-path guests/bls_bench/Cargo.toml --bin bls_bench && \\
    cp target/$T/release/bls_bench /out/

FROM scratch as export
COPY --from=build /out /
DOCKER
    printf '**/Dockerfile\n**/.git\n**/node_modules\n**/target\n**/tmp\n' > "$work/Dockerfile.dockerignore"
    docker build --no-cache --output="$out" -f "$work/Dockerfile" "$src" > "$work/docker.log" 2>&1 \
      || { tail -30 "$work/docker.log" >&2; exit 1; }
    image="docker.io/risczero/$(grep -oE "risc0-guest-builder:$tag@sha256:[0-9a-f]{64}" "$work/docker.log" | head -1)"
    [ "$image" != "docker.io/risczero/" ] || image="risczero/risc0-guest-builder:$tag (digest: $(docker image inspect --format '{{index .RepoDigests 0}}' "risczero/risc0-guest-builder:$tag"))"
    cp "$work/Dockerfile" "$out/Dockerfile"
    ;;
  *) echo "usage: build.sh local|docker" >&2; exit 2 ;;
esac

(cd "$BV_REPO" && RISC0_SKIP_BUILD=1 nice -n 15 cargo build -q --locked -p guest_ids)
line="$("$CARGO_TARGET_DIR/debug/guest-ids" "$out/bls_bench" "$out/bls_bench.bin")"
echo "${line##* }" > "$out/IMAGE_ID"
{
  echo "route=$route"
  echo "commit=$commit$( [ -n "$(git -C "$BV_REPO" status --porcelain)" ] && [ "$route" = local ] && echo ' (working tree dirty)')"
  echo "toolchain=$image"
  echo "started_utc=$started"
  echo "finished_utc=$(date -u +%FT%TZ)"
  echo "elf_sha256=$(sha256sum "$out/bls_bench" | cut -d' ' -f1)"
  echo "image_id=$(cat "$out/IMAGE_ID")"
  echo "user_elf_bytes=$(stat -c %s "$out/bls_bench")"
} > "$out/BUILD_INFO"
cat "$out/BUILD_INFO"
