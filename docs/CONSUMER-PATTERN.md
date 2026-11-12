# Recommended Consumer Pattern (RFP-021 S4, S3)

This guide is for the author of a LEZ program that acts on a fact about Ethereum state. The reference implementation is `programs/consumer_erc20` with its end-to-end test `tests/e2e_consumer`. The statement layout and claim format are documented in the rustdoc at the top of `crates/attest_inclusion/src/lib.rs`.

Status of this draft: written 2026-11-12 from the W3 notes and the consumer source. TODO-REFRESH on 2026-11-15: re-check each rule against `programs/consumer_erc20` and the final `Attest` message.

## 1. Overview

A consumer calls `Attest` with a claim (an account, a storage slot or a log, with its proof), a predicate, and itself as the delivery actor. `Attest` verifies the claim under a header record that the instance has verified as finalised, evaluates the predicate, and sends the consumer a statement. The consumer then decides whether to act. The primitive proves the fact. The consumer owns everything about the action, including uniqueness.

Rules, in the order a handler should apply them:

1. Check the sender program (section 2).
2. Check the instance and chain binding (section 3).
3. Check the statement itself, not the caller's predicate (section 3).
4. Mark the statement consumed with an instance-scoped nullifier (section 5).
5. Act.

If any step fails, the whole transaction fails and nothing is consumed (R2).

## 2. Check the sender program

The runtime sets `from.program_account_id` on a sync message. A consumer accepts a statement only if that field equals the `attest` program id. Fix the id at build time from `deploy/ids.toml`. Reject anything else with E12.

Why. Anyone can deploy a program that sends a message shaped like a statement. Only the sender field tells you which program sent it. A private sender keeps its program id on LEZ v0.4.0-rc3 (`lee/state_machine/core/src/execution_state.rs:1409-1428`), so the check holds when the attestation was proven on a user's device.

## 3. Chain binding, instance binding, and what to check in the statement

A statement carries `instance`, `chain_id`, `slot`, `exec_number`, `kind` (account, storage or log), `subject`, `value`, `predicate_hash` and `id`.

- The consumer fixes the instance id it trusts and the chain id it expects, as constants or as part of its own config. It rejects a statement whose `instance` or `chain_id` differs, with E09. This is what keeps a testnet deposit from satisfying a mainnet check, and one instance's statement from satisfying a consumer bound to another (F5, F15).
- Check that the instance is the one you verified. `attest checkpoint verify --instance-file <file>` compares its checkpoint with several public beacon sources. Do this before you hard-code an instance id.
- Check the statement's kind with E10 if it is not the kind you handle.
- Decode the fact from the statement's own `value` and check it yourself: the emitter address, the event topic, the recipient, the amount. The reference consumer does this for an ERC-20 `Transfer` and returns E08 on any mismatch. Do not rely on the predicate the caller passed. A caller can pass a looser predicate than you intended. Your handler's checks bound what you accept.

## 4. Do not reach verification through a mutable program id (VS3)

The `attest` program id in your consumer must be a constant, not a value read from an account that anyone can write. A program that holds this module's `ProgramId` in mutable state lets whoever controls that pointer substitute a program that attests to anything. Change happens by deploying a new version of your consumer that names the new id, and moving users to it (section 7).

## 5. Enforce uniqueness with a nullifier keyed on the statement id

Uniqueness is the consumer's job. The statement's `id` is deterministic and collision-resistant (F6). Key a nullifier on it, with two rules.

Rule 1. The nullifier is the account `PDA(consumer, sha256(domain || consumer_instance || stmt.id))`. Choose a domain tag for your program, for example `c20/null/v1` as the reference consumer does. Put your own instance id in the seed. Without it, an attacker's instance, which can be configured to trust a fake `attest` program, can accept a fabricated statement carrying the id of a real deposit and mark the id consumed before your user acts. The honest claim then fails with E11 for good. See `docs/TRUST.md` section 8.

Rule 2. The nullifier account must be empty before you use it. Send it a sync message that fails with E11 unless its pre-state is empty, and then mark it. Do not use `write_once()`. It accepts an identical rewrite, so a replay of the same statement would pass.

What the id means.

- A log statement id is unique per log: `sha256("esa/stmt/v1" || chain_id || exec_hash || tx_index || log_index || keccak(log))`.
- An account or storage statement id is `sha256("esa/stmt/v1" || chain_id || address || slot_key || value)`. It does not depend on the block. The same fact has the same id, so a consumer that keys on it acts once per instance on a given (address, slot, value). That is what you want for a state fact, for example "this contract holds this root". If you need a different uniqueness unit, add it to your own nullifier seed.
- If a bridge-style flow shares one statement among many claims (for example, a vault root that every deposit proves against), key the nullifier on something per claim, such as a per-deposit nullifier, and keep the instance in the seed (`docs/DECISIONS.md` D-33).

## 6. Typed errors and what a consumer does with each

On chain a handler panics with the text `ESA:Exx`. The host dry run (`attest attest --dry-run`) returns the same code, so you can test the failure before you send.

