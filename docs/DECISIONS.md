# Design decision log

This log records each major design decision: the context, the options considered, the option chosen, and the rationale and trade-offs. It answers RFP-021 Supportability 11 (S11) and RFP-022 Supportability 4 (S4). It is updated when a decision is made and is current at each milestone delivery.

Status: written 2026-11-12, corrected the same day for the Helios audit claim, the failsafe and the RISC Zero advisory. Written from section 2 of the project plan, the build notes up to 11:40 UTC, and the bridge notes (decisions 1 to 11). Entries from section 2 of the plan are marked "plan". Entries from the build notes are marked "build" with a time. Later edits updated status lines with measured results (the real proofs and the G4 runs). The log was not otherwise re-reviewed in the final docs pass of 2026-11-17.

Terms. A cycle is one RISC-V instruction in the RISC Zero zkVM. Gas on LEZ v0.4.0-rc3 is 1 per cycle, capped at 10,000,000 per transaction and per block (`MAX_GAS_EXEC` in `lez/programs/fee/core/src/market.rs`, read in the pinned tree on 2026-11-12). "Measured" means a raw output file exists in `evidence/`. "Estimate" means it does not.

| Id | Title | Source | Status |
|---|---|---|---|
| D-01 | Runtime target: LEZ v0.4.0-rc3 on a standalone sequencer | plan | decided |
| D-02 | Toolchain pins: Rust, RISC Zero, guest image | plan | decided |
| D-03 | Guest dependency patches | plan | decided |
| D-04 | The `ethereum_hashing` pin | build 2026-11-12 | decided |
| D-05 | Light-client rules: Helios 0.12.0, unmodified | plan | decided |
| D-06 | Two programs: `attest` and `lc_verify` | plan (D1) | decided |
| D-07 | Instance address is the PDA of the config hash | plan | decided |
| D-08 | Path A: prove privately, apply publicly, one transaction | plan (D2) | decided |
| D-09 | Path B: public, sequencer-executed, measured now | plan (D3) | decided, B1 open |
| D-10 | Three `Verify` variants | plan (D4) | V1 and V2 measured, V3 open |
| D-11 | Interface contract and identifier derivations | plan | decided |
| D-12 | `Attest` is a handler usable on public and private accounts | plan | decided |
| D-13 | Typed error codes E01 to E15 | plan | decided |
| D-14 | Consumer rule: sender check and empty-account nullifier | plan | decided |
| D-15 | `Attest` takes the instance and a delivery actor; one claim entry point | build 2026-11-12 10:40 | decided |
| D-16 | Nullifier keyed on the consuming program instance | build 2026-11-12 10:40 | decided |
| D-17 | Update inputs are canonical SSZ; the fork comes from the slot | build 2026-11-12 10:50 | decided |
| D-18 | Apply rule allows committee-only updates | build 2026-11-12 10:50 | decided |
| D-19 | Restart: local store checked against the on-chain commitment | build 2026-11-12 10:50 | decided |
| D-20 | Gloas records carry the execution hash only | build 2026-11-12 11:00 | decided |
| D-21 | ePBS check: a gloas header attests the parent execution block | build 2026-11-12 11:20 | cleared |
| D-22 | Pre-checks before Helios: decode, finality, quorum | build 2026-11-12 11:40 | decided |
| D-23 | G2 error mapping | build 2026-11-12 11:40 | decided |
| D-24 | Statement delivery and host API shape | build 2026-11-12 11:00 | decided |
| D-25 | Guest build profile | plan | decided |
| D-26 | Store witness and commitment | build (light-client notes) | decided |
| D-27 | Beacon source: Logos module first, direct HTTP as stated fallback | plan | decided |
| D-28 | LEZ access through the wallet crate | plan | decided |
| D-29 | Hand-kept IDL with a drift test | plan | decided |
| D-30 | The follower runs as a separate process under the module | build (follower runbook) | decided |
| D-31 | Proving environment: CPU only, proving windows | plan | decided |
| D-32 | RFP-022 inbound design | plan | decided |
| D-33 | Claim nullifier: per deposit and per bridge instance | plan, build | decided |
| D-34 | RFP-022 outbound is staged | plan | decided, stage 1 in grant |
| D-35 | Failsafe: exits that avoid what broke | plan, revised 2026-11-12 | decided (L1, L2); milestone (L3 to L5); to confirm at M0 |
| D-36 | Licence | plan | decided |
| D-37 | Fresh codebase | plan | decided |
| D-38 | Filing structure: two linked proposals | plan | decided, office hours may change it |
| D-39 | Measured cost of V1, and the glue fix that lowered it | build 2026-11-12 | decided |
| D-40 | The vault holds the verifier's emergency-stop wrapper | build (bridge notes 4), revised 2026-11-12 | decided |
| D-51 | RISC Zero advisory GHSA-g6qg-9j8p-7j87 | build 2026-11-12 | decided, platform risk open |
| D-52 | Reproducible guest builds: one release set | build 2026-11-12 | decided |
| D-41 | Two-transaction claim | build (bridge notes 1) | decided |
| D-42 | Root receipt address cannot be squatted; attest program id from config | build (bridge notes 2) | decided |
| D-43 | Deposit root at a hand-fixed storage slot | build (bridge notes 5) | decided |
| D-44 | The vault leaf binds token and denomination | build (bridge notes 6) | decided |
| D-45 | Bridge key derived from the wallet mnemonic by a separate domain | build (bridge notes 7) | decided |
| D-46 | Burn leaf records in public state (spike shortcut) | build (bridge notes 8) | decided, milestone replaces |
| D-47 | Burn memo for seed-only outbound recovery | build (bridge notes 9) | decided |
| D-48 | Wrapped balance is a per-instance ledger, not an LP-0013 token | build (bridge notes 10) | decided, milestone replaces |
| D-49 | The vault's LEZ instance is bound once after deployment | build (bridge notes 11) | decided |
| D-50 | `bridge_dev_source` is a dev-only statement source | build | decided |

## D-01. Runtime target: LEZ v0.4.0-rc3 on a standalone sequencer

