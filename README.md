# beacon-veil

beacon-veil is a working prototype for Logos RFP-021 (Trustless Ethereum State Attestation for LEZ) and RFP-022 (Privacy-Preserving Wrapped ERC-20 and Ether Bridge for LEZ). It runs the Ethereum light client from `a16z/helios` 0.12.0, unmodified, inside a RISC Zero 3.0.5 guest on LEZ v0.4.0-rc3 (variant V1, privately proven), and a staged verifier, `lc_verify_v3`, that runs the same rules as three public transactions with no proof (`docs/PUBLIC-PATH.md`). It builds a consumer, a follower daemon and a bridge spike on top of them. Both RFPs are bid together. The project name is beacon-veil.

Status: prototype, 2026-11-17, with the public path added on 2026-11-20. Each result in this file points to a raw file under `evidence/`. Machine paths in those files are written `${BUILD}` (the build folder) and `~` (the home folder); a sha256 recorded for a configuration file is of the file as it ran, before that substitution. Commit ids in tool-written records made before 2026-11-21 name commits of the pre-publication history; ids in text and commands name commits of this repository. Section 6 lists what is not done.

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

# 7. Reproduce the cost table: cycles and gas from the executor, real fixtures, no proof. The guests come from the
#    release set named in guests/IMAGE_IDS.expected (built in the pinned image; the script checks the image ids).
scripts/guests_release.sh d097847b7ee958e51000a1bde7b68f571be750c6    # about 20 min; output in ../guests-release/<commit>
make cost-exec     # finds that set by its commit, writes evidence/cost/cost.json and cost.md (71 rows)

