# Reuse assessment (RFP-021 R6)

RFP-021 Reliability 6 asks for a statement of what is reused, what is rewritten, and the cost and audit-surface consequences of that split. This file is that statement. It also lists the licence of every reused component, read from the file in the pinned source tree on 2026-11-12.

Status: written 2026-11-12, line counts redone on 2026-11-17 at commit `aab23fc`. The dependency set is the one pinned in `Cargo.lock`; licences were read on 2026-11-12 and have not been re-read since.

## 1. The split

| Part | Source | Audit status of the reused code |
|---|---|---|
| Light-client rules: sync-committee signature check, committee handoff, finality branch, execution payload branch | `helios-consensus-core` 0.12.0 from `a16z/helios`, used unmodified (variant V1, `docs/DECISIONS.md` D-05) | **Not audited by itself.** Its README at 0.12.0 says "It has not been audited". Open source, maintained by a16z since 2022, and the core of SP1 Helios and R0VM Helios. The audits of those products (Zellic, 2025-07, Helios 0.8.6; zkSecurity, 2025-04, Helios 0.8.3, library out of scope) did not review it, and zkSecurity recommends a full audit. Used unmodified, so the auditor reads only the functions we call |
| BLS12-381 and SHA-256 inside the guest | RISC Zero forks of `bls12_381` and `sha2`, patched in by `[patch.crates-io]` | Not audited as crates: the `bls12_381` fork's README says it has not been reviewed or audited. The accelerator circuits they call were audited by Veridise (2024), at earlier commits |
| Merkle-Patricia proofs, RLP, keccak | `alloy-trie` 0.9.9, `alloy-rlp` 0.3.17, `alloy-primitives` 1.7.3: the same calls Helios makes in `core/src/execution/proof.rs` | Widely used. No audit claim by us |
| LEZ SDK, wallet, sequencer | LEZ v0.4.0-rc3 crates | The platform |
| Groth16 verification on Ethereum | `risc0-ethereum` v3.0.1 verifier contracts | RISC Zero's code |
| Vault safe transfers, reentrancy guard, roles | OpenZeppelin Contracts v5.7.0 | OpenZeppelin's audits |
| Instance program `attest`, the glue around Helios, header records, inclusion verification entry point, predicates, statement ids, reference consumer, follower, CLI, bridge program, burn guest, vault | Ours | **Not audited. This is the audit surface, together with the Helios functions we call** |

Consequence. We do not rewrite the signature, committee and finality code. The audit covers our instance and inclusion logic and the glue that calls Helios. The glue includes the three pre-checks of `docs/TRUST.md` section 7, which exist because Helios `verify_*` does not enforce the quorum or finality by itself.

Cost of reuse. Helios runs only on the private path, where the 10M cap does not apply. It costs 46.1M to 65.5M user cycles per update, measured on five real mainnet updates (`evidence/cost/cost.md`, `evidence/cost/raw/exec-lc.json`). Of the 46.1M on the finality update, 36.8M is BLS arithmetic inside Helios and the rest is our glue. A hand-written pipeline sized for the public cap costs 11M to 15M cycles by other applicants' figures, not ours. The proposal states this trade (V1, V2, V3 in `docs/DECISIONS.md` D-10).

New code, lines on 2026-11-17 at commit `aab23fc` (command: `find <dir> \( -name '*.rs' -o -name '*.sol' \) | xargs cat | wc -l`, comments and blank lines included, generated SDK sources and `contracts/lib` excluded):

| Directory | Lines |
|---|---|
| `crates/attest_core` | 1,791 |
| `crates/attest_inclusion` | 1,307 |
| `crates/attest_host` | 780 |
| `crates/follower` | 2,187 |
| `crates/attest_cli` | 604 |
| `programs/` (four programs) | 2,513 |
| `guests/` | 159 |
| `tools/` | 3,895 |
| `tests/` | 2,788 |
| `contracts/src` | 500 |
| `contracts/test` | 758 |
| `modules/attest_follower_module/rust-lib` | 376 |

