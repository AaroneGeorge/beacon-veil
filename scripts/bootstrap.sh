#!/usr/bin/env bash
# Sets up a machine (or a clean clone) for beacon-veil: pinned vendor clones, toolchains, LEZ binaries, run dirs.
#
#   scripts/bootstrap.sh [--check] [--no-lez-build] [--with-sudo] [--with-hello]
#
#   $B (build dir) = $BV_BUILD_DIR, else $B, else the repo's parent directory. Layout under $B:
#     vendor/<dir>     pinned clones (scripts/pins.txt), commit-checked
#     vendor/lez/target/release/{sequencer_service,wallet,run_hello_world*}   LEZ rc3 binaries
#     tools/logosctl/  logosctl 0.3.2 AppImage (sha256-checked)
#     run/ .tmp/ target/   sequencer homes and wallets, temp files, shared cargo target
#
#   --check         report what is missing, change nothing (exit 1 if anything is missing)
#   --no-lez-build  skip building the LEZ binaries (about 20 min on 4 cores)
#   --with-sudo     run the apt-get step (it is always printed first); without it, only print it
#   --with-hello    also build LEZ's hello_world example guests in docker (acceptance and pipeline smoke test)
#
# Versions: rzup components cargo-risczero 3.0.5, r0vm 3.0.5, rust 1.91.1, cpp, risc0-groth16 0.1.0; host Rust from
# rust-toolchain.toml (1.98.1, via rustup); Foundry v1.9.0; just 1.59.0; cargo-nextest 0.9.151. Nix (only for the Logos
# module) is not installed here: see the end of this script's output.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export B="${BV_BUILD_DIR:-${B:-$(cd "$repo/.." && pwd)}}"
check=0; lez_build=1; with_sudo=0; with_hello=0
for a in "$@"; do
  case "$a" in
    --check) check=1 ;; --no-lez-build) lez_build=0 ;; --with-sudo) with_sudo=1 ;; --with-hello) with_hello=1 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done
missing=0
say() { printf '[bootstrap] %s\n' "$*"; }
need() { say "MISSING: $*"; missing=1; }

say "repo $repo"; say "build dir B=$B"
mkdir -p "$B/vendor" "$B/tools" "$B/run" "$B/.tmp" "$B/target"
export TMPDIR="$B/.tmp" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export PATH="$HOME/.risc0/bin:$HOME/.cargo/bin:$HOME/.foundry/bin:$PATH"

# ---- 1. system packages (sudo: printed, run only with --with-sudo) ----
apt_pkgs="build-essential clang libclang-dev libssl-dev pkg-config cmake jq libpcsclite-dev curl git"
apt_missing=""
for p in $apt_pkgs; do dpkg -s "$p" >/dev/null 2>&1 || apt_missing="$apt_missing $p"; done
if [ -n "$apt_missing" ]; then
  cmd="sudo DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get install -y$apt_missing"
  say "system packages needed: $cmd"
  if [ "$with_sudo" = 1 ] && [ "$check" = 0 ]; then $cmd; else need "apt packages:$apt_missing (rerun with --with-sudo, or run the line above)"; fi
fi
docker version >/dev/null 2>&1 || say "NOTE: docker not usable by $(id -un) (needed for guest docker builds): sudo usermod -aG docker $(id -un), then log in again"

# ---- 2. pinned vendor clones ----
while read -r dir repo_name ref commit; do
  case "$dir" in ''|\#*) continue ;; esac
  dest="$B/vendor/$dir"
  if [ -d "$dest/.git" ]; then
    have="$(git -C "$dest" rev-parse HEAD)"
    [ "$have" = "$commit" ] || { say "vendor/$dir is at $have, pinned $commit: fix by hand"; exit 1; }
    continue
  fi
  [ "$check" = 1 ] && { need "vendor/$dir ($repo_name $ref)"; continue; }
  say "clone vendor/$dir = $repo_name $ref ($commit)"
  git init -q "$dest"
  git -C "$dest" remote add origin "https://github.com/$repo_name"
  git -C "$dest" fetch -q --depth 1 origin "$commit"
  git -C "$dest" checkout -q FETCH_HEAD
  [ "$(git -C "$dest" rev-parse HEAD)" = "$commit" ] || { say "commit mismatch for vendor/$dir"; exit 1; }
done < "$repo/scripts/pins.txt"

# ---- 3. contracts/lib submodules (forge-std v1.17.0, OpenZeppelin v5.7.0, risc0-ethereum v3.0.1) ----
if [ -f "$repo/.gitmodules" ] && [ -d "$repo/.git" ]; then
  if [ "$check" = 1 ]; then
    git -C "$repo" submodule status | grep -q '^-' && need "contracts/lib submodules (git submodule update --init)"
  else
    git -C "$repo" submodule update --init --depth 1
  fi
