XTASK = cargo run -p xtask --locked --
CORE_COVERAGE_MIN_LINES ?= 95
AGENT_COVERAGE_MIN_LINES ?= 90
SERVICE_COVERAGE_MIN_LINES ?= 80
PUBLISH_RETRIES ?= 10
PUBLISH_RETRY_DELAY_SECONDS ?= 30
CLI_ARGS ?= $(ARGS)
SW_ARGS ?= $(ARGS)
CLI_DEMO_PROMPT ?= hello
SW_DEMO_PROMPT ?= hello

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

.PHONY: coverage
coverage: ## Generate workspace LCOV coverage report
	@echo "Generating workspace LCOV coverage"
	@mkdir -p target/llvm-cov
	@cargo llvm-cov --workspace --all-features --locked --lcov --output-path target/llvm-cov/lcov.info

.PHONY: coverage-core
coverage-core: ## Run core/model/runtime/tools 95% coverage gate
	@echo "Running core coverage gate ($(CORE_COVERAGE_MIN_LINES)% lines)"
	@$(XTASK) coverage-gate core --threshold $(CORE_COVERAGE_MIN_LINES)

.PHONY: coverage-agent
coverage-agent: ## Run agent SDK 90% coverage gate
	@echo "Running agent SDK coverage gate ($(AGENT_COVERAGE_MIN_LINES)% lines)"
	@$(XTASK) coverage-gate agent --threshold $(AGENT_COVERAGE_MIN_LINES)

.PHONY: coverage-service
coverage-service: ## Run CLI/service 80% coverage gate
	@echo "Running CLI/service coverage gate ($(SERVICE_COVERAGE_MIN_LINES)% lines)"
	@$(XTASK) coverage-gate service --threshold $(SERVICE_COVERAGE_MIN_LINES)

.PHONY: coverage-ci
coverage-ci: coverage-core coverage-agent coverage-service ## Run grouped CI coverage gates

.PHONY: build
build: ## Build the workspace
	@echo "Building Rust workspace"
	@cargo build --workspace --all-targets --all-features --locked

.PHONY: replay-check
replay-check: ## Run model replay and request-parameter compatibility tests
	@echo "Checking model replay fixtures"
	@cargo test -p starweaver-model --test fixture_schema --test replay --test replay_tooling --test request_parameters --test stream_replay --locked

.PHONY: replay-summary
replay-summary: ## Print deterministic model replay fixture coverage summary
	@$(XTASK) summarize-model-fixtures $(ARGS)

.PHONY: record-model-cassette
record-model-cassette: ## Record a model cassette; pass ARGS="request.json --provider openai_chat --output cassette.json"
	@$(XTASK) record-model-cassette $(ARGS)

.PHONY: scrub-model-cassette
scrub-model-cassette: ## Scrub a model cassette; pass ARGS="cassette.json --output cassette.scrubbed.json"
	@$(XTASK) scrub-model-cassette $(ARGS)

.PHONY: import-model-cassette
import-model-cassette: ## Import a scrubbed cassette; pass ARGS="cassette.scrubbed.json"
	@$(XTASK) import-model-cassettes $(ARGS)

.PHONY: cli-examples-check
cli-examples-check: ## Validate CLI configuration examples
	@echo "Checking CLI examples"
	@$(XTASK) check-cli-examples

.PHONY: install-script-check
install-script-check: ## Validate GitHub install and update script semantics
	@echo "Checking install script"
	@$(XTASK) check-install-script

.PHONY: scripts-check
scripts-check: cli-examples-check install-script-check ## Validate repository automation scripts through xtask
	@echo "Checking repository scripts"
	@$(XTASK) check-repository-scripts

.PHONY: cli-smoke
cli-smoke: ## Build release CLI binaries and run product smoke checks
	@echo "Running CLI release smoke"
	@$(XTASK) smoke-cli-release

.PHONY: docs-check
docs-check: ## Compile Rust examples from docs
	@echo "Checking docs examples"
	@$(XTASK) check-docs-examples

.PHONY: docs-build
docs-build: ## Build the static documentation site
	@echo "Building docs site"
	@mdbook build
	@$(XTASK) finalize-docs-site

.PHONY: upversion
upversion: ## Update workspace version; pass VERSION=x.y.z
	@if [ -z "$(VERSION)" ]; then echo "VERSION is required, for example: make upversion VERSION=0.2.0"; exit 1; fi
	@$(XTASK) upversion $(VERSION)
	@cargo check --workspace --all-targets --all-features --locked

.PHONY: publish-dry-run
publish-dry-run: ## Dry-run the root crate publish package
	@$(XTASK) publish-dry-run

.PHONY: publish
publish: ## Publish crates in dependency order
	@PUBLISH_RETRIES=$(PUBLISH_RETRIES) PUBLISH_RETRY_DELAY_SECONDS=$(PUBLISH_RETRY_DELAY_SECONDS) $(XTASK) publish

.PHONY: lint
lint: docs-check ## Run pre-commit hooks and docs example checks across the repository
	@echo "Running pre-commit"
	@pre-commit run -a

.PHONY: ci
ci: fmt-check check replay-check test scripts-check docs-check docs-build ## Run the same core checks as CI

.PHONY: ci-all
ci-all: ci coverage-ci ## Run core CI plus coverage gates

.PHONY: cli
cli: ## Try starweaver-cli; override with ARGS="--help" or ARGS="version"
	@cargo run --package starweaver-cli --bin starweaver-cli --locked -- $(CLI_ARGS)

.PHONY: sw
sw: ## Try sw launcher; override with ARGS="--help" or ARGS="version"
	@cargo run --package starweaver-cli --bin sw --locked -- $(SW_ARGS)

.PHONY: run-cli
run-cli: cli ## Alias for cli