The consensus-critical subset is the number the audit quote needs: `attest_core` `lc.rs` (817) and `rules.rs` (156), the source of `attest_inclusion` without its tests (784), `programs/attest` (94) and `programs/lc_verify` (47). That is 1,898 lines at commit `aab23fc`. The audit quote should recount it on the commit that is frozen for the audit.

## 2. The `ethereum_hashing` pin

The root `Cargo.toml` patches `ethereum_hashing` to `github.com/ncitron/ethereum_hashing`, rev `7ee70944ed4fabe301551da8c447e4f4ae5e6c35`. This is the fork of the Helios author. It is the same patch, at the same revision, as `[patch.crates-io]` in Helios 0.12.0's own workspace (`Cargo.toml:134` at tag `0.12.0`). The fork drops `ring` and uses `sha2` 0.10, so the RISC Zero SHA-256 accelerator applies in the guest. Without it Helios pulls `ring`, which does not build for the zkVM target. Licence: Apache-2.0 (`LICENSE` and `Cargo.toml` in the pinned checkout). Decision: `docs/DECISIONS.md` D-04.

## 3. Licence of every reused component

Our own code is MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`), as both RFPs require. "File" means a licence file exists in the pinned source tree.

| Component | Pin | Licence | Evidence | How used |
|---|---|---|---|---|
| `helios-consensus-core` (a16z/helios) | tag `0.12.0`, commit `2af9b418` | MIT | `LICENSE`, copyright a16z | Cargo git dependency, unmodified |
| `ethereum_hashing` (ncitron fork) | rev `7ee70944` | Apache-2.0 | `LICENSE`, `Cargo.toml` | `[patch.crates-io]` |
| `bls12_381` (risc0/zkcrypto-bls12_381) | tag `bls12_381/v0.8.0-risczero.1`, commit `a3192e1b` | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` | `[patch.crates-io]` |
| `sha2` 0.10.8 (risc0/RustCrypto-hashes) | tag `sha2-v0.10.8-risczero.0`, commit `244dc3b0` | MIT OR Apache-2.0 | `sha2/Cargo.toml`, `sha2/LICENSE-*` | `[patch.crates-io]` |
| `sha2` 0.9.9 (risc0/RustCrypto-hashes) | tag `sha2-v0.9.9-risczero.0`, commit `c2b5d71c` | MIT OR Apache-2.0 | `sha2/LICENSE-*` | `[patch.crates-io]` (Helios uses sha2 0.9) |
| `keccak` 0.2.0 (logos-blockchain/sponges) | rev `3a56e99771be` | Apache-2.0 OR MIT | `keccak/Cargo.toml`, `keccak/LICENSE-*` | `[patch.crates-io]`, the patch LEZ rc3 uses |
| `blst` (risc0/blst) | tag `v0.3.16-risczero.0`, commit `7d1fc3e6` | Apache-2.0 | `LICENSE` | Not used in the prototype (variant V3 only) |
| `risc0-zkvm`, `risc0-build` | 3.0.5 | Apache-2.0 | crate manifests | Cargo dependencies. Advisory GHSA-g6qg-9j8p-7j87 (critical, 2026-11-11) affects versions below 3.1.0. LEZ rc3 pins 3.0.5, so our LEZ-side guests stay on 3.0.5 until LEZ moves. `burn_proof` and vault v3 use 3.1.0 |
| LEZ v0.4.0-rc3 crates (`lee_core`, `lee`, `wallet`, `common`, `sequencer_service_rpc`, `program_loader_core`) | tag `v0.4.0-rc3`, commit `bafd07be` | MIT (file); the workspace manifest says "MIT or Apache-2.0" | `LICENSE` (copyright 2025 Vac); only an MIT file is in the tree | Cargo git dependencies |
| `alloy-trie` | 0.9.9 | MIT OR Apache-2.0 | crate manifest | Dependency of `attest_inclusion` |
| `alloy-rlp` | 0.3.17 | MIT OR Apache-2.0 | crate manifest | Dependency of `attest_inclusion` |
| `alloy-primitives` | 1.7.3 | MIT OR Apache-2.0 | crate manifest | Dependency |
| `risc0-ethereum` | tag `v3.0.1`, commit `365e7b2d` | Apache-2.0 | `LICENSE` | Git submodule in `contracts/lib`. The Groth16 verifier 3.0.0 it ships is affected by the advisory. Vault v3 uses verifier 3.1.0 behind its emergency-stop wrapper |
| OpenZeppelin Contracts | tag `v5.7.0`, commit `cab19933` | MIT | `LICENSE` | Git submodule in `contracts/lib` |
| `forge-std` | v1.17.0 | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` | Git submodule, tests only |
| `logos-module-builder` | 0.3.2, commit `4b799827` | MIT and Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE-v2` | `#rust` template for `attest_follower_module` |
| `logos-logoscore-cli` (`logosctl`) | 0.3.2, commit `ca70bcdd` | MIT and Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE-v2` | Tool for the module runbook |
| Ethereum deposit contract tree algorithm | n/a | CC0 | Ethereum deposit contract source | Algorithm reimplemented in `Vault.sol` and `programs/bridge/src/tree.rs`, no code copied |
| consensus-specs | commit `c3e16117` | CC0 | n/a | Read for `docs/TRUST.md` section 3; no code used |
| Foundry (`forge`, `cast`, `anvil`) | v1.9.0 in CI | MIT OR Apache-2.0 (workspace `Cargo.toml` and `LICENSE-MIT`, `LICENSE-APACHE` at tag v1.9.0) | n/a | Tool, not redistributed |

CI actions, pinned by commit SHA in `.github/workflows/ci.yml`: `actions/checkout` (MIT), `foundry-rs/foundry-toolchain` (Apache-2.0), `taiki-e/install-action` (Apache-2.0), `Swatinem/rust-cache` (LGPL-3.0; a CI-time tool, not linked into any artefact). Licences from the GitHub licence API on 2026-11-12. Choices came from the Code Garage entries `engineering.ci.caching-and-speed`, `web3.evm.foundry` and `security.supplychain.github-actions-hardening`.

## 4. The Logos modules: runtime dependencies only

| Module | Version | LICENSE file | What the repository says |
|---|---|---|---|
| `beacon_api_module` (`logos-co/logos-evm-beacon-api-module`) | v0.2.0, commit `aca138e5` | **None** | The README states no licence |
| `verified_proxy_module` (`logos-co/logos-verified-proxy-module`) | v0.2.0, commit `fa984790` | **None** | The README ends with "MIT / Apache-2.0, matching the Logos workspace", but the repository has no LICENSE file |

Neither repository has a LICENSE file, so we have no written grant we can point to. We therefore use both only as installed runtime dependencies (the `.lgx` package a user installs next to ours). We copy no source from either. `attest_follower_module` declares `beacon_api_module` as a dependency and calls it through `lc_request`. The verified proxy is a stretch dependency; the fallback is direct JSON-RPC, because the proofs are checked in the guest anyway. The same rule holds for the generated example sources the module builder emits: they carry the builder's licence (MIT and Apache-2.0).

Open question for Logos (office hours of 2026-11-13, question 8): which licence applies to these two repositories, and may we propose a LICENSE file by pull request. The fixtures fetcher (`tools/fixtures/fetch.py`) reads the default public provider URLs from `beacon_api_module` v0.2.0 (`rust-lib/src/beacon.rs`, `DEFAULT_PROVIDERS`). That is a list of URLs, which we treat as configuration data. We keep that reading until Logos answers.

## 5. Reference only: nothing copied

| Project | Licence | What we took |
|---|---|---|
| `succinctlabs/sp1-helios` | No LICENSE file | Read for the program split and input layout. Nothing copied |
| `boundless-xyz/r0vm-helios` | No LICENSE file | Read for structure. Not the base: it targets RISC Zero 2.0 and Helios 0.8.3, it targets EVM destination chains with Solidity contracts, and it has no licence file |
| `lez-multisig` (IDL generation, e2e layout) | MIT or Apache-2.0 | Layout ideas only |
| `logos-co/lez-programs` token module | MIT or Apache-2.0 | Shape of a core module |

Earlier applicants' repositories for RFP-021 and RFP-022 were not opened, copied or paraphrased (`docs/DECISIONS.md` D-37).
