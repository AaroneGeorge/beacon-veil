# beacon-veil

beacon-veil is a working prototype for Logos RFP-021 (Trustless Ethereum State Attestation for LEZ) and RFP-022 (Privacy-Preserving Wrapped ERC-20 and Ether Bridge for LEZ). It runs the Ethereum light client from `a16z/helios` 0.12.0, unmodified, inside a RISC Zero 3.0.5 guest on LEZ v0.4.0-rc3, and builds a consumer, a follower daemon and a bridge spike on top of it. Both RFPs are bid together. The project name is beacon-veil.

Status: prototype, 2026-11-17. Each result in this file points to a raw file under `evidence/`. Section 6 lists what is not done.

## 1. Clean-clone commands

Machine: Linux x86_64, 15 GiB RAM or more, about 20 GB of disk. Everything runs in RISC Zero dev mode (`RISC0_DEV_MODE=1`) unless a step says it proves for real. The commands below are the ones the build used on the build machine described in section 3.

```bash
# 1. Get the code (the pinned contract libraries are git submodules).
git clone --recurse-submodules https://github.com/AaroneGeorge/beacon-veil.git
cd beacon-veil

# 2 and 3. Prerequisites, and LEZ v0.4.0-rc3 built once for the local sequencer (commit bafd07be0701398f9bc435f73ab05e9a7936c301).
# scripts/bootstrap.sh does both. It clones the pinned vendor repositories (scripts/pins.txt) next to the repository,
# installs the pinned toolchains (Rust 1.98.1 from rust-toolchain.toml, rzup components, Foundry v1.9.0) and builds the LEZ
# binaries (about 20 minutes on 4 cores). Docker is also needed. --check reports what is missing and changes nothing;
# --with-sudo lets it run the apt-get step, which it otherwise only prints.
scripts/bootstrap.sh --check
scripts/bootstrap.sh --with-sudo
# By hand, the toolchain part of step 2 is:
#   sudo apt-get install -y build-essential clang libclang-dev libssl-dev pkg-config cmake jq
#   curl -L https://risczero.com/install | bash
#   rzup install cargo-risczero 3.0.5 && rzup install r0vm 3.0.5 && rzup install rust 1.91.1

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
make gate          # writes evidence/gate/summary.json; GATE_ARGS=--quick re-checks the committed evidence instead of re-running
```

If a step fails, compare with the logs of the green runs in `evidence/` (`evidence/g1/chain.log`, `evidence/g3/run.log`, `evidence/g5/repro.log`). The result of `make gate` is not committed: the per-gate evidence in section 2 is what backs the table. `scripts/bootstrap.sh` was written for the build machine and a clean clone. The G5 run (`evidence/g5/`) did not use it, and no run of it on another machine is recorded.

## 2. What runs today

Gate status on 2026-11-17, from `evidence/`. G1 to G5 all pass:

| Gate | Result | Evidence |
|---|---|---|
| G1 chain (dev-mode half) | PASS: `Init`, three updates with committee handoffs, replay E14, wrong verifier E12, fresh-account submit | `evidence/g1/chain.json` |
| G1 real proof (V1) | PASS: finality update 15637644, unmodified Helios (V1), proven for real on a dev-mode-off sequencer, block 1784. 20,060 s (5.57 h) on an i5-10300H, 9.259 GiB peak | `evidence/g1/real-proof.json` |
| G1 real proof (V2) | PASS: the same finality update with the V2 verifier (`lc_verify_v2`), proven for real, block 3111. 16,064 s (4.46 h), 9.24 GiB peak | `evidence/g1/real-proof-v2.json` |
| G2 adversarial suite | PASS: 7 of 7 tests on the sequencer, each asserting an exact error code, and green CI on the staging repository (commit 764acf9, a private repository). The workflow has not yet been run on the public repository | `evidence/g2/onchain-20261112T115413Z.log`, `evidence/g2/ci-run.txt` |
| G3 consumer | PASS: one transfer consumed, identical replay refused with E11 | `evidence/g3/run.json` |
| G3 real proof | PASS: a private `Attest` of a mainnet USDT transfer proven for real, 2,120.6 s (35.3 min), 9.207 GiB, block 1949, replay refused with E11. The sequencer excerpt in the JSON was filled by hand from the raw log, see the note | `evidence/g3/real-proof.json`, `evidence/g3/NOTE-real-proof.txt` |
| G4 follower, 24 h | PASS on two runs, both following the Sepolia beacon chain (chain_id 11155111) through the Logos `logos_module` beacon source, with proofs in RISC Zero dev mode. Run B (primary): 34.49 h, 22 of 22 submitted updates applied, one live sync-committee handoff (period 1411 to 1412), longest gap between log lines 354 s, no error lines. Run A (backup): 35.8 h, 23 of 23 applied, one handoff, one transient `source_error` (a 20 s timeout of the module call). Each run had one process id, no restart and no manual help. Both stopped with a clean shutdown at 2026-11-13 23:00 UTC for a host reboot | `evidence/g4/run-b/g4-summary.json` (primary), `evidence/g4/run-a/g4-summary.json` |
| G5 clean-clone cost reproduction | PASS: the cost table was regenerated from a clean clone (the commit is recorded in `evidence/g5/repro.json`) and equals the committed one, 31 rows. The three guest image ids match. The run used the prebuilt release guest set and did not rebuild the guests in Docker | `evidence/g5/repro.json`, `evidence/g5/repro.log`, `scripts/gate/g5_repro.sh` |

