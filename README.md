# beacon-veil

beacon-veil is a working prototype for Logos RFP-021 (Trustless Ethereum State Attestation for LEZ) and RFP-022 (Privacy-Preserving Wrapped ERC-20 and Ether Bridge for LEZ). It runs the Ethereum light client from `a16z/helios` 0.12.0, unmodified, inside a RISC Zero 3.0.5 guest on LEZ v0.4.0-rc3, and builds a consumer, a follower daemon and a bridge spike on top of it. Both RFPs are bid together. The name is a working name. TODO-OWNER: confirm the project name.

Status: draft of 2026-11-12. Sections marked TODO wait for a build track. Nothing in this file is a claim until a raw file under `evidence/` backs it.

## 1. Clean-clone commands

Machine: Linux x86_64, 15 GiB RAM or more, about 20 GB of disk. Everything runs in RISC Zero dev mode (`RISC0_DEV_MODE=1`) unless a step says it proves for real. TODO-REFRESH: the commands below are the ones that exist on 2026-11-12. Those marked TODO do not run yet.

```bash
# 1. Get the code (the pinned contract libraries are git submodules).
git clone --recurse-submodules https://github.com/TODO-OWNER/beacon-veil.git   # TODO-OWNER: public repository URL
cd beacon-veil

# 2. Prerequisites. Versions are pinned: Rust 1.98.1 comes from rust-toolchain.toml.
sudo apt-get install -y build-essential clang libclang-dev libssl-dev pkg-config cmake jq
curl -L https://risczero.com/install | bash
rzup install cargo-risczero 3.0.5 && rzup install r0vm 3.0.5 && rzup install rust 1.91.1
# Foundry (forge) and Docker are also needed. TODO: scripts/bootstrap.sh that does steps 2 and 3 in one go.

# 3. LEZ v0.4.0-rc3, built once, for the local sequencer (commit bafd07be0701398f9bc435f73ab05e9a7936c301).
# TODO: scripts/bootstrap.sh clones and builds it. Today scripts/env.sh expects the layout of the build machine.

# 4. Set the environment, then run the checks CI runs: format, lints, unit tests, Foundry tests.
source scripts/env.sh
make ci

# 5. Build the five guests in the pinned image (image ids are reproducible) and note the ids.
make guests-docker

# 6. Run a local LEZ sequencer and the end-to-end tests.
make seq-dev-start
make deploy-local
cargo test -p e2e_chain --test e2e_chain -- --ignored --nocapture --test-threads 1      # Init, 3 updates, 1 handoff
cargo test -p e2e_consumer -- --ignored --nocapture --test-threads 1                     # attestation consumed once, replay refused
cargo test -p adversarial                                                                 # fabricated chain and six negative cases

# 7. Reproduce the light-client cost measurement (executor, no proof).
cargo build -p e2e_chain --bin v1_exec
"$CARGO_TARGET_DIR/debug/v1_exec" --all --json evidence/v1/v1-exec.json                  # TODO: confirm flags (W1 notes)

# 8. The cost table and the gate checks. TODO (cost harness track): not implemented yet.
make cost-exec     # TODO
make gate          # TODO
```

If a step fails, the log of the last green run is in `evidence/` once the harness lands (TODO-REFRESH).

## 2. What runs today

