# Operator guide (RFP-021 S7, S5, U3)

This guide is for anyone who runs the update-submitting component for an instance: what it costs, how often it submits, how to start one without permission, what to expect when others run one, and how to tell that an instance has stalled.

Status of this draft: written 2026-11-12 from the follower source and `modules/attest_follower_module/RUNBOOK.md`. Sections marked TODO-MEASURE wait for the 24 h run (G4, started by 2026-11-13 20:00 UTC at the latest) and for the cost harness. TODO-REFRESH on 2026-11-15: replace every TODO-MEASURE with a figure and a raw file.

## 1. What an operator does

An operator keeps one instance current. The follower reads the instance's verified head from LEZ, reads the newest light-client data from a beacon source, decides whether an update is needed, waits a short per-instance interval, checks again, and submits. Nothing in this role is trusted. The program checks every update. A bad update is rejected with a typed error. An operator can only stop (a liveness failure), and any other operator can take over.

No party is obliged to operate. Liveness rests on interested parties choosing to spend the gas (section 9).

## 2. Requirements and cost

What an operator needs:

- A beacon source: the Logos `beacon_api_module` (v0.2.0) on a `logosctl` daemon, or any Beacon API endpoint that serves the light-client routes (stated fallback).
- A LEZ wallet with gas for the sequencer it submits to, and the instance id.
- A machine that can prove one update. The follower proves `Verify` (about 74M to 89M user cycles per update on real mainnet data, measured on the executor: `evidence/v1/v1-exec.json`) and the privacy circuit's wrapper.
- No standing on Ethereum. No validator, no Ethereum account.

What it costs, per update. TODO-MEASURE: on-chain gas for `Apply` and storage, proving seconds and peak memory (`evidence/cost/cost.md`, `evidence/calibration.json`). From the plan, as estimates and not measurements: 409,764 flat gas for receipt verification plus `Apply` cycles plus 224,063 storage gas.

Sepolia account for the project's own runs. The project deploys the vault and sends test deposits and releases from one dedicated Sepolia key. Its address is `0x523072A2aC5e5A3E51c107bc17995A3fa03dBD78`, funded with 0.3 SepETH. It holds testnet funds only and is used for nothing else. The key is not in the repository, in the logs or in `evidence/`. An operator of the follower needs no Ethereum account. An operator of the bridge's root attester or relayer uses LEZ gas and, for release calls, an Ethereum account of their own.

## 3. Compute resources (P6)

| Resource | Figure | Source |
|---|---|---|
| Follower daemon, idle between ticks | TODO-MEASURE (CPU, RSS) | 24 h run log |
| Proving one update, CPU only | TODO-MEASURE (seconds, peak RSS) on the build laptop: Intel i5-10300H, 4 cores, 15 GiB | `evidence/calibration.json` |
| Disk | TODO-MEASURE (log growth per day, state cache size) | 24 h run |
| Network | Light-client update 2,109 bytes (finality) to 26,926 bytes (period) on the wire as SSZ, measured on the fixtures | `fixtures/mainnet/manifest.json` |

An operator who wants hourly headers needs a faster prover than this laptop. An operator who only keeps the instance alive needs one proof per sync-committee period plus the finality updates at the chosen cadence.

## 4. Cadence and deferral (R9, R10)

Cadence. The default is one finality update every 16 epochs (6,144 s, about 1.7 hours). A period handoff is submitted at once, without waiting for the cadence. The follower polls every 300 s and writes one log line per poll. Updates below 342 participants (of 512) are not submitted, because the program would reject them.

Check before submitting (R9). The follower reads the on-chain head before it decides, waits, and reads it again. If another party applied the update meanwhile, it logs `skip_not_needed` and spends nothing.

Deferral (R10). Before each submission the follower waits `d` seconds, where `d` is derived from `sha256(instance || follower_id || finalized_slot)` and lies in 0 to `jitter_max_s` (default 60). Two followers of one instance, with different ids, wait different times, and the order changes from update to update. Over many updates the cost spreads across the operators instead of always falling on the fastest. It adds at most `jitter_max_s` to latency. It is a configuration value. A single operator can set it to 0.

