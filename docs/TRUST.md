# Trust assumptions (RFP-021 S6)

This document states what the attestation primitive trusts, what it proves, what each off-chain participant can see and do, and when the guarantees degrade. Requirement ids refer to RFP-021 (F1, U3, VS4). The bridge's own trust model is in `docs/bridge/TRUST.md`.

Status: written 2026-11-12 and not re-read in full since. Section 7 was re-checked against the final code and evidence on 2026-11-18 (its Status paragraph says what is and is not verified). Section 8 has not been re-checked against the final code. For section 4, both G4 follower runs used the Logos `beacon_api_module` path (`beacon_source` is `logos_module` in `evidence/g4/run-a/g4-summary.json` and `evidence/g4/run-b/g4-summary.json`). The fallback to direct HTTP in that table was not exercised in those runs.

## 1. What an attestation proves, and what it does not

An attestation proves three things, in this order (RFP-021 Design Rationale):

1. Finality. The execution block hash it reasons about belongs to a header that the instance's tracked sync committees signed as finalised, along a chain of committee handoffs that starts at the instance's checkpoint.
2. Inclusion. An account, a storage slot or a receipt log is included under that block's state root or receipts root, by Merkle-Patricia proof. The roots come from the verified header record, never from the caller (F3).
3. Predicate. A predicate over the included value holds (F4).

It does not prove:

- That the checkpoint was correct. The checkpoint is the one trust input (section 2).
- That the sync committee was honest. It proves that at least 342 of 512 members signed (section 2).
- That an action happened once. Uniqueness is the consumer's job. The primitive supplies the statement id (F6) and `docs/CONSUMER-PATTERN.md` shows the nullifier.
- Anything about Ethereum activity being private. Ethereum state is public. The attestation path adds no signal beyond it (section 6).
- Anything about LEZ state on Ethereum. That direction is RFP-022 and is not trustless on LEZ v0.4.0-rc3 (`docs/bridge/TRUST.md`).

## 2. Trust assumptions

### 2.1 Honest sync-committee supermajority

Following the chain through committee signatures trusts that fewer than two thirds of the current 512-member sync committee collude. The program rejects an update with fewer than `max(min_participants, 342)` signers. 342 is the smallest integer at or above two thirds of 512.

Quantifying the attack. The committee is sampled from the active validators with probability proportional to effective balance, with replacement (consensus-specs Altair `get_next_sync_committee_indices`). If an attacker controls a fraction `f` of the effective stake, the chance that at least 342 of 512 seats are theirs in one committee period is the binomial tail `P(X >= 342)`, `X ~ Bin(512, f)`. Computed here, not measured, assuming independent sampling:

| Attacker's share of effective stake `f` | Chance of at least 342 seats in one period | Expected such periods per year (321 periods) |
|---|---|---|
| 0.40 | 1.9e-34 | 6.0e-32 |
| 0.50 | 1.2e-14 | 3.8e-12 |
| 0.55 | 3.6e-08 | 1.1e-05 |
| 0.60 | 8.9e-04 | 0.29 |
| 0.65 | 0.21 | 68 |
| 2/3 | 0.50 | 159 |

So buying seats by stake needs roughly 60 percent of all effective stake or more. Not done yet: converting this share to ETH and USD needs the total effective balance read from the beacon API on a stated day. No such reading is recorded in `evidence/`, so no ETH or USD figure is given here.

The cheaper route is not stake. The committee for a period is public one period (256 epochs, 98,304 s, about 27.3 hours) in advance. An attacker who can corrupt the signing keys or operators of 342 specific seats needs no stake share. The consensus spec defines rewards and penalties for sync aggregate participation (`process_sync_aggregate`, Altair `beacon-chain.md:635`) but no slashing condition for signing a header that is off the canonical chain, so the attack costs the attacker no stake. This is weaker than Ethereum's economic finality, which needs at least a third of all stake to be slashable. The RFP states the same trade-off. We do not offer a stronger anchor in this proposal.

What a consumer can do (VS4): cap the value it releases against any one attestation or per window, hold a freeze authority, wait for more than one record before acting on large amounts, and compare a record with an independent beacon source before a large action. `docs/CONSUMER-PATTERN.md` section 8.