| Piece | What it is | Evidence | State on 2026-11-12 |
|---|---|---|---|
| Light client in a LEZ guest | `programs/lc_verify` runs `helios-consensus-core` 0.12.0 unmodified. Pre-checks for quorum and finality in front of it (`docs/TRUST.md` section 7) | `evidence/v1/v1-exec.json` | Measured on 5 real mainnet updates, native and executor results identical |
| Instance program | `programs/attest`: init from a checkpoint, apply updates, write-once header records, attest a claim | `tests/e2e_chain` | In tree. TODO-REFRESH: chain test result on `seq-dev` |
| Inclusion and predicates | `crates/attest_inclusion`: account, storage and log proofs under a record, predicates, statement ids | `crates/attest_inclusion/tests/real_proofs.rs` | In tree, on real mainnet and Sepolia proofs. TODO-REFRESH: pass result |
| Adversarial suite | `tests/adversarial`: fabricated chain and six negative cases, each asserting an exact error code | `tests/adversarial` | Native legs written. On-chain legs pending |
| Reference consumer | `programs/consumer_erc20`: acts once per ERC-20 transfer, replay refused | `tests/e2e_consumer` | In tree. TODO-REFRESH: G3 run |
| Follower daemon | `crates/follower` (`attest-followerd`) and the Logos module `modules/attest_follower_module` | `evidence/g4/follower.jsonl` | In tree. TODO-MEASURE: 24 h run on Sepolia |
| CLI | `crates/attest_cli`: `init`, `update`, `status`, `config`, `attest --dry-run`, `checkpoint verify` | | In tree |
| Vault | `contracts/src/Vault.sol` with Foundry tests, fixed denominations, ETH and ERC-20 | `contracts/test` | In tree. TODO-MEASURE: Sepolia deployment |
| Bridge program and burn proof | `programs/bridge`, `guests/burn_proof`, `tools/bridge_cli` | `docs/bridge/` | In progress. Local round trip uses the DEV-ONLY `bridge_dev_source`. TODO-MEASURE: claim on `seq-dev`, Groth16 proof verified on Sepolia |
| Cost table | Cycles, gas, proof time, state growth | `evidence/cost/` | Planned. TODO-MEASURE |

## 3. Measured so far

