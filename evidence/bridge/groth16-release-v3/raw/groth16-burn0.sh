#!/usr/bin/env bash
# W5: real Groth16 proof of burn 0 (vault v3) with the isolated RISC Zero 3.1.0 toolchain. Deletes the witness after.
export RISC0_HOME=${BUILD}/run/w5/risc0-31
export RISC0_SERVER_PATH=${BUILD}/run/w5/risc0-31/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm
unset RISC0_DEV_MODE
${BUILD}/target-w5-r31/release/burn_prover_r31 prove --witness ${BUILD}/run/w5/sepolia-v3/burn-0-w/witness.borsh --out ${BUILD}/run/w5/sepolia-v3/burn-0-groth16 --mode groth16
rc=$?
rm -f ${BUILD}/run/w5/sepolia-v3/burn-0-w/witness.borsh
exit $rc