fi

# ---- 4. toolchains ----
command -v rustup >/dev/null || need "rustup (https://rustup.rs): the host toolchain comes from rust-toolchain.toml"
if ! command -v rzup >/dev/null; then
  if [ "$check" = 1 ]; then need "rzup"; else
    say "installing rzup: curl -L https://risczero.com/install | bash (RISC Zero's installer; writes ~/.risc0 and a PATH line in your shell rc)"
    curl -sSfL https://risczero.com/install -o "$B/.tmp/rzup-install.sh" && bash "$B/.tmp/rzup-install.sh"
  fi
fi
rz_have() { rzup show 2>/dev/null | awk -v c="$1" '$0==c{f=1;next} /^[a-z]/{f=0} f' | grep -q -- "$2"; }
for cv in "cargo-risczero 3.0.5" "r0vm 3.0.5" "rust 1.91.1" "cpp 2024.1.5" "risc0-groth16 0.1.0"; do
  set -- $cv
  if command -v rzup >/dev/null && rz_have "$1" "$2"; then continue; fi
  if [ "$check" = 1 ]; then need "rzup $1 $2"; else say "rzup install $1 $2"; rzup install "$1" "$2"; fi
done
if command -v rzup >/dev/null && [ "$check" = 0 ]; then rzup default cargo-risczero 3.0.5 >/dev/null 2>&1 || true; rzup default r0vm 3.0.5 >/dev/null 2>&1 || true; fi

if ! command -v forge >/dev/null || ! forge --version | grep -q "Version: 1.9.0"; then
  if [ "$check" = 1 ]; then need "Foundry v1.9.0"; else
    command -v foundryup >/dev/null || { say "installing foundryup: curl -L https://foundry.paradigm.xyz | bash"; curl -sSfL https://foundry.paradigm.xyz | bash; }
    foundryup -i v1.9.0
  fi
fi
ver_ok() { case "$1" in just) just --version ;; cargo-nextest) cargo nextest --version ;; esac 2>/dev/null | grep -q "$2"; }
for tv in "just 1.59.0" "cargo-nextest 0.9.151"; do
  set -- $tv
  ver_ok "$1" "$2" && continue
  if [ "$check" = 1 ]; then need "$1 $2"; else say "cargo install $1 --version $2 --locked"; nice -n 15 cargo install "$1" --version "$2" --locked; fi
done

# ---- 5. logosctl 0.3.2 (Logos module runtime) ----
lc="$B/tools/logosctl/logosctl-x86_64.AppImage"
if [ ! -x "$lc" ]; then
  if [ "$check" = 1 ]; then need "logosctl 0.3.2"; else
    mkdir -p "$B/tools/logosctl"
    t="$B/tools/logosctl/logosctl-x86_64-linux.tar.gz"
    curl -sSfL -o "$t" https://github.com/logos-co/logos-logoscore-cli/releases/download/0.3.2/logosctl-x86_64-linux.tar.gz
    echo "294fbcc6409a8851b7471582bdad2dbfaf681b5d1c67dfffec38ada925dc6762  $t" | sha256sum -c -
    tar -xzf "$t" -C "$B/tools/logosctl" && chmod +x "$lc" && ln -sf logosctl-x86_64.AppImage "$B/tools/logosctl/logosctl"
  fi
fi

# ---- 6. LEZ rc3 binaries: sequencer (standalone), wallet, example runners ----
lez="$B/vendor/lez"; rel="$lez/target/release"
for b in sequencer_service wallet run_hello_world run_hello_world_private; do
  [ -x "$rel/$b" ] && continue
  if [ "$check" = 1 ] || [ "$lez_build" = 0 ]; then need "$rel/$b"; continue; fi
  say "building LEZ binaries (nice -n 15; about 20 min on 4 cores)"
  (cd "$lez" && nice -n 15 cargo build --release --features standalone -p sequencer_service \
     && nice -n 15 cargo build --release -p wallet && nice -n 15 cargo build --release -p program_deployment)
  break
done

# ---- 7. optional: LEZ hello_world guests in the pinned docker image ----
if [ "$with_hello" = 1 ] && [ "$check" = 0 ]; then
  (cd "$lez" && RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 CARGO_TARGET_DIR="$B/run/hello-target" nice -n 15 \
     cargo risczero build --manifest-path examples/program_deployment/methods/guest/Cargo.toml)
fi

say "Nix (only for the Logos module build): official multi-user installer with TMPDIR=$B/.tmp, then add"
say "  'experimental-features = nix-command flakes' to /etc/nix/nix.conf. Not run here (sudo, about 10 GB)."
if [ "$missing" = 1 ]; then say "some items are missing (see above)"; exit 1; fi
say "OK. Next: source scripts/env.sh && scripts/seq.sh dev start"
