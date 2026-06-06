# Pre-1.0 Reference Notes

These notes capture reference mapping and phase-specific implementation observations. They are intentionally kept outside `spec/` so architecture specs can read as Starweaver's own design baseline.

## Reference Mapping

| Reference       | Ideas informing Starweaver                                                                                                                                                                                                              |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pydantic AI     | Agent abstraction, provider-neutral model history, model settings, model profiles, tool schema, toolsets, structured output, validators, retries, usage limits, capabilities, history processors, native tools, and deterministic tests |
| Pydantic Graph  | Explicit graph loop semantics, node state, dependency separation, replayable execution, and persistence boundaries                                                                                                                      |
| ya-agent-sdk    | Lifecycle-wide context, agent assembly, streaming runs, tool bundles, approval policies, subagents, fileops-loaded skills, session export/restore, tool proxy, and environment abstraction                                              |
| ya-mono runtime | Event bus, message bus, resumable resources, service execution, interruption, and workspace/environment patterns                                                                                                                        |
| MCP protocol    | Official `rmcp` SDK, tool discovery/call lifecycle, transports, resources, prompts, sampling, roots, notifications, long-running tasks, and provider-native MCP mapping                                                                 |

## Current Implementation Snapshot

- Runtime kernel is implemented with deterministic loop execution, graph/iteration inspection, structured output, retries, capabilities, usage limits, streaming records, trace spans, and checkpoint emission.
- Model layer includes provider-neutral messages/settings/profiles, native tool request definitions, OpenAI/Anthropic/Gemini/Bedrock adapters, injectable HTTP transport, request guard, deterministic models, and replay tests.
- Tool layer includes JSON-schema tool definitions, typed tool argument schema derivation, `Toolset::get_tools`, `Toolset::get_instructions`, registry instruction aggregation, retry metadata, MCP foundations, `PrefixedToolset`, and core `ToolProxyToolset`.
- SDK facade exists through `AgentBuilder`, `AgentApp`, `AgentSession`, direct re-exports, preset/spec loading, model settings/config/runtime presets, first-party tool bundles, environment attachment, subagent registry foundations, typed delegation tools, and markdown subagent config parsing.
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

### CLI-First Product, Shared Session/Stream Runtime, and Durable Service

- Added `starweaver-session` crate for input parts, `SessionStore`, session/run records, checkpoint refs, approvals, deferred records, resume snapshots, and compact trace projections.
- Added `starweaver-stream` crate for display messages, replay event logs, replay transports, stream archives, realtime compaction buffers, and protocol envelopes.
- Add CLI headless mode through `sw cli -p <prompt>` with text, JSONL, AGUI JSONL, and silent output modes.
- Persist `DisplayMessage` records through `StreamArchive` and use them as the session restore source for CLI, TUI, and future service UI flows.
- Add AGUI-compatible display adapter and replay compaction path based on Starweaver Claw behavior.
- Add `starweaver` launcher dispatch, `sw` alias, `starweaver-{command}` command convention, and GitHub release install/update scripts.
- Define CLI app-profile workflows over `AgentSpec`, environment providers, first-party bundles, shared session/stream contracts, and `SessionStore`.
- Add command-line session create/list/restore/replay/inspect with display-message rendering, compact trace projection, and stream replay.
- Add SQLite session store and stream archive adapters after the CLI display/restore contract is stable, then PostgreSQL after schema stabilizes.
- Add service execution loop, cancellation/interruption, approval/deferred resume endpoints, SSE replay, and compact run trace APIs using the same display/replay contracts.
- Add runtime resume APIs that hydrate from `AgentCheckpoint.state` and continue from safe boundaries.
- Add idempotency metadata for external tool calls, host adapters, environment resources, background shell handles, and deferred tool calls.

### SDK Deepening After P0/P1

- Add binary/resource write extensions for non-text downloads and resource stores.
- Add streaming binary download records with checksum, size, content-type, and resource metadata.
- Add concrete first-party fallback media understanding clients with usage accounting into the parent `AgentContext`.
- Add concrete `rmcp` stdio and streamable HTTP clients behind the `LiveMcpClient` seam.
- Add sandboxed shell provider design and implementation with aligned filesystem and shell path spaces.
- Add bundled first-party skill publishing and upgrade metadata after fileops-loaded skills stabilize through real application use.
- Add durable subagent polling after service runtime cancellation and resume endpoints land.
- Add remote skill registry sync after local/project/global fileops discovery and pre-scan hooks are validated.

### Observability Export

- Add OpenTelemetry/OTLP/Langfuse-friendly exporters behind features.
- Add trace redaction and sampling policies.
- Add provider raw streaming debug capture before canonical normalization.
- Add compact trace projection tools for command-line and service inspection surfaces.

### Provider Coverage

- Add dedicated replay fixtures for OpenAI Responses `code_interpreter`, `image_generation` request mapping, `file_search`, `web_fetch`, and `memory`.
- Add future native tools, media parts, reasoning variants, and gateway/audit routing fixtures as public APIs require them.

## Next Execution Plan

The next product-building path should start with shared session records, reusable `SessionStore`, shared stream records, and replay transport contracts, then deepen durable service runtime and CLI workflows on top of that base.

01. Added `starweaver-session` with input parts, session/run records, checkpoint refs, control records, compact projections, stream cursor refs, and serialization tests.
02. Reuse `starweaver-session` from `starweaver-claw` through re-exports and future concrete adapters.
03. Added `starweaver-stream` with display messages, replay cursors/scopes, replay events, replay snapshots, stream archive records, protocol envelopes, and serialization tests.
04. Add `ReplayEventLog`, `ReplayTransport`, `StreamArchive`, subscriptions, replay snapshots, and realtime compaction traits/types with memory-backed contract tests.
05. Implement SQLite-backed `SessionStore` and `StreamArchive` adapters with migrations, raw stream records, display messages, replay snapshots, and deterministic tests.
06. Build a service execution wrapper that persists runs, checkpoints, raw stream records, display messages, cancellation state, approval/deferred state, and trace correlation.
07. Define runtime checkpoint reload semantics and resume from safe execution nodes.
08. Assemble CLI app-profile/session workflows over `AgentSpec`, `AgentApp`, environment providers, first-party bundles, shared session/stream contracts, and renderers.
09. Add SSE replay and compact trace inspection shared by CLI and service layers.
10. Add Redis Stream replay after memory and SQLite contracts stabilize.
11. Re-run focused `starweaver-session`, `starweaver-stream`, `starweaver-claw`, `starweaver-cli`, `starweaver-agent`, docs, and workspace validation gates.

## Pre-1.0 Cleanup Reminder

Before a 1.0 release:

- remove reference-dependent language from public positioning
- turn phase snapshots into changelog or release notes
- keep specs focused on Starweaver's stable architecture
- keep docs focused on users and API behavior
- keep memos out of published docs unless deliberately curated
