# Differential test: Helios 0.12.0 against our verifiers

Generated 2026-11-19T23:36:40Z by `cargo nextest run -p diff_helios` (or `cargo run -p diff_helios --bin diff_report`). Every row of `cases.tsv` is one case with the SHA-256 of its input (config, store witness, update bytes).

Pass rule: every column agrees with V1 on every case (accept or refuse, error code, and on accept the pre-store and post-store commitments, head and record). Columns: v1, v2, v3, v3_staged.

## Totals

| | |
|---|---|
| total | 838 |
| distinct_inputs | 838 |
| valid | 90 |
| valid_fabricated_positive_controls | 9 |
| gloas_cases | 315 |
| fork_crossing | "sepolia" |
| fork_crossing_valid | 1 |
| fork_crossing_cases | 17 |
| v1_v2_disagreements | 0 |
| v1_v3_disagreements | 0 |
| executor_cases | null |
| executor_mismatches | null |
| pass | true |

## Cases by network

| network | total | valid | tampered | optimistic | fabricated | gloas |
|---|---|---|---|---|---|---|
| fabricated (Fulu) | 19 | 0 | 0 | 0 | 19 | 0 |
| hoodi (Fulu) | 197 | 21 | 173 | 3 | 0 | 0 |
| mainnet (Fulu) | 287 | 31 | 251 | 5 | 0 | 0 |
| sepolia (Fulu, Gloas) | 335 | 38 | 292 | 5 | 0 | 315 |

## Cases by class

"Expected" is the code the test asserts; "V1 seen" is what V1 returned (they must match); "Helios verify" is unmodified Helios `verify_*` on the same input.

| class | cases | expected | V1 seen | Helios verify |
|---|---|---|---|---|
| valid_handoff_update | 30 | accept x30 | accept x30 | ok x30, ok, store moved x30 |
| valid_handoff_finality | 13 | accept x13 | accept x13 | ok x13, ok, store moved x13 |
| valid_learn_next | 34 | accept x34 | accept x34 | ok x34, ok, store moved x34 |
| valid_same_period_finality | 13 | accept x13 | accept x13 | ok x13, ok, store moved x13 |
| sig_byte_flipped | 43 | E03 x43 | E03 x43 | InvalidSignature x43 |
| sig_infinity | 43 | E03 x43 | E03 x43 | InvalidSignature x43 |
| bit_cleared | 43 | E03 x43 | E03 x43 | InvalidSignature x43 |
| bit_set | 32 | E03 x32 | E03 x32 | InvalidSignature x32 |
| attested_field_changed | 43 | E03 x43 | E03 x43 | InvalidSignature x43 |
| finalized_field_changed | 43 | E02 x43 | E02 x43 | InvalidFinalityProof x43 |
| finality_branch_node_changed | 43 | E02 x43 | E02 x43 | InvalidFinalityProof x43 |
| next_committee_key_byte_changed | 30 | E05 x30 | E05 x30 | InvalidNextSyncCommitteeProof x30 |
| next_committee_branch_node_changed | 30 | E05 x30 | E05 x30 | InvalidNextSyncCommitteeProof x30 |
| signature_slot_not_after_attested | 43 | E01 x43 | E01 x43 | InvalidTimestamp x43 |
| finalized_execution_changed | 43 | E15 x43 | E15 x43 | InvalidExecutionPayloadProof x43 |
| attested_execution_changed | 43 | E15 x43 | E15 x43 | InvalidExecutionPayloadProof x43 |
| finalized_header_zeroed | 43 | E02 x43 | E02 x43 | ok x43 |
| other_network_genesis_root | 43 | E03 x43 | E03 x43 | InvalidSignature x43 |
| replay_after_apply | 43 | E14 x43 | E14 x43 | ok x43 |
| truncated_ssz | 43 | E01 x43 | E01 x43 | decode x43 |
| extra_data_shortened | 26 | E15 x26 | E15 x26 | InvalidExecutionPayloadProof x26 |
| two_periods_ahead | 39 | E05 x39 | E05 x39 | InvalidPeriod x39 |
| optimistic_update | 13 | E02 x13 | E02 x13 | ok x13 |
| fab_against_real_store | 1 | E03 x1 | E03 x1 | InvalidSignature x1 |
| fab_empty_finalized_junk_branch | 1 | E02 x1 | E02 x1 | ok x1 |
| fab_finality_one_signer | 1 | E04 x1 | E04 x1 | ok x1 |
| fab_finality_update | 1 | accept x1 | accept x1 | ok x1, ok, store moved x1 |
| fab_handoff_update | 1 | accept x1 | accept x1 | ok x1, ok, store moved x1 |
| fab_identity_member | 2 | accept x2 | accept x2 | ok x2, ok, store moved x2 |
| fab_no_finality | 1 | E02 x1 | E02 x1 | ok x1 |
| fab_no_finality_one_signer | 1 | E02 x1 | E02 x1 | ok x1 |
| fab_participation | 8 | E04 x4, accept x4 | E04 x4, accept x4 | InsufficientParticipation x1, ok x7, ok, store moved x4 |
| fab_period_update | 1 | accept x1 | accept x1 | ok x1, ok, store moved x1 |
| fab_previous_committee | 1 | E03 x1 | E03 x1 | InvalidSignature x1 |

## Where unmodified Helios `verify_*` accepts and we refuse

These are deliberate. E04: fewer than 342 signers (Helios `verify_*` needs one signer; its `apply_*` then skips the update without an error). E02: no finalized header (an optimistic update, or a full or finality update whose finalized header is all zero: Helios reads a zero header as "no finality" (`default_header_to_none`, as the consensus spec does) and `verify_*` passes it as a non-finality update). E14: the update moves nothing (a replay on the store it already produced: Helios `verify_*` passes and `apply_*` changes nothing).

| reason | cases |
|---|---|
| E02 | 59 |
| E04 | 4 |
| E14 | 43 |

## Disagreements with V1

None.

## Designed differences (listed, not counted as agreement)

- `{"column":"v3","id":"fabricated/identity_member_counted","other":"E03","reason":"a participating committee key equal to the identity: Helios counts it, the staged verifier refuses it (a16z/helios issue #871 class)","v1":"accept:a4c98b8c86f0b37f"}`
- `{"column":"v3_staged","id":"fabricated/identity_member_counted","other":"E03","reason":"a participating committee key equal to the identity: Helios counts it, the staged verifier refuses it (a16z/helios issue #871 class)","v1":"accept:a4c98b8c86f0b37f"}`

## V1 different from the expected code

None.

## Helios refuses, V1 accepts

None.

## Panics

None.
