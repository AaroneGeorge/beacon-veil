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

# 7. Reproduce the cost table: cycles and gas from the executor, real mainnet fixtures, no proof.
make cost-exec     # writes evidence/cost/cost.json and cost.md

# 8. The gate checks G1 to G5 (G1 and G5 need a running sequencer and a committed tree; see scripts/gate/).
make gate          # TODO-REFRESH: record the result in evidence/gate/summary.json
```

If a step fails, the log of the last green run is in `evidence/` once the harness lands (TODO-REFRESH).

## 2. What runs today

Gate status on 2026-11-12, from `evidence/`:

| Gate | Result | Evidence |
|---|---|---|
| G1 chain (dev-mode half) | PASS: `Init`, three updates with committee handoffs, replay E14, wrong verifier E12, fresh-account submit. 2026-11-12 11:46 UTC | `evidence/g1/chain.json` |
| G1 real proof | Open. TODO-MEASURE | `evidence/g1/real-proof.json` |
| G2 adversarial suite | PASS on the sequencer: 7 of 7 tests, each asserting an exact error code. CI run URL still needed (`evidence/g2/ci-run.txt`, TODO-MEASURE) | `evidence/g2/onchain-20261112T115413Z.log` |
| G3 consumer | PASS: one transfer consumed, identical replay refused with E11 | `evidence/g3/run.json` |
| G4 follower, 24 h | Run A running since 2026-11-12 11:11:59 UTC, `beacon_source` `logos_module`. Result TODO-MEASURE | `evidence/g4/` |
| G5 clean-clone cost reproduction | Open. TODO-MEASURE | `scripts/gate/g5_repro.sh` |

| Piece | What it is | Evidence | State |
|---|---|---|---|
| Light client in a LEZ guest | `programs/lc_verify` runs `helios-consensus-core` 0.12.0 unmodified. Pre-checks for quorum and finality in front of it (`docs/TRUST.md` section 7) | `evidence/cost/cost.md` | Measured on 5 real mainnet updates, native and executor results equal |
| Instance program | `programs/attest`: init from a checkpoint, apply updates, write-once header records, attest a claim | `tests/e2e_chain`, `evidence/g1/chain.json` | Passed on `seq-dev` |
| Inclusion and predicates | `crates/attest_inclusion`: account, storage and log proofs under a record, predicates, statement ids | `crates/attest_inclusion/tests/real_proofs.rs` | In tree, on real mainnet and Sepolia proofs. The consumer track reports 22 tests green |
| Adversarial suite | `tests/adversarial`: fabricated chain and six negative cases | `evidence/g2/` | 7 of 7 on the sequencer |
| Reference consumer | `programs/consumer_erc20`: acts once per ERC-20 transfer, replay refused | `evidence/g3/run.json` | Passed in dev mode |
| Follower daemon | `crates/follower` (`attest-followerd`) and the Logos module `modules/attest_follower_module` | `evidence/g4/` | Run A in progress |
| CLI | `crates/attest_cli`: `init`, `update`, `status`, `config`, `attest --dry-run`, `checkpoint verify` | | In tree |
| Vault | `contracts/src/Vault.sol` with Foundry tests, fixed denominations, ETH and ERC-20, caps, freeze | `contracts/test`, `evidence/bridge/sepolia-k9-part1.json` | Vaults v1 and v2 deployed on Sepolia with deposits. TODO-MEASURE: vault v3 on the 3.1.0 verifier behind its emergency stop |
| Bridge program and burn proof | `programs/bridge`, `guests/burn_proof`, `tools/bridge_cli` | `evidence/bridge/local-e2e-20261112T104701Z/`, `docs/bridge/` | Local round trip passed with the DEV-ONLY `bridge_dev_source`. TODO-MEASURE: claim against the followed Sepolia instance, Groth16 proof verified on Sepolia |
| Cost table | Cycles, gas, state growth | `evidence/cost/cost.md` | Cycles and gas measured or derived. Proof time: TODO-MEASURE |

Deprecated testnet artefacts. Sepolia vaults v1 and v2 use the RISC Zero Groth16 verifier 3.0.0, which advisory GHSA-g6qg-9j8p-7j87 shows to be broken. They are evidence of the deposit path only and not of the release path. TODO-REFRESH: the same note belongs in `evidence/README.md`, which this draft does not edit.

## 3. Measured so far

Machine: Intel i5-10300H (4 cores, 8 threads), Linux 7.0.0-43, `r0vm` 3.0.5, dev mode (executor only, no proof). Generated by `make cost-exec` on 2026-11-12 10:31 UTC. Raw files: `evidence/cost/raw/exec-lc.json` and `exec-lc.log`; table: `evidence/cost/cost.md`.

| Step | Update bytes | Signers (of 512) | Result | User cycles |
|---|---|---|---|---|
| period update, learns next committee | 26,914 | 511 | slot 15605824, period 1905 | 61,178,234 |
| period update, handoff | 26,926 | 512 | slot 15613952, period 1906 | 64,339,205 |
| period update, handoff | 26,920 | 511 | slot 15622144, period 1907 | 65,467,587 |
| period update, handoff | 26,922 | 512 | slot 15631808, period 1908 | 64,338,286 |
| finality update | 2,109 | 512 | slot 15637568 | 46,099,256 |

- The executor's `Apply` output (pre and post store, head, record, destination) equals the native result for every step.
- Other handlers: `Init` 4,928,793 cycles, `Apply` 54,965, `WriteRecord` 24,047, `CheckHeader` 18,585.
- The `lc_verify` user ELF is 755,980 bytes. The deployed-ELF cap is 1,966,080 bytes (20 segments of 96 KiB).
- The per-transaction execution cap on LEZ v0.4.0-rc3 is 10,000,000 gas, 1 gas per cycle. One update costs 4.6 to 6.5 times that cap with the unmodified library, so it runs as a privately proven transaction and is applied publicly (`docs/DECISIONS.md` D-08, D-39). Path A costs 488,776 execution gas per update, 4.9 percent of the cap.
- On the finality update the BLS arithmetic is 36.84M of the 46.1M cycles (`evidence/cost/raw/w1-v1-trace.log`). Variant V2, which drops one repeated subgroup check, costs 37,032,744 cycles on that update. TODO-REFRESH: copy `v2-exec.json` into `evidence/cost/raw/`.
- An earlier run gave 74M to 89M cycles. The cause was our glue code, which we fixed without touching Helios (`docs/DECISIONS.md` D-39).

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
| `modules/` | `attest_follower_module` (Logos core module), `bridge_ui` (skeleton) |
| `tools/` | `fixtures` (fetch and fabricate), `proofgen`, `cost`, `bridge_cli` |
| `tests/` | `e2e_chain`, `e2e_consumer`, `adversarial` |
| `fixtures/` | Real mainnet and Sepolia light-client data, each file in a manifest with URL, time and sha256 |
| `idl/` | SPEL-shaped IDLs, checked against the message enums by a test |
| `evidence/` | Raw outputs behind every figure |
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

- Platform risk: RISC Zero advisory GHSA-g6qg-9j8p-7j87 (critical, 2026-11-11). LEZ v0.4.0-rc3 pins `risc0-zkvm` 3.0.5, which is affected. Our LEZ-side code moves to 3.1.0 when LEZ does. Vault v3 and `burn_proof` use 3.1.0.
- Helios has no audit of its own (`docs/REUSE.md`). The audit scope has to include the functions we call.
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
