# Full spike, outbound leg: Groth16 release and open release on Sepolia (vault v3), 2026-11-12

All figures measured unless marked. Machine-readable: `record.json`. Raw files: `raw/` and the proving job's own directory `evidence/prove/w5-groth16-burn/20261112T202300Z/`.

| Step | Where | Result |
|---|---|---|
| Claim 100e6 test USDT (second wrapped balance) | seq-dev, dev mode | LEZ tx `a87aaf9e...7da3` |
| Burn 0: 0.001 ETH, relayer fee 1e13, recipient `0x523072A2...BD78` | seq-dev, private holder | LEZ tx `fa5e5189...3426`, leaf 0 |
| Burn 1: 100e6 test USDT, fee 0, same recipient | seq-dev, private holder | LEZ tx `d5a06127...59c8`, leaf 1 |
| STUB anchor post of root `0x8cdfcbe1...63ad` (count 2) | Sepolia | tx `0x348a2eeab2e335bcaabd830a0b9922f7ccf3635958228108fa9171d170387dd9`, 130,982 gas; final after 600 s |
| Real Groth16 proof of burn 0 | this laptop, `prove_job.sh` | 471.3 s prove (7:51 wall incl. setup), peak RSS 2,419,024 KiB (tree), 211,726 user cycles (262,144 padded, 1 segment), seal 260 bytes, selector `0xe71e6620` |
| `release(seal, journal)` through the 3.1.0 emergency stop | Sepolia, vault v3 | tx `0x65412c390ad21f28b686ead8ec065774339229240a63a913a742b6777c221c12`, **527,859 gas**; vault ETH 0.001 -> 0; nullifier spent |
| `releaseOpen` (failsafe L2) for burn 1 | Sepolia, vault v3 | tx `0x0f7e3da0dd63607ee2ec18e59d68794c67b040f8586c0d14dc5edf799166d565`, **325,769 gas**; recipient +100e6 test USDT; nullifier spent |

Verifier-only gas (**estimated**, `eth_estimateGas` of a standalone `verify` call, including 21,000 intrinsic gas): 260,699 through the emergency stop `0xA1DCB16c...bA29`, 250,363 on the raw 3.1.0 verifier `0x8c915b50...aB3b`.

Prover: guest `burn_proof` built reproducibly on RISC Zero 3.1.0 (image `0xf929f9f441d9aa652b9914b4ce6c6a7b445aa90d189c075af1e1e9fe3b1497cf`, `../burn-proof-r31-guest.json`), proven with `guests/burn_proof/r31` host `burn_prover_r31` against the isolated r0vm 3.1.0 and risc0-groth16 component 0.2.0 (`raw/groth16-burn0.sh`). About the machine line: `prove_job.sh` records the r0vm on the default PATH (`~/.cargo/bin/r0vm`, 3.0.5, used by the LEZ tracks), so the job's `meta.txt` and `record.json` say "risc0-r0vm 3.0.5". This proof did not use it. `raw/groth16-burn0.sh` sets `RISC0_SERVER_PATH` to the isolated r0vm 3.1.0 (`$B/run/w5/risc0-31/extensions/v3.1.0-cargo-risczero-x86_64-unknown-linux-gnu/r0vm`), the process list during the run showed that binary, and the host is linked against risc0-zkvm 3.1.0 (`guests/burn_proof/r31/Cargo.lock`). The seal's selector `0xe71e6620` is the 3.1.0 verifier's, which a 3.0.5 prover cannot produce. The receipt was verified locally against the image id before release, and its journal equals the native statement (`raw/burn-0-journal-native.hex`). The witness (burn secret) was deleted after proving.

Trust, stated: the anchor is `StubLezAnchor`, owner-posted. This release shows the zero-knowledge route and the open route working against a posted root; it does not show a trustless anchor (`docs/bridge/TRUST.md` section 1). The LEZ side ran on seq-dev in dev mode (rc3 pins risc0-zkvm 3.0.5, a named platform risk).

Privacy note: `releaseOpen` reveals the burn secret by design (opt-in), so burn 1 is linkable to its release; burn 0 (zero-knowledge route) is not.
