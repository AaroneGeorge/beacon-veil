# Local round trip, 2026-11-12 12:16 UTC: vault v3 code + burn_proof on RISC Zero 3.1.0 (measured, dev mode)

Command: `tools/bridge_cli/e2e_local.sh`. anvil vault with the v3 code (RiscZeroMockVerifier stands in for the 3.1.0 estop), seq-dev in RISC0_DEV_MODE=1, the DEV statement source instead of RFP-021. burn_proof proven by `guests/burn_proof/r31` (risc0-zkvm 3.1.0) from the witness `bridge_cli prove --mode witness` writes. Mechanics only; the image id here is the local-toolchain build, not the docker one in vault v3.

- deposit 0.01 ETH, DEV root attestation, private claim on seq-dev (claim1.json)
- repeat claim refused with ESA:E11_ALREADY_CONSUMED (claim2.txt)
- private burn, STUB anchor post, burn_proof 3.1.0 in the executor (burn0/meta.json), release on anvil: recipient +0.0099 ETH (release.txt, log.txt)
- recover from the mnemonic (recover.json). Script ended "E2E LOCAL OK".
