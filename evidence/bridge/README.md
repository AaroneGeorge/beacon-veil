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

Verifier 3.1.0 behind its emergency-stop wrapper (failsafe L1), the open release route `releaseOpen` (failsafe L2), and `burn_proof` built on RISC Zero 3.1.0 (`guests/burn_proof/r31`). Vault v3 `0x380be74d9F3dFeBAaFC5F911CD6A45bFbeE3aD27`: `sepolia-v3.json` and `raw-v3/`. Guest build: `burn-proof-r31-guest.json`. K9 part 2 on v3 against run B: `k9-part2-runB-v3/`. Full spike outbound (real Groth16 release through the 3.1.0 estop, 527,859 gas; `releaseOpen`, 325,769 gas): `groth16-release-v3/`.

## Current: vault v4, round trip with real proofs on both sides

Vault v4 `0x376Ec2256630029A237Ade83FD428520523dEc56` (`sepolia-v4.json`): verifier 3.1.0 behind its emergency stop, the same `burn_proof` image id, `lezProgram` = the bridge program on `seq-pub` (dev mode unset), STUB anchor with a 600 s delay. Private claim with a real proof, repeat refused with E11: `real-claim-v4/`. Private burn with a real proof, Groth16 proof and release on Sepolia (527,883 gas): `real-burn-v4/`.

## Chain route: the burn proven from LEZ block bytes (`chain-anchor/`)

`VaultChain.releaseChain` takes no burn root from the operator (`docs/bridge/OUTBOUND.md`, "Chain route"). The real burn of `real-burn-v4/` (block 6588 of `seq-pub`) proven again by guest `burn_chain`: the burn transaction in block 6588, not listed as reverted, in one hash chain from pinned block 6583 to tip 6593. Groth16 on RISC Zero 3.1.0 and `releaseChain` on Sepolia through the 3.1.0 emergency stop, on a test vault bound to the same LEZ instance: `chain-anchor/sepolia.json`. The tip's hash is attested by the STUB tip anchor (our key, 600 s delay); that LEZ executed the chain correctly is not proven. Parser on the whole `seq-pub` chain: `chain-anchor/whole-chain-seq-pub.json`. Executor runs: `chain-anchor/executor.json`. Guest build: `chain-anchor/guest.json`. Anvil fork of the whole flow with the mock verifier: `chain-anchor/anvil-fork.json`.

## Chain route with a 3-of-4 tip anchor (`committee-anchor/`)

The same burn, pinned block and tip, on a third test vault whose root and tip anchor is `CommitteeLezAnchor`: 3 of 4 fixed keys, no owner (`docs/bridge/OUTBOUND.md`, "Chain route with a 3-of-4 tip anchor"). One new Groth16 proof (same guest and image id; the journal names the vault), `attestTip` with 3 signatures and `releaseChain` on Sepolia, and seven refusals as mined transactions: `committee-anchor/sepolia.json`. The anvil-fork run with the mock verifier: `committee-anchor/anvil-fork.json`. The signers are a stand-in: four test keys, all ours, four signer runs reading one sequencer. Not LEZ's keys, and not a trustless release.

## What is real and what is dev mode

- Inbound claim: shown in dev mode on Sepolia data (`k9-part2-runA-v2/`, `k9-part2-runB-v3/`): real RFP-021 attestation of the vault root against followed Sepolia instances, private claim on seq-dev with `RISC0_DEV_MODE=1`, repeat refused with E11. With a real proof on vault v4 (`real-claim-v4/`): the private claim on `seq-pub` with `RISC0_DEV_MODE` unset, 1,856.03 s, peak 9.189 GiB, block 6467; the wrapped balance moved by exactly 0.001 ETH; the repeat refused with E11 after 1.87 s, before any proof. The record it checked was written by the Sepolia instance on `seq-pub` through the public path (public transactions, no proof).
- Outbound release: real. Groth16 proof of `burn_proof` on RISC Zero 3.1.0, verified on Sepolia through the 3.1.0 emergency stop (`groth16-release-v3/`). The LEZ burn itself ran on seq-dev in dev mode, and the anchor is the owner-posted stub. On vault v4 the burn was also proven for real (`real-burn-v4/`: 1,811.57 s, peak 9.187 GiB, block 6588), then Groth16 (301.67 s) and `release` on Sepolia, 527,883 gas. The anchor is still the owner-posted stub: not a trustless release.

## Local round trip

`local-e2e-20261112T104701Z/`: anvil vault (mock verifier) and seq-dev in dev mode, with the DEV statement source standing in for RFP-021. Mechanics only.
