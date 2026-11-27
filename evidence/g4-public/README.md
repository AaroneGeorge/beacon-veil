# Runs D and E: followers on the public path

Two `attest-followerd` processes keep two `lc_verify_v3`-only instances on `seq-pub` current through the public path
(`[chain] mode = "public"`): every update is three public transactions, Begin, Miller and Finish, on a sequencer with
`RISC0_DEV_MODE` unset. Run D follows Ethereum mainnet (cadence 6,144 s), run E follows Sepolia (cadence 1,536 s).
Beacon data comes through the Logos `beacon_api_module`. Launch and collect: `crates/follower/scripts/run_public.sh`.

| Folder | Run | Network | Hours | Updates applied | Handoffs | G4 check |
|---|---|---|---|---|---|---|
| `mainnet/run1/` | D, run 1, 20-21 Nov | mainnet | 22.19 | 14 | 1 | fails only on "duration < 24 h" |
| `sepolia/run1/` | E, run 1, 20-21 Nov | Sepolia | 22.23 | 42 | 0 | fails only on "duration < 24 h" |
| `mainnet/run2/` | D, run 2, 2026-11-21T13:00Z to 2026-11-25T23:00Z | mainnet | 105.99 | 67 | 4 | PASS |
| `sepolia/run2/` | E, run 2, 2026-11-21T13:00Z to 2026-11-25T23:00Z | Sepolia | 105.99 | 203 | 5 | PASS |

Run 1 of both stopped on 2026-11-21 at 01:03 UTC, when an unattended operating-system update restarted the service
that hosted the processes. Nothing in the follower failed: the last lines before the stop are normal `tick` lines, and
every update was applied or deferred as designed. The host now keeps that service out of automatic restarts, and run
2 runs in its own process scope. Run 1 is kept as recorded; it is not merged with run 2, and the hours are never
summed. Run 2 is final in `run2/` (collected with `run_public.sh <network> collect`; earlier collects on 2026-11-22 at 26 h
and 2026-11-24 at 76.7 h): the 24 h check passes on both networks, with dev mode unset and no gap over 321 s; each run logged 2
transient `source_error` lines (a 20 s beacon-module timeout, recovered at the next poll).
Step cycles: run D has an exact count for all 201 steps (largest 5,765,199). Run E has an exact count for 608 of 609
(largest 5,765,201); for one step the log has only an upper bound, the gas of its block, 6,227,904 (62.3 percent of
the cap; `steps_bounded_by_block`, `max_step_block_bound` in `sepolia/run2/g4-summary.json`).

Run 2 of both ended on 2026-11-25 at 23:00 UTC, when the host rebooted for an unattended security update. Each
process logged a clean `stop` line (`clean shutdown`); the sequencer logged a clean shutdown too. No follower ran from 2026-11-25T23:00Z to 2026-11-27T22:45Z (47.8 h), and `seq-main` and `seq-pub` were down over the same time: they restarted at 20:20Z on 27 Nov and replayed their chains until about 22:43Z. The instances got no
update during the gap. Run 3 of each, a new process on the same instance, started on 2026-11-27 at 22:45:16 UTC: it
read the on-chain head and at once sent the updates it had missed (run D from its head in period 1920, run E from its head in period 1423).
Since then the sequencer, the Logos session and both followers run as system services that start at boot and restart
on failure; every new process starts a new log, so each log still holds one process id. The top-level files are the
latest collect: today they are the same as `run2/`; once run 3 passes 24 h, a collect writes it there and `run3/`
keeps it. Runs are never merged and their hours are never summed.

`dry-run-20261120/` is the earlier dry run with two followers on one instance.
