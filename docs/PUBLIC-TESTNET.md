# The public update path on LEZ testnet 0.3

Status: written 2026-11-22, updated 2026-11-23. **Sent.** Both programs are deployed on the public testnet, one
Sepolia instance is initialised there, **one finality update is applied** (instance head slot 11,640,832 to 11,641,504)
and one transaction meant to be refused was refused with the instance unchanged: 17 transactions, blocks 85,572 to
85,754, 2026-11-23 (see "Sent"). Everything else on this page is simulation, and says so.

## The network

LEZ testnet 0.3 (`https://testnet.lez.logos.co`, LEZ release `v0.3.2`) is the only public LEZ network with an
endpoint; testnet 0.4 has none yet (Logos's table "Testnet endpoints", read 2026-11-21). The delivered programs target
LEZ v0.4.0-rc3, whose program model (actors) differs from 0.3's (`plan` + `apply`).

## What runs there

`lez-v03/` holds two thin 0.3 wrappers around the same verifier code, compiled unchanged (`lez-v03/README.md`,
`docs/DECISIONS.md` D-60):

- `lc_verify_v3_v03`: one update as `Begin`, `Miller`, `Finish`, through `attest_core::lc_v3::handle`.
- `attest_v03`: `Init` and `Apply`, through `attest_core::rules::init` and `rules::apply`; `Apply` only as
  `Finish`'s chained call, with the caller checked by rule E12 as on rc3.

The 0.4 programs remain the deliverable.

## Measured in simulation

In `lee::V03State` (LEZ v0.3.2's action phase: `plan`, every `apply` and the chained call summed), with locally built
guests, on the committed mainnet, Sepolia and Hoodi fixtures and the 342-signer floor chain
(`lez-v03/harness/tests/cap.rs`):

| Step | Largest, cycles | Note |
|---|---|---|
| `Begin` | 6,906,533 | the 342-signer floor; 4,459,873 on a real Sepolia update |
| `Miller` | 5,786,289 | |
| `Finish` | 4,472,634 | with the chained `attest_v03.Apply` |
| `Init` | 5,228,566 | |

Every step is under the 9,000,000 declared limit and the 10,000,000-cycle block cap. After each update the instance
head equals the rc3 run's on the same data (`lez-v03/harness/tests/equal_rc3.rs`). Refusals exit with their error
code and are charged less than their limit (`lez-v03/harness/tests/refusals.rs`).

## The planned run, simulated against the live testnet

This is the plan of the largest mode, as simulated on 2026-11-22; what is being sent is the minimal mode (next section).

`bv-v03 plan` with the release guests read the testnet at block 85,012 (2026-11-22, both base fees 8) and simulated,
sending nothing (`evidence/public-testnet/plan/`):

| Part | Transactions | Simulated cycles |
|---|---|---|
| Deployment through the program loader, both headers immutable | 12 | none metered (paid by their 860,132 bytes) |
| `Init` of a Sepolia instance from checkpoint slot 11,616,256 | 1 | 5,048,787 |
| Two period updates (`updates_by_range` 1418, 1419), each `Begin`, `Miller`, `Finish` | 6 | 4,443,564 to 5,786,132 per step |
| One finality update (slot 11,627,936) | 3 | 2,677,794 to 5,785,972 per step |
| One refusal (a duplicate `Finish`, refused with exit code 14) | 1 | 96,196 |

The 22 that should apply do so in simulation, and the refusal is refused as intended. Fees at the head: 390,768,528
units; with the reserve of the largest step the payer needs at least 438,448,648 units. Before each send the client
simulates again against freshly read accounts, sends only if the transaction applies, declares at most 9,000,000 gas,
sends one transaction per block and makes at most one RPC request a second.

## Sent (the minimal run, in two stages)

The run is sent in the mode `minimal`: deploy both programs, `Init` a Sepolia instance from a checkpoint of the current
sync-committee period, apply **one** finality update as `Begin`, `Miller`, `Finish`, then send one transaction meant to
be refused. The faucet pays 100,000,000 units per recipient per 24 h, which covers the deployment and `Init` but not
the whole run, so it is sent in two stages.

Stage 1, sent on 2026-11-23 between 12:40 and 13:52 UTC by `bv-v03 deploy` and `bv-v03 init` from the payer
`4kST35tNxNnvQBUdLJga7EMe4DQbeX3Gjfeauw2cAwMm` (record: `evidence/public-testnet/`):

| Part | Transactions | Blocks | Fee paid, units |
|---|---|---|---|
| `attest_v03` through the program loader: 4 segments and an immutable header, at `8SXSF3i2Ht8acLuzDYYKitVtDfdkKmg4iAkkhFV1GYM3`, image id `6daec75d...67aae0` | 5 | 85,572 to 85,580 | 2,741,408 |
| `lc_verify_v3_v03` through the program loader: 6 segments and an immutable header, at `6DS5234UqhtRk67ZDmVjTFftJQkgdUBWmxsY8vsmbWSU`, image id `a7f5cffb...9ac845d` | 7 | 85,583 to 85,595 | 4,139,648 |
| `Init` of the Sepolia instance `FAVCkd5avEa6Hz3snkpyUVf6NubtEMHTY89mMj7wSUaR` from checkpoint slot 11,640,832 | 1 | 85,597 | 40,599,968 |
| Total | 13 | | 47,481,024 |

- The deployed image ids are those of `guests/IMAGE_IDS.v03.expected`, the reproducible Docker build of the committed
  tree; `verify.sh` reads both headers from the testnet and finds them immutable with these ids.
- Fees are read from the payer's balance before and after each transaction (100,000,000 before, 52,518,976 after),
  at base fees 8 and 8. The deployment pays only for its 860,132 bytes. `Init` was charged for 5,048,787 cycles
  (derived from the balance: (fee - bytes x 8) / 8), equal to its simulated cycles.
- The faucet's status reports the network's LEZ release as `v0.3.1`; Logos's endpoint table says `v0.3.2`. The client
  simulates with LEZ v0.3.2's state machine; all 13 transactions took effect on the testnet as simulated (the two
  headers and the instance state read back as recorded).

Stage 2, sent on 2026-11-23 between 18:42 and 19:00 UTC by `bv-v03 update --max 1` and `bv-v03 refuse-demo`:

| Part | Transaction | Block | Bytes | Cycles (simulated = charged) | Fee paid, units |
|---|---|---|---|---|---|
| `Begin` of the finality update at attested slot 11,641,568 | `ec330ef5...376634` | 85,746 | 29,344 | 2,780,396 | 22,477,920 |
| `Miller` | `9854f067...dc021` | 85,748 | 297 | 5,785,760 | 46,288,456 |
| `Finish`: the update applies; the instance head moves from slot 11,640,832 to 11,641,504 | `c591b135...199f0` | 85,751 | 906 | 4,472,624 | 35,788,240 |
| The same `Finish` sent again: refused with exit code 14, the instance unchanged (it pays its fee) | `b35b0ba2...9f258a` | 85,754 | 906 | 96,197 | 776,824 |
| Total | 4 | | | | 105,331,440 |

- Stage 2 cost exactly what `bv-v03 plan` simulated at block 85,554 (105,331,440 units at base fees 8 and 8), and
  each step was charged its simulated cycles (derived from the payer's balance before and after, as above).
- After the `Finish`, the instance account on the testnet holds the head slot 11,641,504 and its beacon block root
  `17dc7af1...bebba3`; `run.json` records the instance state read back from the testnet, and the checkpoint and head
  records.
- Funds for stage 2: the payer's own faucet window was still closed, so one faucet payment went to a second account
  (`ChGA4puYvTfthQT3ZAMLoBGKzg3YJntxYiTDiPord7a8`, claim transaction `fb8fec29...763d7d`) and 95,000,000 units were
  moved to the payer by a native transfer (`17bbbcb8...97ef7a`, block 85,741; `bv-v03 transfer`). The payer held
  147,518,976 before stage 2 and 42,187,536 after. `run.json` (`mode_choice`) records both stages and this funding.

Checks of the whole record on 2026-11-23 at about 19:10 UTC:
- `verify.sh` (against the testnet): 81 checks passed, 0 failed. The sequencer gives each of the 17 transactions in
  the block `txs.tsv` names, each block contains its bytes, both program headers are immutable with the expected
  image ids, and the instance state read now equals the recorded one. The sequencer showed all 17 blocks as
  `Pending` on the Logos Blockchain at that time.
- `verify_offline.py` (no network): 42 checks passed. `mutation_test.sh`: 4 of 4 changes to a copy of the record
  caught.

## Funds

The payer is funded from the LEZ v0.3 testnet claim service (`logos-co/lez-faucet`), which pays 100,000,000 units per
recipient per 24 h; the first payment arrived on 2026-11-23, and a second payment, to a second account, was moved to
the payer for stage 2 (see "Sent"). Paying account: `4kST35tNxNnvQBUdLJga7EMe4DQbeX3Gjfeauw2cAwMm`.

## Re-check

```sh
cd lez-v03 && scripts/build_guests.sh local && cargo test --profile host --locked -p v03_harness
lez-v03/scripts/build_guests.sh docker        # image ids, compare with guests/IMAGE_IDS.v03.expected (built twice, equal)
```

The record: `lez-v03/scripts/verify.sh` (against the testnet),
`python3 -I lez-v03/scripts/verify_offline.py evidence/public-testnet` (no network) and
`lez-v03/scripts/mutation_test.sh` (four changes to a copy of the record, each must fail `verify.sh`). (A record marked
`"stage": "deploy+init"`, as the stage-1 record was, is checked for the deployment and `Init` only.)
