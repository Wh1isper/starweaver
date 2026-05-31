# Pre-1.0 Reference Notes

These notes capture reference mapping and phase-specific implementation observations. They are intentionally kept outside `spec/` so architecture specs can read as Starweaver's own design baseline.

## Reference Mapping

| Reference       | Ideas informing Starweaver                                                                                                                                                                                                              |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pydantic AI     | Agent abstraction, provider-neutral model history, model settings, model profiles, tool schema, toolsets, structured output, validators, retries, usage limits, capabilities, history processors, native tools, and deterministic tests |
| Pydantic Graph  | Explicit graph loop semantics, node state, dependency separation, replayable execution, and persistence boundaries                                                                                                                      |
| ya-agent-sdk    | Lifecycle-wide context, agent assembly, streaming runs, tool bundles, approval policies, subagents, session export/restore, tool proxy, and environment abstraction                                                                     |
| ya-mono runtime | Event bus, message bus, resumable resources, service execution, interruption, and workspace/environment patterns                                                                                                                        |
| MCP protocol    | Official `rmcp` SDK, tool discovery/call lifecycle, transports, resources, prompts, sampling, roots, notifications, long-running tasks, and provider-native MCP mapping                                                                 |

## Current Implementation Snapshot

- Runtime kernel is implemented with deterministic loop execution, graph/iteration inspection, structured output, retries, capabilities, usage limits, streaming records, trace spans, and checkpoint emission.
- Model layer includes provider-neutral messages/settings/profiles, native tool request definitions, OpenAI/Anthropic/Gemini/Bedrock adapters, injectable HTTP transport, request guard, deterministic models, and replay tests.
- Tool layer includes JSON-schema tool definitions, typed tool argument schema derivation, `Toolset::get_tools`, `Toolset::get_instructions`, registry instruction aggregation, retry metadata, MCP foundations, `PrefixedToolset`, and core `ToolProxyToolset`.
- SDK facade exists through `AgentBuilder`, `AgentApp`, `AgentSession`, direct re-exports, preset/spec loading, first-party tool bundles, environment attachment, subagent registry foundations, and markdown subagent config parsing.
- Environment layer exists through `EnvironmentProvider`, file/shell policies, resource refs, state snapshots, deterministic virtual provider, and policy-aware local provider file/search operations.
- Durable session layer exists through `SessionStore`, `InMemorySessionStore`, session/run records, checkpoint persistence, stream replay, resume snapshots, `SessionStoreExecutor`, and compact run traces.
- Command-line binary includes deterministic `version`, `run`, `diagnostics`, `session inspect`, and replay-check guidance surfaces.
- Docs examples compile through `make docs-check` and the Rust `xtask` crate.
- GitHub CI includes docs example validation.
- Last verified local validation set: `make fmt-check && make check && make test && make docs-check && make replay-check`.

## Landed Since Earlier Snapshots

### SDK Ergonomics

- `AgentBuilder` supports settings, request params, output policy, validators, output functions, toolsets, capability bundles, subagents, and scoped test-model overrides.
- `AgentApp` and `AgentSession` provide context/session entrypoints, state export/restore, context helpers, and streaming helpers.
- SDK presets and serializable `AgentSpec` load YAML specs into `AgentBuilder` via `AgentSpecRegistry`.
- First-party tool bundle registration is available through `filesystem_tools`, `shell_tools`, `task_tools`, `host_operation_tools`, and `tool_proxy_toolset`.
- Typed output helpers parse structured outputs into Rust types.

### Subagents

- Serializable `SubagentSpec` foundations and markdown frontmatter parsing are implemented.
- SDK subagent registry supports task delegation through configured agents.
- Lifecycle events cover requested, started, completed, and failed transitions.
- Parent-child usage, notes, and context inheritance foundations are covered by tests.

### Tool Bundles

- Environment abstraction path is implemented from `AgentContext` dependencies to first-party tools.
- Filesystem and shell bundles are implemented with stable tool names and compact bundle instructions.
- Task and host-operation bundles are implemented as operation envelopes.
- Deterministic virtual provider fakes support file operations, shell output, glob/grep, and state export.
- `ToolProxyToolset` lives in `starweaver-tools` and is re-exported by `starweaver-agent`.

### MCP

- MCP toolset foundations are implemented.
- Provider-native MCP definitions map into `NativeToolDefinition` and OpenAI Responses requests.
- Tests cover native MCP server mapping and OpenAI Responses native MCP replay fixtures.

