# televim - Makefile
# A portable task runner for the televim project.
# All commands are run with `make <target>`.

# --- Variables ---
CARGO := cargo
# The binary name, derived from the workspace root Cargo.toml
BINARY_NAME := televim

# --- Help ---
.PHONY: help
help: ## Display this help screen
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}'

# --- Development ---
.PHONY: run
run: ## Run the application in debug mode
	$(CARGO) run

.PHONY: watch
watch: ## Run in watch mode (requires cargo-watch: `cargo install cargo-watch`)
	$(CARGO) watch -x run

# --- Build ---
.PHONY: build
build: ## Build the application in debug mode
	$(CARGO) build

.PHONY: build-release
build-release: ## Build the optimized release binary
	$(CARGO) build --release

# --- Measurement ---
# Builds and runs the memory harness, times the release binary, and writes
# target/memory-report.json plus a Markdown table on stdout. The baseline the
# later regression check compares against is docs/memory-baseline.json, which
# this target deliberately does not write: a new baseline is a copy a person
# makes after reading a report. Not part of `ci`, because it takes a release
# build and a real terminal.
.PHONY: measure
measure: ## Measure RSS at idle, startup, input latency, and binary size
	scripts/memory/measure.py

# The regression budget: compares the report `measure` wrote against the stored
# baseline and fails past it. Separate from `measure` because it reads a report
# rather than producing one, so CI can run them as two steps and keep the
# artifact even when the comparison fails.
.PHONY: measure-check
measure-check: ## Check the last measurement against the stored baseline
	scripts/memory/check.py

# A/B compare of the criterion micro-benches. Writes target/bench-compare.json
# and .md. BENCH_MODE=smoke (default, short fixed flags) or full (long fixed
# flags, the numbers a claim may cite). BENCH_ARGS="--save-baseline NAME" saves
# this run; BENCH_ARGS="--baseline NAME" compares against a saved one. Reports
# deltas only: slowness does not fail it, a harness fault does.
BENCH_MODE ?= smoke
.PHONY: bench
bench: ## Run the micro-benches and write the A/B comparison report
	scripts/bench/compare.py --mode $(BENCH_MODE) $(BENCH_ARGS)

# --- Quality Assurance ---
.PHONY: fmt
fmt: ## Format all code in the workspace
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## Check code formatting without modifying files (for CI)
	$(CARGO) fmt --all -- --check

.PHONY: lint
lint: ## Run clippy to catch common mistakes and improve code
	$(CARGO) clippy --all-targets --all-features -- -D warnings

.PHONY: boundary
boundary: ## Assert that only telegram-framework can reach grammers
	@fail=0; \
	for crate in domain proto tui telegram-framework; do \
		if ! tree=$$($(CARGO) tree -p $$crate 2>&1); then \
			echo "error: could not resolve $$crate, so the boundary is unverified"; \
			fail=1; \
			continue; \
		fi; \
		if echo "$$tree" | grep -q grammers; then \
			echo "error: $$crate must not depend on grammers; see crates/proto/src/lib.rs"; \
			fail=1; \
		fi; \
	done; \
	if [ $$fail -ne 0 ]; then exit 1; fi; \
	echo "✅ no grammers outside telegram-framework's live feature"

.PHONY: check
check: ## Type-check the entire workspace (faster than a full build)
	$(CARGO) check --all-targets

.PHONY: test
test: ## Run all tests (all features, so the grammers-backed code is covered)
	$(CARGO) test --all --all-features

.PHONY: audit
audit: ## Audit dependencies for security vulnerabilities (requires cargo-audit)
	$(CARGO) audit

# --- Design ---
# The design project is vendored under design/ and kept in step with the
# OpenDesign project by scripts/design-sync.sh. `check` is part of `ci`, so two
# copies of a screen cannot drift apart unnoticed. See design/README.md.
.PHONY: design-check
design-check: ## Verify the vendored design model, and diff it against OpenDesign
	@cd design && node scripts/check-model.js
	@scripts/design-sync.sh check || { \
		if [ -z "$${OPEN_DESIGN_PROJECT:-}" ] && [ ! -d "$$HOME/Library/Application Support/Open Design/namespaces/release-stable/data/projects/televim-app" ]; then \
			echo "⚠️  the OpenDesign half was SKIPPED: no project on this machine."; \
			echo "    The vendored copy is verified above; only the diff against"; \
			echo "    OpenDesign was not run. Set OPEN_DESIGN_PROJECT to check it."; \
			exit 0; \
		fi; \
		exit 1; \
	}

.PHONY: design-pull
design-pull: ## Copy the OpenDesign project's files into this repo
	scripts/design-sync.sh pull

.PHONY: design-push
design-push: ## Copy this repo's design files into the OpenDesign project
	scripts/design-sync.sh push

.PHONY: design-specimen
design-specimen: ## Rebuild the specimen's frame bodies from the engine
	cd design && node design-system/build-specimen.js

# --- CI / Pre-commit ---
.PHONY: ci
ci: fmt-check lint boundary test design-check build-release ## Run all checks required for CI
	@echo "✅ CI checks passed!"

.PHONY: clean
clean: ## Remove the target directory
	$(CARGO) clean

.PHONY: update
update: ## Update dependencies
	$(CARGO) update
