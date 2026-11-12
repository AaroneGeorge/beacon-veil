#!/usr/bin/env bash
# W0 calibration (PLAN.md 3.4): LEZ's `run_hello_world_private` example against seq-real (RISC0_DEV_MODE unset),
# under /usr/bin/time -v, nice -n 19 ionice -c3. Raw output goes to evidence/calibration/; scripts/calibration_json.py
# turns it into evidence/calibration.json.
#
#   scripts/calibrate.sh            # seq-real must be running: scripts/seq.sh real start
#
# Scope (coordinator, 2026-11-12): this is the real-proof PIPELINE smoke test ("pipeline_smoke" in
# evidence/calibration.json), not the K4 number; W6 owns K4 with LEZ's cycle_bench. Do not run inside W6's window.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/env.sh
source "$here/env.sh"
unset RISC0_DEV_MODE
export LEE_WALLET_HOME_DIR="$B/run/wallet-real" LEE_WALLET_PASSWORD=""
"$here/seq.sh" real status >/dev/null

LEZ="$B/vendor/lez"
bin="${HELLO_BIN:-$B/run/hello-target/riscv32im-risc0-zkvm-elf/docker/hello_world.bin}"
runner="$LEZ/target/release/run_hello_world_private"
out="$BV_REPO/evidence/calibration"; mkdir -p "$out"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"

# Funded payer (LEZ local genesis test key, public) and a fresh private account.
payer_label=bv-payer
LEZ_TEST_KEY="7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb" # LEZ Justfile, local genesis
if ! wallet account get --account-id "$payer_label" >/dev/null 2>&1; then
  pid="$(wallet account import public --private-key "$LEZ_TEST_KEY" | sed -n 's#.*Public/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1)"
  wallet account label --account-id "Public/$pid" --label "$payer_label" >/dev/null
fi
# The example runner takes a bare account id: the test key's public account id (same on every fresh wallet).
payer="$(wallet account list | sed -n "s#^Public/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\) \[$payer_label\]\$#\1#p" | head -1)"
priv="$(wallet account new private --label "calib-$stamp" | sed -n 's#.*Private/\([1-9A-HJ-NP-Za-km-z]\{32,44\}\).*#\1#p' | head -1)"
[ -n "$payer" ] && [ -n "$priv" ] || { echo "could not resolve payer ($payer) or private account ($priv)" >&2; exit 1; }
# rc3: the private transaction pays its fee reserve from the private account, so fund it first (this transfer is itself
# a privacy-preserving transaction; on seq-real it is a real proof too, timed separately).
/usr/bin/time -v -o "$out/fund-$stamp.time" nice -n 19 ionice -c3 \
  wallet auth-transfer send --from "$payer_label" --to "Private/$priv" --amount 1000000000 > "$out/fund-$stamp.log" 2>&1
wallet account sync-private >/dev/null

{
  echo "date_utc=$(date -u +%FT%TZ)"
  echo "machine=$(lscpu | sed -n 's/^Model name: *//p'), $(nproc) threads, $(free -g | awk '/Mem:/{print $2}') GiB RAM, kernel $(uname -r)"
  echo "uptime=$(uptime)"
  echo "r0vm=$(r0vm --version)"
  echo "bin=$bin sha256=$(sha256sum "$bin" | cut -c1-64)"
  echo "cmd=/usr/bin/time -v nice -n 19 ionice -c3 env RISC0_INFO=1 RUST_LOG=info $runner $bin $priv $payer"
} > "$out/run-$stamp.meta"

set +e
/usr/bin/time -v -o "$out/run-$stamp.time" nice -n 19 ionice -c3 \
  env RISC0_INFO=1 RUST_LOG="${RUST_LOG:-info}" "$runner" "$bin" "$priv" "$payer" > "$out/run-$stamp.log" 2>&1
rc=$?
set -e
echo "exit=$rc" >> "$out/run-$stamp.meta"
echo "raw files: $out/run-$stamp.{meta,log,time} (exit $rc)"
exit $rc
