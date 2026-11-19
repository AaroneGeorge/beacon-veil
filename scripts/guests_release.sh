#!/usr/bin/env bash
# Reproducible release set of every guest, built from the COMMITTED tree in the pinned image
# risczero/risc0-guest-builder:r0.1.91.1, into $B/guests-release/<sha>/ with SHA256SUMS and IMAGE_IDS.
#
#   scripts/guests_release.sh [<commit>]       # default HEAD
#
# The Dockerfile is risc0-build 3.0.5's own (crates/guest_methods with BV_GUEST_DOCKER=1 generates the same one: same
# image, ENV, workdir /src, CARGO_TARGET_DIR=target, `cargo +risc0 build --release --locked`), with one change: all
# guests are built in ONE container after ONE `cargo fetch`, instead of one container and one fetch per guest
# (about 15 min of git fetching each on this machine). ELFs are wrapped with the v1compat kernel by tools/guest_ids
# (same logic as tests/e2e_chain mkbin), as risc0-build does, which also prints the image id.
#
# BV_RELEASE_DIR overrides the output dir. CI runs this and then scripts/check_image_ids.sh against
# guests/IMAGE_IDS.expected.
#
# Outputs: attest, lc_verify, lc_verify_v2 (lc_verify --features v2), consumer_erc20, bridge, bridge_dev_source,
# burn_proof, lc_verify_v3 (.bin + user ELF). lc_verify_v3 alone is built with $BV_V3_CARGO_CONFIG (scripts/env.sh:
# the bigint shim at opt-level 3), given on its cargo command line; the other guests are built exactly as before., SHA256SUMS, IMAGE_IDS, BUILD_INFO.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"

commit="$(git -C "$BV_REPO" rev-parse "${1:-HEAD}")"
tag="${RISC0_DOCKER_CONTAINER_TAG:-r0.1.91.1}"
out="${BV_RELEASE_DIR:-$B/guests-release/$commit}"
src="$B/.tmp/release-src-${commit:0:12}"
work="$B/.tmp/release-docker-${commit:0:12}"
[ -e "$out/SHA256SUMS" ] && { echo "$out already complete"; exit 0; }
mkdir -p "$out" "$work"

# Committed tree only: a detached worktree (no untracked or modified files reach the docker context).
[ -d "$src" ] || git -C "$BV_REPO" worktree add -q --detach "$src" "$commit"
[ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || { echo "$src is not at $commit" >&2; exit 1; }

T=riscv32im-risc0-zkvm-elf
build="CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4} cargo +risc0 build --release --locked --target $T"
cat > "$work/Dockerfile" <<DOCKER
FROM risczero/risc0-guest-builder:$tag as build
WORKDIR /src
COPY . .
ENV CARGO_ENCODED_RUSTFLAGS="-C$(printf '\x1f')passes=lower-atomic$(printf '\x1f')-C$(printf '\x1f')link-arg=-Ttext=0x00200800$(printf '\x1f')-C$(printf '\x1f')link-arg=--fatal-warnings$(printf '\x1f')-C$(printf '\x1f')panic=abort$(printf '\x1f')--cfg$(printf '\x1f')getrandom_backend=\"custom\""
ENV CARGO_TARGET_DIR="target"
ENV RISC0_FEATURE_bigint2=""
ENV CC_riscv32im_risc0_zkvm_elf="/root/.risc0/cpp/bin/riscv32-unknown-elf-gcc"
ENV CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib"
RUN cargo +risc0 fetch --locked --target $T --manifest-path programs/attest/Cargo.toml
RUN mkdir -p /out && R=target/$T/release && \\
    CARGO_MANIFEST_PATH=programs/attest/Cargo.toml $build --manifest-path programs/attest/Cargo.toml && cp \$R/attest /out/ && \\
    CARGO_MANIFEST_PATH=programs/consumer_erc20/Cargo.toml $build --manifest-path programs/consumer_erc20/Cargo.toml && cp \$R/consumer_erc20 /out/ && \\
    CARGO_MANIFEST_PATH=programs/bridge/Cargo.toml $build --manifest-path programs/bridge/Cargo.toml && cp \$R/bridge \$R/bridge_dev_source /out/ && \\
    CARGO_MANIFEST_PATH=guests/burn_proof/Cargo.toml $build --manifest-path guests/burn_proof/Cargo.toml && cp \$R/burn_proof /out/ && \\
    CARGO_MANIFEST_PATH=programs/lc_verify/Cargo.toml $build --manifest-path programs/lc_verify/Cargo.toml && cp \$R/lc_verify /out/ && \\
    CARGO_MANIFEST_PATH=programs/lc_verify/Cargo.toml $build --manifest-path programs/lc_verify/Cargo.toml --features v2 && cp \$R/lc_verify /out/lc_verify_v2 && \\
    CARGO_MANIFEST_PATH=programs/lc_verify_v3/Cargo.toml $build --manifest-path programs/lc_verify_v3/Cargo.toml --config '$BV_V3_CARGO_CONFIG' && cp \$R/lc_verify_v3 /out/

FROM scratch as export
COPY --from=build /out /
DOCKER
printf '**/Dockerfile\n**/.git\n**/node_modules\n**/target\n**/tmp\n' > "$work/Dockerfile.dockerignore"

echo "building guests of $commit in $tag (log: $work/docker.log)"
docker build --output="$out/elf" -f "$work/Dockerfile" "$src" > "$work/docker.log" 2>&1 || { tail -30 "$work/docker.log" >&2; exit 1; }

# Wrap with the v1compat kernel (risc0-build's ProgramBinary) and record image ids.
(cd "$BV_REPO" && RISC0_SKIP_BUILD=1 nice -n 15 cargo build -q --locked -p guest_ids)
: > "$out/IMAGE_IDS"
for g in attest lc_verify lc_verify_v2 consumer_erc20 bridge bridge_dev_source burn_proof lc_verify_v3; do
  "$CARGO_TARGET_DIR/debug/guest-ids" "$out/elf/$g" "$out/$g.bin" | sed "s#^$out/##" >> "$out/IMAGE_IDS"
done
(cd "$out" && sha256sum ./*.bin elf/* > SHA256SUMS)
{
  echo "commit=$commit"
  echo "image=docker.io/risczero/$(grep -oE "risc0-guest-builder:$tag@sha256:[0-9a-f]{64}" "$work/docker.log" | head -1)"
  echo "note_burn_proof=burn_proof.bin is built with risc0-zkvm 3.0.5 like the rest; the deployed burn_proof is built with risc0 3.1.0 (advisory GHSA-g6qg-9j8p-7j87), so this one is NOT the vault's"
  echo "built_utc=$(date -u +%FT%TZ)"
  echo "machine=$(uname -srm), $(lscpu | sed -n 's/^Model name: *//p')"
  echo "dockerfile=$work/Dockerfile"
} > "$out/BUILD_INFO"
cp "$work/Dockerfile" "$out/Dockerfile"
cat "$out/IMAGE_IDS"
echo "release set: $out"
