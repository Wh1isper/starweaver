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

Use `docs/` for user-facing guides and examples. The organization follows a docs-site shape with `docs/nav.json` plus focused topic pages.

Documentation maintenance rules:

- Keep examples concise, complete, and runnable.
- Put Rust examples in fenced `rust` blocks.
- Use hidden `# async fn example() -> Result<..., ...>` wrappers for async examples so `scripts/check-docs-examples.py` can compile them.
- Run `python3 scripts/check-docs-examples.py` after changing docs examples.
- Update `docs/nav.json` when adding, removing, or renaming docs pages.
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
- `spec/00-repository.md` — repository state, current workspace, and crate graduation rule
- `spec/01-sdk-vision-and-boundaries.md` — SDK product vision and layer boundaries
- `spec/02-crate-map.md` — current and planned crate map, dependencies, features, and milestones
- `spec/03-model-and-transport.md` — provider-neutral model protocol, settings, profiles, transport, and test models
- `spec/04-agent-runtime-loop.md` — graph loop, tool loop boundary, output retry, usage limits, stream records, and checkpoints
- `spec/05-tools-output-and-capabilities.md` — tools, toolsets, structured output, validators, history processors, and capability bundles
- `spec/06-context-state-and-events.md` — AgentContext, typed dependencies, state store, event bus, message bus, and resumable state
- `spec/07-agent-sdk.md` — AgentBuilder, AgentApp, facade policy, presets, subagents, and application protocols
- `spec/08-environment-and-tool-bundles.md` — environment abstraction, filesystem/shell/resources, policies, and first-party tool bundles
- `spec/09-mcp-strategy.md` — MCP foundations, live client path, and split triggers
- `spec/10-readiness-and-capability-status.md` — readiness levels, capability gates, and validation commands
- `spec/11-durability-and-service-runtime.md` — executor checkpoints, resumable sessions, service runtime, SSE, and AGUI direction
- `spec/12-cli-product.md` — CLI product surface, configuration, local runs, sessions, approvals, and diagnostics

Use `memos/` for working notes, design comparisons, implementation evidence, and release-preparation reminders.

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