### Provider Depth

- Replay coverage includes major text, tool, structured output, native tool, streaming, multimodal, status-error, and provider-specific edge cases across OpenAI Chat, OpenAI Responses, Anthropic, Gemini, and Bedrock.
- Native tool coverage includes OpenAI Responses native pass-through, Gemini native tool mapping, and provider-native MCP.
- Dedicated replay fixtures remain the acceptance path for new provider behavior.

### Durability and Service Runtime

- `SessionStore` and in-memory storage are implemented.
- Runtime checkpoints include full `AgentRunState`, execution node, resume evidence, trace context, stream cursor slot, usage snapshot, and metadata.
- Checkpoint append/load/latest and stream replay are implemented.
- `SessionStoreExecutor` persists runtime checkpoints into a session store.
- Compact run projection and resume snapshot APIs are implemented.

### Observability

- Runtime trace recorder abstraction and deterministic in-memory recorder are implemented.
- Agent/model/tool/checkpoint/history spans are emitted in the runtime loop.
- Canonical model request/response/stream events are recorded.
- Debug raw LLM evidence seam exists through model request context metadata.

## Remaining Pre-1.0 Gaps

### Agent SDK Foundation Hardening

- Re-audit reference Agent SDK and pydantic-ai agent/toolset patterns against current Starweaver SDK code.
- Refine reusable agent configuration, per-run/session overrides, environment composition, toolsets, and subagent boundaries.
- Add focused tests for public SDK contracts, composition order, override precedence, tool inheritance, approval metadata, and session/context behavior.
- Update docs after stable API decisions land.

### Tool and Environment Deepening

- Split rich provider operations into extension traits after call sites stabilize.
- Add provider-backed implementations for envelope-only filesystem operations, shell lifecycle tools, task persistence, and host operations.
- Add process-capable provider with resumable background handles and output cursors.
- Add sandbox provider design and implementation.
- Add skill-contributed toolsets and unified delegation tools.

### MCP Deepening

- Add live client traits.
- Add stdio and HTTP transports.
- Add live discovery/call integration.
- Add resources, prompts, and local test server coverage.

### Subagents and Skills

- Complete `SubagentSpec` frontmatter fields.
- Add subagent factory and builtin registry.
- Implement unified delegation tool and inherited tool policy.
- Add nested delegation guardrails, trace parent propagation, and durable subagent polling extension.
- Add skill parser, registry, precedence rules, and skill-contributed toolsets.

### Observability Export

- Add OpenTelemetry/OTLP/Langfuse-friendly exporters behind features.
- Add trace redaction and sampling policies.
- Add provider raw streaming debug capture before canonical normalization.
- Add compact trace projection tools for later inspection surfaces.

### Checkpoint Reload and Application Surfaces

- Add runtime resume APIs that hydrate from `AgentCheckpoint.state` after SDK foundations are solid.
- Define safe continuation semantics per `AgentExecutionNode`.
- Bridge checkpoint stream cursors with `SessionStore::replay_stream_after`.
- Add idempotency metadata for tool calls and external resources.
- Add storage-backed service runtime, command-line workflows, SSE replay, and platform adapters in the application phase.

### Provider Coverage

- Add dedicated replay fixtures for OpenAI Responses `code_interpreter`, `image_generation` request mapping, `file_search`, `web_fetch`, and `memory`.
- Add future native tools, media parts, reasoning variants, and gateway/audit routing fixtures as public APIs require them.

## Next Execution Plan

The next product-building path focuses on Agent SDK foundation hardening. Durable sessions, command-line product workflows, service orchestration, and platform adapters remain later application surfaces.

1. Re-audit the local reference clones for agent construction, context deps, toolsets, environments, subagents, streaming, and SDK tests.
2. Map reference patterns into `memos/agent-sdk-foundation-plan.md` with concrete Starweaver target files.
3. Review `crates/starweaver-agent/src` for API seams that can be simplified or made more composable.
4. Implement high-confidence SDK improvements with tests first: per-run composition, environment/resource toolsets, unified delegation, inherited tool policy, approval metadata, and session/context behavior.
5. Update docs and specs after API shape stabilizes, keeping examples covered by `make docs-check`.

## Pre-1.0 Cleanup Reminder

Before a 1.0 release:

- remove reference-dependent language from public positioning
- turn phase snapshots into changelog or release notes
- keep specs focused on Starweaver's stable architecture
- keep docs focused on users and API behavior
- keep memos out of published docs unless deliberately curated
