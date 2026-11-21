# K9 part 2 on vault v3 against W4 run B (measured, dev mode), 2026-11-12

Claim stage: start 2026-11-12T14:20:01Z, end 2026-11-12T14:21:19Z (UTC, `claim-stage.txt`). It overlapped W6's CPU calibration window by permission of the coordinator (dev mode, executor only).

- Vault v3 `0x380be74d9F3dFeBAaFC5F911CD6A45bFbeE3aD27` (verifier 3.1.0 behind its emergency stop; `../sepolia-v3.json`). Deposit 0.001 ETH, tx `0x8f7d5bd98436f83a0e1ad2eaa5fc3f66bfc108a812c114f71b6148652a775afb`, block 12118891; bound to private account w5-claimer under bridge instance `BTpCBokrFB8mvmPT6QukEXtiYUeWmgupHGpxpftvaXD2` (`config.json`).
- Programs on seq-dev from the reproducible release set b3a954c: `bridge` `7ahb3h5n3MTeq2JtYhLbQL75gRpL7xNAmV54SZjvJQ6y` (image fa3bd9c3...), `attest` `D4EHJuxjYVFfTN47A6GbWxgPdYweb9QkC5DmzxbJboDF`.
- Run B instance `3YmV7VDujQF7zqtfMLf9VMKc4G9Vg1qYwYiTFXmJ8kBX`, records only read. Its head moved from execution block 12118762 to 12119301 at about 14:20 UTC (`wait-head.log`).
- `attest-root` (public, RFP-021): record slot 11563200, Sepolia block 12119301, storage claim of the vault's root slot with the RLP execution header; statement id `0xcb6cc908...5311`; root `0x4d92152e...5407` (covers both v3 deposits) delivered to receipt `99w5dkqNWCRowntRfcFDce2Mpo86fkt7Wa9L2aBefwPD`; LEZ tx `0bdf64966160d3047b30399c662379fb597ce19feb94e20db6fd42f3f367ce2a`.
- Private claim (dev mode): LEZ tx `48e096c4aa769a66663dd7e7562b8cb2f074f3706c95d119e265dccc3cb0b2c7`, 37.7 s wall, nullifier PDA `E6QvbozTsTLjbXns1qXQFS2rEnnAWh5kYLzLYMgdw7wS` (`claim1.json`).
- Repeat claim refused: `ESA:E11_ALREADY_CONSUMED` (`claim2.txt`).

Command: `$B/run/w5/poll-v3-claim.sh` (polls the head, then `BV_FLOW_DIR=$B/run/w5/sepolia-v3 tools/bridge_cli/e2e_sepolia.sh claim`).
