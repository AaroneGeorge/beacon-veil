# The public update path on LEZ testnet 0.3

Status: written 2026-11-22, updated 2026-11-24. **Sent, then updated in rounds.** Both programs are deployed on the public
testnet, one Sepolia instance is initialised there, and **five updates are applied** over 24 hours, the last one across
the sync-committee period boundary 1421 to 1422 (instance head slot 11,640,832 to 11,649,024, store period 1421 to
1422); one transaction meant to be refused was refused with the instance unchanged. 29 transactions, blocks 85,572 to
86,065, 2026-11-23 13:52 UTC to 2026-11-24 13:33 UTC (see "Sent" and "Kept current"). Then one `Attest` proved a
Sepolia storage slot against the head's header record (4 transactions, blocks 86,095 to 86,102, see "Attest").
Everything else on this page is simulation, and says so.

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
- `attest_fact_v03`: `Attest`, through `attest_inclusion::verify_claim` and `rules::check_header`, against the header
  records of the deployed `attest_v03` (see "Attest").

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
| The same `Finish` sent again: refused, the instance unchanged (it pays its fee; exit code 14 in the client's simulation, as the testnet returns no receipt) | `b35b0ba2...9f258a` | 85,754 | 906 | 96,197 | 776,824 |
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

## Kept current (mode `keeper`, from 2026-11-23 19:30 UTC)

After the minimal run the instance was kept following Sepolia until 2026-11-24 13:33 UTC, across the sync-committee
period boundary 1421 to 1422 (slot 11,649,024, 2026-11-24 12:04:48 UTC). A keeper script on our
machine (not part of the repository; every transaction it sends is a `bv-v03` command) ran rounds, planned every ~2.5 h; the testnet stall and the faucet outage below made the gaps between sends about 11.3 h, 0.5 h and 6.3 h, so the head lagged Sepolia by up to about 19 h:

1. A fresh data folder: the instance's history (the bootstrap and every update applied so far, in the order applied,
   listed in `manifest.json` `apply_order`) plus the latest finality update from the Beacon API
   (`tools/fixtures/fetch.py --finality-only`). While the store does not know the next sync committee, the period's
   `updates_by_range` item is added before it (`tools/fixtures/fetch_range.py --range 1421:1`): it carries the
   period-1422 committee, which the store must hold to accept updates signed after the boundary.