### 2.2 The weak-subjectivity checkpoint

Tracking starts from a checkpoint: a finalised block root with its sync committees. Whoever calls `Init` supplies it. The program checks that the bootstrap's committee branch matches the checkpoint root, and no more can be checked on chain. The checkpoint is written into the instance config, and the instance address is the PDA of the config hash, so it cannot change (F9, F13).

How a verifier confirms it independently. `attest checkpoint verify --instance-file <file>` asks N public beacon sources for the block root at the checkpoint slot and for a finalised slot at or above it. Each source must report the same root. This is our fallback for F9. RFP-021 asks for the check as an API of the Logos Ethereum core module. Whether the module will carry it is an open item with Logos (upstream pull request planned, `docs/DECISIONS.md` D-27). A user who does not trust a checkpoint can initialise their own instance with a corrected one. A failed bootstrap is repaired by initialising again (F10).

When an instance must be replaced (F10). An instance that falls behind can still catch up by submitting every period update in order (R7). Its risk grows with age: after the weak-subjectivity period, validators that left the set can sign a long-range fork without penalty. Recommended rule: replace any instance whose head is older than 14 days. Not derived yet: the 14-day figure is a working rule. It has not been computed from `compute_weak_subjectivity_period` in the consensus-specs `weak-subjectivity.md` for the current validator-set size. The replacement is a new instance with a recent, independently verified checkpoint. `docs/OPERATOR.md` section 7 and `docs/CONSUMER-PATTERN.md` section 7 give the steps.

## 3. Ethereum finality under gloas (ePBS)

Question. Sepolia runs gloas, which separates the block proposer's bid from the execution payload (EIP-7732, ePBS). A gloas light-client header carries only an execution block hash. Could the hash a sync committee signs name a payload that was never delivered, so that it is not on the canonical chain?

Answer. No, under the current spec. Sources: `ethereum/consensus-specs` commit `c3e16117a66523902fd21d6ba708d987b56bd4a4` (master on 2026-11-11), read on 2026-11-12 and re-read independently the same day. Line numbers refer to that commit.

1. The hash in the gloas light-client header is the bid's `parent_block_hash`, not its `block_hash`. `EXECUTION_BLOCK_HASH_GINDEX_GLOAS = get_generalized_index(BeaconBlockBody, "signed_execution_payload_bid", "message", "parent_block_hash")`, which is 2,856 (`specs/gloas/light-client/sync-protocol.md:113`). Helios 0.12.0 uses index 808 at depth 11 in `ethereum/consensus-core/src/proof.rs`. 2^11 + 808 = 2,856, the same leaf.
2. `process_execution_payload_bid` asserts `bid.parent_block_hash == state.latest_block_hash` (`specs/gloas/beacon-chain.md:2193`).
3. `state.latest_block_hash` moves to the parent bid's `block_hash` only when the child block treats the parent as full: `process_parent_execution_payload` (`beacon-chain.md:1841`), with the assignment at `beacon-chain.md:1830`.
4. Fork choice `on_block` asserts: "If this block builds on the parent's full payload, that payload must have been verified by `on_execution_payload_envelope`" (`specs/gloas/fork-choice.md:1058-1061`).

Result. The hash in a finalised gloas header names an execution block whose payload was delivered and verified by the honest majority. A builder that withholds a payload cannot get its `block_hash` into a finalised header. The trust assumption is the same as before gloas: an honest sync-committee supermajority.

Consequences.

- A gloas record attests the parent execution block of the finalised beacon block. It lags by one or more slots (more after empty slots). A deposit or transfer becomes attestable once a later block that builds on it is finalised.
- A gloas record holds only `exec_hash`. Its `exec_number`, `exec_state_root` and `exec_receipts_root` are zero. A claim against it carries the RLP execution header. `attest_inclusion` checks `keccak(rlp) == record.exec_hash`, then reads `stateRoot` (field 3), `receiptsRoot` (field 5) and `number` (field 8). Later header fields are bound by the hash and not parsed, so a new field in a later fork needs no change. Errors: E01 if the header is missing when the roots are zero, E15 if the hash differs (`docs/DECISIONS.md` D-20).
- The risk is cleared for the spec at this commit. If the gloas rules change before release, this section must be re-checked. The four cited lines were read on 2026-11-12 and have not been re-read since.

