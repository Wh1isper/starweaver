.PHONY: help
help: ## Show available commands
	@awk 'BEGIN {FS = ":.*##"; printf "Available commands:\n"} /^[a-zA-Z0-9_-]+:.*##/ {printf "  %-24s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

.PHONY: install
install: ## Install repository developer hooks
	@echo "Installing pre-commit hooks"
	@pre-commit install

.PHONY: fmt
fmt: ## Format Rust code
	@echo "Formatting Rust workspace"
	@cargo fmt --all

.PHONY: fmt-check
fmt-check: ## Check Rust formatting
	@echo "Checking Rust formatting"
	@cargo fmt --all -- --check

.PHONY: clippy
clippy: ## Run clippy for all targets and features
	@echo "Running clippy"
	@cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

.PHONY: check
check: ## Run repository quality checks
	@echo "Checking Rust workspace"
	@cargo check --workspace --all-targets --all-features --locked
	@echo "Running clippy"
	@cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

.PHONY: test
test: ## Run workspace tests
	@echo "Running Rust tests"
	@cargo test --workspace --all-targets --all-features --locked

.PHONY: build
build: ## Build the workspace
	@echo "Building Rust workspace"
	@cargo build --workspace --all-targets --all-features --locked

.PHONY: docs-check
docs-check: ## Compile Rust examples from docs
	@echo "Checking docs examples"
	@python3 scripts/check-docs-examples.py

.PHONY: lint
lint: docs-check ## Run pre-commit hooks and docs example checks across the repository
	@echo "Running pre-commit"
	@pre-commit run -a

.PHONY: ci
ci: fmt-check check test docs-check ## Run the same core checks as CI

.PHONY: run-cli
run-cli: ## Run the Starweaver CLI
	@cargo run --package starweaver-cli --bin starweaver --locked