# 8. The gate checks G1 to G5 (G1 and G5 need a running sequencer and a committed tree; see scripts/gate/).
make gate          # writes evidence/gate/summary.json; GATE_ARGS=--quick re-checks the committed evidence instead of re-running
```

If a step fails, compare with the logs of the green runs in `evidence/` (`evidence/g1/chain.log`, `evidence/g3/run.log`, `evidence/g5/repro.log`). The result of `make gate` is not committed: the per-gate evidence in section 2 is what backs the table. `scripts/bootstrap.sh` was written for the build machine and a clean clone. The G5 run (`evidence/g5/`) did not use it, and no run of it on another machine is recorded.

## 2. What runs today

Gate status on 2026-11-17 for G1 to G5 (all pass), with the public-path and differential rows of 2026-11-20, the later follower and chain-route rows of 2026-11-22, and the tampered-update and public-testnet rows of 2026-11-23, from `evidence/`:

| Gate | Result | Evidence |
|---|---|---|
| G1 chain (dev-mode half) | PASS: `Init`, three updates with committee handoffs, replay E14, wrong verifier E12, fresh-account submit | `evidence/g1/chain.json` |
| G1 real proof (V1) | PASS: finality update 15637644, unmodified Helios (V1), proven for real on a dev-mode-off sequencer, block 1784. 20,060 s (5.57 h) on an i5-10300H, 9.259 GiB peak | `evidence/g1/real-proof.json` |
| G1 real proof (V2) | PASS: the same finality update with the V2 verifier (`lc_verify_v2`), proven for real, block 3111. 16,064 s (4.46 h), 9.24 GiB peak | `evidence/g1/real-proof-v2.json` |
| G2 adversarial suite | PASS: 7 of 7 tests on the sequencer, each asserting an exact error code, and green CI on the public repository, run 45036470466 on commit 233d4f5 (formatting and lints, Rust tests, guest build with image-id check, Foundry tests, read-only endpoint tests, the `lez_chain` workspace); the commit after it changes only the CI record and documentation | `evidence/g2/onchain-20261112T115413Z.log`, `evidence/g2/ci-run.txt` |
| G3 consumer | PASS: one transfer consumed, identical replay refused with E11 | `evidence/g3/run.json` |
| G3 real proof | PASS: a private `Attest` of a mainnet USDT transfer proven for real, 2,120.6 s (35.3 min), 9.207 GiB, block 1949, replay refused with E11. The sequencer excerpt in the JSON was filled by hand from the raw log, see the note | `evidence/g3/real-proof.json`, `evidence/g3/NOTE-real-proof.txt` |
| G4 follower, 24 h | PASS on two runs, both following the Sepolia beacon chain (chain_id 11155111) through the Logos `logos_module` beacon source, with proofs in RISC Zero dev mode. Run B (primary): 34.49 h, 22 of 22 submitted updates applied, one live sync-committee handoff (period 1411 to 1412), longest gap between log lines 354 s, no error lines. Run A (backup): 35.8 h, 23 of 23 applied, one handoff, one transient `source_error` (a 20 s timeout of the module call). Each run had one process id, no restart and no manual help. Both stopped with a clean shutdown at 2026-11-13 23:00 UTC for a host reboot | `evidence/g4/run-b/g4-summary.json` (primary), `evidence/g4/run-a/g4-summary.json` |
| G4 follower on mainnet (run C), private path | PASS: 105.1 h following the Ethereum mainnet beacon chain (chain_id 1) through the Logos `logos_module` beacon source, 66 of 66 submitted updates applied, 4 live sync-committee handoffs, one process id, no restart, longest gap between log lines 320.6 s, 5 transient `source_error` lines. Path A (private submitter) on a local standalone sequencer with `RISC0_DEV_MODE=1`: each update runs the release guests' full checks in the executor and lands with a dev-mode receipt, not a real proof, so the run shows the follow-and-submit loop on mainnet data, not proving cost. The summary is a snapshot taken on 2026-11-22 of a run that keeps going | `evidence/g4-mainnet/g4-summary.json`, `evidence/g4-mainnet/README.md` |
| G4 followers on the public path (runs D and E) | PASS on both: run D follows mainnet for 26.10 h, 17 of 17 updates applied, 1 handoff; run E follows Sepolia for 26.08 h, 51 of 51 applied, 2 handoffs. Each update is three public transactions (`Begin`, `Miller`, `Finish`) to `lc_verify_v3` on `seq-pub` with `RISC0_DEV_MODE` unset and no proof: 51 and 153 transactions, none reverted, at most 5,765,201 cycles in one step (57.7 percent of the 10M cap). One process id each, no error lines, no gap between log lines over 302 s. Run 1 of both stopped at 22.2 h when an operating-system update restarted the service that hosted them; it is kept as recorded, apart from run 2, and the hours are never summed | `evidence/g4-public/mainnet/g4-summary.json`, `evidence/g4-public/sepolia/g4-summary.json`, `evidence/g4-public/README.md` |
| Public path (`lc_verify_v3`), on a standalone sequencer with dev mode unset | PASS: 30 transactions, 9 each on mainnet, Sepolia (Gloas) and Hoodi data and 3 on a fabricated update signed by exactly 342 of 512 members. Every update is 3 public transactions and the head equals V1's after each. Largest step 6,327,247 gas, 63.3 percent of the 10M cap; largest on real data 5,765,037, 57.7 percent. Whole update 12,602,313 to 16,471,213 gas. No proof is made. 7 refusals as real transactions, head unchanged after each. Measured public execution, not a public testnet. The same verifier in private mode was also proven for real on one mainnet update (section 3) | `evidence/public-path/onchain/{mainnet,sepolia,hoodi,floor}/run.json`, `evidence/public-path/onchain/refusals.json`, `evidence/public-path/onchain/deployment.txt`, `docs/PUBLIC-PATH.md`; re-read with `scripts/gate/gp_onchain.sh` |
| Public path, tampered updates refused on chain | PASS: 10 tampered updates from the differential test sent to `lc_verify_v3` on `seq-pub` (dev mode unset) as real public transactions, each refused at the step the program logic predicts (7 at `Begin`, 1 at `Miller`, 2 at `Finish`) with the expected code: finalized header zeroed and empty finalized header (E02, both passed by unmodified Helios `verify_update`), next-committee branch tampered (E05), execution header changed (E15), signature slot not after the attested slot (E01), identity signature, one participation bit cleared and a fabricated-chain update (E03), 341 of 512 signers (E04), replay of an applied update (E14). Each refusing transaction included and reverted, instance state byte-identical before and after every case; one untampered update then applied on each instance (342 of 512 on the fabricated one), head equal to V1's. 23 transactions | `evidence/public-path/refusals-onchain.json`, `docs/PUBLIC-PATH.md` (Tampered updates refused on chain); re-read with `v3_onchain check` |
| Public LEZ testnet 0.3 (minimal run: deployment, `Init`, one update, one refusal) | PASS: on `https://testnet.lez.logos.co`, 2026-11-23, 17 transactions in blocks 85,572 to 85,754. The 0.3 wrappers `lc_verify_v3_v03` and `attest_v03` deployed through the program loader with immutable headers whose image ids equal the reproducible build (`guests/IMAGE_IDS.v03.expected`); one Sepolia instance initialised from checkpoint slot 11,640,832; **one finality update applied as `Begin`, `Miller`, `Finish`** (head slot 11,640,832 to 11,641,504; largest step 5,785,760 cycles); the same `Finish` sent again and refused with exit code 14, the instance unchanged. Each step was charged its simulated cycles. Fees 152,812,464 units, sent in two stages from faucet payments. Re-read from the testnet: 81 checks passed, 0 failed; offline 42 passed; 4 of 4 mutations caught. The sequencer showed the blocks as `Pending` on the Logos Blockchain | `evidence/public-testnet/`, `docs/PUBLIC-TESTNET.md` (Sent); re-read with `lez-v03/scripts/verify.sh` |
| Differential test, Helios 0.12.0 against V1, V2, V3 | PASS: 881 cases (90 valid): 862 from mainnet, Sepolia and Hoodi, and 19 on a fabricated chain, 0 disagreements between V1 and V3; the deployed V3 guest ran all 881 in the executor with 0 mismatches; 44 designed differences, all refusals by V3 | `evidence/diff/report.json`, `evidence/diff/report.md`, `evidence/diff/executor.json` |
| Public path, executor cost per step | `Begin` at most 6,327,326 cycles, `Miller` at most 5,765,197, `Finish` at most 4,378,929 (executor, no proof) | `evidence/public-path/exec-steps.json` (`make exec-v3`) |
| Public read-only endpoint | https://beacon-veil-seq.tailcdec7a.ts.net: 12 read methods of seq-pub, writes refused; the 27 recorded mainnet, Sepolia and Hoodi transactions re-read through it. Live outside proof windows | `evidence/public-path/endpoint/` |
| Bridge outbound, chain route (Sepolia) | Measured once: the real private burn in `seq-pub` block 6588 (dev mode off) proven from LEZ block bytes by guest `burn_chain` (RISC Zero 3.1.0, Groth16, 3,209 s) and released on Sepolia by `VaultChain.releaseChain` through the 3.1.0 emergency stop, 651,764 gas; a release before the tip's 600 s delay refused with `TipNotFinal` and a repeat refused with `AlreadyReleased`, both as mined transactions. Proven: the burn transaction is in block 6588, not listed as reverted, in an unbroken hash chain from pinned block 6583 to tip 6593. Attested, not proven: that the tip is on the real, final LEZ chain (`StubLezTipAnchor`, one key, ours, after 600 s). Not proven: that LEZ executed those blocks correctly. A test vault bound to the same instance, holding only the 0.001 ETH of this release; not a trustless release | `evidence/bridge/chain-anchor/sepolia.json`, `docs/bridge/OUTBOUND.md` |
| Bridge outbound, chain route with a 3-of-4 tip anchor (Sepolia) | Measured once: the same burn, pinned block and tip on a third test vault whose tip anchor is `CommitteeLezAnchor` (3 of 4 fixed keys, no owner, anyone posts the signatures): one Groth16 proof (2,740 s), `attestTip` with 3 signatures 264,343 gas, `releaseChain` 654,147 gas; 2 signatures, a key outside the set, one signer twice, signatures for another block, a repeat attestation, an early release and a repeat release each refused as a mined transaction. A stand-in key set: all four keys are ours and all four signers read one sequencer, so it measures the vault side of a quorum check; it is not a quorum of LEZ's sequencers and does not meet RFP-022's no-trusted-signer rule | `evidence/bridge/committee-anchor/sepolia.json`, `docs/bridge/OUTBOUND.md` |
| G5 clean-clone cost reproduction | PASS: the cost table was regenerated from a clean clone with a cold build (the commit is recorded in `evidence/g5/repro.json`) and equals the committed one, 71 rows (40 of them `lc_verify_v3`), compared with the table committed at that commit; a self-test with one changed cycle count fails as it should. The run used the prebuilt release guest set and did not rebuild the guests in Docker | `evidence/g5/repro.json`, `evidence/g5/repro.log`, `scripts/gate/g5_repro.sh` |

