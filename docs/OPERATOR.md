# Operator guide (RFP-021 S7, S5, U3)

This guide is for anyone who runs the update-submitting component for an instance: what it costs, how often it submits, how to start one without permission, what to expect when others run one, and how to tell that an instance has stalled.

Status: written 2026-11-12 from the follower source and `modules/attest_follower_module/RUNBOOK.md`. Figures from the G4 follower runs (`evidence/g4/`, 34.5 h and 35.8 h on Sepolia, proofs in dev mode) were added on 2026-11-17. Figures that the runs did not record are marked "Not measured". Section 10 (public mode) and the bound in section 7 were added on 2026-11-20.

## 1. What an operator does

An operator keeps one instance current. The follower reads the instance's verified head from LEZ, reads the newest light-client data from a beacon source, decides whether an update is needed, waits a short per-instance interval, checks again, and submits. Nothing in this role is trusted. The program checks every update. A bad update is rejected with a typed error. An operator can only stop (a liveness failure), and any other operator can take over.

No party is obliged to operate. Liveness rests on interested parties choosing to spend the gas (section 9). An operator can submit through one of two paths: Path A, a privately proven transaction (sections 2 to 6), or the public path, three public transactions with no proof (section 10).

## 2. Requirements and cost

What an operator needs:

- A beacon source: the Logos `beacon_api_module` (v0.2.0) on a `logosctl` daemon, or any Beacon API endpoint that serves the light-client routes (stated fallback).
- A LEZ wallet with gas for the sequencer it submits to, and the instance id. The follower needs a funded public payer (`BV_PAYER`) and, on Path A, a funded private submitter (`BV_SUBMITTER`). One private transaction reserves about 5.07e6 in fees.
- A machine that can prove one update. One Path A update takes about 40 s end to end in dev mode (follower dev log), which measures the executor and not a real proof. The follower proves `Verify` (46.1M to 65.5M user cycles per update on real mainnet data, measured on the executor: `evidence/cost/cost.md`) and the privacy circuit's wrapper.
- No standing on Ethereum. No validator, no Ethereum account.

What it costs, per update, measured. Proving one V1 finality update for real took 20,060 s (5.57 hours) and peaked at 9.259 GiB on the build laptop (`evidence/g1/real-proof.json`). Proving the V2 finality update for real took 16,064 s (4.46 hours) and peaked at 9.24 GiB on the same laptop (`evidence/g1/real-proof-v2.json`). Derived from measured cycles and LEZ constants (`evidence/cost/cost.md` section 2): 409,764 gas for receipt verification plus 54,965 cycles for `Apply` plus 24,047 for `WriteRecord`, which is 488,776 execution gas, and 224,063 storage gas.

Sepolia account for the project's own runs. The project deploys the vault and sends test deposits and releases from one dedicated Sepolia key. Its address is `0x523072A2aC5e5A3E51c107bc17995A3fa03dBD78`, funded with 0.3 SepETH. It holds testnet funds only and is used for nothing else. The key is not in the repository, in the logs or in `evidence/`. An operator of the follower needs no Ethereum account. An operator of the bridge's root attester or relayer uses LEZ gas and, for release calls, an Ethereum account of their own.

## 3. Compute resources (P6)

| Resource | Figure | Source |
|---|---|---|
| Follower daemon, idle between ticks | Not measured. The follower log records no CPU or memory figures | `evidence/g4/` |
| Proving one V1 finality update, CPU only | 20,060 s (5.57 hours), peak 9.259 GiB, at most 92,700 KiB of swap, on an Intel i5-10300H with 15 GiB of RAM and 11 GiB of swap. Declared minimum for a user machine: 16 GB of RAM | `evidence/g1/real-proof.json` |
| Disk | Log growth about 81 KB per day (run B: 116,454 bytes in 34.49 h; run A: 121,408 bytes in 35.8 h). State cache size: not measured | file sizes of `evidence/g4/run-b/follower.jsonl` and `evidence/g4/run-a/follower.jsonl` |
| Network | Mainnet (fulu): light-client update 2,109 bytes (finality) to 26,926 bytes (period) as SSZ. Sepolia (gloas): 1,448 bytes (finality) and 26,424 bytes (period) | `fixtures/*/manifest.json`, follower build notes |

An operator who wants hourly headers needs a faster prover than this laptop. An operator who only keeps the instance alive needs one proof per sync-committee period plus the finality updates at the chosen cadence.

## 4. Cadence and deferral (R9, R10)