- Context. The RFPs require deployment on LEZ testnet 0.4 (021 S1, 022 S1). On 2026-11-11 the public testnet 0.4 had no endpoint. It is expected about 2026-11-30.
- Options. (a) Wait for testnet 0.4. (b) Build against v0.3 on the public devnet. (c) Build against the v0.4.0-rc3 tag and run a local standalone sequencer.
- Chosen. (c). Tag `v0.4.0-rc3`, commit `bafd07be0701398f9bc435f73ab05e9a7936c301`. Two sequencer homes: `seq-dev` (`RISC0_DEV_MODE=1`, RPC 3040, long-running) and `seq-real` (dev mode unset, RPC 3041, proof windows only).
- Rationale. The programs, the cost caps and the private-to-public message path of v0.4 differ from v0.3. Evidence taken on another version would not transfer. The standalone sequencer is the mode the RFPs name for integration tests (021 S2, 022 R10).
- Trade-off. No evidence from a public network until testnet 0.4 opens. The proposal says so and schedules the same instance on testnet 0.4 on the first day it is available.

## D-02. Toolchain pins

- Chosen. Rust 1.98.1 (`rust-toolchain.toml`, same as LEZ rc3). `risc0-zkvm` and `risc0-build` pinned to `=3.0.5`. Guest builder image `RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1`.
- Options. Move to RISC Zero 3.1.0, released 2026-11-10.
- Rationale. LEZ rc3 resolves to 3.0.5. A guest built with another version has a different image id and may not be accepted by the sequencer's verifier. 3.1.0 is semver-compatible and would be picked up silently, so the pin uses `=`.
- Trade-off. We do not get 3.1.0 fixes until LEZ moves. Revised 2026-11-12: advisory GHSA-g6qg-9j8p-7j87 makes 3.0.5 an affected version. See D-51.

## D-03. Guest dependency patches

- Context. Helios uses `bls12_381` 0.8 and `sha2` 0.9. LEZ uses `sha2` 0.10 and a patched `keccak`. Without accelerated crates the guest costs many times more cycles.
- Chosen. `[patch.crates-io]` entries in the root `Cargo.toml`:
  - `bls12_381` to `risc0/zkcrypto-bls12_381` tag `bls12_381/v0.8.0-risczero.1`.
  - `sha2` 0.10 to `risc0/RustCrypto-hashes` tag `sha2-v0.10.8-risczero.0`; `sha2` 0.9 to the same repository, tag `sha2-v0.9.9-risczero.0`.
  - `keccak` to `logos-blockchain/sponges` rev `3a56e99` (the patch LEZ rc3 uses).
  - `ethereum_hashing` (D-04).
- Rationale. These are the RISC Zero accelerator forks and the patch LEZ uses. We write no cryptography. Licences: `docs/REUSE.md`.

## D-04. The `ethereum_hashing` pin

- Context. Helios 0.12.0 patches `ethereum_hashing` in its own workspace. Our guest build must resolve the same crate or Helios pulls in `ring`, which does not build for the zkVM target.
- Options. (a) Vendor a trimmed copy of Helios. (b) Use the same patch Helios uses, pinned to the same revision. (c) Move to Helios 0.11.1.
- Chosen. (b). `github.com/ncitron/ethereum_hashing` rev `7ee70944ed4fabe301551da8c447e4f4ae5e6c35`. This is the fork of the Helios author. It is the same revision in Helios 0.12.0's `[patch.crates-io]` (`Cargo.toml:134` at the tag). Decision for kill criterion K1, 2026-11-12. Added to the root `Cargo.toml` at about 10:30 UTC.
- Rationale. The fork drops `ring` and uses `sha2` 0.10, so the RISC Zero SHA-256 accelerator applies in the guest. The same revision means the Helios code path is the one its authors test.
- Trade-off. A fork by one maintainer. Licence Apache-2.0. It is pinned by revision, and `docs/REUSE.md` lists it.

## D-05. Light-client rules: Helios 0.12.0, unmodified

- Context. RFP-021 R6 asks for mature, audited implementations and for a statement of what is reused and what is rewritten. Helios has no audit of its own (below).
- Options. (a) Hand-written BLS pipeline sized for the 10M cap. (b) `r0vm-helios` (RISC Zero 2.0, Helios 0.8.3, no LICENSE file). (c) `helios-consensus-core` 0.12.0 from `a16z/helios`, unmodified.
- Chosen. (c), tag `0.12.0` (commit `2af9b41887e069144ee56cd710f3ddca195473f8`), licence MIT. It supports Gloas and carries the 2026 committee-update fix.
- Rationale. Only a private proof is free of the 10M cap (D-08), so the upstream library can run as published. The audit surface shrinks to the instance program, the inclusion code and the glue. `r0vm-helios` is stale and has no licence file. We read `sp1-helios` and `r0vm-helios` for structure only and copy nothing.
- Trade-off. Cost. Measured at 46.1M to 65.5M user cycles per update (D-39), more than the figures other applicants cite for hand-written code. V2 (D-10) is the mitigation: 37.0M on the finality update.
- Audit status, corrected 2026-11-12. Helios is not audited by itself. Its README at 0.12.0 (line 178) says "It has not been audited". The audits of SP1 Helios (Zellic, 2025-07, Helios 0.8.6) and R0VM Helios (zkSecurity, 2025-04, Helios 0.8.3, library out of scope) did not review it. What holds is that it is unmodified upstream code, maintained by a16z since 2022 and the core of both products. The audit scope must include the Helios functions we call. The `bls12_381` fork is not audited as a crate either (its README says so). Source: `research/10-failsafe-and-helios-audit.md` part B.
- Check. Passed on real mainnet data: Helios 0.12.0 runs unmodified inside a RISC Zero 3.0.5 guest, five consecutive updates, native and executor results equal (`evidence/cost/raw/exec-lc.json`). Not done yet: a CI job that diffs the resolved Helios source against the tag. Today the source is pinned by tag and commit in `Cargo.lock` (`git+https://github.com/a16z/helios?tag=0.12.0#2af9b418...`).