## 4. Off-chain participants (F12, VS5)

No off-chain participant is trusted with safety. Every check below happens in a program on LEZ. The table lists who can do what.

| Participant | Role | Sees | Can it block an attestation? | Can it forge or alter one? |
|---|---|---|---|---|
| Update submitter (follower, anyone) | Fetches update material, proves it, submits it | Public beacon data. Its own LEZ account | It can stop. Any other party can submit the same update (F11) | No. An invalid update is rejected with a typed error. A valid one is the same for everyone |
| Beacon API provider | Serves light-client updates | The requester's IP address and the routes requested | It can refuse or lag. The follower fails over to other providers (`DirectHttpSource` endpoints, or `beacon_api_module` provider health checks) | It can serve a bad update. The program rejects it |
| Execution RPC provider | Serves `eth_getProof` and receipts for a claimant | Which account, slot or transaction the claimant asks about | It can refuse. Another provider serves the same proof | It can serve a bad proof. Inclusion verification rejects it |
| The claimant's own prover | Proves `Attest` on the user's device | Everything about the claim | n/a | n/a |
| LEZ sequencer | Orders and executes transactions | Public transactions in full. For a private transaction, only what the privacy circuit publishes | It can censor, as it can any transaction. This is a platform assumption | It cannot make a program accept an invalid proof |
| Logos modules (`beacon_api_module`) | Provides beacon data to the follower | The follower's requests | The follower falls back to direct HTTP (stated bypass) | No |

Safety rests on: the sync-committee supermajority (section 2.1), the checkpoint (section 2.2), the correctness of our programs and of Helios, and LEZ's own execution and proof checking. Liveness rests on at least one party willing to pay for updates and at least one reachable beacon provider. No party is obliged to submit updates (`docs/OPERATOR.md` section 9). A stalled instance can be advanced by anyone, so the failure mode is delay, not loss (R7). An attestation over an already recorded header stays valid whatever happens later (R3).

## 5. Failure modes and exposure (VS4)

| Failure | Effect | What a consumer can do |
|---|---|---|
| Colluding or compromised committee (342 or more seats) | A false header can be recorded as finalised. Attestations against it verify | Caps and a freeze authority on the consumer side. A delay between attestation and release. An out-of-band check of large amounts against independent beacon sources |
| Wrong checkpoint at `Init` | The instance follows a chain that is not Ethereum's | Verify the checkpoint with `attest checkpoint verify` before relying on an instance. Bind the consumer to an instance id you checked |
| Instance outside the weak-subjectivity period | Old committee keys may sign a long-range fork | Move to a replacement instance (`docs/CONSUMER-PATTERN.md` section 7) |
| Stalled instance | No new records. Existing records still verify | Wait or submit the update yourself. Refuse actions that need a newer record |
| Bug in `lc_verify`, `attest` or `attest_inclusion` | Depends on the bug. This is why the audit is a milestone | Caps and a freeze authority. Verifier ids are fixed per instance, so migration is a new instance (section 9) |
| Bug in Helios | Same as above, in code we reuse unmodified | Same |

## 6. Privacy of the attestation path (F8)

Verifying and consuming an attestation must not add a correlation signal beyond what Ethereum already shows. The design:

- Header records are public, write-once accounts shared by every consumer. Reading one is not a signal about the reader.
- `Attest` can run on the user's private account. The user's device proves it. What leaves the device is which shared header record was read, and the statement delivered to the consumer. The consumer chooses what the statement exposes.
- Records are never written by a consumer's transaction, so private transactions in one block do not contend (R11).
- The statement's public inputs are limited to what the consumer's own handler exposes. It does not carry the consuming account's identity.

Residual signal, stated plainly. The header record a private consumer reads is visible. Using the latest record, as most users will, makes it uninformative. An execution RPC provider that serves the proof sees which account or slot the claimant asked about. Network-level correlation is out of scope (RFP-022). Not tested yet: a state-diff test over a consumer round trip. It is planned in the grant (`docs/COVERAGE-021.md` F8).

## 7. Pre-checks before Helios: the quorum and finality finding

Finding (2026-11-12), confirmed against `a16z/helios` tag 0.12.0 (commit `2af9b418`), `ethereum/consensus-core/src/`:

- `consensus_core.rs:248-256`, `verify_generic_update`, rejects only an update with no participants (`InsufficientParticipation` on zero bits). The two-thirds quorum is checked in `apply_generic_update` (`consensus_core.rs:176`, `has_majority`, used at `:184`). There, an update below quorum is silently not applied. It is not an error.
- `types/mod.rs:468`, `:482` and `:520-525` (`default_header_to_none`): an update whose finalized header has slot 0 or is a default header becomes `finalized_header: None`. `verify_generic_update` then skips the finality-branch check (`consensus_core.rs:302`). An optimistic update never has a finalized header (`types/mod.rs:488-496`).

Consequence. Helios as a client is safe, because its `apply_*` functions filter. A program that records a header after "Helios `verify` returned Ok" is not. It would accept a header signed by one of 512 members, or a header that is not finalised.

Our rule (approved 2026-11-12, `docs/DECISIONS.md` D-22). `lc_verify` checks, in this order, before and after Helios:

1. SSZ decode of the update (E01).
2. A finalized header is present and is not the default header (E02).
3. Participants at least `max(config.min_participants, 342)` (E04), before the signature check.
4. Helios `verify` and `apply`, with Helios errors mapped to E01 to E06, E14 and E15.
5. The store moved: a newer finalized slot or a newly known next committee (E14 otherwise).

Tests: `vs2b_below_participation` and `vs2c_header_not_finalised` in `tests/adversarial`, both passing on the sequencer (`evidence/g2/onchain-20261112T115413Z.log`). Helios is used unmodified and is not audited by itself. The pre-checks are ours and sit in the audit surface.

Status, re-checked 2026-11-18 against the final code. Verified:

- Helios side. Both bullets of the finding hold in the pinned copy of `a16z/helios` tag 0.12.0 (commit `2af9b41`): `verify_generic_update` rejects only zero participants (`consensus_core.rs:256-257`), the two-thirds rule is in `apply_generic_update` (`:176-184`), and `default_header_to_none` turns a finalized header of slot 0 into `None` (`types/mod.rs:520-525`, called at `:468` and `:482`), which makes `verify_generic_update` skip the finality check (`consensus_core.rs:302`). A finalized header that is present with an all-zero finality branch is rejected by Helios (`consensus_core.rs:320-322`), and we map that to E02 (`crates/attest_core/src/lc.rs:263`).
- Our pre-checks are in the code, in the order listed above, in one function that both verifier variants use (`verify_and_apply_variant`, `lc.rs:636`; called for V1 and V2 from `programs/lc_verify/src/bin/lc_verify.rs:30` and `:32`). Decode, E01: `lc.rs:652`. An optimistic update has no finalized header, E02: `lc.rs:242`. Finalized slot 0, E02: `lc.rs:653-655`. This test is the same as Helios's own (slot 0 means no finalized header). Participation below `max(min_participants, 342)`, E04, before any signature work: `lc.rs:658-660`, with `QUORUM = 342` at `lc.rs:35` (341 * 3 = 1023 is below 1024; 342 * 3 = 1026 is not). Helios `verify` and `apply`: `lc.rs:672-694`. Store moved, else E14: `lc.rs:700-702`.
- Tests. `vs2b_below_participation` (`tests/adversarial/tests/adversarial.rs:275`) shows that Helios `verify_update` accepts a validly signed update with 341 of 512 (`:283-299`) and that `verify_and_apply` refuses it with E04 (`:300-305`); 342 is accepted; a stricter `min_participants` of 400 refuses 399 and accepts 400; a real mainnet update cut to 300 and one with zero participants give E04, not E03. `vs2c_header_not_finalised` (`:364`) gets E02 for a validly signed update with the zero finalized header as a full update and as a finality update, for the attested header presented as finalised, for a real update with its finality stripped and for the real optimistic update. The on-chain legs cover only the canonical cases: vs2b at 300 of 512 (`:347-361`), vs2c with finality stripped and the optimistic update (`:437-451`). The 341 of 512, the 342 boundary, the `min_participants` and the zero-participant cases, and the cases on the fabricated chain, run natively only.
- Evidence. `evidence/g2/onchain-20261112T115413Z.log` shows all 7 adversarial tests passing, including those legs against the sequencer, with the codes `ESA:E04` for vs2b and `ESA:E02` for vs2c. `lc.rs`, `programs/lc_verify` and `tests/adversarial` have not changed since the commit that added those legs.