Cadence. The default is one finality update every 16 epochs (6,144 s, about 1.7 hours), measured from the last applied update of any kind. A period handoff is submitted at once, without waiting for the cadence. The follower polls every 300 s and writes one log line per poll. Updates below 342 participants (of 512) are not submitted, because the program would reject them.

Check before submitting (R9). The follower reads the on-chain head before it decides, waits, and reads it again. If another party applied the update meanwhile, it logs `skip_not_needed` and spends nothing.

Deferral (R10). Before each submission the follower waits `d` seconds, where `d` is derived from `sha256(instance || follower_id || finalized_slot)` and lies in 0 to `jitter_max_s` (default 60). Two followers of one instance, with different ids, wait different times, and the order changes from update to update. Over many updates the cost spreads across the operators instead of always falling on the fastest. It adds at most `jitter_max_s` to latency. It is a configuration value. A single operator can set it to 0.

Effect on latency. The bound is `poll_s` plus `jitter_max_s` plus proving time. In the G4 runs the deferral `d` ranged from 4 s to 60 s (run A: 23 values, median 31 s; run B: 22 values, median 45 s). The time from `submit` to `applied` was 37.7 s to 48.9 s over both runs (median about 42 s). Proofs were in dev mode, so that figure contains no real proving time. All values come from the `defer`, `submit` and `applied` lines of `evidence/g4/run-a/follower.jsonl` and `evidence/g4/run-b/follower.jsonl`.

Contention. With several operators on one instance, the first valid submission wins. A no-op update (E14) is refused by `attest_host` before it is sent, costs nothing, and is logged as `noop`. A lost race that does reach the chain is rejected there (E13 or E14) and costs the fee. The deferral rotates who pays it. Not measured: a one-week run with two operators on one instance. The two G4 runs each had one follower on their own instance, so they do not test contention. The run is planned in the grant (R9, M2 gate).

## 5. Install, configure, run, monitor

Paths below are relative to the repository root. Always run `source scripts/env.sh` first. The full command list, with the Nix build of the module, is in `modules/attest_follower_module/RUNBOOK.md`.

1. Install. Build the daemon: `cargo build --profile host -p follower -p attest_cli`. Build and install the module and its dependency with `logosctl install` (RUNBOOK section 1 and 2). No module catalog exists yet, so there is no catalog install line. One is planned in the grant (S9).
2. Configure. Use one wallet home per follower (helper: `crates/follower/scripts/wallet_home.sh`). Copy `crates/follower/examples/followerd.sepolia.toml`. Set `follower.id` (unique per operator), `follower.chain_id`, `follower.state_dir`, the `[source]` section (`logos_module` or `direct_http` with `endpoints`), and `[chain] instance`. Consensus and execution endpoints are configuration values (U3). Environment variables: `LEE_WALLET_HOME_DIR`, `LEE_WALLET_PASSWORD`, `BV_PAYER`, `BV_SUBMITTER`, `BV_GUESTS`, `BV_IDS`, `BV_STORE_DIR`. With `logos_module` also set `logosctl_config_dir` and `logosctl_tmpdir`. The client must use the same `TMPDIR` the `logosctl` daemon was started with.
3. Create or find an instance. `attest init --checkpoint finalized --out instance.json` creates one. To follow an existing one, take its id from the consumer you serve and verify its checkpoint first: `attest checkpoint verify --instance-file instance.json`.
4. Run. Standalone: `attest-followerd --config followerd.toml`. Under Logos: `logosctl call attest_follower_module configure @settings.json`, then `start`. Stop: the module's `stop` sends SIGTERM, waits 60 s, then sends SIGKILL. A daemon run directly exits with 143 on a second signal.
5. Monitor. The daemon writes one JSON line per step to `follower.jsonl` with the fields `ts, pid, event, slot, period, tx, beacon_source, uptime_s`. Events: `start`, `head`, `resume`, `tick`, `defer`, `skip_not_needed`, `skip_low_participation`, `submit`, `applied`, `noop`, `submit_failed`, `source_error`, `chain_error`, `no_instance`, `store_rebuilt`, `rebuild_error`, `stop`. A `tick` event's `kind` is one of `wait`, `period`, `finality`, `finality_handoff`. An `applied` event carries `extra.handoff`. `logosctl` 0.3.2 `watch` repeats each event once per earlier watch session that was killed, so de-duplicate on (`pid`, `n`). `logosctl watch attest_follower_module --json` streams the module events (`update_submitted`, `handoff_applied`, `follower_error`). `attest status --instance <id>` prints the head and store status.