## D-06. Two programs: `attest` and `lc_verify`

- Context. A deployed ELF is capped at 20 segments of 96 KiB, 1,966,080 bytes (LEZ rc3 `program/mod.rs:32`, `program_loader/core/src/lib.rs:23`).
- Options. One program with everything. Two programs.
- Chosen. Two. `attest` is small and long-lived: instance state, `Apply`, write-once header records, `Attest`. `lc_verify` is large and replaceable per Ethereum fork: Helios verify and apply, nothing else, deployed immutable. An instance's config lists its accepted verifier program ids.
- Rationale. LEZ's smallest guests are about 340 KB. Helios with alloy needs most of the budget. The measured `lc_verify` user ELF is 754,956 bytes (8 of 20 segments, `evidence/cost/cost.md`). A new Ethereum fork means a new verifier and a new instance under the same `attest` program id, so consumers keep one program id.
- Fallback. One program gated by `from_own_program()`, the pattern LEZ's own programs use, if the cross-program check misbehaves. Not needed so far.

## D-07. Instance address is the PDA of the config hash

- Chosen. `instance_id = PDA(attest, sha256("esa/instance/v1" || borsh(config)))`, config = chain id, genesis validators root, genesis time, fork list, checkpoint root and slot, verifier ids (1 to 2), minimum participants.
- Rationale. The config is the address, so no instruction can change it (021 F9, F13, VS3). Recovery from a wrong checkpoint is a new instance (021 F10). Two instances of different chains share one program (021 F14, F15).
- Trade-off. Any config change gives a different address, so consumers must be told the new one. `docs/CONSUMER-PATTERN.md` section 7 states the migration.

## D-08. Path A: prove privately, apply publicly, one transaction

- Context. One update costs about 11M to 15M cycles hand-written and more with the unmodified Helios library. The per-transaction cap is 10M.
- Chosen. `Verify` runs in a private actor of `lc_verify`. The privacy circuit composes its receipt with `env::verify`, so the cap does not apply to it. It sends a sync message `Apply` to the public instance actor, which checks the sender's program id against its config and the pre-state against its stored commitment. On chain: 409,764 verify gas, the `Apply` cycles, and 224,063 storage gas (`PRIVATE_VERIFY_GAS` = 409,764 and `PRIVATE_GAS_STOR` = 223,551 + 512 = 224,063 in `lez/programs/fee/core/src/market.rs`, read 2026-11-12; the source marks them provisional).
- Rationale. A private sender keeps its program id (LEZ rc3 `execution_state.rs:1409-1428`), so the public side can trust the sender.
- Trade-offs. Cost moves to the submitter's hardware. Each private update takes one of about four private slots in a block (storage gas is capped at 1,000,000 per block, so four transactions of 224,063 fit; our arithmetic). The RFP text expects tracking on the public path (021:366-373). D-09 answers that.
- Check. Confirmed on `seq-dev` (T1.0): the private sender's program id reaches `Apply`. The chain test passed (`evidence/g1/chain.json`). Cost per update: 409,764 + 54,965 + 24,047 = 488,776 execution gas (4.9 percent of the cap) and 224,063 storage gas (`evidence/cost/cost.md` section 2).

## D-09. Path B: public, sequencer-executed, measured now

- Context. The RFP calls in-program BLS12-381 verification the central feasibility question and asks for the cost of the public path.
- Chosen. The tracked state is public and shared on both paths. Only where verification is paid differs. Path B is a second verifier id in the same config. B0 (sprint, required): run the monolithic `Verify` in the public executor on `seq-dev`, record cycles per phase and the cap rejection. B1 (milestone): split the pairing over three or more public transactions that end in the same `Apply`. Competitors' split figures are cited as theirs.
- Rationale. It gives our own numbers for P8, and the proposal can lead with either path after the office-hours answer.
- Status. B0 measured: `Verify` is 4.6 to 6.5 times the cap (`evidence/cost/cost.md` section 3). Sent as a public transaction it is refused in the wallet's simulation, and when forced it is included and reverted (tx `43902552...c38d`, block 334; raw `evidence/cost/raw/b0-public-verify-2026-11-12.log`). B1 is milestone work.

## D-10. Three `Verify` variants

- V1: Helios unmodified. V2: the Helios rules transcribed from `verify_generic_update` (0.12.0, MIT) with one function replaced, the signature check. Helios decompresses the signature, which already rejects points outside the subgroup, then runs the subgroup check again (8.77M cycles). V2 drops the repeat. `apply_*` is still Helios's. (The plan's version of V2, witnessed non-signer keys, is not built.) V3: hand-written `blst` pipeline, not built.
- Rationale. V1 is the audit story. V2 is the likely operating variant. Each variant has the same interface, so the instance config can accept more than one. V2 is not unmodified Helios, so it adds one short function to the audit surface.
- Status. V1 and V2 measured. V2: 37,032,744 cycles on the finality update, 45.6M to 49.9M on the period updates, 717,088-byte user ELF (`evidence/cost/raw/v2-exec.json`). A differential test (`tests/e2e_chain/tests/v1_v2.rs`) gives V1 and V2 the same post-store or the same error code on the real chain and on 165 tampered updates. V3 is milestone work if Logos prefers it.

## D-11. Interface contract and identifier derivations

- Chosen. The types in `crates/attest_core` (`InstanceConfig`, `Head`, `HeaderRecord`, `Statement`, `Predicate`), borsh-encoded, frozen as compiling stubs on 2026-11-12 12:00 UTC (freeze 1).
- Identifiers. Record: `PDA(attest, sha256("esa/record/v1" || instance_id || slot))`, written once. Statement id: for a log, `sha256("esa/stmt/v1" || chain_id || exec_hash || tx_index || log_index || keccak(log))`; for an account or storage fact, `sha256("esa/stmt/v1" || chain_id || address || slot_key || value)`. The same state gives the same id (021 F6).
- Rationale. A consumer can key a nullifier on the id without inventing its own derivation. Account and storage ids do not depend on the block, so a consumer accepts one fact once per instance. That is the intended uniqueness for state facts.

