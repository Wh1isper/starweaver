# Repository Guidelines

## Repository Overview

`starweaver-agent-sdk` is a Rust workspace for building Starweaver, an agent SDK with CLI tooling and planned runtime/platform capabilities.

Primary design references:

- `https://github.com/pydantic/pydantic-ai` — core agent concepts, model abstraction, tool schema, output validation, retries, capabilities, and testing patterns
- `https://github.com/Wh1isper/ya-mono` — application runtime, context, tool implementations, interruption, resumable execution, and service patterns

Current workspace members:

- `crates/starweaver-core` — shared SDK identity, IDs, metadata, and usage primitives
- `crates/starweaver-model` — provider-neutral model messages, settings, profiles, native tool request definitions, protocol clients, injectable HTTP transport, deterministic test models, production-request guard, and replay tests
- `crates/starweaver-context` — AgentContext, typed dependencies, resumable state, state store, event bus, message bus, and usage ledger
- `crates/starweaver-runtime` — core agent loop, graph state machine, static and dynamic instructions, semantic retry, tool execution over provider-neutral tool schema, per-tool retry budgets, approval/deferred control-flow recording, prepare-tools hooks, structured output, typed structured output parsing, output functions, message history continuation, history processors, system prompt reinjection, usage/tool-call/cost budgets, typed stream events, scoped overrides, context integration, capability hooks, capability bundles, trace recording, and durable executor checkpoints
- `crates/starweaver-tools` — function tool schema, prefixed tools/toolsets, MCP toolset foundations, tool metadata, retry budget metadata, approval/deferred control-flow metadata, tool registries, and execution primitives
- `crates/starweaver-agent` — ergonomic SDK facade, `AgentBuilder`, `AgentApp`, SDK-level subagent registry, first-party tool bundles, spec presets, session helpers, and application-facing helpers
- `crates/starweaver-environment` — `EnvironmentProvider`, virtual and local provider foundations, file and shell policies, resource references, and environment state snapshots
- `crates/starweaver-session` — shared durable session contracts for input parts, `SessionStore` traits, session/run records, resume snapshots, approvals, deferred records, and compact trace projections
- `crates/starweaver-stream` — shared display and replay stream contracts for display messages, replay event logs, replay transports, realtime compaction buffers, stream archives, and protocol envelopes
- `crates/starweaver-claw` — durable orchestration host that re-exports shared session/stream contracts and will provide concrete storage, stream, service, and coordinator adapters
- `crates/starweaver-cli` — CLI-first product surface for headless stdio runs, display-message rendering, session restore, launcher dispatch, and install/update workflows

Planned areas live in `spec/` until their responsibilities, integration points, and validation paths are clear:

- Agent platform capabilities (`starweaver-platform`)

## Layering Rules

- `starweaver-model`: provider-neutral model protocol, settings, profiles, transports, and provider request mapping.
- `starweaver-tools`: tool schema, toolsets, metadata, tool context, and protocol-level tool execution primitives.
- `starweaver-runtime`: core agent loop, state transitions, tool loop, output loop, capabilities, usage limits, streaming events, trace spans, and executor checkpoints.
- `starweaver-agent`: SDK ergonomics, tool implementation bundles, subagent protocols, application wrappers, and policy presets.
- `starweaver-environment`: environment provider contracts, file/shell policy, resource references, and resumable environment state snapshots.
- `starweaver-session`: shared durable session contracts for input parts, `SessionStore` traits, session/run records, resume snapshots, approvals, deferred records, and compact trace projections.
- `starweaver-stream`: shared display and replay stream contracts for display messages, replay event logs, replay transports, realtime compaction buffers, stream archives, and protocol envelopes.
- `starweaver-claw`: durable orchestration, concrete `SessionStore` and stream archive adapters, service execution, checkpoint storage, interruption, resume, SSE transport, trace correlation, and storage adapters.
- `starweaver-platform`: hosted orchestration and external protocol adapters such as A2A and AGUI.

## Documentation Workflow

Use `docs/` for user-facing guides and examples. The docs site is built with mdBook from `book.toml`, `docs/SUMMARY.md`, and focused topic pages. Keep `docs/nav.json` aligned for repository tooling that consumes the docs map.

Documentation maintenance rules:

- Keep examples concise, complete, and runnable.
- Put Rust examples in fenced `rust` blocks.
- Use hidden `# async fn example() -> Result<..., ...>` wrappers for async examples so `make docs-check` can compile them.
- Run `make docs-check` after changing docs examples.
- Run `make docs-build` after changing the docs site structure, mdBook configuration, sitemap generation, or deployment metadata.
- Update `docs/SUMMARY.md` and `docs/nav.json` when adding, removing, or renaming docs pages.
- Keep `docs/` user-facing and keep architecture decisions in `spec/`.
- Prefer mermaid diagrams for architecture flows.

Current docs:

- `docs/index.md` — overview and documentation map
- `docs/install.md` — install and local validation
- `docs/agent.md` — agent builder and result basics
- `docs/models.md` — test models, function models, and production-request guard
- `docs/tools.md` — function tools, registries, toolsets, and retry metadata
- `docs/output.md` — structured output schemas and typed parsing
- `docs/message-history.md` — history continuation and new messages
- `docs/dependencies.md` — typed dependencies in context and tools
- `docs/capabilities.md` — runtime capability hooks
- `docs/durability.md` — executor checkpoints
- `docs/sdk-app.md` — `AgentApp` usage
- `docs/subagents.md` — SDK-level subagent delegation
- `docs/mcp.md` — MCP foundations and official `rmcp` direction
- `docs/testing.md` — deterministic testing, request guard, scripts, and coverage
- `docs/release.md` — release, upversion, crate publishing, and binary artifact workflow

