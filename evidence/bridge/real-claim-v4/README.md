# Inbound leg with a real proof: private claim on vault v4, 2026-11-20 and 2026-11-21

All figures measured. Machine-readable: `record.json`, written by `tools/bridge_cli/e2e_v4.sh record-claim`
(`tools/bridge_cli/record_v4.py`) from the stage files copied to `raw/`. Vault deployment: `../sepolia-v4.json`.
Sequencer: `seq-pub`, a standalone LEZ v0.4.0-rc3 sequencer started with `RISC0_DEV_MODE` unset.

| Step | Where | Result |
|---|---|---|
| Vault v4, STUB anchor, test USDT; verifier 3.1.0 behind its emergency stop, `burnImageId` `0xf929...97cf`, `lezProgram` = the bridge program on `seq-pub` | Sepolia | `0x376Ec2256630029A237Ade83FD428520523dEc56`, immutables read back on chain (`../sepolia-v4.json`) |
| Bridge config bound to the vault (`bindLezInstance`) | Sepolia | tx `0x1436a483f427bc7fc945673355294226f176cfe7e3874027494e891e59815518` |
| Deposit 0.001 ETH, index 0 | Sepolia | tx `0xd7d7e6dc26bc4f669f4623b72e75766da846e69b819d80b61d185a308581062a`, block 12,176,877 |
| Fund the private claimer (private transfer) | `seq-pub`, real proof | 1,188.61 s, peak 8.897 GiB (`raw/prove-bridge-fund-claimer-20261120T185408Z/`) |
| `attest-root`: the vault's root slot proved against the Sepolia light-client instance `5ydZVRKH...4UXN` | `seq-pub`, public transaction | tx `4049dd4f3d62c1de15500e2b4458d4d1aa935c38ea15b9d2a8d2d5f3233a1832`, record slot 11,627,968 (Sepolia block 12,182,246), root `0x9e4f3d03...d209` |
| Private `Claim` of deposit 0 | `seq-pub`, real proof | tx `7824f2f12fd5600aae44579a3203f3c608408010d9d77ed1409b8ff06052bec3`, block 6467, not reverted; 1,856.03 s, peak 9.189 GiB; witness set 239,202 bytes |
| Wrapped balance of the claimer | `seq-pub` | 0 before, 1,000,000,000,000,000 wei after: moved by exactly the denomination |
| The same claim again | `seq-pub` | refused with `ESA:E11` (`E11_ALREADY_CONSUMED`) after 1.87 s, while the wallet executes the program, before any proof is made (`raw/claim2.txt`, `raw/claim2-seconds.txt`) |

Peak memory is the larger of `/usr/bin/time -v` and the per-second process-tree sampler (`time.txt`, `samples.csv` in
each job folder). `meta.txt` in each job folder records `risc0_dev_mode=unset`.

What this shows, and what it does not:

- The private claim was proven for real and verified by the sequencer. The record it was checked against was written by
  the Sepolia instance on `seq-pub` that a follower keeps current on the public path: three public transactions per
  update to `lc_verify_v3`, executed by the sequencer, with no proof (`docs/PUBLIC-PATH.md`).
- `seq-pub` is a local standalone sequencer, not a public Logos testnet.
- The first claim attempt (2026-11-20, `raw/flow-20261120T191503Z-claim.log`) stopped before any proof: the public
  Sepolia RPC no longer served the state for `eth_getProof` at the record's block. The claim stage ran again on
  2026-11-21 against a newer record; the deposit and the funding were not repeated.
- A claim to a public destination was not run.

Paths in the copied files name the build folder `${BUILD}` and the home folder `~`.
