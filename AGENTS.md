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
- `crates/starweaver-runtime` — core agent loop, graph state machine, static and dynamic instructions, semantic retry, tool execution over provider-neutral tool schema, per-tool retry budgets, approval/deferred control-flow recording, prepare-tools hooks, structured output, typed structured output parsing, output functions, message history continuation, history processors, system prompt reinjection, usage/tool-call/cost budgets, typed stream events, scoped overrides, context integration, capability hooks, capability bundles, and durable executor checkpoints
- `crates/starweaver-tools` — function tool schema, prefixed tools/toolsets, MCP toolset foundations, tool metadata, retry budget metadata, approval/deferred control-flow metadata, tool registries, and execution primitives
- `crates/starweaver-agent` — ergonomic SDK facade, `AgentBuilder`, `AgentApp`, SDK-level subagent registry, and application-facing helpers
- `crates/starweaver-cli` — `starweaver` command-line entry point

Planned areas live in `spec/` until their responsibilities, integration points, and validation paths are clear:

- Filesystem, shell, resources, and sandbox mapping (`starweaver-environment`)
- Claw runtime services (`starweaver-claw`)
- Agent platform capabilities (`starweaver-platform`)

## Layering Rules

- `starweaver-model`: provider-neutral model protocol, settings, profiles, transports, and provider request mapping.
- `starweaver-tools`: tool schema, toolsets, metadata, tool context, and protocol-level tool execution primitives.
- `starweaver-runtime`: core agent loop, state transitions, tool loop, output loop, capabilities, usage limits, streaming events, and executor checkpoints.
- `starweaver-agent`: SDK ergonomics, tool implementation bundles, subagent protocols, application wrappers, and policy presets.
- `starweaver-claw`: durable sessions, service execution, checkpoint storage, interruption, resume, SSE, and AGUI adapters.

## Documentation Workflow

Use `docs/` for user-facing guides and examples. The docs site is built with mdBook from `book.toml`, `docs/SUMMARY.md`, and focused topic pages. Keep `docs/nav.json` aligned for repository tooling that consumes the docs map.

Documentation maintenance rules:

- Keep examples concise, complete, and runnable.
- Put Rust examples in fenced `rust` blocks.
- Use hidden `# async fn example() -> Result<..., ...>` wrappers for async examples so `scripts/check-docs-examples.py` can compile them.
- Run `python3 scripts/check-docs-examples.py` after changing docs examples.
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
- `docs/mcp.md` — MCP foundations
- `docs/testing.md` — deterministic testing and request guard

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
- `spec/sdk/05-ya-agent-sdk-integration-map.md` — ya-agent-sdk module integration map for agents, context, filters, environment, toolsets, subagents, media, and presets
- `spec/ops/README.md` — operational layer scope and readiness model
- `spec/ops/01-ci-readiness.md` — replay CI, docs examples, feature coverage matrix, and release acceptance gates
- `spec/ops/02-durable-service-runtime.md` — durable sessions, execution records, resume, interruption, SSE, AGUI, and storage contracts
- `spec/ops/03-cli-product.md` — CLI Product surface built over SDK, environment providers, and service runtime contracts

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
python3 scripts/check-docs-examples.py
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

For repository-wide hooks, run:

```bash
make lint
```

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Keep workspace metadata consistent across `Cargo.toml`, crate manifests, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Keep early abstractions minimal and add SDK concepts as concrete needs emerge.
- Treat runtime primitives as first-class: `AgentContext`, typed dependencies, `StateStore`, `EventBus`, `MessageBus`, executor checkpoints, and environment resources.
- Add crates from specs when the boundary has clear responsibilities, call sites, and validation commands.
- Model transport must support injectable HTTP clients, custom headers, extra body fields, endpoint overrides, and audit/gateway routing requirements.
- Core runtime should prioritize prompt runs, model history, static and dynamic instructions, structured output retry, per-tool retry, capability hooks and bundles, prepare-tools hooks, settings/params forwarding, skip responses, tool execution, explicit tool-call boundaries, and checkpoint emission.
- SDK and platform layers should deepen tool implementations, MCP live transports, subagent task protocols, live model delta streams, dependency-aware hooks, durable sessions, SSE, and AGUI adapters.