Machine: Intel i5-10300H (4 cores, 8 threads), Linux 7.0.0-43, `r0vm` 3.0.5, dev mode (executor only, no proof). Measured 2026-11-12 09:45 UTC. Raw files: `evidence/v1/v1-exec.json` and `v1-exec.log` (TODO-REFRESH: copy from the build machine's `.tmp/w1/evidence/`).

| Step | Update bytes | Signers (of 512) | Result | User cycles |
|---|---|---|---|---|
| period update, learns next committee | 26,914 | 511 | slot 15605824, period 1905 | 76,383,417 |
| period update, handoff | 26,926 | 512 | slot 15613952, period 1906 | 88,212,420 |
| period update, handoff | 26,920 | 511 | slot 15622144, period 1907 | 89,340,805 |
| period update, handoff | 26,922 | 512 | slot 15631808, period 1908 | 88,211,504 |
| finality update | 2,109 | 512 | slot 15637568 | 74,446,829 |

- The executor's `Apply` output (pre and post store, head, record, destination) equals the native result for every step.
- A replay of the last update is rejected with E14.
- The `lc_verify` user ELF is 750,844 bytes. The deployed-ELF cap is 1,966,080 bytes (20 segments of 96 KiB).
- The per-transaction execution cap on LEZ v0.4.0-rc3 is 10,000,000 gas, 1 gas per cycle. One update costs 7.4 to 8.9 times that cap with the unmodified library, so it runs as a privately proven transaction and is applied publicly (`docs/DECISIONS.md` D-08, D-39).
- The cycle counts are about three times the plan's estimate of 28M, which came from other applicants' figures for a similar library. We have not yet broken the cost down by phase. TODO-MEASURE.

## 4. Repository layout

| Path | Contents |
|---|---|
| `crates/attest_core` | Shared types, identifiers, error codes, the Helios glue (`lc.rs`), program rules (`rules.rs`) |
| `crates/attest_inclusion` | Inclusion proofs, predicates, statements |
| `crates/attest_host` | Host library over the LEZ wallet |
| `crates/follower`, `crates/attest_cli` | Follower daemon and CLI |
| `programs/` | `attest`, `lc_verify`, `consumer_erc20`, `bridge` (RISC Zero guests) |
| `guests/burn_proof` | Burn proof guest |
| `contracts/` | Foundry project: `Vault.sol`, stub anchor, tests |
| `modules/` | `attest_follower_module` (Logos core module), `bridge_ui` (skeleton, TODO) |
| `tools/` | `fixtures` (fetch and fabricate), `proofgen`, `cost`, `bridge_cli` |
| `tests/` | `e2e_chain`, `e2e_consumer`, `adversarial` |
| `fixtures/` | Real mainnet and Sepolia light-client data, each file in a manifest with URL, time and sha256 |
| `idl/` | SPEL-shaped IDLs, checked against the message enums by a test |
| `evidence/` | Raw outputs behind every figure. TODO |
| `docs/` | Documents below |
| `proposal/` | Drafts of the two grant proposals |

## 5. Documents

| Document | Purpose | Requirement |
|---|---|---|
| `docs/DECISIONS.md` | Design decision log | 021 S11, 022 S4 |
| `docs/REUSE.md` | What is reused, what is ours, every licence | 021 R6 |
| `docs/TRUST.md` | Trust assumptions of the attestation primitive | 021 S6, VS4, VS5 |
| `docs/OPERATOR.md` | Running the update submitter | 021 S5, S7, U3 |
| `docs/CONSUMER-PATTERN.md` | Recommended Consumer Pattern | 021 S3, S4 |
| `docs/COVERAGE-021.md` | 59 requirements, milestone, evidence | 021 |
| `docs/COVERAGE-022.md` | 68 requirements, milestone, evidence | 022 |
| `docs/bridge/INBOUND.md`, `OUTBOUND.md`, `TRUST.md` | Bridge design, staged outbound leg, privacy analysis | 022 |
| `docs/FILM.md` | Script and shot list for the walkthrough film | |

## 6. Gaps and stated bypasses

These are listed here so that no reader has to find them.

- LEZ testnet 0.4 has no public endpoint yet (expected about 2026-11-30). Everything runs on a local standalone v0.4.0-rc3 sequencer.
- The outbound release uses `StubLezAnchor`, an owner-posted stub. Whoever holds its owner key can make any root final. It stands for the stage-1 optimistic anchor of `docs/bridge/OUTBOUND.md`. RFP-022 F7 cannot be met in full on LEZ v0.4.0-rc3: headers carry no state root and public execution is not proven.
- `bridge_dev_source` (`programs/bridge/src/bin/bridge_dev_source.rs`) is a DEV-ONLY statement source. It forwards any statement unchecked. It exists only for the local anvil round trip (`tools/bridge_cli/e2e_local.sh`), where no light client follows the chain. It is never part of a deployment that holds value. Any bridge result in this repository states which statement source it used. The Sepolia run uses the real attestation through RFP-021.
- The follower's beacon source is recorded in every log line. If the Logos `beacon_api_module` path did not run, the 24 h result says `direct_http`. TODO-MEASURE: state which ran.
- LEZ access is through the `wallet` crate, not Logos's LEZ module, which does not target v0.4 yet.
- The Logos `beacon_api_module` and `verified_proxy_module` have no LICENSE file. We use them only as installed runtime dependencies (`docs/REUSE.md` section 4).
- Path B (sequencer-executed verification split over public transactions) is measured for the monolithic case only. The split is milestone work. TODO-MEASURE.
- Native-ETH consumer, registry and cap libraries on RFP-001/002, relayer, module catalog, release action, macOS, public testnet deployment and the audit are milestone work and are not shown here.
- No code here has been through a third-party audit.
- Before this work, none of the team had shipped a RISC Zero guest, a LEZ program or an Ethereum light client.

## 7. How this was built

The team builds with AI coding agents. A named engineer reviews and tests every change before it is merged, and answers for it. The commit history shows the reviews. TODO-OWNER: confirm this sentence is final (decision D5 in the plan).

## 8. Licence, team, contact

Dual licensed under MIT or Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). Reused components and their licences: `docs/REUSE.md`.

Team: `TEAM.md` (TODO-OWNER: due 2026-11-13 23:59 UTC). Contact: TODO-OWNER (Discord handle and email).