Not verified, and what that means:

- The tests were not run again on 2026-11-18. The evidence for them is the log of 2026-11-12, and the code under test is unchanged since. The code and test text were read, not re-executed.
- The adversarial suite ran against the V1 verifier only. It was not built with the `v2` feature, so no test drives `lc_verify` V2 with these inputs. V2 shares the pre-checks above through `verify_and_apply_variant`, and its own `verify` (`crates/attest_core/src/lc_v2.rs:135`) repeats the zero-participants and finality-branch rules of Helios, but a V2 run of vs2b and vs2c has not been made.
- The on-chain legs ran on the `seq-dev` sequencer, which runs with `RISC0_DEV_MODE=1` (`scripts/seq.sh:6`). They show the guest's rejection codes, not a real proof of a rejection. A rejection aborts before any proof is made.
- The Helios source was read from the pinned local copy of tag 0.12.0, not fetched again from upstream. Helios is not audited, and the pre-checks are ours; both sit in the audit surface.

## 8. Nullifier keying and griefing

The reference consumer and the bridge both keep a nullifier so that one statement is acted on once. The key matters.

Attack on a naive key. Suppose a consumer's nullifier is the account `PDA(consumer, stmt.id)`. Anyone can create a consumer instance with any config, including one that trusts a fake `attest` program. That instance can accept a fabricated statement that carries the id of a real deposit or transfer. Doing so marks the id as consumed in the shared nullifier space before the honest instance sees it. The honest user's claim then fails with E11 for good. It costs the attacker one transaction.

Rule (approved 2026-11-12, `docs/DECISIONS.md` D-16). The nullifier is `PDA(program, sha256(domain || program_instance || stmt.id))`. The domain tag is `c20/null/v1` for the reference consumer and `bridge/null/v1` for the bridge. The consuming program's own instance id is in the seed, so one instance can never mark another's nullifiers. The account must be empty before use. `write_once()` is not used, because it accepts an identical rewrite.

For the bridge there is a second reason for a per-deposit key: the statement about the vault's root is shared by every deposit under that root, so it cannot be the only key (`docs/DECISIONS.md` D-33).

State facts. The id of an account or storage statement does not depend on the block (F6). A consumer that keys on it accepts a given (address, slot, value) once per instance. That is the intended uniqueness for state facts. Log ids are unique per log.

## 9. Verifier immutability and migration (VS3)

`lc_verify` is deployed with the immutable flag. The verifier ids an instance accepts are part of its config, and the config is the instance address. No instruction changes them. Nothing reaches consensus or inclusion verification through a `ProgramId` held in mutable account state. A new Ethereum fork, or a fix, means a new verifier program and a new instance. The consumer's migration steps are in `docs/CONSUMER-PATTERN.md` section 7. A consumer that holds this program's id in mutable state lets whoever controls that pointer substitute a program that attests to anything. `docs/CONSUMER-PATTERN.md` section 4 states the rule.

## 10. When the guarantees degrade or fail

- A committee of 342 or more colluding seats in any period the instance has tracked.
- A wrong checkpoint.
- An instance older than the weak-subjectivity period.
- A change to the gloas rules that breaks the argument in section 3.
- A bug in code we wrote or in Helios (neither is audited yet; Helios has no audit of its own, `docs/REUSE.md` section 1).
- RISC Zero advisory GHSA-g6qg-9j8p-7j87 (critical, 2026-11-11): an attacker who can produce one valid Groth16 receipt can have an arbitrary claim accepted. It affects `risc0-zkvm` below 3.1.0. LEZ v0.4.0-rc3 pins 3.0.5, so receipt checks on LEZ rest on an affected version until LEZ moves to 3.1.0.
- LEZ itself: a bug in the sequencer's proof checking, or censorship of transactions.
- A fork-change in Ethereum containers that our pinned Helios does not support. The follower then submits nothing, and the instance stalls until a new verifier is deployed (section 9).

No audit report exists yet. When the audit is done, the report and the changes it caused will be linked here.
