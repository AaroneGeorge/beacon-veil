# Local round trip, e2e-20261112T104701Z (measured, dev mode)

Command: `tools/bridge_cli/e2e_local.sh` (anvil vault with RiscZeroMockVerifier, seq-dev in RISC0_DEV_MODE=1, DEV statement source instead of RFP-021).
The release step failed in the script on a Foundry file-permission error (fixed: ReleaseOne now takes SEAL and JOURNAL as env hex). It was then run by hand with the same seal and journal; output in release.txt.

Results:
- deposit 0.01 ETH on anvil, DEV attest of the root, private claim on seq-dev in dev mode: claim1.json (33.6 s wall, dev mode)
- repeat claim refused with ESA:E11_ALREADY_CONSUMED: claim2.txt
- private burn of 0.01 to 0x...beef with fee 0.0001: burn.json; accumulator root posted to the STUB anchor
- burn_proof in the RISC Zero executor: 261,662 user cycles, 1 segment (burn0/meta.json); mock seal only
- release on anvil: recipient +0.0099 ETH, relayer +0.0001 ETH, nullifier spent, 152,329 gas with the mock verifier (local EVM)
- recover from the mnemonic: deposit claimed, burn released (recover.json)

Machine: lechu, 8 cpus, Intel(R) Core(TM) i5-10300H CPU @ 2.50GHz.
