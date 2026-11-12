# shellcheck shell=bash
# Source me: `source scripts/env.sh`. Sets the shared-machine build environment (PLAN.md section 0 rule 5).
# Every build and test on this machine runs with these variables, prefixed with `nice -n 15`.

_bv_repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export BV_REPO="$_bv_repo"
# Build dir: BV_BUILD_DIR, else an exported B, else the repo's parent (layout: scripts/bootstrap.sh).
export B="${BV_BUILD_DIR:-${B:-$(cd "$_bv_repo/.." && pwd)}}"
export R="$BV_REPO"

# One shared target dir: cargo's lock serialises builds across tracks.
export CARGO_TARGET_DIR="$B/target"
export CARGO_BUILD_JOBS=4
# /tmp is a 7.8 GB tmpfs with a per-user quota: keep temp files on disk.
export TMPDIR="$B/.tmp"
mkdir -p "$TMPDIR"

# No real proving outside the windows of PLAN.md 3.4.
export RISC0_DEV_MODE=1
# Guest builder image, same tag as LEZ v0.4.0-rc3 Justfile.
export RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1

# LEZ wallet home for the dev sequencer (seq-dev).
export LEE_WALLET_HOME_DIR="${LEE_WALLET_HOME_DIR:-$B/run/wallet-dev}"

# Tools: rzup (cargo-risczero, r0vm 3.0.5), cargo bins (just, nextest), Foundry, LEZ release binaries, logosctl, Nix.
for _p in "$HOME/.risc0/bin" "$HOME/.cargo/bin" "$HOME/.foundry/bin" "$B/vendor/lez/target/release" "$B/tools/logosctl" \
  "/nix/var/nix/profiles/default/bin"; do
  case ":$PATH:" in *":$_p:"*) ;; *) [ -d "$_p" ] && PATH="$_p:$PATH" ;; esac
done
export PATH
unset _p _bv_repo
