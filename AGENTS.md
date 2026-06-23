# Repository Guidelines

## Repository Overview

`starweaver-agent-sdk` is a Rust workspace for building Starweaver, an agent SDK with CLI tooling and planned runtime/platform capabilities.

Implementation boundary rule:

- Code, tests, public symbols, module names, and observable IDs should use Starweaver-native names.
- `starweaver-runtime` owns the agentic loop; `starweaver-context` owns neutral run/session evidence; `starweaver-agent` owns SDK ergonomics and first-party bundles; `starweaver-usage` owns usage accounting, limits, snapshots, and optional USD pricing estimates.
- Durable session/run IDs are generic request metadata (`starweaver.durable_session_id`, `starweaver.durable_run_id`, plus CLI-scoped aliases). Stable provider-routing affinity lives in `AgentContext.session_id` and typed `ModelSettings` provider settings, not generic durable metadata or model HTTP headers. Provider-specific headers such as `session_id`, `session-id`, `thread_id`, `thread-id`, and `x-client-request-id` belong in Codex/OpenAI subscription OAuth code only and must be sourced from typed provider routing settings.

Current workspace members:

- `crates/starweaver-core` — shared SDK identity, IDs, metadata, trace context, cooperative cancellation token, subagent specs, and XML helpers
- `crates/starweaver-usage` — usage accounting, snapshot contracts, usage limits, and optional USD pricing estimates
- `crates/starweaver-model` — provider-neutral model messages, settings, profiles, native tool request definitions, protocol clients, injectable HTTP transport, deterministic test models, production-request guard, model wrappers, OAuth-backed provider model adapters, and replay tests
- `crates/starweaver-oauth` — OAuth credential storage under `~/.starweaver`, Codex device login, token refresh, JWT metadata extraction, and store-backed token sources
- `crates/starweaver-oauth-provider` — OAuth-backed provider helpers, Codex model construction helpers, and refresh supervisor utilities
- `crates/starweaver-context` — AgentContext, typed dependencies, resumable state, state store, event bus, message bus, and usage ledger
- `crates/starweaver-runtime` — core agent loop, graph state machine, static and dynamic instructions, semantic retry, tool execution over provider-neutral tool schema, per-tool retry budgets, approval/deferred control-flow recording, prepare-tools hooks, structured output, typed structured output parsing, output functions, message history continuation, history processors, system prompt reinjection, usage-limit enforcement, typed usage snapshot events, typed stream events, scoped overrides, context integration, capability hooks, capability bundles, trace recording, and durable executor checkpoints
- `crates/starweaver-tools` — function tool schema, prefixed tools/toolsets, MCP toolset foundations, tool metadata, retry budget metadata, approval/deferred control-flow metadata, tool registries, toolset combinators, and execution primitives
- `crates/starweaver-agent` — ergonomic SDK facade, `AgentBuilder`, `AgentApp`, SDK-level subagent registry, first-party tool bundles, spec presets, session helpers, media/filter helpers, and application-facing helpers
- `crates/starweaver-environment` — `EnvironmentProvider`, virtual and local provider foundations, file and shell policies, resource references, environment state snapshots, and envd-backed provider adapters
- `crates/starweaver-envd-core` — runtime-neutral envd service trait, DTOs, protocol identity, JSON-RPC frame helpers, state descriptors, and error mapping
- `crates/starweaver-envd-client` — stdio/http `EnvdRpcClient` over the shared envd service interface
- `crates/starweaver-envd` — `LocalEnvd`, local ephemeral envd state, JSON-RPC dispatcher, stdio/http server transports, and standalone `starweaver-envd` binary
- `crates/starweaver-session` — shared durable session contracts for input parts, `SessionStore` traits, session/run records, resume snapshots, approvals, deferred records, and compact trace projections
- `crates/starweaver-stream` — shared display and replay stream contracts for display messages, replay event logs, replay transports, realtime compaction buffers, stream archives, UI adapters, sanitization, and protocol envelopes
- `crates/starweaver-storage` — shared SQLite migrations, concrete `SessionStore`, replay event-log, stream archive adapters, and migration status reporting
- `crates/starweaver-cli` — CLI-first product surface for headless stdio runs, display-message rendering, session restore, launcher dispatch, and install/update workflows
- `crates/starweaver-rpc-core` — shared JSON-RPC host protocol helpers, envelopes, errors, stream payload projection, and replay result helpers
- `crates/starweaver-rpc` — standalone JSON-RPC host process for Desktop and local host integrations

Planned areas live in `spec/` until their responsibilities, integration points, and validation paths are clear:

- Agent platform capabilities (`starweaver-platform`)

## Layering Rules