## D-12. `Attest` is a handler usable on public and private accounts

- Context. The direct consumption path must verify inclusion inside the consumer's own transaction (021 F7).
- Chosen. `Attest` is a program handler, not only a library. On a public account it runs metered. On the user's private account it is proven on the user's device and discloses only which shared header record was read. Header records are write-once, so private consumers never contend (021 F8, R11).
- Fallback. If a probe shows the private-to-public message path does not carry the right sender, use one program and link `attest_inclusion` as a library.

## D-13. Typed error codes E01 to E15

- Chosen. A panic text `ESA:Exx` on chain and the same enum from the host dry run (021 U2, U5): E01 malformed, E02 not finalised, E03 bad signature, E04 low participation, E05 handoff not verified, E06 stale checkpoint, E07 inclusion invalid, E08 predicate false, E09 chain mismatch, E10 predicate not covered, E11 already consumed, E12 unknown verifier, E13 store mismatch, E14 already applied, E15 header mismatch.
- Rationale. A test can assert an exact code, so "rejected" never hides the wrong reason.

## D-14. Consumer rule: sender check and empty-account nullifier

- Chosen. A consumer accepts a statement only if the runtime's `from.program_account_id` equals the `attest` program id fixed at build time (from `deploy/ids.toml`) and the statement's instance and chain id equal the consumer's constants. Its nullifier is a PDA whose pre-state must be empty. It does not use `write_once()`, because that accepts an identical rewrite.
- Rationale. A program id held in mutable state lets whoever controls the pointer substitute a verifier (021 VS3).

## D-15. `Attest` takes the instance and a delivery actor; one claim entry point

- Date. 2026-11-12 about 10:40 UTC. Approved by the team on a proposal from the inclusion-proof work.
- Chosen. `Attest.deliver_to` becomes `lee_core::account::Actor`. `Attest` gains `instance: AccountId`, as do `attest_host::attest` and `dry_run_attest`. There is a single claim entry point: `attest_inclusion::verify_claim(instance, record, claim_bytes, predicate) -> Result<Statement, EsaError>`.
- Rationale. The record must be bound to the instance by `CheckHeader`. One entry point means the dry run and the on-chain path run the same code (021 U2).

## D-16. Nullifier keyed on the consuming program instance

- Date. 2026-11-12 about 10:40 UTC. Approved by the team.
- Context. Anyone can create a consumer instance with any config, including one that trusts a fake `attest` program.
- Options. (a) Nullifier at `PDA(consumer, stmt.id)`. (b) Nullifier at `PDA(consumer, sha256(domain || consumer_instance || stmt.id))`.
- Chosen. (b). Domain `c20/null/v1` for the reference consumer, `bridge/null/v1` for the bridge.
- Rationale. With (a) an attacker's instance could accept a fabricated statement that carries the id of a real deposit or transfer. That marks the id consumed before the honest instance sees it, and the honest user's claim then fails with E11 for good. Scoping the seed to the instance closes it. See `docs/TRUST.md` section 8.

## D-17. Update inputs are canonical SSZ; the fork comes from the slot

- Date. 2026-11-12 about 10:50 UTC. Chosen. The guest takes the SSZ bytes of the light-client update as the Beacon API serves them. The fork is chosen from the update's slot through `config.forks`. Rationale: the same bytes the network produces, no re-encoding to get wrong.

## D-18. Apply rule allows committee-only updates

- Date. 2026-11-12 about 10:50 UTC. Chosen. `Apply` accepts when `post_store != pre_store` and `head.slot >= old head slot`. The header record is written only when the slot is new. An update that moves nothing is E14.
- Rationale. An update that only teaches the next committee must be accepted, or a period could not be handed off when no new finalised slot exists yet.

## D-19. Restart: local store checked against the on-chain commitment

- Date. 2026-11-12 about 10:50 UTC. Chosen. The follower keeps a local store file and checks it against the on-chain `store_commitment` on restart. Fallback: a bootstrap at `head.beacon_root`, then the period update. `attest_host::read_store_status(instance)` supports it.
- Rationale. 021 R4: resume without re-verifying from the checkpoint and without accepting an unverified handoff. The commitment makes the local file checkable.

## D-20. Gloas records carry the execution hash only

- Date. 2026-11-12 about 11:00 UTC. Context. Sepolia runs gloas. A gloas light-client header carries only `execution_block_hash`.
- Chosen. A gloas record has `exec_hash` set and `exec_number`, `exec_state_root`, `exec_receipts_root` at zero. A claim against such a record carries the RLP execution header. `attest_inclusion` checks `keccak(rlp) == record.exec_hash` and reads `stateRoot`, `receiptsRoot` and `number` from it. Errors: E01 if the header is missing when the roots are zero, E15 if the hash differs. Later header fields are bound by the hash and never parsed, so a new field in a fork needs no change.

## D-21. ePBS check: a gloas header attests the parent execution block

- Date. 2026-11-12 about 11:20 UTC. Question raised during the light-client work: under ePBS, can the proven hash name a withheld bid, which is not canonical?
- Finding. No, under the current spec (consensus-specs `c3e16117`, 2026-11-11). The hash in the gloas light-client header is `bid.parent_block_hash`, generalized index 2856, and Helios 0.12.0 uses the same leaf. It is reachable only after the payload was delivered and verified. Full reasoning and line citations: `docs/TRUST.md` section 3.
- Consequence. A gloas record attests the parent execution block, so it lags by one or more slots. Documentation and the bridge say so.

## D-22. Pre-checks before Helios: decode, finality, quorum

- Date. 2026-11-12 about 11:40 UTC. Finding from the adversarial-test work, confirmed against `a16z/helios` 0.12.0. Helios `verify_generic_update` does not enforce the two-thirds quorum, and it treats a zero finalized header as "no finality" and skips the finality branch.
- Chosen. `lc_verify` checks, in this order: SSZ decode (E01), finalized header present and non-default (E02), participants at least `max(min_participants, 342)` (E04), then Helios, then that the store moved (E14). Full explanation: `docs/TRUST.md` section 7.
- Rationale. A program that records the header after "Helios returned Ok" would accept a header signed by one of 512 members, or one that is not finalised.