| Code | Meaning | Consumer behaviour |
|---|---|---|
| E01 | Malformed input, bad claim encoding, or a missing RLP header on a gloas record | Fix the input. Do not retry unchanged |
| E02 | Header not finalised, or no record for that slot | Wait for a record. Refuse the action until one exists |
| E03 | Sync-committee signature invalid | Treat the update source as faulty. Do not act |
| E04 | Participation below the required threshold | Wait for a better update |
| E05 | Committee handoff not verified | The instance lacks a verified committee for that period. Wait for the update that teaches it |
| E06 | Stale or wrong checkpoint | Do not use this instance. Move to a replacement (section 7) |
| E07 | Inclusion proof invalid | Rebuild the proof from a trusted execution source. Do not act |
| E08 | Predicate false | The fact is not true. Refuse |
| E09 | Chain or instance mismatch | Wrong network or wrong instance. Refuse. This is a configuration error |
| E10 | Predicate names something the claim does not carry | Fix the predicate |
| E11 | Already consumed | The action already happened. Report it. Never repeat the action |
| E12 | Unknown verifier or wrong sender program | Reject. Check the program ids |
| E13 | Store mismatch (update built on an older state) | Rebuild the update on the current head |
| E14 | Already applied | Someone else submitted it. Nothing to do |
| E15 | Header mismatch (record hash differs, or keccak of the RLP header differs from the hash) | Rebuild the claim against the record the instance holds |

Error text names the failed check and never a user identifier.

## 7. Migrate to a replacement instance (F10, VS3)

A replacement is a new instance with a new checkpoint (`docs/OPERATOR.md` section 7). An instance is never refreshed in place. A consumer migrates like this:

1. Verify the new instance's checkpoint with `attest checkpoint verify` against independent sources, and check that its config (chain id, forks, verifiers, minimum participants) is what you expect.
2. Deploy a new version of your consumer, with the new instance id as a constant. Know the hazard first. A statement id does not include the light-client instance, so the same Ethereum fact has the same id under the old and the new instance. A new consumer instance has its own nullifier space, so a fact already acted on under the old consumer could be acted on again under the new one. Close this at cut-over: freeze the old consumer's intake, or have the new consumer check the old consumer's nullifier accounts before it acts.
3. Keep the old consumer running for as long as it holds positions that depend on it. In-flight claims against records of the old instance stay valid, because a record is valid indefinitely (R3).
4. Announce the new consumer. Users move by calling the new one.

TODO-TEST: a migration test with two instances, one consumer per instance, and the same transfer attested through both (`docs/COVERAGE-021.md` F10).

## 8. When an attestation cannot be produced: refuse

If no attestation can be produced (the instance is stalled, the record is too old, the proof source is down, the checks fail), the consumer refuses the action. It never falls back to an unverified input. A "temporary" fallback is the one input that makes the whole design pointless.

Further rules for consumers that hold value:

- Bound exposure. Cap the value released per attestation and per window, and keep a freeze authority. A compromised sync committee (`docs/TRUST.md` section 2.1) can make a false header verify. Caps limit what it can do.
- Check the record age. Refuse if the record's slot is older than your policy allows.
- Act on large amounts after a delay, and compare the record against an independent beacon source first.
- A gloas record attests the parent execution block, so a fact becomes attestable one record later than its own block's finalisation (`docs/TRUST.md` section 3). Do not treat the delay as a failure.

## 9. Reference consumer

`programs/consumer_erc20` implements the pattern for the first case in RFP-021 U6: an ERC-20 `Transfer` to a specified contract, attesting token, sender, recipient and amount. Its instance is `PDA(consumer, sha256("c20/config/v1" || borsh(config)))`, with the config holding the attest program id, the light-client instance, the chain id, the token, the recipient and a salt. On each statement it checks, in order:

1. the sender program is the configured `attest` program (E12);
2. the instance and chain id match the config (E09);
3. the statement is a log statement (E10);
4. it decodes the log from `Statement.value` and checks emitter, `Transfer` topic and recipient (E08);
5. it sends `MarkConsumed` to the instance-scoped nullifier, which must be empty (E11).

Test: `tests/e2e_consumer` `g3_consumer_accepts_once_and_refuses_replay` (first transaction accepted and consumer state changed, identical second transaction rejected with E11) and `private_placement_attest_in_private_account` (the same flow with `Attest` proven on a private account, dev mode). The native ether case is planned (`docs/COVERAGE-021.md` U6).

## 10. Checklist

- [ ] The `attest` program id is a build-time constant.
- [ ] The instance id and chain id are constants or part of the consumer's own immutable config.
- [ ] The handler checks the sender program, instance, chain, kind and the fact itself.
- [ ] The nullifier seed has a domain tag, the consumer's own instance id, and the statement id.
- [ ] The nullifier account must be empty. No `write_once()`.
- [ ] Every error code is handled as in section 6.
- [ ] No fallback to an unverified input.
- [ ] Caps and a freeze authority bound the value at risk.
