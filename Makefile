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

.PHONY: check
check: ## Type-check the entire workspace (faster than a full build)
	$(CARGO) check --all-targets

.PHONY: test
test: ## Run all tests (all features, so the grammers-backed code is covered)
	$(CARGO) test --all --all-features

.PHONY: audit
audit: ## Audit dependencies for security vulnerabilities (requires cargo-audit)
	$(CARGO) audit

# --- CI / Pre-commit ---
.PHONY: ci
ci: fmt-check lint test build-release ## Run all checks required for CI
	@echo "✅ CI checks passed!"

.PHONY: clean
clean: ## Remove the target directory
	$(CARGO) clean

.PHONY: update
update: ## Update dependencies
	$(CARGO) update
