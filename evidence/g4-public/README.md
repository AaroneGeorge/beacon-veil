# Runs D and E: followers on the public path

Two `attest-followerd` processes keep two `lc_verify_v3`-only instances on `seq-pub` current through the public path
(`[chain] mode = "public"`): every update is three public transactions, Begin, Miller and Finish, on a sequencer with
`RISC0_DEV_MODE` unset. Run D follows Ethereum mainnet (cadence 6,144 s), run E follows Sepolia (cadence 1,536 s).
Beacon data comes through the Logos `beacon_api_module`. Launch and collect: `crates/follower/scripts/run_public.sh`.

| Folder | Run | Network | Hours | Updates applied | Handoffs | G4 check |
|---|---|---|---|---|---|---|
| `mainnet/run1/` | D, run 1, 20-21 Nov | mainnet | 22.19 | 14 | 1 | fails only on "duration < 24 h" |
| `sepolia/run1/` | E, run 1, 20-21 Nov | Sepolia | 22.23 | 42 | 0 | fails only on "duration < 24 h" |
| `mainnet/` | D, run 2, from 2026-11-21T13:00Z | mainnet | 76.70 | 49 | 3 | PASS |
| `sepolia/` | E, run 2, from 2026-11-21T13:00Z | Sepolia | 76.72 | 147 | 4 | PASS |

Run 1 of both stopped on 2026-11-21 at 01:03 UTC, when an unattended operating-system update restarted the service
that hosted the processes. Nothing in the follower failed: the last lines before the stop are normal `tick` lines, and
every update was applied or deferred as designed. The host now keeps that service out of automatic restarts, and run
2 runs in its own process scope. Run 1 is kept as recorded; it is not merged with run 2, and the hours are never
summed. The top-level files are run 2, collected on 2026-11-24 at 76.7 h with `run_public.sh <network> collect` (first collect
2026-11-22 at 26 h): the 24 h check passes on both networks, with dev mode unset and no gap over 321 s; each run logged 2
transient `source_error` lines (a 20 s beacon-module timeout, recovered at the next poll). Both runs keep going; later collects refresh
these files.

`dry-run-20261120/` is the earlier dry run with two followers on one instance.
