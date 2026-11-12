# Bridge evidence (RFP-022 spike, W5)

Every figure here has a raw file beside it. Gas from Sepolia receipts is measured; gas from `forge --gas-report` is measured on the local EVM only and differs from Sepolia (see the note in `sepolia-k9-part1.json`).

## Deprecated: vaults v1 and v2 (verifier 3.0.0, affected by GHSA-g6qg-9j8p-7j87)

RISC Zero advisories GHSA-g6qg-9j8p-7j87 (risc0) and GHSA-mm6c-35cm-g4x2 (risc0-ethereum), published 2026-11-11, affect the Groth16 verifier up to 3.0.0 and `risc0-zkvm` below 3.1.0. Vaults v1 and v2 held verifier 3.0.0 (`0x2a098988600d87650Fb061FfAff08B97149Fa84D`) directly as an immutable. They are **deprecated testnet artefacts**. Do not cite a release through them as secure evidence.

Both were frozen on 2026-11-12 (global deposits and releases, `setGlobalFreeze(true, true)`):

| Vault | Address | Freeze tx |
|---|---|---|
| v1 | `0xf4d3EE2274842bf32CE6873bdA4e4D0617D911b4` | `0x6b01dfd7c0265276ad87012a20c127a19fc7fc64bead3edabf993ba7077400e9` |
| v2 | `0xC13643c6846B1df7464ebdB94349288a99b1bb23` | `0xfc1a83248dedf0a63cc97062f181f1d002adf3697bae11863b6685ff45e22514` |

What they still show (mechanics only, not security): fixed-denomination ETH and ERC-20 deposits, including a USDT-style token that returns nothing (F17); the deposit root at a fixed storage slot; recovery from the seed. Files: `sepolia-k9-part1.json` and `raw/` (v1); the v2 deposits and the run against W4's run A are in `$B/TRACK-W5-NOTES.md`.

## Current: vault v3

Verifier 3.1.0 behind its emergency-stop wrapper (failsafe L1), the open release route `releaseOpen` (failsafe L2), and `burn_proof` built on RISC Zero 3.1.0 (`guests/burn_proof/r31`). Addresses and transactions are added here when deployed.

## Local round trip

`local-e2e-20261112T104701Z/`: anvil vault (mock verifier) and seq-dev in dev mode, with the DEV statement source standing in for RFP-021. Mechanics only.
