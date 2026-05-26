# Contributing

This guide covers local development for Starweaver. Product and architecture decisions live in `spec/`; user-facing guides live in `docs/`; implementation planning and review evidence live in `memos/`.

## Repository Layout

- `crates/starweaver-core` — shared identifiers, metadata, and usage primitives.
- `crates/starweaver-model` — provider-neutral model protocol, profiles, transports, request mapping, response parsing, and replay tests.
- `crates/starweaver-context` — `AgentContext`, typed dependencies, resumable state, state store, event bus, message bus, notes, and usage ledger.
- `crates/starweaver-tools` — function tool schema, toolsets, metadata, registries, retry metadata, approval/deferred metadata, and MCP foundations.
- `crates/starweaver-runtime` — core agent loop, graph state machine, stream records, output validation, capability hooks, and executor checkpoints.
- `crates/starweaver-agent` — SDK facade, builder, app wrapper, subagent registry, and application-facing helpers.
- `crates/starweaver-cli` — command-line entry point.
- `docs/` — mdBook user documentation with runnable Rust examples.
- `spec/` — architecture and product specs.
- `memos/` — implementation roadmap, reference notes, and review evidence.

## Development Workflow

Install hooks:

```bash
make install
```

Run core validation:

```bash
make fmt-check
make check
make test
```

Run focused replay validation:

```bash
make replay-check
```

Run docs validation:

```bash
make docs-check
make docs-build
```

Run the full local gate:

```bash
make ci
```

## Documentation Rules

- Keep user-facing docs in `docs/`.
- Keep architecture decisions in `spec/`.
- Keep roadmap and review notes in `memos/`.
- Put Rust examples in fenced `rust` blocks.
- Use hidden async wrappers for docs examples compiled by `scripts/check-docs-examples.py`.
- Update `docs/SUMMARY.md` and `docs/nav.json` when adding, removing, or renaming docs pages.
- Run `make docs-build` after changing mdBook structure, sitemap generation, deployment metadata, or `book.toml`.
- Use mermaid diagrams for architecture and flow documentation.

## Replay and Provider Compatibility

Provider mapping changes need replay evidence:

1. Add or update a replay fixture.
2. Assert canonical request or response shape.
3. Run `make replay-check`.
4. Update `memos/implementation-todo.md` when a captured provider behavior remains queued.

Replay tests cover the compatibility boundary for OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, Gemini generateContent, Bedrock Converse, request parameters, model settings, and provider profiles.

## Spec Workflow

Specs are the review gate for public API and crate boundary changes. Update specs before graduating planned areas into crates or stable APIs.

Current spec layers:

- `spec/core/` — Pydantic AI-style core agent foundation.
- `spec/sdk/` — first-party Agent SDK surface and ya-agent-sdk integration.
- `spec/ops/` — CI readiness, durable runtime, observability, CLI, and product operations.

Update `README.md`, `AGENTS.md`, docs, CI, and workspace manifests when spec changes affect commands, public structure, or crate responsibilities.

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Follow existing Rust formatting and workspace lint rules.
- Keep runtime primitives first-class: `AgentContext`, typed dependencies, `StateStore`, `EventBus`, `MessageBus`, executor checkpoints, and environment resources.
- Keep provider transport configurable: injectable HTTP clients, custom headers, endpoint overrides, extra body fields, and gateway routing.
- Prefer small public APIs with tests and docs examples.

## External Protocol Boundaries

MCP integration uses the official Model Context Protocol Rust SDK at <https://github.com/modelcontextprotocol/rust-sdk> through the `rmcp` crate.

A2A and AGUI adapters belong to the platform or service-adapter layer. The core runtime and first-party SDK should expose stable events, traces, checkpoints, and session records that those adapters can consume.