## Spec Workflow

Use `spec/` for product and architecture decisions before introducing new crates or public APIs. Use `memos/` for working notes, design comparisons, implementation evidence, and release-preparation reminders.

Current specs:

- `spec/README.md` — architecture baseline map and design rules
- `spec/core/README.md` — core scope, contracts, and acceptance gates
- `spec/core/01-agent-loop.md` — deterministic run loop, graph states, retries, streaming, and durable execution seam
- `spec/core/02-model-provider-replay.md` — provider-neutral model protocol, replay fixtures, transport, settings, profiles, and CI gates
- `spec/core/03-tools-output-capabilities.md` — tool schema, tool loop, structured output, output functions, validators, hooks, and capability bundles
- `spec/core/04-context-state-executor.md` — AgentContext, StateStore, events, messages, notes, usage, checkpoints, and executor preparation
- `spec/core/05-pydantic-ai-feature-map.md` — Pydantic AI feature coverage map across agents, providers, tools, output, streaming, and testing
- `spec/sdk/README.md` — SDK product boundary and application-facing contract
- `spec/sdk/01-agent-sdk-app.md` — AgentBuilder, AgentApp, AgentSession, policy presets, app composition, and docs surface
- `spec/sdk/02-environment-provider.md` — EnvironmentProvider, filesystem, shell, resources, environment state, policies, and sandbox mapping
- `spec/sdk/03-first-party-tool-bundles.md` — filesystem, shell, search, media, task, skill, and tool-proxy bundles implemented through capabilities and context
- `spec/sdk/04-subagents-skills.md` — serializable subagent specs, delegation lifecycle, inherited tools, skills, and nested coordination
- `spec/sdk/05-sdk-integration-map.md` — SDK integration map for agents, context, filters, environment, toolsets, subagents, media, and presets
- `spec/ops/README.md` — operational layer scope and readiness model
- `spec/ops/01-ci-readiness.md` — replay CI, docs examples, feature coverage matrix, and release acceptance gates
- `spec/ops/02-shared-execution-components.md` — shared session storage and stream protocol contracts for CLI and Claw
- `spec/ops/03-durable-service-runtime.md` — durable sessions, `SessionStore`, stream archive, resume, interruption, SSE, display-message replay, and storage contracts
- `spec/ops/04-cli-product.md` — CLI-first product surface with headless stdio display streams, session restore from display messages, AGUI-compatible rendering, launcher dispatch, and GitHub install/update flow
- `spec/ops/05-observability.md` — OpenTelemetry GenAI tracing, Langfuse-friendly OTLP export, nested agent/model/tool spans, and trace-to-session correlation

Use `memos/` for working notes, design comparisons, implementation evidence, and release-preparation reminders. The current detailed implementation roadmap is `memos/implementation-todo.md`.

After changing repository structure, workspace boundaries, command behavior, CI, or planned module responsibilities, review and update:

- `docs/*`
- `spec/*`
- `README.md`
- `AGENTS.md`
- `Cargo.toml`
- crate manifests under `crates/*/Cargo.toml`
- `Makefile`
- `.pre-commit-config.yaml`
- `.github/workflows/*.yml`

## Development Workflow

After changing code, run:

1. `make fmt-check`
2. `make check`
3. `make test`

After changing docs examples, run:

```bash
make docs-check
```

After changing docs site structure or mdBook configuration, run:

```bash
make docs-build
```

For focused model/provider replay validation, run:

```bash
make replay-check
```

For full local validation, run:

```bash
make ci
```

For coverage validation, run:

```bash
make coverage-core
make coverage-agent
make coverage-service
make coverage-ci
```

For repository automation, run:

```bash
make scripts-check
```

To ask the assistant to prepare a unified-version release, use GitHub CLI from the repository root:

```bash
gh workflow run prepare-release.yml -f version=0.2.0 -f run_full_ci=true
```

This creates a `release/v0.2.0` pull request. After the pull request merges, `draft-release.yml` creates a draft GitHub Release with `starweaver-cli` archives containing `starweaver`, `starweaver-cli`, and `sw`, `starweaver-claw` archives containing `starweaver-claw`, and `checksums.txt`. Publishing that draft release triggers `release.yml`, which publishes crates through the `Release` environment.

For repository-wide hooks, run:

```bash
make lint
```

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Keep workspace metadata consistent across `Cargo.toml`, crate manifests, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Keep early abstractions minimal and add SDK concepts as concrete needs emerge.
- Treat runtime primitives as first-class: `AgentContext`, typed dependencies, `StateStore`, `EventBus`, `MessageBus`, executor checkpoints, trace context, `SessionStore` contracts, and environment resources.
- Add crates from specs when the boundary has clear responsibilities, call sites, and validation commands.
- Model transport must support injectable HTTP clients, custom headers, extra body fields, endpoint overrides, and audit/gateway routing requirements.
- Core runtime should prioritize prompt runs, model history, static and dynamic instructions, structured output retry, per-tool retry, capability hooks and bundles, prepare-tools hooks, settings/params forwarding, skip responses, tool execution, explicit tool-call boundaries, checkpoint emission, and OpenTelemetry GenAI span seams.
- SDK and platform layers should deepen tool implementations, official `rmcp` MCP live transports, subagent task protocols, live model delta streams, dependency-aware hooks, durable sessions, SSE, OpenTelemetry GenAI traces, and external protocol adapters.
