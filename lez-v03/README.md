# lez-v03: the two programs on LEZ 0.3, for the public testnet

The public LEZ testnet runs LEZ 0.3 (`https://testnet.lez.logos.co`, release `v0.3.2`); testnet 0.4 has no public
endpoint yet. The deliverable programs of this repository target LEZ v0.4.0-rc3, whose program model (actors) differs
from 0.3's (`plan` + `apply`). This folder holds thin 0.3 wrappers around the **same verifier code**, compiled
unchanged, so the public update path can run on a public Logos network:

| Program | Wraps | Instructions |
|---|---|---|
| `lc_verify_v3_v03` | `attest_core::lc_v3::handle` | `Begin`, `Miller`, `Finish` of one update, on a job account (as `lc_verify_v3`) |
| `attest_v03` | `attest_core::rules::init`, `rules::apply`, `rules::write_record` | `Init`, `Apply` (only as `Finish`'s chained call) |

Not ported: `Attest`, `CheckHeader`, `VerifyOnce` (the private path), the consumer and the bridge. `crates/attest_core`
and `crates/bls_blst` are used as they are, by path; nothing outside this folder is changed.

## What differs from rc3, and the claim pattern

On LEZ 0.3 a program is two entry points ([Logos's guide](https://github.com/logos-co/logos-docs/blob/main/docs/lez/programs/write-lez-program-with-plan-and-apply.md)):
`plan` sees the instruction and the account handles but no state, declares effects on shards, chained calls and the
timestamp window; `apply` sees one shard and one effect and **cannot make a chained call**
(`ChainedCallsFromApply`, `lee/state_machine/core/src/program/mod.rs` in v0.3.2). So the step that reads the job
(`apply`) cannot be the step that sets the window or calls `attest_v03.Apply`.

The instruction therefore carries **claims**: the window `[from, to)` the step sets and, for `Finish`, the exact
`Apply` fields. `plan` sets the window and declares the chained call from the claims; `apply` runs `lc_v3::handle`
on the job and refuses unless the window and the `Apply` fields it computed equal the claims (E06, E15). A refusal
reverts the whole transaction, the chained call included, so what reaches the instance is what the verifier
computed. `attest_v03.Apply` gets the caller from LEZ (`PlanInput.caller_account_id`), so rule E12 (caller in
`config.verifiers`) is unchanged; a `writes_record` claim tells `plan` whether to declare the record effect, and
`attest_v03`'s `apply` refuses (E15) unless it equals what `rules::apply` does. This is LEZ's own "pinning" pattern.

Every refusal exits with the `EsaError` number (LEZ charges a non-zero exit by its cycles; a panic would be charged
the whole gas limit), malformed instructions included.

Two `lee_core` versions are in one build on purpose: `attest_core` keeps rc3's (it resolves `workspace = true`
against the root workspace), the wrappers use v0.3.2's; they meet as 32-byte ids (`attest_v03::core_id`,
`v03_id`). Account addresses (`instance_id`, `record_id`, `job_id`) are the core's derivations, unchanged: on LEZ 0.3
any program may write its own shard on any account, so they are addresses, not grants.

## Layout

- `programs/*/src/lib.rs`: `plan` and `apply` as pure functions; `src/bin/*.rs`: the guest entry (decode, refuse by
  exit code).
- `harness/`: LEZ v0.3.2's own state machine (`lee::V03State`) with both programs deployed; `tests/cap.rs` (every
  transaction at most 9,000,000 cycles, committed chains and the 342-signer floor), `tests/refusals.rs`,
  `tests/equal_rc3.rs` (heads equal to the rc3 runs in `evidence/public-path/onchain/`).
- `client/` (`bv-v03`): payer key, `plan` (simulated, sends nothing), `deploy`, `init`, `update`, `refuse-demo`,
  `record`. Every transaction is simulated in `lee::V03State` seeded with the accounts just read from the testnet,
  and sent only if it applies there.
- `scripts/build_guests.sh local|docker`, `scripts/verify.sh`, `scripts/verify_offline.py`,
  `scripts/mutation_test.sh`.

## Re-check

```sh
lez-v03/scripts/verify.sh                                  # against https://testnet.lez.logos.co
python3 -I lez-v03/scripts/verify_offline.py evidence/public-testnet   # no network
lez-v03/scripts/build_guests.sh docker                     # image ids, compare with guests/IMAGE_IDS.v03.expected
cd lez-v03 && scripts/build_guests.sh local && cargo test --profile host --locked -p v03_harness
```

The record of the public run is `evidence/public-testnet/`. On 2026-11-23 it holds stage 1 only (deployment and
`Init`, `run.json` marked `"stage": "deploy+init"`); no update has been sent yet (`docs/PUBLIC-TESTNET.md`, Sent so
far).
