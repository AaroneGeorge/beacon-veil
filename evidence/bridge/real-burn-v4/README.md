# Outbound leg with real proofs: private burn on seq-pub, release on vault v4, 2026-11-21

All figures measured. Machine-readable: `record.json`, written by `tools/bridge_cli/e2e_v4.sh record-burn`
(`tools/bridge_cli/record_v4.py`) from the stage files copied to `raw/`. The 0.001 ETH burned here is the one claimed in
`../real-claim-v4/`.

| Step | Where | Result |
|---|---|---|
| Private `burn` of 0.001 ETH, relayer fee 1e13, recipient `0x523072A2...BD78` | `seq-pub` (`RISC0_DEV_MODE` unset), real proof | tx `9b8e7dba01b2efe2b0e57d3e6779fad53acb38dc65b1cd309e8456730fd015cc`, block 6588, not reverted; 1,811.57 s, peak 9.187 GiB; wrapped balance 1e15 wei -> 0 |
| Burn accumulator root posted to the STUB anchor by its owner | Sepolia | tx `0x35e81110ed6880ef1c774117dd0d96816c2dc90b06350764885c84569f5d6bf0`, block 12,182,647; final after 600 s |
| Groth16 proof of `burn_proof` (RISC Zero 3.1.0, image `0xf929f9f441d9aa652b9914b4ce6c6a7b445aa90d189c075af1e1e9fe3b1497cf`) | this laptop | 301.67 s, peak 2.306 GiB, 211,726 user cycles (1 segment), seal 260 bytes, selector `0xe71e6620` (`raw/burn-0-groth16/meta.json`) |
| `Vault.release(seal, journal)` through the 3.1.0 emergency stop | Sepolia, vault v4 | tx `0x85431747f71d38399e41569ad70c88d6822650d079bd106ebf514f42411c7161`, block 12,182,725, **527,883 gas**, status 1 |

Peak memory is the larger of `/usr/bin/time -v` and the per-second process-tree sampler. The Groth16 step runs the
prover in a docker container, so its time and memory cover the local process, not the container. Its job `meta.txt`
names the r0vm on the default PATH (3.0.5); the command in `time.txt` sets `RISC0_SERVER_PATH` to the isolated r0vm
3.1.0, `meta.json` says `risc0_zkvm` 3.1.0, and the selector `0xe71e6620` is the 3.1.0 verifier's. The witness (burn
secret) was deleted after proving; `raw/burn-0-witness/` keeps only its public metadata and journal.

Trust, stated: **the anchor is `StubLezAnchor`, owner-posted.** Whoever holds its owner key can make any root final.
This release shows the zero-knowledge route against a posted root with real proofs on both sides; it is not a
trustless release (`docs/bridge/TRUST.md` section 1). RFP-022 F7 cannot be met in full on LEZ v0.4.0-rc3. `seq-pub`
is a local standalone sequencer, not a public Logos testnet.

Paths in the copied files name the build folder `${BUILD}` and the home folder `~`.
