.PHONY: help build test check fmt clippy wasm coverage audit deny clean install-hooks deploy-local

# Default target: show help
.DEFAULT_GOAL := help

# Color output
BLUE := \033[0;34m
GREEN := \033[0;32m
NC := \033[0m

help: ## Show this help message
	@echo "$(BLUE)Stellabill Contracts — Development Workflow$(NC)"
	@echo ""
	@echo "$(GREEN)Usage:$(NC) make [target]"
	@echo ""
	@echo "$(GREEN)Targets:$(NC)"
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | sed 's/: */:/g' | awk 'BEGIN {FS=":.*?## "}; {printf "  %-20s %s\n", $$1, $$2}'
	@echo ""

# ── Build targets ───────────────────────────────────────────────────────────

build: ## Build the workspace (debug)
	cargo build

build-release: ## Build the workspace (release)
	cargo build --release

wasm: ## Build contract WASM target (wasm32-unknown-unknown)
	cargo build --target wasm32-unknown-unknown

wasm-release: ## Build contract WASM target (release)
	cargo build --target wasm32-unknown-unknown --release

# ── Test targets ────────────────────────────────────────────────────────────

test: ## Run all unit tests
	cargo test --all

test-verbose: ## Run all tests with output
	cargo test --all -- --nocapture

test-perf: ## Run query performance budget tests (with output)
	cargo test -p subscription_vault --test query_performance -- --nocapture

test-gas: ## Run gas and storage budget regression tests
	cargo test -p subscription_vault --test gas_budget -- --nocapture

# ── Code quality targets ─────────────────────────────────────────────────────

fmt: ## Format code with rustfmt
	cargo fmt --all

fmt-check: ## Check code formatting without changes
	cargo fmt --all -- --check

clippy: ## Run linter (clippy)
	cargo clippy --all-targets -- -D warnings

check: fmt-check clippy test ## Run all checks: format, lint, and tests

# ── Coverage and security ────────────────────────────────────────────────────

coverage: ## Generate code coverage report (lcov)
	@command -v cargo-llvm-cov >/dev/null 2>&1 || cargo install cargo-llvm-cov --locked
	cargo llvm-cov --workspace --lcov --output-path lcov.txt

audit: ## Run cargo audit for known security advisories
	@command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked
	cargo audit

deny: ## Run cargo deny (license, advisory, source, ban checks)
	@command -v cargo-deny >/dev/null 2>&1 || cargo install cargo-deny --locked
	cargo deny check

security: audit deny ## Run all security checks: audit and deny

# ── Setup targets ───────────────────────────────────────────────────────────

install-hooks: ## Install Git pre-commit hooks (formatting and linting)
	./scripts/install_git_hooks.sh

verify-hooks: ## Verify Git hooks installation without modifying
	./scripts/install_git_hooks.sh --check

# ── Deployment targets ──────────────────────────────────────────────────────

deploy-local: ## Deploy contract locally (builds, starts Docker network, deploys, initializes)
	./scripts/deploy_local.sh

deploy-local-skip-build: ## Deploy locally, reusing existing WASM build
	./scripts/deploy_local.sh --skip-build

deploy-local-skip-smoke: ## Deploy locally without running smoke tests
	./scripts/deploy_local.sh --skip-smoke

# ── Maintenance targets ──────────────────────────────────────────────────────

clean: ## Clean build artifacts and cache
	cargo clean

clean-all: clean ## Deep clean (includes test artifacts)
	rm -rf target/
	rm -rf .deploy-state

.PHONY: help build build-release wasm wasm-release test test-verbose test-perf test-gas fmt fmt-check clippy check coverage audit deny security install-hooks verify-hooks deploy-local deploy-local-skip-build deploy-local-skip-smoke clean clean-all