## D-23. G2 error mapping

- Date. 2026-11-12 about 11:40 UTC. The table is in `crates/attest_core/src/error.rs`. Every adversarial test asserts an exact code. Init errors map to E06, including `InvalidCurrentSyncCommitteeProof` (team decision).

## D-24. Statement delivery and host API shape

- Date. 2026-11-12 about 11:00 UTC. A statement is delivered as a `SyncMessage` whose whole message is `borsh(Statement)`. `read_head` takes the deployed ids and the instance. `read_store_status`, `DeployedIds::from_env` and an environment-based wallet config exist.

## D-25. Guest build profile

- Chosen. The root `[profile.release]` is the guest profile: `opt-level = "z"`, LTO, one codegen unit (as LEZ does), with `opt-level = 3` for `bls12_381` and `sha2`. Host long-run binaries use `--profile host`.
- Rationale. Small ELF under the cap with the hot crypto loops optimised for speed.

## D-26. Store witness and commitment

- Chosen. Helios reads only the beacon part of the store's finalized header, so the witness carries the `BeaconBlockHeader` and rebuilds the store with a Bellatrix-shaped light-client header. `optimistic_header` equals the finalized one and `best_valid_update` is none. Neither enters the commitment or affects `verify_*` or the finality path of `apply_*`.
- `store_commitment = sha256(finalized beacon root || current committee root || next committee root or zero || previous max active participants (u64 le) || current max active participants (u64 le))`. A committee root is `hash_tree_root(SyncCommittee)`.
- `expected_current_slot` passed to Helios is the `signature_slot`. The chain-time side is the transaction's timestamp window (`genesis_time + 12 * signature_slot`, LEZ timestamps are milliseconds).

## D-27. Beacon source: Logos module first, direct HTTP as stated fallback

- Context. Any Ethereum-side interaction must use the Logos Ethereum core module (021 platform dependency).
- Chosen. `LogosModuleSource` through `beacon_api_module` `lc_request` is preferred. `DirectHttpSource` calls the same Beacon API routes on the same public providers and is a stated fallback. Every log line carries `beacon_source`, and the G4 gate reports which was used.
- Trade-off. If the module path is not what ran, the proposal says so. Status: the module path ran. Both G4 runs report `beacon_source` `logos_module` (`evidence/g4/run-a/g4-summary.json`, `evidence/g4/run-b/g4-summary.json`).

## D-28. LEZ access through the wallet crate

- Chosen. The LEZ `wallet` crate at rc3 as a library. Stated bypass: Logos's LEZ module does not target v0.4 yet.

## D-29. Hand-kept IDL with a drift test

- Chosen. `idl/*.json` are written by hand in SPEL's JSON shape. A test compares each IDL with the message enum (`tests/e2e_chain` `idl_drift`).
- Rationale. SPEL has no v0.4 release. The drift test keeps the file honest until it does.

## D-30. The follower runs as a separate process under the module

- Chosen. `attest_follower_module` (Rust, `#rust` template) supervises `attest-followerd`. Methods: `configure`, `start`, `stop`, `status`. Events: update submitted, handoff applied, follower error.
- Rationale. The daemon links the LEZ wallet and RISC Zero host crates, which do not belong inside a Qt plugin. It must also survive `kill -9` and restart by itself (021 R4).

## D-31. Proving environment: CPU only, proving windows

- Chosen. No local GPU build (4 GB VRAM is too small for large segments, estimate). Real proofs ran only in agreed windows, niced, one at a time, under `prove_job.sh` with wall time and peak memory recorded. All other work uses `RISC0_DEV_MODE=1`.
- Measured. One po2-20 segment takes 363.0 s on average (358.7 s to 365.8 s, three runs), 346.2 microseconds per padded cycle, peak 9.149 GiB (`evidence/cost/k4-projection.json`, `evidence/cost/prove.md`). risc0-zkvm 3.0.5 proves segments one at a time and spills them to disk, so peak memory is set by the segment size and not by the cycle count. Both real proofs peaked near 9.3 GiB with under 100 MB of swap.
- Rationale. The build machine is shared and has 15 GiB of RAM. We declare 16 GB of RAM as the minimum machine for users.

## D-32. RFP-022 inbound design

