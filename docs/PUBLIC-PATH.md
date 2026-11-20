# The public path: three transactions per update

Status: written 2026-11-20. Every figure is copied from the file named next to it. Labels: **measured public execution** is a transaction on a standalone LEZ v0.4.0-rc3 sequencer (`seq-pub`) started with dev mode unset, where public transactions are executed by the sequencer and carry no proof; **executor** is the RISC Zero 3.0.5 executor, no sequencer, no proof; **derived** is a sum or difference of measured figures. Not shown here: a public Logos testnet (none has an endpoint yet), and a day-long run on the public path.

LEZ rc3 caps execution at 10,000,000 gas per transaction and per block, one gas per cycle. A V1 update is 46M to 65M cycles (`docs/DECISIONS.md` D-39), so the public path runs the staged verifier `lc_verify_v3` (D-53): one update is three public transactions on a job account, and anyone may send any step. Deployed image `cc8acbc5226d77734cef3013f28838f2df268469e95ed2bd680a6620ff934510`, user ELF 498,916 bytes against a cap of 1,966,080 (`evidence/public-path/exec-steps.json` `guests.lc_verify_v3`; `evidence/public-path/onchain/deployment.txt` shows it matches the on-chain program header).

## The three steps

| Step | What it checks or does | Refusal codes |
|---|---|---|
| `Begin` | Decodes the update, requires a finalized header and at least `max(min_participants, 342)` signers, applies Helios 0.12.0's `verify_generic_update` rules in Helios's order, recomputes the pre-store commitment from the witness, computes the new store, head and record, and builds the aggregate key (the committee's key minus the absent keys). Writes the job account | E01 to E05, E09, E13, E14, E15 (E06 cannot occur, D-55) |
| `Miller` | Decodes the signature with its subgroup check (the identity is refused), hashes the signing root to G2, runs one Miller loop over both pairs | E03; E14 or E01 on a wrong stage |
| `Finish` | Final exponentiation and the test against one, then a synchronous call to the unchanged `attest` program, which refuses a stale pre-store and writes the store, head and record | E03; E01 or E14 on a wrong stage; E13 or E14 from `attest.Apply` (a panic, charged the declared gas) |

A job is `PDA(lc_verify_v3, sha256("bv/job/v1" || instance || pre_store || sha256(update)))`. Each step needs the previous one, so none can be skipped or replayed. A job built on a store that has since moved cannot land. A step sent after 14 days from the store's finalized header is not accepted (below).

## Cost, per step and per update

Measured public execution, charged gas per step (`evidence/public-path/onchain/<network>/run.json`, `updates[].steps[].charged_gas`; the total is derived). Three kinds of update per network: a period update that teaches the next committee, the first update signed by the next committee (handoff), and a finality update.

| Network | Update | Signers | `Begin` | `Miller` | `Finish` | Total |
|---|---|---|---|---|---|---|
| mainnet | period | 511 | 4,309,071 | 5,764,775 | 4,378,929 | 14,452,775 |
| mainnet | handoff | 512 | 4,295,619 | 5,764,985 | 4,378,927 | 14,439,531 |
| mainnet | finality | 512 | 2,560,204 | 5,764,825 | 4,378,929 | 12,703,958 |
| Sepolia (Gloas) | period | 506 | 4,031,288 | 5,764,827 | 4,345,704 | 14,141,819 |
| Sepolia (Gloas) | handoff | 497 | 4,139,283 | 5,764,823 | 4,378,927 | 14,283,033 |
| Sepolia (Gloas) | finality | 492 | 2,458,609 | 5,764,775 | 4,378,929 | 12,602,313 |
| Hoodi (Fulu) | period | 480 | 4,679,020 (derived) | 5,765,037 | 4,378,929 | 14,822,986 |
| Hoodi (Fulu) | handoff | 478 | 4,700,889 | 5,764,985 | 4,378,927 | 14,844,801 |
| Hoodi (Fulu) | finality | 466 | 3,121,826 | 5,765,037 | 4,378,929 | 13,265,792 |
| floor (fabricated chain) | period | 342 | 6,327,247 | 5,765,037 | 4,378,929 | 16,471,213 |

- The largest step is the `Begin` of an update signed by exactly 342 of 512 members (the most absent keys): 6,327,247 gas, 63.3 percent of the cap (`floor/run.json` `max_charged_gas`, `share_of_cap`). This update is fabricated: a test chain with test keys, the first 342 participation bits set, re-signed (`v3_onchain run --network floor`). The largest on real data is a `Miller`, 5,765,037 (`hoodi/run.json` `max_charged_gas`), 57.7 percent.
- The Hoodi `Begin` of the first update shared its block with one refused transaction of another payer, so its charge is derived, not measured: the block total of 4,716,085 minus the 37,065 cycles of that transaction, re-executed in the executor (`hoodi/run.json`, `block_gas_used_exec`, `other_charged_txs`, `charged_gas_method`).
- 30 transactions: 9 on each of three networks and 3 on the floor case. Every update was three transactions and the instance head equalled V1's after each (`head_equals_v1_every_update`, `txs_per_update`). The fee of each step is in `fee_paid`.
- Executor, per step, on the same ELF (`evidence/public-path/exec-steps.json`, `rows`, `worst_case_three_steps`): `Begin` 2,256,234 to 6,327,326 cycles, `Miller` 5,764,611 to 5,765,197, `Finish` 4,378,927 to 4,378,929 including `Apply` and `WriteRecord` (79,012 cycles, derived). The table above is of the recorded transactions, whose inputs are not the fixtures of `exec-steps.json`. The driver re-executed each recorded message in the executor (`metered_cycles_total` in `run.json`). Charged gas equals that count on the 29 steps that were alone in their block. For the 30th, the Hoodi `Begin` above, the block total equals the sum of the executor counts of its two transactions.
- Declared gas per step is 9.0M (`Begin`), 7.0M (`Miller`) and 5.5M (`Finish`) in the submitter (`crates/attest_host/src/lib.rs`, D-58). The recorded runs declared 9,000,000 for every step (`declared_gas` in each `run.json`).
- The per-step cost split into phases, and the four variants against the `bls12_381` fork: `evidence/public-path/bench/bench.md`.

## Refusals

Seven real transactions on `seq-pub`, each reverted with the head unchanged (`evidence/public-path/onchain/refusals.json`, `charged_gas`, `declared_gas`). The refusal tool declares 10,000,000.

| Case | Charged | Why |
|---|---|---|
| `Begin` again on a done job | 163,275 | exit code 14 |
| `Miller` on a done job | 37,003 | exit code 14 |
| `Finish` before `Miller` | 37,064 | exit code 1 |
| Forged signature, refused at `Finish` | 4,273,741 | exit code 3, after the final exponentiation |
| Sepolia update sent to a mainnet instance | 3,413,535 | exit code 1 |
| Stale job: the store moved | 10,000,000 | chained `attest.Apply` panics with E13 (`ESA:E13` in the sequencer log) |
| `Begin` on a store older than 14 days | 10,000,000 | timestamp window missed |

`lc_verify_v3` refuses by exit code, which LEZ charges by the cycles used. A panic, a window miss or running out of gas is charged the whole declared gas. The code of an exit-code row is `expected_code`, confirmed by re-executing the deployed ELF on the same input (`executor_lc_verify_v3.exit`); the chain shows only the charge. Anyone who sends a step on someone else's job pays for it, and the job's owner's own later step then ends with an exit code.

## The 14-day bound

`Begin` and `Finish` carry a timestamp window that ends 1,209,600 s after the time of the store's finalized header. The sequencer checks it when the transaction settles. A late step is included, reverted and charged its whole declared gas (row 7 above), and the instance does not move. The submitter refuses to send a step within 10 minutes of the bound. V1 has no bound, so the bound holds for an instance whose only verifier is `lc_verify_v3` (`attest init --only-verifier`, D-59). D-55 and `docs/TRUST.md` section 2.2 give the rest.

## Equivalence with V1

`tests/diff_helios` runs unmodified Helios 0.12.0, V1, V2 and V3 (one execution, and three steps) on 881 cases, 862 from mainnet, Sepolia and Hoodi and 19 on a fabricated chain: 90 valid, 0 disagreements between V1 and V3. The deployed ELF ran all 881 in the executor with 0 mismatches. 44 designed differences are listed, all refusals by V3 (`evidence/diff/report.json`: `total`, `valid`, `v1_v3_disagreements`, `executor_cases`, `executor_mismatches`, `executor.designed_differences`).

## What this relies on

The trust model is `docs/TRUST.md`. V3 does not subgroup-check committee keys and relies on the beacon chain's key validation (D-54). Its BLS layer is the RISC Zero fork of `blst`, whose changes have no audit statement (`docs/REUSE.md` section 6). A public transaction carries no proof, so the result is the sequencer's execution, as for every public LEZ transaction (`README.md`, section 6).

## How to re-run

Prerequisites: a sequencer started with dev mode unset (`scripts/seq.sh pub start`), and the release set (`scripts/guests_release.sh`) deployed with `scripts/deploy_local.sh pub --bins <release set> --programs "attest lc_verify lc_verify_v3" --ids deploy/ids-pub.toml`, then `export BV_IDS=deploy/ids-pub.toml`. `seq-pub` is a local chain, so a reviewer cannot re-read the 30 recorded transactions from it. A reviewer reproduces the runs by running them again, which gives new transaction hashes; the committed `run.json` files stay as the record.

Run `source scripts/env.sh` first.

- Executor cost per step: `make exec-v3` writes `evidence/public-path/exec-steps.json`. With `COST_BINS=<release set>` it uses the deployed ELFs.
- Re-read recorded transactions from the sequencer: `scripts/gate/gp_onchain.sh` needs the `seq-pub` that holds them (on the build machine) and prints `PASS: 30 transactions re-read from the sequencer match the records`. It runs `v3_onchain check evidence/public-path/onchain/*/run.json` (build it with `cargo build -p cost --bin v3_onchain`). `v3_onchain check-images <release IMAGE_IDS>` compares the on-chain program headers with the release image ids.
- Run the updates on a fresh sequencer: `v3_onchain run --network mainnet|sepolia|hoodi|floor --v3-elf <elf> --attest-elf <elf> --out run.json`. `BV_ALLOW_REVERT=1 v3_onchain refusals --out refusals.json` sends the refusals. Do not reset a `seq-pub` that holds recorded evidence.
- Differential test: `cargo nextest run -p diff_helios`.
- BLS bench: `make bench-bls` writes `evidence/public-path/bench/`.
- Run a follower on the public path: `docs/OPERATOR.md` section 10.