2. `bv-v03 plan` simulates the folder; the round needs the largest (fees before a step + that step's reserve) over its
   pending steps, plus 10%. If the payer holds less, one faucet payment goes to a fresh account and is moved to the
   payer by `bv-v03 transfer` (at most 12 payments in all; the last 2 are kept for the crossing round).
3. `bv-v03 update --data <folder> --max <pending>` (each update as `Begin`, `Miller`, `Finish`, every transaction
   simulated first), then `bv-v03 record` and `verify.sh`.

Near the boundary it waits for the first finality update whose finalized header is in period 1422 and applies it: the
store hands over to the period-1422 committee (the crossing round). It stops after that round.

`run.json` holds `mode: "keeper"` and a `keeper` object: per round the updates applied, their transactions and blocks,
the instance head slot and store period before and after, the fees paid, and the faucet payments that funded it (the
paying account, the claim and transfer transactions; the transfers' raw bytes are in `raw/`). `verify.sh` checks, in
this mode, that every round's transactions are in `txs.tsv` and on chain, that the head slot strictly increases from
the minimal run's head through every round to the instance's current head, that every head has its header record on
chain, that the store's period never goes back, and, once it has moved from 1421 to 1422, that the next committee was
learned from an `updates_by_range` item in or before that round; and that each transfer to the payer is in the block
recorded.

Round 1, sent on 2026-11-24 between 06:02 and 06:30 UTC (the testnet produced no block from 20:40 UTC on 2026-11-23
until about 23:20 UTC, and stalled for 10 to 20 minutes at a time until about 05:10 UTC; the keeper waited each time):

| Part | Transactions | Blocks | Fee paid, units |
|---|---|---|---|
| `updates_1421_1[0]` (attested slot 11,640,896): the store learns the period-1422 committee; the head does not move | `22b71c3a...`, `9d591145...`, `ed8627ec...` | 85,863 to 85,867 | 120,093,144 |
| `finality_update_11645216`: the head moves from slot 11,641,504 to 11,645,152 | `634462cd...`, `50dad261...`, `78fde0bd...` | 85,869 to 85,872 | 104,551,264 |
| Total | 6 | | 224,644,408 |

- The fees equal those `bv-v03 plan` simulated for the round (224,644,408 at base fees 8 and 8).
- Funding: three faucet payments to fresh accounts, moved to the payer by native transfers (`e7c311ee...` block
  85,776, `1418a9ae...` block 85,786, `5f050ce7...` block 85,861). The payer held 339,184,568 before the round and
  114,540,160 after.
- `verify.sh` after the round: 119 checks passed, 0 failed; `verify_offline.py`: 64 passed.

Round 2, sent on 2026-11-24 between 06:56 and 07:01 UTC:

| Part | Transactions | Blocks | Fee paid, units |
|---|---|---|---|
| `finality_update_11647328`: the head moves from slot 11,645,152 to 11,647,264 | `39c9f287...`, `72caaded...`, `3b5f8ead...` | 85,887 to 85,891 | 104,551,264 |

Funded by one faucet payment to a fresh account (transfer `bdb2bb6b...`, block 85,885). `verify.sh` after the round:
135 checks passed, 0 failed.

The faucet answered HTTP 502 from about 08:30 UTC on 2026-11-24 until after the crossing round, so no further payment
arrived (a fifth request was never accepted, and is listed in `run.json` with no claim transaction). The regular rounds
were stopped after round 2, a round prepared at 09:10 UTC was dropped before it sent anything, and the payer's
108,988,896 units were kept for the crossing round, which then ran on that balance without the 10% margin (each step's
reserve is still checked by the client before it is sent).

Round 3, the crossing round, sent on 2026-11-24 between 13:16 and 13:33 UTC:

| Part | Transactions | Blocks | Cycles (simulated = charged) | Fee paid, units |
|---|---|---|---|---|
| `finality_update_11649088` (attested slot 11,649,088, signed by the period-1422 committee the store learned in round 1) as `Begin`, `Miller`, `Finish`: the head moves from slot 11,647,264 to 11,649,024, the first slot of period 1422, and the store's period from 1421 to 1422 | `f4d540e4...`, `b94ed18e...`, `c166efd8...` | 86,061 to 86,065 | 2,678,082; 5,786,344; 4,472,622 | 103,734,616 |

- The fees equal those `bv-v03 plan` simulated (103,734,616 at base fees 8 and 8). The payer held 108,988,896 before
  the round and 5,254,280 after.
- `verify.sh` after the round: 150 checks passed, 0 failed (against the testnet).

Over the whole run: 29 transactions in blocks 85,572 to 86,065, five updates applied, one refusal, and 585,742,752
units of fees, every step charged the cycles its simulation gave. Funds: six faucet payments of 100,000,000 (one
directly to the payer, five to fresh accounts moved to the payer by native transfers). One leftover of 3,997,032 units
was also moved from the first keeper account to the payer (`bv-v03 transfer`, outside the keeper's record).

## Attest (2026-11-24)

`attest_v03` was deployed immutable with `Init` and `Apply` only, so `Attest` is a third program, `attest_fact_v03`
(`lez-v03/README.md`). Its `plan` runs `attest_inclusion::verify_claim` on the instruction (the execution header
against the record's block hash, the account and storage proofs, the predicate); its `apply` reads the header record
on `attest_v03`'s shard through LEZ 0.3's inspection of a foreign shard and refuses unless it equals the instruction's
record (`CheckHeader`: E02 if absent, E15 if different), then writes the `Statement` once at
`PDA(attest_fact_v03, sha256("esa/fact/v03" || instance || statement id))`. The `attest_v03` it trusts
(`8SXSF3i2...GYM3`) is fixed in its code.

What was attested: slot `0xc48ce8c9...3a200` (the deposit root, `docs/bridge/INBOUND.md`) of the beacon-veil vault v3
on Sepolia, `0x380be74d9F3dFeBAaFC5F911CD6A45bFbeE3aD27`, equals `0x4d92152e...d8075407`, in execution block
12,202,472 (`0xa80030fe...eaf8fa0`), the execution block whose hash the header record of beacon slot 11,649,024
holds (under Gloas a beacon header attests its parent execution block, D-21); slot 11,649,024 is the instance's head
and has that header record on chain (`8siyKPh6oE76HQreEPz8ToBq1fhYeT6z7UX87gRDdSuj`). The record holds the execution block
hash with zero execution roots, so the claim carries the RLP header and the program checks its keccak against the
record (`attest_inclusion::exec_roots`). Witnesses: `eth_getBlockByHash` and `eth_getProof` from the Sepolia
RPCs that `tools/fixtures/fetch.py` already uses (`tools/fixtures/fetch_claim.py`; saved byte for byte in
`evidence/public-testnet/attest/data/` with a manifest). The statement id, `cb6cc908...5e05311`, is the one the rc3
`Attest` gave for the same vault root on seq-dev (`evidence/bridge/k9-part2-runB-v3/attest-root.json`): same chain,
slot and value, same id.

Sent on 2026-11-24 between 14:45 and 14:53 UTC by `bv-v03 deploy --program attest_fact_v03` and `bv-v03 attest`
(each simulated in LEZ v0.3.2's state machine seeded from the testnet before it was sent), recorded by
`bv-v03 record-attest`:

| Part | Transactions | Blocks | Bytes | Cycles (simulated = charged) | Fee paid, units |
|---|---|---|---|---|---|
| `attest_fact_v03` through the program loader: 2 segments and an immutable header, at `77eS8E7V65M7ZbhVXWdxQsPpoiN4fEWPY7qkGyZEpQeb`, image id `98a3be6b...899b704` | 3 | 86,095 to 86,099 | 175,592 | 0 | 1,404,736 |
| `Attest`: the statement written at the fact account `6RTsnpeDBTrxPLm9qoRDmiv4k2mmRJzoNUzngBafvHZn` | `11492b1f...575105f5` | 86,102 | 5,547 | 400,531 | 3,248,624 |
| Total | 4 | | | | 4,653,360 |

- `bv-v03 attest --plan` (deployment and `Attest` in process, from the chain's accounts, at block 86,093) gave the
  same fees, 4,653,360 at base fees 8 and 8, and a need of 4,749,504 (the `Attest`'s reserve after the deployment's
  fees); the payer held 5,254,280, above the need plus 10%, so nothing waited for the faucet. `Attest` was charged
  400,531 cycles (derived from the payer's balance, as above), its simulated cycles, with a gas limit of 412,549.
- A payment of 4,990,000 units reached the payer while the header was in flight (the account sweep), so that row's
  derived cycles in `txs.tsv` read `n/a`; its fee is its 564 bytes at 8. The payer held 5,254,280 before and 5,590,920
  after.
- The image id is that of the reproducible Docker build (`guests/IMAGE_IDS.v03.expected`), built twice (the second
  time with no cache) with the same ELF; both builds also gave the two earlier deployed ids again. `attest_fact_v03` is deployed with its symbols stripped, as `build_guests.sh` does for it:
  the loader charges by the byte (about 308 KB unstripped, 174 KB stripped).
- In `lee::V03State` (`lez-v03/harness/tests/attest.rs`, on this claim and the testnet's record bytes): `Attest`
  400,531 cycles; refused by exit code, with nothing changed: no record (E02), a record that is not the stored one
  (E15), a false predicate (E08), a changed proof node (E07), another record's account (E09), the same fact again
  (E14).
- `verify.sh` now also checks the `Attest`: the program's deployment and the `Attest` in `txs.tsv` and on chain, the
  record used is the instance's record of `run.json` (`attest`), and, without the programs, the recorded Sepolia
  data proves the fact again (keccak of the header equals the record's block hash; the account and storage proofs
  verify under its state root and give the value, in stdlib Python), the statement decodes to it with its id and
  predicate hash recomputed, the fact account is the program's address for it, and, online, holds the statement.
  Checks after the record (about 15:00 UTC): `verify.sh` 179 passed, 0 failed (33 transactions);
  `verify_offline.py`: 99 passed; `mutation_test.sh`: 4 of 4 changes caught (the changed block number is the
  `Attest`'s).

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