- Chosen. A Sepolia `Vault.sol` with fixed denominations per token for native ETH and ERC-20 (OpenZeppelin `SafeERC20`). `deposit(token, denomIndex, commitment)` appends the commitment to an incremental SHA-256 tree (the deposit contract's algorithm) and stores the root at a fixed storage slot. `commitment = sha256("wb/c/v1" || secret || destBinding || token || denom)`. The secret derives from the wallet seed on a fixed path, so recovery needs the seed only (022 F9).
- The claim is one LEZ transaction from the user's private account: `Attest` of the vault's root slot at a finalised header (RFP-021), then `Claim`, which proves the Merkle path and the opening and reveals a nullifier, token and denomination. It is zero-knowledge also for a public destination (a hybrid transaction).
- Rationale. Nothing on Ethereum names the destination (022 F2). The design is documented in `docs/bridge/INBOUND.md`.

## D-33. Claim nullifier: per deposit and per bridge instance

- Context. 022 F3 asks for uniqueness "keyed on the statement identifier". The statement about the vault root is shared by every deposit under one root, so it cannot be the only key. This is a reasoned deviation from F3 (`docs/bridge/INBOUND.md`).
- Chosen. Nullifier account `PDA(bridge, sha256("bridge/null/v1" || bridge_instance || deposit_nullifier))`. The key is per deposit. The instance is in the seed for the reason in D-16 (the griefing finding): another instance, real or fake, cannot mark this instance's nullifiers.

## D-34. RFP-022 outbound is staged

- Context. 022 F7 asks for release on a trustless proof of a LEZ burn. On LEZ v0.4.0-rc3 headers carry no state root and public execution is not proven. We cannot meet F7 in full.
- Chosen. Stage 1 is the grant. A burn appends a note to an accumulator. A RISC Zero guest proves membership and reveals recipient, amount, token, fee, nullifier, vault and chain id; a Groth16 wrap is verified on Ethereum. The accumulator root is made final by an anchor. Stages 2 and 3 (block-derived accumulator, signer-diverse depth) are design only. Stage 4 is an optimistic Bedrock checkpoint with a bond and a challenge window. Stage 5 is a named arbiter set for disputes. The stated residual trust: the canonical Bedrock header, attested optimistically, with an arbiter on dispute.
- In the sprint the anchor is `StubLezAnchor`, an owner-posted stub, and every document names it as a stub. The milestone is worded "stage-1 anchor agreed at M0", never "trustless release".
- Later. A dispute proof in the zkVM replaces the arbiter, then direct verification once LEZ headers commit to state.

## D-35. Failsafe: exits that avoid what broke

- Context. 022 Sec9 asks for a failsafe that is not a backdoor, or a reasoned statement that none should exist. Sec8 forbids changing what the bridge accepts after deployment. The bridge has one trustless direction: LEZ reads finalised Ethereum state through RFP-021. Ethereum learns LEZ state only through the anchor (a stub now, optimistic with an arbiter at M4).
- Options. (a) A depositor proves the opening on Ethereum and is refunded (draft in `docs/bridge/TRUST.md`, 2026-11-12). (b) A refund that needs a non-mint proof through the anchor. (c) A refund after a long "LEZ halted" timeout. (d) A freeze only, with no failsafe. (e) Layered exits that avoid the broken component.
- Rejected. (a) pays a deposit twice: the vault cannot see that it was claimed on LEZ. (c) has the same flaw for claims made after the last anchored state, and the vault cannot tell a halted LEZ from a silent anchor. (d) leaves the whole escrow locked or drainable if the proof system breaks, as RISC Zero advisory GHSA-g6qg-9j8p-7j87 (2026-11-11) shows can happen.
- Chosen. (e). L1: the vault calls the RISC Zero verifier through its emergency-stop wrapper; anyone with a proof of exploit can stop it, and it never restarts. L2: an open release route checks the `burn_proof` statement with plain SHA-256 in the vault; it shares the nullifier map with `release`. L3: void on Ethereum and re-credit on LEZ for burns that cannot be released (RFP-021 only). L4: cancel an unclaimed deposit on LEZ into a release (anchor trust). L5: after a permanent mint latch, refund an unclaimed deposit on Ethereum against the anchored claimed-nullifier set (anchor trust). Each exit is claimed by the owner with the seed-derived secret and pays only the destination bound in it.
- Not offered, with reasons. No exit for the escrow if the anchor fails. No exodus if LEZ halts. No refund of unclaimed deposits after an inbound-verification fault before L5 exists. These funds stay in the vault, frozen, and are not lost to an attacker beyond the caps.
- Trade-offs. L2 reveals the user's own burn-to-release link, so it is opt-in. L1 lets RISC Zero's guardian halt the zero-knowledge route; L2 keeps exits open. L2 and L5 are new code on the payout path and go in the audit scope.
- Status. L1 and L2 are in vault v3 on Sepolia and were exercised: a real release through the emergency stop (527,859 gas) and an open release (325,769 gas) (`evidence/bridge/groth16-release-v3/`). L3 and L4 at M4. L5 designed at M4, built at M6 only if the stage-1 anchor is agreed at M0. Confirm with Logos at M0. Full analysis: `research/10-failsafe-and-helios-audit.md`.

## D-36. Licence

- Chosen. MIT OR Apache-2.0 for all our code, as both RFPs require. Each reused component and its licence: `docs/REUSE.md`.

## D-37. Fresh codebase

- Chosen. No code from earlier applicants' repositories was opened, copied or paraphrased. `sp1-helios` and `r0vm-helios` have no LICENSE file and were read for structure only. A cloned bid is visible as such to reviewers.

## D-38. Filing structure: two linked proposals

- Chosen. Two issues, one per RFP, each stating the combined price in its first paragraph. Each submission under the Terms targets one RFP. The outcome of the office hours of 2026-11-13 is not recorded in this repository.

## D-39. Measured cost of V1, and the glue fix that lowered it

- Date. 2026-11-12. Measured on an i5-10300H, RISC Zero 3.0.5 executor, dev mode, five real mainnet updates. The first run (09:45 UTC) gave 74,446,829 to 89,340,805 user cycles per update, about three times the plan estimate of 28M (raw: `.tmp/w1/evidence/v1-exec.json` on the build machine). A cycle trace showed the cause was our glue and not Helios: `tree_hash` packs every byte of a 48-byte key separately (about 11M cycles per committee root), and the glue hashed committees three times per update.
- Fix. Glue only, Helios unmodified: direct SSZ committee roots over the accelerated `sha2`, raw witness bytes, no rehash of an unchanged next committee. A native test checks equality with `tree_hash`. `Init` also stopped calling Helios `verify_bootstrap` (11M cycles for its committee hash, above the public cap) and runs the same three checks in `lc::bootstrap_valid`, cross-checked against Helios by a native test (light-client notes).
- Result. 46,095,505 (finality update) to 65,461,983 (handoff with next committee) user cycles per update on the reproducible release guests. `Init` 4,926,875. `Apply` 54,965 (`evidence/cost/cost.md`, regenerated 2026-11-12 20:20 UTC). The BLS phases are 36.84M of the 46.1M on the finality update, so most of the cost is Helios's own BLS work.
- Consequences. A public single-transaction update cannot fit the 10M cap with V1 (4.6 to 6.5 times the cap). Path A remains the way to run Helios unmodified. V2 (D-10) saves 8.77M cycles per update. Real proving time, measured: 20,060 s (5.57 hours) and 9.259 GiB for the V1 finality update, accepted in block 1784 on a dev-mode-off sequencer (`evidence/g1/real-proof.json`). The K4 projection was 6.08 to 7.01 hours (`evidence/cost/k4-projection.json`). The V2 finality update was also proven for real: 16,064 s (4.46 hours) and 9.24 GiB, accepted in block 3111 (`evidence/g1/real-proof-v2.json`).

## D-40. The vault holds the verifier's emergency-stop wrapper

- Source. Bridge notes 4, revised 2026-11-12. The plan (section 2.4) said router. The first sprint decision was to hold the RISC Zero Groth16 verifier 3.0.0 directly, because the `RiscZeroVerifierRouter` has an owner who can add or remove verifiers.
- Revised. The vault holds the verifier's emergency-stop wrapper, an immutable address, and not the raw verifier. The wrapper can only stop verification, never change it, so Sec8 holds. Advisory GHSA-g6qg-9j8p-7j87 affects verifier 3.0.0. Vault v3 uses verifier 3.1.0: Sepolia verifier `0x8c915b5071206C6E328BC18529586c513408aB3b`, emergency stop `0xA1DCB16cb765b3CA71605339F0C37Bfe0F60bA29`. Vaults v1 and v2 use 3.0.0 and are deprecated testnet artefacts.
- Rationale. 022 Security 8: no party may change what the bridge accepts after deployment. A new proof system means a new vault and a migration. The stop-only wrapper also gives L1 of the failsafe (D-35).

## D-41. Two-transaction claim

- Date. 2026-11-12, approved by the team. The plan (section 2.4) described one transaction.
- Chosen. Step 1 is a public `attest` of `vault.depositRoot` into a root receipt (shared by every claimant against that root). Step 2 is a private `Claim` that does CheckRoot, Nullify and Credit. The `AttestCall` option in the message keeps the one-transaction variant available.
- Rationale. Step 1 is shared and permissionless, so one attestation serves every claimant, and the private proof stays small.

## D-42. Root receipt address cannot be squatted; attest program id from config

- Chosen. The receipt PDA seed is `sender_program || attest_instance || stmt_id`. The bridge recomputes it from the real sender program and the statement's own instance and id, and refuses any other receiver. So the attest program id comes from the bridge config and not from a build-time constant (the consumer rule, plan section 2.3).
- Rationale. A fake attest program or a fake light-client instance can write only receipts no honest config reads. The safety equals the build-time constant, because the instance id commits to the config.

## D-43. Deposit root at a hand-fixed storage slot

- Chosen. Slot `keccak256("beacon-veil.vault.depositRoot") - 1`, fixed by hand and not by the compiler's layout.
- Rationale. The storage proof of the root does not depend on the Solidity build.

## D-44. The vault leaf binds token and denomination

- Chosen. `leaf = sha256("wb/leaf/v1" || token || word(denom) || commitment)`, computed by the vault from what it really received.
- Rationale. A leaf can never claim more, or another token, than was escrowed.

## D-45. Bridge key derived from the wallet mnemonic by a separate domain

- Chosen. `bridge key = sha256("wb/key/v1" || bip39_seed(mnemonic, ""))`: the LEZ wallet's own seed through a separate domain (022 F9). The LEZ `SeedHolder` seed is crate-private, so `bridge_cli` reads the mnemonic from `BV_BRIDGE_MNEMONIC_FILE`.
- Trade-off. The CLI handles the mnemonic itself. A wallet SDK would remove that (adapter planned). `tools/bridge_cli/testdata/TEST-ONLY-mnemonic.txt` is the public BIP-39 test vector, not a secret.

## D-46. Burn leaf records in public state (spike shortcut)

- Chosen. Each burn writes a write-once PDA per accumulator index. About 200 bytes of public state per burn.
- Rationale. Our standalone sequencers run no indexer, and LP-0012 events are served only by LEZ's indexer service. The milestone design reads events and drops the records.

## D-47. Burn memo for seed-only outbound recovery

- Chosen. `memo = (recipient || fee) XOR sha256 keystream(burn_secret)`, 36 bytes, published with the burn.
- Rationale. The owner can rebuild a release from the seed alone (022 F9). Without the burn secret it is pseudorandom.

## D-48. Wrapped balance is a per-instance ledger, not an LP-0013 token

- Chosen. A ledger keyed by (instance, token) in the bridge actor's state. Decimal conversion is the identity: the wrapped base unit equals the token's base unit, so it is exact.
- Trade-off. Not ATA-compatible. The milestone moves to the LEZ token program with ATA-compatible accounts (022 dependency text).

## D-49. The vault's LEZ instance is bound once after deployment

- Chosen. `bindLezInstance` writes the instance once. The instance id hashes a config that contains the vault address, so it cannot be known before the vault exists. Tests: `test_instanceBindsOnce`, `test_releaseNeedsBoundInstance`.

## D-50. `bridge_dev_source` is a dev-only statement source

- Chosen. `programs/bridge/src/bin/bridge_dev_source.rs` forwards any statement unchecked. It exists only for the local round trip (`tools/bridge_cli/e2e_local.sh`) against an anvil vault that no light client follows.
- Why it is safe to exist. By D-42 it can write only receipts keyed by its own program id. Only a config that names it as `attest_program` reads those, and the local config does so on purpose. A real deployment names the real `attest` program. The Sepolia run uses `attest-root` through RFP-021.
- Rule. Every document that mentions a bridge run says which source it used. It is never part of a deployment that holds value.

## D-51. RISC Zero advisory GHSA-g6qg-9j8p-7j87

- Date. 2026-11-12. Critical, published 2026-11-11 (companion GHSA-mm6c-35cm-g4x2 for `risc0-ethereum`), verified by the team with `gh api`. An attacker who can produce one valid Groth16 receipt can have an arbitrary claim accepted. Affected: `risc0-zkvm` below 3.1.0, and every Groth16 verifier up to 3.0.0. Fixed in 3.1.0 and verifier 3.1.0.
- Our pins. `risc0-zkvm` 3.0.5 in `Cargo.lock`. LEZ v0.4.0-rc3 pins the same, so the platform is affected.
- Chosen. (1) Vault v3 uses verifier 3.1.0 through its emergency-stop wrapper (D-40). (2) `burn_proof` is proven with RISC Zero 3.1.0. (3) The LEZ-side programs stay on 3.0.5, because a different version gives different image ids that LEZ may not accept, and move to 3.1.0 when LEZ does. (4) Both proposals name the platform risk in one line. (5) Vaults v1 and v2 are deprecated testnet artefacts and say so in the evidence notes.
- Trade-off. Until LEZ moves, inbound claims and RFP-021 Path A rely on receipt checks of an affected version. We cannot remove this risk.
- Status. Done on our side: vault v3 is deployed on Sepolia with verifier 3.1.0 behind its emergency stop, and `burn_proof` is built on 3.1.0 (image `0xf929f9f4...97cf`, two clean docker builds gave the same ELF hash) and proven for real (`evidence/bridge/sepolia-v3.json`, `evidence/bridge/burn-proof-r31-guest.json`). Vaults v1 and v2 are frozen. The platform risk (LEZ rc3 on 3.0.5) stays open.

## D-52. Reproducible guest builds: one release set

- Date. 2026-11-12. Context. Image ids must match on every machine, or a reviewer cannot check what is deployed.
- Options. (a) Build each guest with the local rzup toolchain. (b) Build per guest in docker. (c) Build all guests from one commit in one container of the pinned image.
- Chosen. (c). Release set `bf95e359` (`scripts/guests_release.sh`), image `risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a44e1cfca415577c48d2f7012bda99ed36f2fae27f9a663b9fe6048f0`, using risc0-build 3.0.5's own docker recipe. The image ids are in `guests/IMAGE_IDS.expected`, and CI rebuilds the guests and fails if they differ (`evidence/g2/ci-run.txt`: guest build and image-id check success). The programs on the real-proof sequencers match these ids (`evidence/g1/seq-real-image-check.log`, `evidence/g3/seq-real-image-check.log`). `burn_proof` is excluded and built separately on RISC Zero 3.1.0 in image `r0.1.97.0` (D-51), where two clean builds gave the same ELF hash and image id.
- Trade-off. A local rzup build embeds the checkout path and panic locations, so it gives different ids. Evidence that names an image id says which build it is.

## D-53. Public path: the staged verifier V3 (`lc_verify_v3`)

- Date. 2026-11-20. Context. V1 (Helios unmodified) needs 46M to 65M cycles per update, so the public path, capped at 10M cycles per transaction and per block, cannot run it. Measured alternatives: `evidence/public-path/bench/bench.md`.
- Chosen. One update runs as three public transactions on a job account `PDA(lc_verify_v3, sha256("bv/job/v1" || instance || pre_store || sha256(update)))`: `Begin` (V1's pre-checks, Helios 0.12.0 `verify_generic_update` rules in Helios's order, the `Apply` fields, the aggregate key), `Miller` (signature decode with its subgroup check, hash to G2, one Miller loop over both pairs), `Finish` (final exponentiation, then sync `Apply` to the unchanged `attest` program). The job id binds the pre-store commitment, so a job built on a store that has since moved cannot land (`attest.Apply` refuses it, E13 or E14), and two submitters of the same update share one job. Anyone may send any step. `VerifyOnce` runs the same code in one execution for a private actor. Code: `crates/attest_core/src/lc_v3.rs`, `programs/lc_verify_v3`.
- What is replaced, against V1. (1) The BLS layer: `blst` (RISC Zero fork, D-54) instead of the `bls12_381` fork. (2) The store witness: the signing committee's bytes and the other committee's root, instead of both committees decoded. The pre-store commitment is recomputed from them, so the on-chain commitment binds the witness. (3) Helios `apply_generic_update` is reproduced on committee roots. V1 stays deployed as the reference and the second verifier.
- Equivalence. `tests/diff_helios` runs V1 and V3 (one execution, and three steps through the program logic) on every case: no disagreement on 835 cases. One designed difference: a participating committee key equal to the identity is refused by V3 (E03); Helios counts it as a signer (a16z/helios issue 871).
- Cost. Every step of every measured update is at most 6.8M cycles including the chained `attest.Apply` and `WriteRecord` (`evidence/public-path/exec-steps.json`). If a `Begin` ever exceeds the declared 9.0M, the submitter sends it as `BeginPrepare` and `BeginKeys` (four steps); the keys step re-checks the committee bytes against the root the first step stored.

## D-54. No subgroup check on committee keys in V3

- Chosen. V3 checks each committee key it uses (the aggregate key and the absent keys) only for lying on the curve, and binds each absent key, given uncompressed, to the exact 48 bytes at its index of the committee. It does not run a subgroup check on them. The signature, which the submitter chooses, is subgroup-checked.
- Why. The committee bytes are bound through the committee root and the store commitment to a beacon state. A key reaches a sync committee only as a validator key, and a validator key enters the beacon state only with a deposit that carries a valid proof of possession for it, which the consensus layer checks. Checking 170 or more keys for the subgroup would cost several million cycles per update for no gain on keys the chain already admitted.
- Library. `blst` is upstream code that was audited (NCC Group, 2021); the RISC Zero fork (tag `v0.3.16-risczero.0`) replaces its field arithmetic with calls to the zkVM accelerator and was not audited as such. `crates/bls_blst` is our small layer over it.

## D-55. Weak-subjectivity bound and exit-code refusals in V3

- Weak subjectivity. `Begin` and `Finish` set a timestamp window that ends 1,209,600 s (14 days, Helios's mainnet `max_checkpoint_age`) after the time of the store's finalized header. An instance left without updates for longer cannot be moved by updates; it needs a new instance from a fresh checkpoint (E06 when the window is empty).
- Refusals. `lc_verify_v3` refuses by ending with exit code = the error number (1 to 15), not by a panic: LEZ charges a non-zero exit by the cycles used and a panic by the whole declared gas. A refusal of `attest.Apply` inside `Finish` still panics with `ESA:Exx` (the `attest` program is unchanged).
