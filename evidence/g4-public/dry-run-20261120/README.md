# Follower on the public path: dry run on seq-pub, 2026-11-20

Short dry run of `attest-followerd` with `[chain] mode = "public"` against `seq-pub` (LEZ v0.4.0-rc3, RISC0_DEV_MODE
unset: public transactions are executed by the sequencer, no proof is involved). Not the 24 h runs.

- Instance `7bYsJLGb32uVQeehJNKRAETssUJQ6xpABB8yWB93VoYQ` (Sepolia, checkpoint slot 11,617,056, period 1418;
  verifiers `lc_verify` and `lc_verify_v3`; `instance.json`), created with `attest init --extra-verifier`.
- Program ids: `deploy/ids-pub.toml` of the deployment of release set d097847 (`lc_verify_v3`
  `8niQPec4Ng9yrm17Aj3VFbtebrXPxd6JVhhdZsAXQ1Tq`).
- Beacon data: `direct_http` (lodestar Sepolia), cadence shortened to 600 s and 120 s for the dry run.

| log | what | result (`g4_check.py --min-hours 0`) |
|---|---|---|
| `run-dry-e/follower.single.jsonl` | one follower, one finality update | PASS: 1 applied, 3 transactions (Begin, Miller, Finish), charged execution gas 2,435,679 / 5,764,613 / 4,379,765 (24% / 58% / 44% of the 10M block cap), each step alone in its block |
| `run-dry-e/follower.jsonl` and `run-dry-e2/follower.jsonl` | two followers on the same instance, started together, no deferral | `run-dry-e`: 2 applied (the second with only its own Finish). `run-dry-e2`: both its submissions `superseded` (it shared the job of each update: for the second update it sent Begin and Miller, `run-dry-e` sent Finish); its first lines rebuild its missing local store from a bootstrap at the on-chain head |

The summaries were regenerated with the public-run rules of `g4_check.py` added after the dry run (three
transactions per applied update, no superseded submission, a cycle figure for every step): the single-follower log
still passes; the two-follower logs now fail on purpose (contention is what they show). In this dry run every step
declared 9,000,000 gas; the submitter now declares 9.0M for Begin, 7.0M for Miller and 5.5M for Finish.

Charged gas is read back from each step's block (the fee program's block summary; rc3 has no per-transaction
receipt). The period update that taught the instance its next committee (before these logs, with an earlier build
that did not yet read the charges) landed the same way, in about 92 s for the three steps.