| Piece | What it is | Evidence | State |
|---|---|---|---|
| Light client in a LEZ guest | `programs/lc_verify` runs `helios-consensus-core` 0.12.0 unmodified. Pre-checks for quorum and finality in front of it (`docs/TRUST.md` section 7) | `evidence/cost/cost.md`, `evidence/g1/real-proof.json` | Measured on 5 real mainnet updates, native and executor results equal. One update proven for real with V1 and again with V2 |
| Instance program | `programs/attest`: init from a checkpoint, apply updates, write-once header records, attest a claim | `evidence/g1/chain.json` | Passed on `seq-dev` |
| Inclusion and predicates | `crates/attest_inclusion`: account, storage and log proofs under a record, predicates, statement ids | `crates/attest_inclusion/tests/real_proofs.rs` | In tree, on real mainnet and Sepolia proofs. The consumer track reports 22 tests green |
| Adversarial suite | `tests/adversarial`: fabricated chain and six negative cases | `evidence/g2/` | 7 of 7 on the sequencer |
| Reference consumer | `programs/consumer_erc20`: acts once per ERC-20 transfer, replay refused | `evidence/g3/` | Passed in dev mode and with a real proof of the private `Attest` |
| Follower daemon | `crates/follower` (`attest-followerd`) and the Logos module `modules/attest_follower_module` | `evidence/g4/` | Two runs on Sepolia, 34.5 h and 35.8 h, with a live handoff in each (G4) |
| CLI | `crates/attest_cli`: `init`, `update`, `status`, `config`, `attest --dry-run`, `checkpoint verify` | | In tree |
| Vault v3 | `contracts/src/Vault.sol`: fixed denominations, ETH and ERC-20, caps, freeze, verifier 3.1.0 behind its emergency stop (L1), open release route (L2) | `evidence/bridge/sepolia-v3.json` | Deployed on Sepolia at `0x380be74d9F3dFeBAaFC5F911CD6A45bFbeE3aD27` with an ETH and a USDT-style deposit |
| Bridge inbound | `programs/bridge` `Claim`, `tools/bridge_cli` | `evidence/bridge/k9-part2-runA-v2/`, `evidence/bridge/k9-part2-runB-v3/` | Shown in dev mode against both follower runs: real attestation of the vault root, private claim, repeat refused with E11. Not done yet: a real proof of a private claim, and a claim to a public destination |
| Bridge outbound | `guests/burn_proof` (RISC Zero 3.1.0), `Vault.release`, `releaseOpen` | `evidence/bridge/groth16-release-v3/` | Real Groth16 proof (471.3 s) verified on Sepolia through the emergency stop: release 527,859 gas, open release 325,769 gas. The anchor is the owner-posted stub and the burns ran in dev mode |
| Reproducible guests | Release set `bf95e359` built in `risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a...`; CI checks `guests/IMAGE_IDS.expected` | `evidence/g1/deployment.txt`, `evidence/g2/ci-run.txt` | The on-chain image ids match (`evidence/g1/seq-real-image-check.log`) |
| Cost table | Cycles, gas, state growth, real proof times | `evidence/cost/cost.md` (31 rows), `evidence/cost/prove.md`, `evidence/cost/k4-projection.json` | Measured or derived. Includes the V2 real proof |

Deprecated testnet artefacts. Sepolia vaults v1 and v2 used the RISC Zero Groth16 verifier 3.0.0, which advisory GHSA-g6qg-9j8p-7j87 shows to be broken. Both were frozen on 2026-11-12 and are evidence of the deposit path only (`evidence/bridge/README.md`).

## 3. Measured so far

Machine: Intel i5-10300H (4 cores, 8 threads), 15 GiB RAM, 11 GiB swap, Linux 7.0.0-43, `r0vm` 3.0.5. The cycle table is executor-only, generated by `make cost-exec` on 2026-11-12 20:20 UTC from the reproducible guest set. Raw files: `evidence/cost/raw/exec-lc.json` and `exec-lc.log`; table: `evidence/cost/cost.md`.