| Piece | What it is | Evidence | State |
|---|---|---|---|
| Light client in a LEZ guest | `programs/lc_verify` runs `helios-consensus-core` 0.12.0 unmodified. Pre-checks for quorum and finality in front of it (`docs/TRUST.md` section 7) | `evidence/cost/cost.md`, `evidence/g1/real-proof.json` | Measured on 5 real mainnet updates, native and executor results equal. One update proven for real with V1 and again with V2 |
| Public update path | `programs/lc_verify_v3`, `crates/attest_core/src/lc_v3.rs`, `crates/bls_blst`: one update as `Begin`, `Miller`, `Finish`, with the 14-day bound and refusals by exit code (`docs/DECISIONS.md` D-53 to D-59) | `evidence/public-path/` | Measured on a standalone sequencer, no proof. Kept current by two followers for 26.1 h each, on mainnet and Sepolia (runs D and E, `evidence/g4-public/`). On the public testnet 0.3 (as 0.3 wrappers): deployed, one instance initialised and one update applied, one refusal (`docs/PUBLIC-TESTNET.md`) |
| Instance program | `programs/attest`: init from a checkpoint, apply updates, write-once header records, attest a claim | `evidence/g1/chain.json` | Passed on `seq-dev` |
| Inclusion and predicates | `crates/attest_inclusion`: account, storage and log proofs under a record, predicates, statement ids | `crates/attest_inclusion/tests/real_proofs.rs` | In tree, on real mainnet and Sepolia proofs. The consumer suite reports 22 tests green |
| Adversarial suite | `tests/adversarial`: fabricated chain and six negative cases | `evidence/g2/` | 7 of 7 on the sequencer |
| Reference consumer | `programs/consumer_erc20`: acts once per ERC-20 transfer, replay refused | `evidence/g3/` | Passed in dev mode and with a real proof of the private `Attest` |
| Follower daemon | `crates/follower` (`attest-followerd`) and the Logos module `modules/attest_follower_module` | `evidence/g4/`, `evidence/g4-mainnet/`, `evidence/g4-public/` | Runs A and B on Sepolia, 34.5 h and 35.8 h, with a live handoff in each (G4). Run C on mainnet, private path with dev-mode receipts, 105.1 h, 66 of 66 applied, 4 handoffs. Runs D and E on the public path with dev mode unset, 26.10 h on mainnet and 26.08 h on Sepolia, 17 and 51 applied |
| CLI | `crates/attest_cli`: `init`, `update`, `status`, `config`, `attest --dry-run`, `checkpoint verify` | | In tree |
| Vault v3 | `contracts/src/Vault.sol`: fixed denominations, ETH and ERC-20, caps, freeze, verifier 3.1.0 behind its emergency stop (L1), open release route (L2) | `evidence/bridge/sepolia-v3.json` | Deployed on Sepolia at `0x380be74d9F3dFeBAaFC5F911CD6A45bFbeE3aD27` with an ETH and a USDT-style deposit |
| Vault v4 | The same contract, `lezProgram` = the bridge program on the dev-mode-off sequencer `seq-pub`, STUB anchor | `evidence/bridge/sepolia-v4.json` | Deployed on Sepolia at `0x376Ec2256630029A237Ade83FD428520523dEc56`; one 0.001 ETH deposit, claimed, burned and released with real proofs |
| Bridge inbound | `programs/bridge` `Claim`, `tools/bridge_cli` | `evidence/bridge/real-claim-v4/`, `evidence/bridge/k9-part2-runA-v2/`, `evidence/bridge/k9-part2-runB-v3/` | Shown in dev mode against both follower runs: real attestation of the vault root, private claim, repeat refused with E11. On vault v4: the private claim proven for real on a dev-mode-off sequencer (1,856.03 s, 9.189 GiB), the wrapped balance moved by exactly 0.001 ETH, the repeat refused with E11 before any proof. Not done yet: a claim to a public destination |
| Bridge outbound | `guests/burn_proof` (RISC Zero 3.1.0), `Vault.release`, `releaseOpen` | `evidence/bridge/groth16-release-v3/`, `evidence/bridge/real-burn-v4/` | Real Groth16 proof (471.3 s) verified on Sepolia through the emergency stop: release 527,859 gas, open release 325,769 gas; those burns ran in dev mode. On vault v4 the burn itself was proven for real (1,811.57 s, 9.187 GiB), then Groth16 (301.67 s) and release, 527,883 gas. The anchor is the owner-posted stub in both: not a trustless release |
| Bridge outbound, chain route | `crates/lez_chain`, `guests/burn_chain` (RISC Zero 3.1.0), `VaultChain.releaseChain`, `StubLezTipAnchor` (STUB) | `evidence/bridge/chain-anchor/` | The real `seq-pub` burn proven from LEZ block bytes: the burn transaction in block 6588, not listed as reverted, in an unbroken hash chain from pinned block 6583 to tip 6593 (1,761,615 user cycles; Groth16 3,209 s for 16 segments, peak 2.33 GiB in the prover's process tree (the final compression runs in RISC Zero's prover container, outside it), on a shared machine); released on Sepolia through the 3.1.0 emergency stop, 651,764 gas, on a test vault bound to the same instance. Attested, not proven: that the tip is on the real, final LEZ chain (one key, ours, after 600 s). Not proven: that LEZ executed the chain correctly. The parser reproduces every block of four sequencers with LEZ's own hash |
| Bridge outbound, chain route, 3-of-4 tip anchor (spike) | `CommitteeLezAnchor`, `DeployCommittee.s.sol`, `tools/bridge_cli/committee_sign.sh` | `evidence/bridge/committee-anchor/` | No owner; a fixed set of 4 test keys, threshold 3 (LEZ's own rule for 4 keys), EIP-712 signatures bound to one anchor on one chain, checked in the contract. One release on a Sepolia test vault and seven refusals mined. Stand-in signers: all four keys are ours, not LEZ's Ed25519 sequencer keys |
| Reproducible guests | Release set `b3a954c` built in `risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a...`; CI checks `guests/IMAGE_IDS.expected` | `evidence/g1/deployment.txt`, `evidence/g2/ci-run.txt` | The on-chain image ids match (`evidence/g1/seq-real-image-check.log`) |
| Cost table | Cycles, gas, state growth, real proof times; section 8: the public path per step, executor and on chain side by side | `evidence/cost/cost.md` (71 rows: 31 for V1, V2 and the consumer, 40 for `lc_verify_v3`), `evidence/cost/prove.md`, `evidence/cost/k4-projection.json` | Measured or derived. Includes the V2 real proof |

Deprecated testnet artefacts. Sepolia vaults v1 and v2 used the RISC Zero Groth16 verifier 3.0.0, which advisory GHSA-g6qg-9j8p-7j87 shows to be broken. Both were frozen on 2026-11-12 and are evidence of the deposit path only (`evidence/bridge/README.md`).

## 3. Measured so far

Machine: Intel i5-10300H (4 cores, 8 threads), 15 GiB RAM, 11 GiB swap, Linux 7.0.0-43, `r0vm` 3.0.5. The cycle table is executor-only, generated by `make cost-exec` on 2026-11-21 from the release guest set d097847 (the rows below did not change from the 12 Nov table). Raw files: `evidence/cost/raw/exec-lc.json` and `exec-lc.log`; table: `evidence/cost/cost.md`.

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
| V3 (`lc_verify_v3` `VerifyOnce`, private mode), mainnet finality update 15,704,320, dev mode off (block 7249) | 7,413 s (2.06 h), peak 9.21 GiB, 12,528,311 user cycles, 13 segments; head equals the native result | `evidence/public-path/real-proof.json` |
| `burn_proof` Groth16 on RISC Zero 3.1.0 | 471.3 s, peak 2,419,024 KiB, 211,726 user cycles, 260-byte seal | `evidence/bridge/groth16-release-v3/record.json` |
| Private bridge `Claim` on `seq-pub`, dev mode off (block 6467) | 1,856.03 s (30.9 min), peak 9.189 GiB, witness set 239,202 bytes | `evidence/bridge/real-claim-v4/record.json` |
| Private bridge `burn` on `seq-pub`, dev mode off (block 6588) | 1,811.57 s (30.2 min), peak 9.187 GiB | `evidence/bridge/real-burn-v4/record.json` |
| `burn_proof` Groth16 for the vault v4 release | 301.67 s, peak 2.306 GiB, 260-byte seal; release 527,883 gas | `evidence/bridge/real-burn-v4/record.json` |
| `burn_chain` Groth16 on RISC Zero 3.1.0 for the chain route (the same burn, blocks 6583 to 6593) | 3,209.5 s (53.5 min), 16 segments of 2^18 cycles, 1,761,615 user cycles, peak 2.33 GiB in the prover's process tree on a shared machine (the final compression runs in RISC Zero's prover container, outside it); `releaseChain` 651,764 gas | `evidence/bridge/chain-anchor/sepolia.json` |
| One po2-20 segment, 3 runs | mean 363.0 s, 346.2 microseconds per padded cycle | `evidence/cost/k4-projection.json` |