Effect on latency. TODO-MEASURE: submission delay distribution from the 24 h run. The bound is `poll_s` plus `jitter_max_s` plus proving time.

Contention. With several operators on one instance, the first valid submission wins and the others get E14 (already applied) or `skip_not_needed`. They lose proving time and, if they submitted, a transaction fee. The deferral makes that loss rotate. TODO-MEASURE: a one-week run with two operators, no double application (R9, M2 gate).

## 5. Install, configure, run, monitor

Paths below are relative to the repository root. Always run `source scripts/env.sh` first. The full command list, with the Nix build of the module, is in `modules/attest_follower_module/RUNBOOK.md`.

1. Install. Build the daemon: `cargo build --profile host -p follower -p attest_cli`. Build and install the module and its dependency with `logosctl install` (RUNBOOK section 1 and 2). TODO-REFRESH: replace with the catalog install line once the catalog exists (S9).
2. Configure. Copy `crates/follower/examples/followerd.sepolia.toml`. Set `follower.id` (unique per operator), `follower.chain_id`, `follower.state_dir`, the `[source]` section (`logos_module` or `direct_http` with `endpoints`), and `[chain] instance`. Consensus and execution endpoints are configuration values (U3). Wallet access comes from `LEE_WALLET_HOME_DIR`, `LEE_WALLET_PASSWORD` and `BV_STORE_DIR` in the environment.
3. Create or find an instance. `attest init --checkpoint finalized --out instance.json` creates one. To follow an existing one, take its id from the consumer you serve and verify its checkpoint first: `attest checkpoint verify --instance-file instance.json`.
4. Run. Standalone: `attest-followerd --config followerd.toml`. Under Logos: `logosctl call attest_follower_module configure @settings.json`, then `start`. Stop with SIGTERM (clean stop, up to 60 s for the step in progress) or `stop`.
5. Monitor. The daemon writes one JSON line per step to `follower.jsonl` with the fields `ts, pid, event, slot, period, tx, beacon_source, uptime_s`. Events to watch: `tick` (every poll), `defer`, `submit`, `applied`, `skip_not_needed`, `skip_low_participation`, `source_error`, `chain_error`, `no_instance`. `logosctl watch attest_follower_module --json` streams the module events (`update_submitted`, `handoff_applied`, `follower_error`). `attest status --instance <id>` prints the head and store status.

Restart (R4). After a crash or `kill -9`, restart with the same state directory. The follower logs one `resume` line at the pre-kill head, does not re-initialise, and does not submit anything older than the head. Its local store is checked against the on-chain `store_commitment`. Drill: `crates/follower/scripts/r4_drill.sh`.

Failure of a source (R5). A beacon provider failure logs `source_error` and the next poll tries again, failing over across the configured endpoints. A LEZ failure logs `chain_error` and the follower recovers its store before the next step. Nothing is repeated that was already applied.

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

The recommended rule for when to replace is in `docs/TRUST.md` section 2.2. TODO-VERIFY the 14-day figure.

## 8. Running next to other operators

Run your own follower with its own `follower.id`. Do not copy another operator's id: two followers with one id wait the same time and collide. Expect `skip_not_needed` lines when another operator was first. That is normal and cost you nothing.

## 9. No party is obliged to operate

The design does not oblige anyone to submit updates. Liveness rests on interested parties, for example consumers whose users need fresh headers, choosing to spend the gas. The guarantee is not that someone will act. It is that a stalled instance can be advanced by any party willing to pay, without permission and without any prior relationship to it. The failure mode is delay, not loss. An operator incentive that keeps submission permissionless is a soft requirement that this proposal does not build (`docs/COVERAGE-021.md`).

Our own commitment: during the support window we run one follower on testnet as one operator among any number.
