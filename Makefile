# beacon-veil Makefile.
# Every recipe sources scripts/env.sh (shared target dir, 4 jobs, RISC0_DEV_MODE=1, TMPDIR on disk) and runs niced.

SHELL := /usr/bin/env bash
.SHELLFLAGS := -eu -o pipefail -c
ENV := source scripts/env.sh &&
NICE := nice -n 15
GUESTS := attest lc_verify consumer_erc20 bridge bridge_dev_source burn_proof

.PHONY: help fmt fmt-check clippy test test-host guests guests-docker forge-test test-seq-read ci seq-dev-start seq-dev-stop \
        deploy-local cost-exec cost-prove bench-bls exec-v3 gate clean-guests

help: ## List targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

fmt: ## Format Rust and Solidity
	$(ENV) cargo fmt --all
	$(ENV) cd contracts && forge fmt

fmt-check: ## Check formatting (CI)
	$(ENV) cargo fmt --all -- --check
	$(ENV) cd contracts && forge fmt --check

clippy: ## Clippy on every member, warnings are errors (guest build skipped)
	$(ENV) RISC0_SKIP_BUILD=1 $(NICE) cargo clippy --workspace --all-targets -- -D warnings

test: ## Unit and integration tests in dev mode (builds guests with the local rzup toolchain), and the V3 tests CI runs
	$(ENV) $(NICE) cargo nextest run --workspace --no-fail-fast
	$(ENV) RISC0_SKIP_BUILD=1 $(NICE) cargo nextest run -p e2e_chain --features v3 --no-fail-fast

test-host: ## Tests without building guests, and the V3 tests CI runs
	$(ENV) RISC0_SKIP_BUILD=1 $(NICE) cargo nextest run --workspace --no-fail-fast
	$(ENV) RISC0_SKIP_BUILD=1 $(NICE) cargo nextest run -p e2e_chain --features v3 --no-fail-fast

guests: ## Build every guest with the local rzup toolchain; copy .bin files to $$B/target/guests (lc_verify_v3: scripts/build_guest.sh with $$BV_V3_CARGO_CONFIG)
	$(ENV) $(NICE) cargo build -p guest_methods && scripts/collect_guests.sh $(GUESTS)
	$(ENV) GUEST_CARGO_CONFIG="$$BV_V3_CARGO_CONFIG" $(NICE) scripts/build_guest.sh programs/lc_verify_v3/Cargo.toml lc_verify_v3 "$$CARGO_TARGET_DIR/guests"

guests-docker: ## Build the guests of crates/guest_methods in the pinned image r0.1.91.1 (lc_verify_v3 and the release set: scripts/guests_release.sh)
	$(ENV) BV_GUEST_DOCKER=1 $(NICE) cargo build -p guest_methods && scripts/collect_guests.sh $(GUESTS)

forge-test: ## Foundry tests
	$(ENV) cd contracts && $(NICE) forge test

test-seq-read: ## Tests of the read-only endpoint (tools/seq_read: policy, dialer, relay; loopback only)
	python3 -I tools/seq_read/test_policy.py

ci: fmt-check clippy test forge-test test-seq-read ## Everything CI runs (guest docker build is a separate CI job)

seq-dev-start: ## Start the dev-mode sequencer (RPC 3040)
	scripts/seq.sh dev start

seq-dev-stop: ## Stop the dev-mode sequencer
	scripts/seq.sh dev stop

deploy-local: guests ## Deploy attest and lc_verify to seq-dev, write deploy/ids.toml
	scripts/deploy_local.sh dev

cost-exec: ## deterministic cycles and gas (executor, real fixtures) -> evidence/cost/cost.{json,md}
	tools/cost/cost_exec.sh

bench-bls: ## BLS phases and whole-update variants, zkcrypto vs blst (executor) -> evidence/public-path/bench (BENCH_ROUTE=docker for the pinned image)
	tools/cost/bench_bls.sh

exec-v3: ## lc_verify_v3 per step in the executor (COST_BINS=<release set> for deployed ELFs) -> evidence/public-path/exec-steps.json
	tools/cost/exec_v3.sh

cost-prove: ## proof-window job, JOB=calib-seg20|calib-ppe|g1-real (needs BV_PROOF_WINDOW_OK=1); no JOB: summary
	tools/cost/cost_prove.sh $(JOB) $(ARGS)

gate: ## gate checks G1 to G5 -> evidence/gate/summary.json (GATE_ARGS=--quick re-checks evidence only)
	scripts/gate/gate.sh $(GATE_ARGS)
