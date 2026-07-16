XTASK = cargo run -p xtask --locked --
PY_PACKAGE = packages/starweaver-py
PY_SOURCES = $(PY_PACKAGE)/python $(PY_PACKAGE)/tests scripts/check_python_api.py scripts/python_wheel_smoke.py examples/python
PY_DIST_DIR ?= dist/python
CORE_COVERAGE_MIN_LINES ?= 95
AGENT_COVERAGE_MIN_LINES ?= 90
SERVICE_COVERAGE_MIN_LINES ?= 80
PUBLISH_RETRIES ?= 60
PUBLISH_RETRY_DELAY_SECONDS ?= 60
CLI_MAKE_ARGS = $(wordlist 2,$(words $(MAKECMDGOALS)),$(MAKECMDGOALS))
SW_MAKE_ARGS = $(wordlist 2,$(words $(MAKECMDGOALS)),$(MAKECMDGOALS))
CLI_ARGS ?= $(if $(ARGS),$(ARGS),$(CLI_MAKE_ARGS))
SW_ARGS ?= $(if $(ARGS),$(ARGS),$(SW_MAKE_ARGS))

ifneq ($(filter cli sw,$(firstword $(MAKECMDGOALS))),)
%:
	@:
endif

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

.PHONY: architecture-check
architecture-check: ## Enforce product dependency and storage ownership boundaries
	@echo "Checking architecture boundaries"
	@$(XTASK) check-architecture

.PHONY: capability-check
capability-check: ## Validate the capability registry and its implementation evidence
	@echo "Checking capability registry"
	@$(XTASK) check-capabilities

.PHONY: agent-api-check
agent-api-check: ## Validate the reviewed starweaver-agent root/prelude/advanced API snapshot
	@echo "Checking starweaver-agent API snapshot"
	@$(XTASK) check-agent-api

.PHONY: check
check: agent-api-check architecture-check capability-check ## Run repository quality checks
	@echo "Checking Rust workspace"
	@cargo check --workspace --all-targets --all-features --locked
	@echo "Running clippy"
	@cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

.PHONY: test
test: ## Run workspace tests
	@echo "Running Rust tests"
	@cargo test --workspace --all-targets --all-features --locked

.PHONY: coverage
coverage: ## Collect workspace coverage and generate an LCOV report
	@echo "Collecting workspace coverage"
	@cargo llvm-cov clean --workspace
	@cargo llvm-cov --workspace --all-features --locked --no-report
	@$(MAKE) --no-print-directory coverage-report

.PHONY: coverage-report
coverage-report: ## Generate LCOV from previously collected coverage profiles
	@echo "Generating workspace LCOV coverage"
	@mkdir -p target/llvm-cov
	@cargo llvm-cov report --failure-mode all --lcov --output-path target/llvm-cov/lcov.info

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
coverage-ci: ## Collect coverage once and run all grouped CI gates
	@echo "Collecting workspace coverage for grouped gates"
	@cargo llvm-cov clean --workspace
	@cargo llvm-cov --workspace --all-features --locked --no-report
	@echo "Running core coverage gate ($(CORE_COVERAGE_MIN_LINES)% lines)"
	@$(XTASK) coverage-gate core --threshold $(CORE_COVERAGE_MIN_LINES) --report-only
	@echo "Running agent SDK coverage gate ($(AGENT_COVERAGE_MIN_LINES)% lines)"
	@$(XTASK) coverage-gate agent --threshold $(AGENT_COVERAGE_MIN_LINES) --report-only
	@echo "Running CLI/service coverage gate ($(SERVICE_COVERAGE_MIN_LINES)% lines)"
	@$(XTASK) coverage-gate service --threshold $(SERVICE_COVERAGE_MIN_LINES) --report-only

.PHONY: build
build: ## Build the workspace
	@echo "Building Rust workspace"
	@cargo build --workspace --all-targets --all-features --locked

.PHONY: py-sync
py-sync: ## Sync Python dependencies without building workspace packages
	@echo "Syncing Python workspace dependencies"
	@uv sync --all-packages --no-install-workspace --locked

.PHONY: py-version
py-version: py-sync ## Show the Python interpreter selected by uv
	@uv run --no-sync python --version

.PHONY: py-fmt
py-fmt: py-sync ## Format Python package files with ruff
	@echo "Formatting Python package"
	@uv run --no-sync ruff format $(PY_SOURCES)
	@uv run --no-sync ruff check --fix $(PY_SOURCES)

.PHONY: py-api-check
py-api-check: ## Validate classified Python top-level API snapshot
	@echo "Checking Python API snapshot"
	@python3 scripts/check_python_api.py

.PHONY: py-lint
py-lint: py-api-check py-sync ## Lint and type-check Python package files with uv, ruff, and pyright
	@echo "Running ruff"
	@uv run --no-sync ruff check $(PY_SOURCES)
	@uv run --no-sync ruff format --check $(PY_SOURCES)
	@echo "Running pyright"
	@uv run --no-sync pyright

