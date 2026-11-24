#!/usr/bin/env bash
# Builds the three LEZ v0.3 guests (attest_v03, lc_verify_v3_v03, attest_fact_v03) and wraps them into deployable `.bin` files.
#
#   lez-v03/scripts/build_guests.sh local     # local rzup toolchain (rust 1.91.1): fast, for tests
#   lez-v03/scripts/build_guests.sh docker    # pinned image risczero/risc0-guest-builder:r0.1.91.1, from the
#                                             # committed tree: the release build whose image ids are recorded
#
# Output: $V03_GUESTS (default lez-v03/target-guests/<mode>): <name> (user ELF), <name>.bin, IMAGE_IDS.
# attest_fact_v03 is deployed with its symbols stripped (`riscv32-unknown-elf-strip --strip-all` of the toolchain that
# links it; deterministic): the loader charges by the byte, and it was added after the other two were deployed, whose
# recorded image ids are of unstripped ELFs, so they are left as they are.
# The environment is risc0-build 3.0.5's for a guest (the same as the root workspace's scripts/build_guest.sh).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ws="$(cd "$here/.." && pwd)"
repo="$(cd "$ws/.." && pwd)"
mode="${1:-local}"
T=riscv32im-risc0-zkvm-elf
GUESTS=(attest_v03 lc_verify_v3_v03 attest_fact_v03)
out="${V03_GUESTS:-$ws/target-guests/$mode}"
host_target="${CARGO_TARGET_DIR:-$ws/target}"
mkdir -p "$out"
us=$'\x1f'
flags="-C${us}passes=lower-atomic${us}-C${us}link-arg=-Ttext=0x00200800${us}-C${us}link-arg=--fatal-warnings${us}-C${us}panic=abort${us}--cfg${us}getrandom_backend=\"custom\""

case "$mode" in
local)
  tc="$HOME/.risc0/toolchains/v1.91.1-rust-x86_64-unknown-linux-gnu"
  tdir="$host_target/riscv-guest"
  for g in "${GUESTS[@]}"; do
    (cd "$ws" && env -u RUSTFLAGS RUSTC="$tc/bin/rustc" CARGO_ENCODED_RUSTFLAGS="$flags" RISC0_FEATURE_bigint2="" \
      CC="$HOME/.risc0/cpp/bin/riscv32-unknown-elf-gcc" CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" \
      CARGO_TARGET_DIR="$tdir" nice -n 15 "$tc/bin/cargo" build --release --locked --target "$T" \
      --manifest-path "programs/$g/Cargo.toml" --bin "$g")
    if [ "$g" = attest_fact_v03 ]; then
      "$HOME/.risc0/cpp/bin/riscv32-unknown-elf-strip" --strip-all -o "$out/$g" "$tdir/$T/release/$g"
    else
      cp "$tdir/$T/release/$g" "$out/$g"
    fi
  done
  ;;
docker)
  tag="${RISC0_DOCKER_CONTAINER_TAG:-r0.1.91.1}"
  commit="$(git -C "$repo" rev-parse HEAD)"
  src="${V03_DOCKER_SRC:-$host_target/docker-src-${commit:0:12}}"
  work="$host_target/docker-work-${commit:0:12}"
  mkdir -p "$work"
  # Committed tree only: a detached worktree, so no untracked or modified file reaches the image.
  [ -d "$src" ] || git -C "$repo" worktree add -q --detach "$src" "$commit"
  [ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || { echo "$src is not at $commit" >&2; exit 1; }
  build="CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4} cargo +risc0 build --release --locked --target $T"
  cat > "$work/Dockerfile" <<DOCKER
FROM risczero/risc0-guest-builder:$tag as build
WORKDIR /src
COPY . .
ENV CARGO_ENCODED_RUSTFLAGS="${flags//\"/\\\"}"
ENV CARGO_TARGET_DIR="target"
ENV RISC0_FEATURE_bigint2=""
ENV CC_riscv32im_risc0_zkvm_elf="/root/.risc0/cpp/bin/riscv32-unknown-elf-gcc"
ENV CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib"
WORKDIR /src/lez-v03
RUN cargo +risc0 fetch --locked --target $T --manifest-path programs/lc_verify_v3_v03/Cargo.toml
RUN mkdir -p /out && R=target/$T/release && \\
    $build --manifest-path programs/attest_v03/Cargo.toml --bin attest_v03 && cp \$R/attest_v03 /out/ && \\
    $build --manifest-path programs/lc_verify_v3_v03/Cargo.toml --bin lc_verify_v3_v03 && cp \$R/lc_verify_v3_v03 /out/ && \\
    $build --manifest-path programs/attest_fact_v03/Cargo.toml --bin attest_fact_v03 && \\
    /root/.risc0/cpp/bin/riscv32-unknown-elf-strip --strip-all -o /out/attest_fact_v03 \$R/attest_fact_v03

FROM scratch as export
COPY --from=build /out /
DOCKER
  printf '**/Dockerfile\n**/.git\n**/node_modules\n**/target\n**/target-guests\n**/tmp\n' > "$work/Dockerfile.dockerignore"
  echo "building guests of $commit in $tag (log: $work/docker.log)"
  docker build ${V03_DOCKER_NO_CACHE:+--no-cache} --output="$out" -f "$work/Dockerfile" "$src" > "$work/docker.log" 2>&1 \
    || { tail -30 "$work/docker.log" >&2; exit 1; }
  grep -oE "risc0-guest-builder:$tag@sha256:[0-9a-f]{64}" "$work/docker.log" | head -1 > "$out/IMAGE" || true
  ;;
*) echo "usage: $0 local|docker" >&2; exit 2 ;;
esac

(cd "$ws" && nice -n 15 cargo build -q --locked --profile host -p v03_harness --bin wrap)
: > "$out/IMAGE_IDS"
for g in "${GUESTS[@]}"; do
  line="$("$host_target/host/wrap" "$out/$g" "$out/$g.bin")"
  echo "$g ${line##* }" >> "$out/IMAGE_IDS"
  echo "$line"
done
