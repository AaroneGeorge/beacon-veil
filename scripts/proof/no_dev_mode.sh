#!/usr/bin/env bash
# Runs COMMAND only if RISC0_DEV_MODE is unset in its environment, else fails loudly (exit 9). Every proving command of
# the proof window goes through it, right where the proof is made, so no wrapper on the way can turn real proofs off.
#   scripts/proof/no_dev_mode.sh COMMAND [ARGS...]
if [ -n "${RISC0_DEV_MODE+x}" ]; then
  echo "REFUSED: RISC0_DEV_MODE=${RISC0_DEV_MODE} is set; this command must make a real proof: $*" >&2
  exit 9
fi
exec "$@"