Restart (R4). After a crash or `kill -9`, restart with the same state directory. The follower logs one `resume` line at the pre-kill head, does not re-initialise, and does not submit anything older than the head. Its verified store is checked against the on-chain `store_commitment`. If the local store is missing or mismatched (E13), the follower rebuilds it with `attest_host::rebuild_store` from a bootstrap at the on-chain head plus that period's update, and logs `store_rebuilt`. Drill: `WALLET_ENV=<env> CHAIN=host INSTANCE=<id> crates/follower/scripts/r4_drill.sh`. The pass result of the host drill is in the follower run directory (`r4-result.json`).

Failure of a source (R5). A beacon provider failure logs `source_error` and the next poll tries again, failing over across the configured endpoints. A LEZ failure logs `chain_error` and the follower recovers its store before the next step. Nothing is repeated that was already applied.

G4 evidence. Each run writes to `evidence/g4/<run>/`. `PRIMARY=1` also writes `evidence/g4/follower.jsonl`. Run A has run since 2026-11-12 11:11:59 UTC and is the backup. Run B, built from the docker-reproducible guest images, is preferred for the gate.

## 6. Detect that an instance has stalled

An instance has stalled when its head slot stops advancing while Ethereum finalises new slots. To check:

- `attest status --instance <id>`: compare the head slot's age with 6,144 s plus one period handoff. A head older than that with a finalised slot ahead of it means nobody is advancing it.
- The log of your own follower: repeated `tick` lines with decision `finality` or `period` and no `applied` line.
- A consumer should also check the record age before acting (`docs/CONSUMER-PATTERN.md` section 8).

A stalled instance loses nothing. Existing records still verify, and any party can resume it by submitting the outstanding updates in order, with no manual repair (R7). If the instance missed a period handoff, submit the period updates one by one: `attest update --instance <id> --period <P>`.

## 7. Replace an instance outside the weak-subjectivity period (F10)

1. Decide the new checkpoint: a recent finalised block root, checked with `attest checkpoint verify` against at least three independent sources.
2. `attest init --checkpoint <root> --out new-instance.json`. The new instance is a new address. The old one is not touched.
3. Publish the new instance id and the checkpoint evidence to the consumers that used the old one. They move as in `docs/CONSUMER-PATTERN.md` section 7.
4. Point your follower at the new instance. Stop following the old one when no consumer needs it.

The recommended rule for when to replace is in `docs/TRUST.md` section 2.2. The 14-day figure there is a working rule and is not derived from the consensus specs yet.

The bound on the public path. `lc_verify_v3` enforces the 14 days. `Begin` and `Finish` carry a timestamp window that ends 1,209,600 s after the time of the store's finalized header, and the sequencer checks it when the transaction settles. A `Begin` or `Finish` sent after the end is included, reverted and charged its whole declared gas, and the instance does not move (measured: `evidence/public-path/onchain/refusals.json`, row "Begin on an instance whose store is older than 14 days", 10,000,000 gas charged at a declared limit of 10,000,000; `docs/DECISIONS.md` D-55). Only the sender of the late step pays. So an instance on the public path that has had no applied update for 14 days cannot be advanced again, and its operators must replace it before that. Our submitter refuses to send a step when the store's finalized header is within 10 minutes of the bound, before every step, and logs E06. If E06 appears, create a new instance as above and do not retry.

The bound is a property of `lc_verify_v3`. `lc_verify` (V1) has no bound. It holds for the instance only when `lc_verify_v3` is the instance's only verifier. Create such an instance with `attest init --only-verifier <lc_verify_v3 program id> --checkpoint <root> --out new-instance.json` (it conflicts with `--extra-verifier`). The verifier list is part of the instance id and cannot change. On an instance that lists both, the bound holds on the public path only.

## 8. Running next to other operators

Run your own follower with its own `follower.id`. Do not copy another operator's id: two followers with one id wait the same time and collide. Expect `skip_not_needed` lines when another operator was first. That is normal and cost you nothing.

## 9. No party is obliged to operate

The design does not oblige anyone to submit updates. Liveness rests on interested parties, for example consumers whose users need fresh headers, choosing to spend the gas. The guarantee is not that someone will act. It is that a stalled instance can be advanced by any party willing to pay, without permission and without any prior relationship to it. The failure mode is delay, not loss. An operator incentive that keeps submission permissionless is a soft requirement that this proposal does not build (`docs/COVERAGE-021.md`).

Our own commitment: during the support window we run one follower on testnet as one operator among any number.

## 10. Public mode: three public transactions, no proof