| Step | Update bytes | Signers (of 512) | Result | User cycles |
|---|---|---|---|---|
| period update, learns next committee | 26,914 | 511 | slot 15605824, period 1905 | 61,174,483 |
| period update, handoff | 26,926 | 512 | slot 15613952, period 1906 | 64,333,601 |
| period update, handoff | 26,920 | 511 | slot 15622144, period 1907 | 65,461,983 |
| period update, handoff | 26,922 | 512 | slot 15631808, period 1908 | 64,332,682 |
| finality update | 2,109 | 512 | slot 15637568 | 46,095,505 |

- The executor's `Apply` output (pre and post store, head, record, destination) equals the native result for every step.
- Other handlers: `Init` 4,926,875 cycles, `Apply` 54,965, `WriteRecord` 24,047, `CheckHeader` 18,585.
- The `lc_verify` user ELF is 754,956 bytes. The deployed-ELF cap is 1,966,080 bytes (20 segments of 96 KiB).
- The per-transaction execution cap on LEZ v0.4.0-rc3 is 10,000,000 gas, 1 gas per cycle. One update costs 4.6 to 6.5 times that cap with the unmodified library, so it runs as a privately proven transaction and is applied publicly (`docs/DECISIONS.md` D-08, D-39). Path A costs 488,776 execution gas per update, 4.9 percent of the cap.
- On the finality update the BLS arithmetic is 36.84M of the 46.1M cycles (`evidence/cost/raw/w1-v1-trace.log`). Variant V2, which drops one repeated subgroup check, costs 37,032,744 cycles on that update (`evidence/cost/cost.md`).
- An earlier run gave 74M to 89M cycles. The cause was our glue code, which we fixed without touching Helios (`docs/DECISIONS.md` D-39).

Real proofs on the same laptop:

| Job | Result | Raw |
|---|---|---|
| V1 finality update, dev mode off (block 1784) | 20,060 s (5.57 h), peak 9.259 GiB, at most 92,700 KiB swap | `evidence/g1/real-proof.json` |
| V2 finality update, same update, dev mode off (block 3111) | 16,064 s (4.46 h), peak 9.24 GiB, at most 113,712 KiB swap | `evidence/g1/real-proof-v2.json` |
| Private `Attest` of a USDT transfer (block 1949) | 2,120.6 s (35.3 min), peak 9.207 GiB | `evidence/g3/real-proof.json` |
| `burn_proof` Groth16 on RISC Zero 3.1.0 | 471.3 s, peak 2,419,024 KiB, 211,726 user cycles, 260-byte seal | `evidence/bridge/groth16-release-v3/record.json` |
| One po2-20 segment, 3 runs | mean 363.0 s, 346.2 microseconds per padded cycle | `evidence/cost/k4-projection.json` |

The K4 projection for the V1 finality update was 6.08 to 7.01 hours, an estimate. The measured 5.57 hours is below it. The V2 update took 4.46 hours, 20 percent less than V1 (16,064 s against 20,060 s).

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
| `proposal/` | Pointer to the filed combined grant proposal |

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
- The follower's beacon source is recorded in every log line. Both G4 runs report `logos_module`, so the Logos `beacon_api_module` path ran (`evidence/g4/run-a/g4-summary.json`, `evidence/g4/run-b/g4-summary.json`). Both ran with `RISC0_DEV_MODE=1`, so the follower runs show the submission path and the module path, not proving cost.
- The G4 follower runs followed Sepolia, not mainnet, against a local standalone sequencer, with proofs in RISC Zero dev mode. They show that the submission loop and a live committee handoff work over 34 to 36 hours. They do not show proving time over a long run. Real proofs were run as separate jobs (section 3).
- LEZ access is through the `wallet` crate, not Logos's LEZ module, which does not target v0.4 yet.
- The Logos `beacon_api_module` and `verified_proxy_module` have no LICENSE file. We use them only as installed runtime dependencies (`docs/REUSE.md` section 4).
- Path B (sequencer-executed verification split over public transactions) is measured for the monolithic case only. The split is milestone work.
- Not done yet: a real proof of a private bridge `Claim`, and a claim to a public destination (`evidence/bridge/README.md`).
- Native-ETH consumer, registry and cap libraries on RFP-001/002, relayer, module catalog, release action, macOS, public testnet deployment and the audit are milestone work and are not shown here.
- No code here has been through a third-party audit.
- Before this work, none of the team had shipped a RISC Zero guest, a LEZ program or an Ethereum light client.

## 7. Licence, team, contact

Dual licensed under MIT or Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). Reused components and their licences: `docs/REUSE.md`.

Team: the members are listed in the filed grant proposal (see `proposal/README.md` for its link). There is no `TEAM.md` in this repository. Contact: open an issue at https://github.com/AaroneGeorge/beacon-veil. A Discord handle and an email address are not listed here.