The K4 projection for the V1 finality update was 6.08 to 7.01 hours, an estimate. The measured 5.57 hours is below it. The V2 update took 4.46 hours, 20 percent less than V1 (16,064 s against 20,060 s).

## 4. Repository layout

| Path | Contents |
|---|---|
| `crates/attest_core` | Shared types, identifiers, error codes, the Helios glue (`lc.rs`), program rules (`rules.rs`) |
| `crates/attest_inclusion` | Inclusion proofs, predicates, statements |
| `crates/attest_host` | Host library over the LEZ wallet |
| `crates/follower`, `crates/attest_cli` | Follower daemon and CLI |
| `programs/` | `attest`, `lc_verify`, `lc_verify_v3`, `consumer_erc20`, `bridge` (RISC Zero guests) |
| `crates/lez_chain` | LEZ block parser, hash chain and burn statement for `guests/burn_chain` (its own workspace) |
| `crates/bls_blst` | Small layer over the `blst` BLS12-381 library (RISC Zero fork) for `lc_verify_v3` |
| `guests/burn_proof`, `guests/burn_chain` | Burn proof guests: the root route, and the chain route from LEZ block bytes |
| `contracts/` | Foundry project: `Vault.sol`, stub anchor, tests |
| `modules/` | `attest_follower_module` (Logos core module), `bridge_ui` (skeleton) |
| `tools/` | `fixtures` (fetch and fabricate), `proofgen`, `cost`, `bridge_cli`, `seq_read` (read-only endpoint) |
| `tests/` | `e2e_chain`, `e2e_consumer`, `adversarial` |
| `fixtures/` | Real mainnet and Sepolia light-client data, each file in a manifest with URL, time and sha256 |
| `idl/` | SPEL-shaped IDLs, checked against the message enums by a test |
| `evidence/` | Raw outputs behind every figure |
| `docs/` | Documents below |