- `starweaver-core`: shared SDK identity, IDs, metadata, trace context, cooperative cancellation token, subagent specs, and XML helpers.
- `starweaver-usage`: leaf crate for usage accounting, usage snapshot contracts, usage limits, and optional `pricing` feature helpers. Pricing estimates use fixed-point micro USD in `PricingEstimate::amount_micros_usd`.
- `starweaver-model`: provider-neutral model protocol, settings, profiles, transports, model wrappers, provider request mapping, and OAuth-backed provider adapters.
- `starweaver-oauth`: OAuth auth file storage, Codex device-code login, token refresh, and store-backed token sources. Default auth path is `~/.starweaver/auth.json`.
- `starweaver-oauth-provider`: OAuth provider construction helpers and proactive refresh supervision.
- `starweaver-tools`: tool schema, toolsets, metadata, tool context, combinators, and protocol-level tool execution primitives.
- `starweaver-runtime`: core agent loop, state transitions, tool loop, output loop, capabilities, usage-limit enforcement, usage snapshot publication, streaming events, trace spans, and executor checkpoints.
- `starweaver-agent`: SDK ergonomics, tool implementation bundles, subagent protocols, application wrappers, filters, media helpers, and policy presets.
- `starweaver-environment`: environment provider contracts, file/shell policy, resource references, resumable environment state snapshots, and `EnvdEnvironmentProvider`.
- `starweaver-envd-core`: runtime-neutral envd service protocol, DTOs, state descriptors, JSON-RPC frame helpers, and error mapping.
- `starweaver-envd-client`: stdio/http envd client implementing the shared envd service interface.
- `starweaver-envd`: local envd implementation, ephemeral local state, service dispatcher, stdio/http transports, and standalone envd binary.
- `starweaver-session`: shared durable session contracts for input parts, `SessionStore` traits, session/run records, resume snapshots, approvals, deferred records, and compact trace projections.
- `starweaver-stream`: shared display/replay stream contracts, UI adapters, sanitizers, realtime compaction buffers, stream archives, and protocol envelopes.
- `starweaver-storage`: shared SQLite migrations, concrete `SessionStore`, `StreamArchive`, and `ReplayEventLog` adapters, plus migration status reporting.
- `starweaver-cli`: command-line product surface and local automation entry point.
- `starweaver-rpc-core`: shared JSON-RPC host protocol helpers and target home for typed host-control protocol definitions.
- `starweaver-rpc`: standalone JSON-RPC host process.
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
- `docs/quickstart.md` — first agent, tools, structured output, sessions, and CLI run
- `docs/agent-sdk.md` — SDK surface, layers, capabilities, bundles, and entry points
- `docs/agent.md` — agent builder and result basics
- `docs/models.md` — test models, function models, and production-request guard
- `docs/direct.md` — direct model, stream, and tool APIs
- `docs/tools.md` — function tools, registries, toolsets, and retry metadata
- `docs/output.md` — structured output schemas and typed parsing
- `docs/message-history.md` — history continuation and new messages
- `docs/dependencies.md` — typed dependencies in context and tools
- `docs/capabilities.md` — runtime capability hooks
- `docs/graph.md` — graph inspection and iteration trace
- `docs/durability.md` — executor checkpoints
- `docs/sdk-app.md` — `AgentApp` usage
- `docs/subagents.md` — SDK-level subagent delegation
- `docs/mcp.md` — MCP foundations and official `rmcp` direction
- `docs/testing.md` — deterministic testing, request guard, scripts, and coverage
- `docs/release.md` — release, upversion, crate publishing, and binary artifact workflow
- `docs/session-stream.md` — shared session, display stream, replay, and storage contracts

## Spec Workflow

Use `spec/` for product and architecture decisions before introducing new crates or public APIs. Use `spec/alignment/` for implementation evidence, readiness notes, and prioritized gap tracking.

Current specs:

- `spec/README.md` — architecture baseline map and design rules
- `spec/core/README.md` — core scope, contracts, and acceptance gates
- `spec/core/01-agent-loop.md` — deterministic run loop, graph states, retries, streaming, and durable execution seam
- `spec/core/02-model-provider-replay.md` — provider-neutral model protocol, replay fixtures, transport, settings, profiles, and CI gates
- `spec/core/03-tools-output-capabilities.md` — tool schema, tool loop, structured output, output functions, validators, hooks, and capability bundles
- `spec/core/04-context-state-executor.md` — AgentContext, StateStore, events, messages, notes, usage, checkpoints, and executor preparation
- `spec/core/05-agent-foundation-feature-map.md` — Agent foundation feature coverage map across agents, providers, tools, output, streaming, and testing
- `spec/core/06-message-request-abstractions.md` — Starweaver-native message AST, model request envelope, preparation pipeline, streaming parts, and provider boundary
- `spec/core/08-boundaries-and-usage.md` — native runtime/context/SDK/usage boundaries, usage snapshot pricing contract, and cleanup acceptance gates
- `spec/sdk/README.md` — SDK product boundary and application-facing contract
- `spec/sdk/01-agent-sdk-app.md` — AgentBuilder, AgentApp, AgentSession, policy presets, app composition, and docs surface
- `spec/sdk/02-environment-provider.md` — EnvironmentProvider, filesystem, shell, resources, environment state, policies, and sandbox mapping
- `spec/sdk/03-first-party-tool-bundles.md` — filesystem, shell, search, media, task, skill, and tool-proxy bundles implemented through capabilities and context
- `spec/sdk/04-subagents-skills.md` — serializable subagent specs, delegation lifecycle, inherited tools, skills, and nested coordination
- `spec/sdk/05-sdk-integration-map.md` — SDK integration map for agents, context, filters, environment, toolsets, subagents, media, and presets
- `spec/environment/README.md` — Starweaver Agent SDK environment layer, ownership rules, provider families, and envd relationship
- `spec/environment/01-sdk-provider-contract.md` — `EnvironmentProvider`, process/shell extension traits, descriptors, capabilities, snapshots, and restore boundary
- `spec/environment/02-tool-binding-and-envd-adapter.md` — environment-backed tool binding, `EnvdEnvironmentProvider`, CLI direct mode, host RPC attachments, and boundary rules
- `spec/envd/README.md` — standalone envd service architecture, ownership rules, implementation shape, and Starweaver reference integration
- `spec/envd/01-service-interface-and-state.md` — envd service trait, environment state, mount state, process state, operation/effect records, and capability model
- `spec/envd/02-implementations-and-modes.md` — local ephemeral mode, implementation-owned state lifecycle, RPC server mode, RPC client mode, and future sandbox/composite backends
- `spec/envd/03-rpc-protocol.md` — JSON-RPC method groups, stdio/http transports, request/response envelopes, errors, streaming, and idempotency
- `spec/envd/04-provider-and-host-integration.md` — reference Starweaver provider adapter, host RPC, session metadata, approval, and dependency boundaries
- `spec/envd/05-api-backlog.md` — unfinished envd API work that should wait for a concrete implementation or call site
- `spec/ops/README.md` — operational layer scope and readiness model
- `spec/ops/01-ci-readiness.md` — replay CI, docs examples, feature coverage matrix, and release acceptance gates
- `spec/ops/02-shared-execution-components.md` — shared session storage and stream protocol contracts
- `spec/ops/03-durable-service-runtime.md` — durable sessions, `SessionStore`, stream archive, resume, interruption, service transports, display-message replay, and storage contracts
- `spec/ops/04-cli-product.md` — CLI-first product surface with headless stdio display streams, session restore from display messages, DisplayMessage rendering with AGUI display adapters, launcher dispatch, and GitHub install/update flow
- `spec/ops/05-observability.md` — OpenTelemetry GenAI tracing, Langfuse-friendly OTLP export, nested agent/model/tool spans, and trace-to-session correlation
- `spec/ops/06-json-rpc-host-protocol.md` — Starweaver-owned JSON-RPC host-control protocol, stdio/HTTP transport profiles, typed method/event/error contracts, replay subscriptions, projections, and idempotency

Use `spec/alignment/` for readiness notes, design comparisons, implementation evidence, and roadmap reminders. Keep unfinished work in the spec that owns the changed contract.

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
gh workflow run prepare-release.yml -f version=0.0.1
```

This pushes `release/v0.0.1` for review. After the release commit reaches `main`, publish `v0.0.1` as a GitHub Release. The `release.yml` workflow runs from the published Release event, builds `starweaver-cli` archives containing `starweaver`, `starweaver-cli`, `sw`, and `starweaver-rpc`, uploads `checksums.txt`, and publishes crates through the `Release` environment.

Use squash merge only for GitHub pull requests. Do not merge pull requests with merge commits into `main`.

Keep release-event publishing packaging-only. Do not run CI, smoke checks, or publish dry-runs inside `.github/workflows/release.yml`; run validation before merging the release pull request.

For repository-wide hooks, run:

```bash
make lint
```

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Keep workspace metadata consistent across `Cargo.toml`, crate manifests, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Keep early abstractions minimal and add SDK concepts as concrete needs emerge.
- Treat runtime primitives as first-class: `AgentContext`, typed dependencies, `StateStore`, `EventBus`, `MessageBus`, executor checkpoints, trace context, `SessionStore` contracts, stream contracts, and environment resources.
- Add crates from specs when the boundary has clear responsibilities, call sites, and validation commands.
- Model transport must support injectable HTTP clients, custom headers, extra body fields, endpoint overrides, and audit/gateway routing requirements.
- Model protocol must preserve typed request/response parts, prepared request snapshots, profile-driven message normalization, tool-call argument state, provider details, and structured stream part events.
- Core runtime should prioritize prompt runs, model history, static and dynamic instructions, structured output retry, per-tool retry, capability hooks and bundles, prepare-tools hooks, settings/params forwarding, skip responses, tool execution, explicit tool-call boundaries, checkpoint emission, and OpenTelemetry GenAI span seams.
- SDK and platform layers should deepen tool implementations, official `rmcp` MCP live transports, subagent task protocols, live model delta streams, dependency-aware hooks, durable sessions, service transports, OpenTelemetry GenAI traces, and external protocol adapters.
