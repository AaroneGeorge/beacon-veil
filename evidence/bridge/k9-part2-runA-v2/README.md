# K9 part 2 against W4 run A (measured, dev mode), 2026-11-12

Claim stage: start 2026-11-12T13:01:22Z, end 2026-11-12T13:02:31Z (UTC, `claim-stage.txt`).

- Vault v2 `0xC13643c6846B1df7464ebdB94349288a99b1bb23` (Sepolia; deprecated for the outbound leg because of verifier 3.0.0; the inbound path does not use the vault's verifier). Deposit 0.001 ETH, tx `0x16e462338e9d2b7a32af044b14922dada3762ba78db3ba6ce5e9270a0e963b30`, block 12118608, bound to private account w5-claimer under bridge instance `0xff8018fe...efe0` (`config.json`).
- Run A: attest program `CvWQWXteVtA67suCS32ovZZ4CxbwK7YcQT1fSfpMnR43`, instance `FwXdD7CsPiuGVcX88UUcTdCpRTpaweNQderEuSdrzakd`, records only read. Its head moved from execution block 12118287 to 12118919 at about 13:01 UTC (`wait-head.log`).
- `attest-root` (public, RFP-021): record slot 11562816, proves Sepolia block 12118919; storage claim of the vault's root slot with the RLP execution header (gloas); statement id `0xb6ccdbd4...5101`; root `0x3cc3c4cb...98e5` delivered to the root receipt `3CYiWYssZ1UD3g1a4X5nerowSkpQnHcn5s2XeAUKnGHb`; LEZ tx `95ca6ac00680af0fcd9e3368d78c6d677721b4f54edf8c6a9f6c8e6ac08b7d55`. The `gas` field (10,000,000) is the declared gas limit of that transaction, not a measured figure.
- Private claim (dev mode): LEZ tx `8886086636485960cf11ececfd8429776a516c19399af1abdc09b0c1b2d1fbef`, 34.1 s wall, nullifier PDA `5JYC94W1WtR2AWoV2FMTU49afcpmRpPuoU4a8KUoqXZk` (`claim1.json`).
- Repeat claim refused: `ESA:E11_ALREADY_CONSUMED` (`claim2.txt`).

Command: `BV_IDS=$B/run/w4/run-a/ids.toml tools/bridge_cli/e2e_sepolia.sh claim` (flow state `$B/run/w5/sepolia/flow.env`).