.PHONY: py-rust-check
py-rust-check: ## Check the Rust extension crate for the Python package
	@echo "Checking Python Rust extension formatting"
	@cargo fmt --manifest-path $(PY_PACKAGE)/Cargo.toml -- --check
	@echo "Checking Python Rust extension"
	@cargo check --manifest-path $(PY_PACKAGE)/Cargo.toml --all-targets --locked
	@echo "Running clippy for Python Rust extension"
	@cargo clippy --manifest-path $(PY_PACKAGE)/Cargo.toml --all-targets --locked -- -D warnings

.PHONY: py-test
py-test: py-sync ## Run Python package tests through uv
	@echo "Building Python package in editable mode"
	@env -u CONDA_PREFIX -u CONDA_DEFAULT_ENV uv run --no-sync maturin develop --skip-install --manifest-path $(PY_PACKAGE)/Cargo.toml --locked
	@echo "Running Python package tests"
	@env PYTHONPATH="$(abspath $(PY_PACKAGE)/python)" uv run --no-sync pytest $(PY_PACKAGE)/tests -vv

.PHONY: py-build
py-build: py-sync ## Build Python package distributions with uv
	@echo "Building Python package distributions"
	@rm -rf $(PY_DIST_DIR)
	@uv build --package starweaver -o $(PY_DIST_DIR)

.PHONY: py-wheel-smoke
py-wheel-smoke: py-build ## Install the built wheel into a clean venv and run smoke checks
	@echo "Running Python wheel smoke"
	@env -u CONDA_PREFIX -u CONDA_DEFAULT_ENV uv run --no-sync python scripts/python_wheel_smoke.py $(PY_DIST_DIR)

.PHONY: py-check
py-check: py-lint py-rust-check py-test py-wheel-smoke ## Run all Python package checks; defaults to Python 3.13

.PHONY: replay-check
replay-check: ## Run model replay and request-parameter contract tests
	@echo "Checking model replay fixtures"
	@cargo test -p starweaver-model --test fixture_schema --test replay --test replay_tooling --test request_parameters --test stream_replay --locked

.PHONY: oauth-check
oauth-check: ## Check OAuth crates and CLI/model integration
	@echo "Checking OAuth crates and integrations"
	@cargo check -p starweaver-oauth -p starweaver-oauth-provider -p starweaver-model -p starweaver-cli --all-targets --locked

.PHONY: oauth-test
oauth-test: ## Run focused OAuth tests
	@echo "Running OAuth tests"
	@cargo test -p starweaver-oauth --test oauth -p starweaver-model --test oauth_provider -p starweaver-oauth-provider --test refresh --locked

.PHONY: oauth-ci
oauth-ci: oauth-check oauth-test ## Run OAuth focused check and tests

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
scripts-check: architecture-check capability-check cli-examples-check install-script-check ## Validate repository automation scripts through xtask
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
	@if [ -z "$(VERSION)" ]; then echo "VERSION is required, for example: make upversion VERSION=0.0.1"; exit 1; fi
	@$(XTASK) upversion $(VERSION)
	@uv lock
	@cargo check --workspace --all-targets --all-features --locked

.PHONY: semver-check
semver-check: ## Check Rust public API compatibility against the latest release
	@command -v cargo-semver-checks >/dev/null || { echo "cargo-semver-checks is required"; exit 1; }
	@# starweaver-storage has no published 0.6 baseline; include it after the first 0.7 release.
	@cargo semver-checks check-release --workspace --exclude starweaver-storage

.PHONY: release-api-check
release-api-check: agent-api-check py-api-check semver-check py-wheel-smoke ## Validate reviewed Rust/Python APIs and the built wheel before release

.PHONY: publish-dry-run
publish-dry-run: ## Dry-run first-wave crate publish packages
	@$(XTASK) publish-dry-run

.PHONY: publish
publish: ## Publish crates in dependency order
	@PUBLISH_RETRIES=$(PUBLISH_RETRIES) PUBLISH_RETRY_DELAY_SECONDS=$(PUBLISH_RETRY_DELAY_SECONDS) $(XTASK) publish

.PHONY: lint
lint: docs-check py-lint ## Run pre-commit hooks, Python lint, and docs example checks across the repository
	@echo "Running pre-commit"
	@pre-commit run -a

.PHONY: ci
ci: fmt-check check replay-check test py-check scripts-check docs-check docs-build ## Run the same core checks as CI

.PHONY: ci-all
ci-all: ci coverage-ci ## Run core CI plus coverage gates

.PHONY: cli
cli: ## Run sw cli; no args renders TUI, pass args with `make cli -- -p "prompt"`
	@set -e; \
	args='$(CLI_ARGS)'; \
	case "$$args" in \
		"") cargo run --package starweaver-cli --bin sw --locked -- cli ;; \
		-p\ *) prompt=$${args#-p }; cargo run --package starweaver-cli --bin sw --locked -- cli -p "$$prompt" ;; \
		--prompt\ *) prompt=$${args#--prompt }; cargo run --package starweaver-cli --bin sw --locked -- cli --prompt "$$prompt" ;; \
		*) cargo run --package starweaver-cli --bin sw --locked -- cli $$args ;; \
	esac

.PHONY: sw
sw: ## Run sw launcher; pass args with `make sw -- version` or ARGS="version"
	@cargo run --package starweaver-cli --bin sw --locked -- $(SW_ARGS)

.PHONY: run-cli
run-cli: cli ## Alias for cli