In public mode the follower sends each update as three public transactions to `lc_verify_v3` (`Begin`, `Miller`, `Finish`), executed by the sequencer. No proof is made, so the operator needs no prover and no private submitter, only a funded public payer. The steps, what each checks and the measured costs are in `docs/PUBLIC-PATH.md`. Anyone may send any step of an update's job, so two operators on one instance share the job and the second one's step is logged as `superseded`.

Configure. Start from `crates/follower/examples/followerd.mainnet.public.toml` or `followerd.sepolia.public.toml`. The public part is in `[chain]`:

```toml
[chain]
kind = "host"
instance = "<instance id>"
ids_file = "deploy/ids-pub.toml"   # program ids of the deployment
mode = "public"                     # staged public path (attest_host::submit_update_staged)
verifier = "lc_verify_v3"           # the verifier's name in [programs] of the ids file
sequencer = "seq-pub"
```

Create the instance with `attest init --only-verifier <lc_verify_v3 program id> --out instance.json` so that the instance's only verifier is `lc_verify_v3` (section 7 says why). Dev mode must be unset: the launch script `crates/follower/scripts/run_a.sh` refuses to start a follower of a sequencer that runs with `RISC0_DEV_MODE` unset when the variable is set in its environment.

Launch. `crates/follower/scripts/run_public.sh mainnet|sepolia session|wallet|start|status|stop|collect` is a thin layer over `run_a.sh` with the public defaults: `session` starts a Logos session of its own, `wallet` makes a wallet home with a funded public payer and writes `wallet.env`, `start` starts the follower detached, `collect` writes the summary to `evidence/g4-public/<network>/`. Never restart a run by hand: the long-run check needs one process id.

Funding. The payer pays the fee of every step. The fee per update is not fixed. Over the 9 recorded updates that have a full fee record (the `Begin` of the first Hoodi update has no `fee_paid`, because its block held a second charged transaction), the three `fee_paid` fields add up to 101,057,784 (Sepolia finality update) to 133,121,828 (Sepolia handoff update `update_1417`); the mainnet finality update is 101,860,776 (`evidence/public-path/onchain/*/run.json`, sums of `fee_paid`; measured on the standalone sequencer). Fee per unit of gas, `fee_paid` divided by `charged_gas` on steps that were alone in their block (derived): 8.0 to 8.1 on 25 steps, and 9.0 to 10.1 on four Sepolia steps (blocks 464, 466, 468 and 470). The script's own estimate is about 1e8 per finality update at 8 per unit (its header comment). `run_public.sh wallet` funds the payer of the mainnet run with 30,000,000,000 and of the Sepolia run with 100,000,000,000 (`PUB_FUND` overrides), and tops an existing payer up under the deployment lock. A Sepolia run at a 1,536 s cadence is 56 updates a day, so budget the payer for the days the run is to last.

The reserve and `payer_error`. The sequencer drops a transaction whose reserve the payer cannot cover (the declared gas at the fee per unit), and the follower cannot see the drop as an error. So the follower reads the payer balance before each submission. Below `min_payer_balance` it logs a `payer_error` line and sends nothing. The default is 216,000,000, the reserve of one update at three steps of 9,000,000 declared gas and 8 per unit (`crates/follower/src/config.rs`). It covers one update at 8 per unit only: at 10 per unit the reserve of three 9,000,000 steps is 270,000,000. Set `min_payer_balance` in `[chain]` to change it. Fund the payer when `payer_error` appears. Without funds the follower sends nothing, and the long-run check fails because no update is applied.

Declared gas. A step declares 9.0M (`Begin`), 7.0M (`Miller`) and 5.5M (`Finish`), so one update has a limit of 21.5M declared gas across three blocks (`docs/DECISIONS.md` D-58). A block admits a transaction by its declared gas next to the gas already charged in it, so a step may wait a block or two. Events specific to this mode: `submit_deferred` and `submit_resume` (a step that did not fit its block is sent again in a later one), `superseded` (another submitter finished the update), `payer_error`.

Never reset `seq-pub`. `scripts/seq.sh pub reset` wipes the chain: the deployed programs, the instances, the job accounts and every transaction that the evidence under `evidence/public-path/onchain/` records. Do not reset it, and do not restart it while a public run follows it.

Check. `scripts/gate/g4_check.py` fails a public run on any applied update that is not three transactions, on any superseded submission, on a step without a cycle figure, on a step above 9,000,000 cycles, on a sequencer other than `seq-pub`, and on a dev-mode flag that is set.

The public read-only endpoint has two parts, the hosted relay and the host dialer: `tools/seq_read/README.md` gives the commands to start and stop them.