## 5. Documents

| Document | Purpose | Requirement |
|---|---|---|
| `docs/DECISIONS.md` | Design decision log | 021 S11, 022 S4 |
| `docs/REUSE.md` | What is reused, what is ours, every licence | 021 R6 |
| `docs/TRUST.md` | Trust assumptions of the attestation primitive | 021 S6, VS4, VS5 |
| `docs/OPERATOR.md` | Running the update submitter, including public mode | 021 S5, S7, U3 |
| `docs/PUBLIC-PATH.md` | The public path: three steps, measured numbers, refusals, how to re-run | 021 F1, P2 |
| `docs/CONSUMER-PATTERN.md` | Recommended Consumer Pattern | 021 S3, S4 |
| `docs/COVERAGE-021.md` | 59 requirements, milestone, evidence | 021 |
| `docs/COVERAGE-022.md` | 68 requirements, milestone, evidence | 022 |
| `docs/bridge/INBOUND.md`, `OUTBOUND.md`, `TRUST.md` | Bridge design, staged outbound leg, privacy analysis | 022 |

## 6. Gaps and stated bypasses

These are listed here so that no reader has to find them.

- Platform risk: RISC Zero advisory GHSA-g6qg-9j8p-7j87 (critical, 2026-11-11). LEZ v0.4.0-rc3 pins `risc0-zkvm` 3.0.5, which is affected. Our LEZ-side code moves to 3.1.0 when LEZ does. Vault v3 and `burn_proof` use 3.1.0.
- Helios has no audit of its own (`docs/REUSE.md`). The audit scope has to include the functions we call.
- LEZ testnet 0.4 has no public endpoint yet (expected about 2026-11-30). Everything runs on a local standalone v0.4.0-rc3 sequencer. The public update path is also ported to the public LEZ testnet 0.3 (`v0.3.2`) as thin 0.3 wrappers around the same verifier code (`lez-v03/`); on that testnet both wrapper programs are deployed, one Sepolia instance is initialised and one finality update is applied, and a repeated `Finish` is refused (17 transactions, 2026-11-23, `docs/PUBLIC-TESTNET.md`). That is one update, not a follower kept current there.
- The outbound release uses `StubLezAnchor`, an owner-posted stub. Whoever holds its owner key can make any root final. The chain route narrows it to one LEZ block hash (`StubLezTipAnchor`), which is still posted by one key, ours: whoever holds that key can make any tip final. On one Sepolia test vault the tip is attested by `CommitteeLezAnchor` instead: 3 of 4 fixed keys, no owner; all four keys are ours, so it is still one party and still a set of signers. The two stubs stand for the stage-1 optimistic anchor of `docs/bridge/OUTBOUND.md`; `CommitteeLezAnchor` is a stand-in for its stage 3. RFP-022 F7 cannot be met in full on LEZ v0.4.0-rc3: headers carry no state root and public execution is not proven.
- `bridge_dev_source` (`programs/bridge/src/bin/bridge_dev_source.rs`) is a DEV-ONLY statement source. It forwards any statement unchecked. It exists only for the local anvil round trip (`tools/bridge_cli/e2e_local.sh`), where no light client follows the chain. It is never part of a deployment that holds value. Any bridge result in this repository states which statement source it used. The Sepolia run uses the real attestation through RFP-021.
- The follower's beacon source is recorded in every log line. Runs A to E all report `logos_module`, so the Logos `beacon_api_module` path ran (`evidence/g4/run-a/g4-summary.json`, `evidence/g4/run-b/g4-summary.json`, `evidence/g4-mainnet/g4-summary.json`, `evidence/g4-public/`). Runs A, B and C ran with `RISC0_DEV_MODE=1` and runs D and E make no proof, so the follower runs show the submission path and the module path, not proving cost.
- The private-path follower runs (A and B on Sepolia, 34 to 36 hours; C on mainnet, 105.1 hours) ran against local standalone sequencers with proofs in RISC Zero dev mode. They show that the submission loop and live committee handoffs work over days. They do not show proving time over a long run. Real proofs were run as separate jobs (section 3). The public-path runs D and E (`evidence/g4-public/`) make no proof at all: each update is three public transactions on `seq-pub`.
- LEZ access is through the `wallet` crate, not Logos's LEZ module, which does not target v0.4 yet.
- The Logos `beacon_api_module` and `verified_proxy_module` have no LICENSE file. We use them only as installed runtime dependencies (`docs/REUSE.md` section 4).
- The public path (Path B, `lc_verify_v3`) is measured on a standalone rc3 sequencer with dev mode unset, as 30 public transactions with no proof (`docs/PUBLIC-PATH.md`). On a public Logos testnet it has applied one update, as the 0.3 port on testnet 0.3 (`docs/PUBLIC-TESTNET.md`); no follower runs there, and testnet 0.4, the target, has no endpoint yet. Two followers have kept instances on it current unattended for a day: 26.1 h on mainnet and 26.1 h on Sepolia, 17 and 51 updates applied, each update three public transactions (`evidence/g4-public/`). Its BLS layer is the `blst` fork of RISC Zero, whose changes have no audit statement that we found (`docs/REUSE.md`). The 14-day bound is enforced by `lc_verify_v3`, not by V1 (`docs/TRUST.md` section 2.2).
- Not done yet: a claim to a public destination (`evidence/bridge/README.md`). The private claim proven for real (`evidence/bridge/real-claim-v4/`) checked a record that the Sepolia instance on `seq-pub` wrote through the public path, with no proof.
- Native-ETH consumer, registry and cap libraries on RFP-001/002, relayer, module catalog, release action, macOS, deployment on testnet 0.4 and the audit are milestone work and are not shown here.
- No code here has been through a third-party audit.
- Before this work, none of the team had shipped a RISC Zero guest, a LEZ program or an Ethereum light client.

## 7. Licence, team, contact

Dual licensed under MIT or Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). Reused components and their licences: `docs/REUSE.md`.

Team: the members are listed in the grant proposal filed at github.com/logos-co/rfp. There is no `TEAM.md` in this repository. Contact: open an issue at https://github.com/AaroneGeorge/beacon-veil. A Discord handle and an email address are not listed here.
