# beacon-veil Makefile (skeleton, owner W0). Targets `cost-exec`, `cost-prove`, `gate` belong to W6.
# Every recipe sources scripts/env.sh (shared target dir, 4 jobs, RISC0_DEV_MODE=1, TMPDIR on disk) and runs niced.

SHELL := /usr/bin/env bash
.SHELLFLAGS := -eu -o pipefail -c
ENV := source scripts/env.sh &&
NICE := nice -n 15
GUESTS := attest lc_verify consumer_erc20 bridge bridge_dev_source burn_proof

.PHONY: help fmt fmt-check clippy test test-host guests guests-docker forge-test ci seq-dev-start seq-dev-stop \
        deploy-local cost-exec cost-prove gate clean-guests

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

test: ## Unit and integration tests in dev mode (builds guests with the local rzup toolchain)
	$(ENV) $(NICE) cargo nextest run --workspace --no-fail-fast

test-host: ## Tests without building guests
	$(ENV) RISC0_SKIP_BUILD=1 $(NICE) cargo nextest run --workspace --no-fail-fast

guests: ## Build every guest with the local rzup toolchain; copy .bin files to $$B/target/guests
	$(ENV) $(NICE) cargo build -p guest_methods && scripts/collect_guests.sh $(GUESTS)

guests-docker: ## Build every guest in the pinned image r0.1.91.1 (reproducible image ids)
	$(ENV) BV_GUEST_DOCKER=1 $(NICE) cargo build -p guest_methods && scripts/collect_guests.sh $(GUESTS)

forge-test: ## Foundry tests
	$(ENV) cd contracts && $(NICE) forge test

ci: fmt-check clippy test forge-test ## Everything CI runs (guest docker build is a separate CI job)

seq-dev-start: ## Start the dev-mode sequencer (RPC 3040)
	scripts/seq.sh dev start

seq-dev-stop: ## Stop the dev-mode sequencer
	scripts/seq.sh dev stop

deploy-local: guests ## Deploy attest and lc_verify to seq-dev, write deploy/ids.toml
	scripts/deploy_local.sh dev

cost-exec: ## (W6) deterministic cycles and gas (executor, real fixtures) -> evidence/cost/cost.{json,md}
	tools/cost/cost_exec.sh

cost-prove: ## (W6) proof-window job, JOB=calib-seg20|calib-ppe|g1-real (needs BV_PROOF_WINDOW_OK=1); no JOB: summary
	tools/cost/cost_prove.sh $(JOB) $(ARGS)

gate: ## (W6) gate checks G1 to G5 -> evidence/gate/summary.json (GATE_ARGS=--quick re-checks evidence only)
	scripts/gate/gate.sh $(GATE_ARGS)
