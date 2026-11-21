# G4-mainnet: run C, the follower on Ethereum mainnet's live beacon chain

Run C is one `attest-followerd` process that follows the **live Ethereum mainnet beacon chain** (chain_id 1). It
reads light-client updates through the Logos `beacon_api_module` and keeps a beacon-veil instance current. Runs A and
B (`evidence/g4/`) did the same against Sepolia. Run C was started on 2026-11-17 to run unattended and is meant to
keep going. The files in this folder are snapshots: the live log is
`$B/run/w4/run-c/follower.jsonl` (`$B` = the build folder).

## What it is, and what it is not

- **Real mainnet data.** Every update is a real mainnet `LightClientUpdate` or `LightClientFinalityUpdate` (fork
  `fulu`). The instance was initialised from a real mainnet bootstrap at a finalized checkpoint taken at start time.
- **Fully checked guests.** Each update runs through the `lc_verify` and `attest` guests in the RISC Zero executor.
  That includes the sync-committee BLS signature check and the 2/3 quorum pre-check (`min_participants` 342 of 512).
  These are the release guests, set `bf95e359…`; their image ids match `guests/IMAGE_IDS.expected`.
- **Dev-mode receipts, not real proofs.** The sequencer runs with `RISC0_DEV_MODE=1`, so receipts are dev-mode
  receipts. Real proving of the same guests is evidenced separately (G1 real proof, `evidence/g1/`).
- **A local standalone sequencer, not a public network.** The sequencer is LEZ v0.4.0-rc3 `sequencer_service`, built
  with `--features standalone` (mock Bedrock, 15 s blocks). It is a single local node, `seq-main`, with RPC
  `127.0.0.1:3042`. It is a fresh chain started for this run. The older `seq-dev` chain used by runs A and B is kept
  untouched; it takes about 2.5 h to replay on restart.
- **No manual help after start.** The follower picks every update, submits it (Path A, private submitter) and logs
  the outcome. Nobody submits updates by hand to this instance.

## Run record

| Item | Value |
|---|---|
| Start (the `start` line) | 2026-11-17T19:51:27.944Z |
| PID | 1308007 (`$B/run/w4/run-c/followerd.pid`) |
| chain_id | 1 (Ethereum mainnet) |
| Beacon source | `logos_module`: Logos `beacon_api_module` v0.2.0 via logosctl 0.3.2, session `$B/run/w4/logos-session`. Its chain-1 providers, in order: `https://lodestar-mainnet.chainsafe.io`, `http://testing.mainnet.beacon-api.nimbus.team`, `http://unstable.mainnet.beacon-api.nimbus.team` |
| direct_http fallback endpoints (configured, unused) | `https://lodestar-mainnet.chainsafe.io`, `http://unstable.mainnet.beacon-api.nimbus.team` |
| Instance | `HkTRK41GMDEpuNSP4wEbDhYEqw8pxmZhoJu3GXzZ4Exn` |
| Init tx | `c6eba34fcf1fb62042b6bc9815b4df9d4f454b2eaa6cfbedd3bfec6283655c7b` |
| Checkpoint | slot 15676672, root `0xb6deacff700f5b697c971461cc43e0c9ea9f85d21587d23af058470d71ff1368`, period 1913 (bootstrap from lodestar via the module, fork fulu) |
| First update | period update (next sync committee), tx `5dea5f1e45859813d218aa015230edb163f544ec857134570b52d52ca4a1d162`, applied 2026-11-17T19:52:21Z, participation 511/512 |
| Programs on seq-main | attest `3fKaD7YYgsmtTikrPn3QHshBMGmdzNZNojaCk9rKS8Eq`, lc_verify `2N8thEJAgBRrGqfdJHgzjzy1dsnn5mTjGmZziae4q7Du` (`ids.toml`; bins sha256 `4f59c574…` and `31a3ff1d…`, the same files as run B) |
| Follower binary | `$B/run/w4/bin/attest-followerd-a6338c45b330`, sha256 `a6338c45…`. This is run B's binary: `crates/` are unchanged since it was built |
| Cadence | finality update every 6144 s (16 epochs); a tick line every 300 s; sync-committee handoffs are submitted as soon as they are available |
| Next sync-committee boundary | period 1914 starts at slot 15679488 = **2026-11-18 04:57:59 UTC**. The live handoff follows once a finalized header lies in period 1914, which takes about 2 more epochs. Period 1915 starts 2026-11-19 08:16:23 UTC |

## Files

- `run.json`: launcher record (pid, binary sha256, config sha256, instance, source, repo commit, machine).
- `followerd.toml`: the exact config the process runs.
- `instance.json` and `init.out`: the instance config (borsh, fork schedule, genesis root, verifiers) and the init tx.
- `ids.toml`: the program ids on seq-main.
- `preflight-status.json`, `preflight-module.txt`: the on-chain store check, and one mainnet probe through the
  module, both taken just before start.
- `follower.jsonl`: snapshot of the JSONL log (the same format as runs A and B).
- `g4-summary.json`: summary at snapshot time, from `scripts/gate/g4_status.sh`. It holds the G4 checks with no 24 h
  minimum, plus a `status` block: pid alive, last line age, last applied update.

## How to check

```bash
B=<the build folder>; R=$B/repo; cd $R
# Summary of the live run (hours, submits, applied, handoffs, max gap, error lines, head first/last, pid alive):
scripts/gate/g4_status.sh $B/run/w4/run-c/follower.jsonl
# The same through the run launcher (pid, last line, G4 check so far):
SEQ=main RUN_DIR=$B/run/w4/run-c crates/follower/scripts/run_a.sh status
# On-chain head of the instance, read from seq-main:
( source scripts/env.sh; source <(grep '^export' $B/run/w4/run-c/wallet.env)
  $B/target/host/attest status --instance HkTRK41GMDEpuNSP4wEbDhYEqw8pxmZhoJu3GXzZ4Exn )
# Refresh the snapshot in this folder:
cp $B/run/w4/run-c/follower.jsonl evidence/g4-mainnet/follower.jsonl
scripts/gate/g4_status.sh $B/run/w4/run-c/follower.jsonl --out evidence/g4-mainnet/g4-summary.json
```

Compare `head_last` in the summary with the current mainnet finalized slot, for example
`curl -s https://lodestar-mainnet.chainsafe.io/eth/v1/beacon/light_client/finality_update | jq .data.finalized_header.beacon.slot`.
It should trail by no more than one cadence, 6144 s or 512 slots, plus finality lag.

## Dependencies while it runs

The run needs three things to stay up: `seq-main` (`scripts/seq.sh main status`), the logosctl session daemon with
`beacon_api_module` loaded (`crates/follower/scripts/logos_session.sh status`), and the machine itself. There is no
auto-restart, by design: a restart writes a `resume` line, and the summary then reports it.
