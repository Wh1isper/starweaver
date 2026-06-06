# Starweaver Implementation TODO

This memo tracks the execution roadmap for the architecture in `spec/`. It is organized around landed foundations, active design decisions, validation evidence, near-term milestones, later milestones, open design questions, and acceptance gates.

## Current Status

### Landed Foundations

- Workspace structure, unified workspace version, two-step release workflow, and Rust `xtask` repository automation are landed.
- Replay compatibility is broad across OpenAI Chat, OpenAI Responses, Anthropic Messages, Gemini generateContent, and Bedrock Converse.
- Core runtime foundations are landed: deterministic agent loop, graph inspection, iteration inspection, stream records, direct model/tool APIs, output policy, retry handling, capability hooks, message history processors, usage limits, trace recording, and executor checkpoints.
- SDK facade foundations are landed: `AgentBuilder`, `AgentApp`, `AgentSession`, context export/restore, direct API re-exports, first-party tool bundles, agent spec presets, subagent registry foundations, and markdown subagent config parsing.
- Environment foundations are landed in `starweaver-environment`: provider trait, file and shell policies, resource references, state snapshots, virtual provider, and local provider with policy-aware read/write/list/glob support.
- Durable session foundations are landed in `starweaver-claw`: `SessionStore`, in-memory store, session/run records, checkpoint append/load/latest, stream replay, resume snapshots, and compact run trace projection.
- Command-line foundations are landed: `version`, `run`, `diagnostics`, session list/show/replay/trim, replay-check guidance, setup/auth/catalog commands, default first-party tool catalog assembly, tool approval policy loading, MCP metadata wiring, retained TUI snapshots, approval/deferred commands, continuation-run resume, and release CLI smoke validation with deterministic tests.
- First-party tool abstraction foundations are landed: typed tool argument schemas via `schemars`, `Toolset::get_tools`, `Toolset::get_instructions`, registry-level instruction aggregation, `PrefixedToolset`, and core `ToolProxyToolset`.
- Specs are organized across `spec/core`, `spec/sdk`, and `spec/ops` with matching roadmap ownership.

### Active Design Decisions

- Trace/span design has two layers:
  - info-level canonical SDK telemetry: agent, step, model, tool, checkpoint, compact history/filter spans, canonical request/response/stream events, and usage/correlation attributes.
  - debug-level raw LLM telemetry: exact provider HTTP request/response and future raw provider stream chunks, enabled by application policy.
- Model reconstruction should use canonical model-layer events by default. Raw LLM request traces are for targeted provider, gateway, replay, and audit debugging.
- Compact capability/filter spans record structural before/after evidence by default. Full all-filter snapshots are debug-level high-volume telemetry.
- `AgentContext` is the short-lived native evidence carrier. `EnvironmentProvider` is the long-lived resource owner. Context stores typed provider dependencies and serializable environment refs.
- `EnvironmentProvider` should stay small until concrete host/service call sites prove richer operators. Rich file, process, resource, sandbox, and background-shell APIs should grow through extension traits and first-party bundles.
- Checkpoint reload uses session state, latest checkpoint, and stream replay-after-cursor as separate concerns. Stream persistence is delivery-oriented; checkpoint persistence is execution-oriented.
- Tool discovery for large tool surfaces uses a core fixed two-tool proxy: `ToolProxyToolset` exposes `search_tools` and `call_tool`; callers compose namespacing with `PrefixedToolset` or `namespaced_toolset`.

## Validation Baseline

Use these commands while executing TODO items:

```bash
make replay-check
make coverage-ci
make fmt-check
make check
make test
make scripts-check
make docs-check
make ci
```

Focused gates:

```bash
cargo test -p starweaver-model --test fixture_schema --test replay --test replay_tooling --test request_parameters --test stream_replay --locked
cargo test -p starweaver-agent --test bundles --locked
cargo test -p starweaver-tools --test typed_tool --test toolset --test prefixed --locked
cargo llvm-cov --workspace --all-features --locked --fail-under-lines 70 --summary-only
```

Last verified local validation set after the toolset/tool-proxy refactor:

```bash
make fmt-check && make check && make test && make docs-check && make replay-check
```

Last recorded workspace line coverage snapshot before the latest toolset/tool-proxy pass: 83.08% with the default 70% gate. Core grouped measured floor was 75% while acceptance paths kept a stricter 95% gate over stable high-coverage contract files. Rerun coverage after major API changes before using coverage numbers for release evidence.

## Provider Replay Status

Current fixture-driven replay coverage is a maintenance area. New provider work should add fixtures before changing request/response mapping.

| Provider family  | Coverage status                                                                                                                                                                                                                                                                                        |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| OpenAI Chat      | text, tools, structured output, JSON object mode, tool choice, parallel tools, refusal, malformed choices, streaming, multimodal input landed                                                                                                                                                          |
| OpenAI Responses | text, function calls, structured output, reasoning, thinking summaries, native web search, native MCP, image/file output parsing, refusal, streaming, status errors landed; dedicated request fixtures pending for image generation, file search, web fetch, memory, and code interpreter native tools |
| Anthropic        | text, tools, tool result history, thinking, signatures, image input, cache control, max token stop, safety-style refusal, streaming landed                                                                                                                                                             |
| Gemini           | text, function calls, function responses, safety, tool config, code execution, Google search, multimodal input, streaming, malformed candidates landed                                                                                                                                                 |
| Bedrock          | text, tools, strict tool calls, tool result errors, max token stop, content block variants, additional fields, status errors, streaming, SigV4/gateway metadata landed                                                                                                                                 |
| Cross-provider   | cassette record/scrub/import/summary, schema validation, errors, retries, params merge precedence, profiles, native tool serialization landed                                                                                                                                                          |

## Completed Near-Term Milestones

### M1 SDK Usability

Status: first implementation landed.

- SDK preset types: `ModelPreset`, `SdkPreset`, `text_output_preset`; model-layer built-in settings/config/runtime presets are available through `starweaver-model` and re-exported by `starweaver-agent`.
- Serializable `AgentSpec` YAML loader and `AgentSpecRegistry` that resolve model ids, toolsets, subagents, runtime policy, settings, and usage limits into `AgentBuilder`.
- `AgentSession` convenience APIs for state, notes, message bus, metadata, trace context, and W3C traceparent setup.
- Public SDK re-exports for new presets, bundles, trace recorder types, and runtime/tool/model helpers.
- Tests cover spec loading, session helpers, trace parent propagation, and SDK builder behavior.

### M2 Environment Provider

Status: first implementation landed.

- `starweaver-environment` crate with `EnvironmentProvider`, `DynEnvironmentProvider`, `FilePolicy`, `ShellPolicy`, `EnvironmentPolicy`, `ResourceRef`, `ShellOutput`, and `EnvironmentState`.
- `VirtualEnvironmentProvider` supports deterministic shared in-memory files, fake shell outputs, policy checks, glob/grep, and state export.
- `LocalEnvironmentProvider` supports policy-guarded read, write, list, glob, and state export. Local shell execution is reserved for a shell tool/provider implementation.
- Workspace, README, and AGENTS include the new crate boundary.
- Tests cover file read/write/list, shell fake execution, policy denial, glob/grep with native matchers, local gitignore/hidden behavior, and state export.

### M3 First-Party Tool Bundles

Status: first implementation landed.

- Filesystem bundle with `view`, `ls`, `write`, `edit`, `multi_edit`, `glob`, `grep`, `mkdir`, `delete`, `move`, `copy`, and `resource_ref` over `EnvironmentProvider` through `AgentContext` dependencies.
- Filesystem execution state: `view`, `ls`, `write`, `edit`, `multi_edit`, `glob`, `grep`, and `resource_ref` execute against the active provider; `mkdir`, `delete`, `move`, and `copy` emit operation envelopes pending richer provider operation traits.
- Shell bundle with `shell_exec`, `shell_wait`, `shell_status`, `shell_input`, `shell_signal`, `shell_kill`, stdout/stderr/status evidence, and approval metadata.
- Shell execution state: foreground `shell_exec` executes through `EnvironmentProvider::run_shell`; background `shell_exec` returns an explicit durable-shell-provider requirement, and lifecycle tools emit durable operation envelopes pending a process-capable provider.
- Task bundle with `task_create`, `task_get`, `task_update`, and `task_list` operation envelopes.
- Host-operation bundle with web, fetch/scrape/download, media, summarize, note, and thinking tools; document conversion is planned as skill-driven shell workflows. Bundle internals are split by tool category under `crates/starweaver-agent/src/bundles/`.
- Core tool proxy foundation through fixed `search_tools` and `call_tool` via `ToolProxyToolset`, plus `PrefixedToolset`/`namespaced_toolset` for namespace-prefixed proxy surfaces or wrapped tools.
- Bundle APIs return `DynToolset` for direct `ToolRegistry` and `AgentBuilder` registration.
- Tests cover stable tool names, instructions, fake-backed execution, resource refs, explicit background-shell provider requirements, context propagation, proxy search/call, namespacing, and agent builder registration.

### M4 Durable Session Runtime

Status: first implementation landed and refined.

- `starweaver-claw` crate with `SessionStore` and `InMemorySessionStore`.
- `SessionRecord`, `RunRecord`, `CompactRunTrace`, `SessionId`, and `SessionResumeSnapshot`.
- Session save/load, run append, checkpoint append/load/latest, stream record append, stream replay, stream replay after cursor, resume snapshot, and compact run projection.
- `SessionStoreExecutor` persists runtime checkpoints into any `SessionStore`.
- Compact projections include run id, checkpoint ids, latest checkpoint id, stream event count, stream cursor, and trace context.
- Tests cover session save/load, run append, checkpoint persistence/load/latest, runtime executor persistence, stream replay, replay after cursor, and compact projection.

### M5 Observability and Command-Line Inspection

Status: first implementation landed and refined.

- Runtime `TraceRecorder` abstraction with `SpanSpec`, `SpanKind`, `TraceLevel`, `SpanHandle`, `SpanEvent`, `SpanStatus`, `RecordedSpan`, `NoopTraceRecorder`, `InMemoryTraceRecorder`, and `AdapterTraceRecorder` exporter seam.
- Runtime loop spans for `gen_ai.invoke_agent`, `starweaver.loop.step`, `gen_ai.inference`, `gen_ai.execute_tool`, `starweaver.history.compaction`, and `starweaver.checkpoint`.
- Default model-layer canonical events for request, stream events, and response.
- Debug LLM-request recorder seam for raw provider request/response evidence through `ModelRequestContext` metadata.
- Tool spans record tool call arguments and tool return result events.
- Trace context propagation into model request contexts, tool contexts, checkpoints, and compact run projections.
- Nested span tests cover agent, loop step, model, tool, checkpoint spans, and the adapter seam in one trace.
- Command-line commands cover local run, diagnostics, version, session inspect, and replay-check guidance with deterministic tests.

### M6 Toolset and Tool Registration Alignment

Status: first implementation landed.

- `Toolset` exposes `get_tools()` and `get_instructions()`.
- `ToolRegistry::insert_toolset` registers tool definitions and deduplicated instruction groups.
- `TypedFunctionTool<Args, F>` and `typed_tool::<Args, _, _>()` derive JSON Schema from typed Rust argument objects through `schemars`.
- `ToolContext` carries execution metadata and typed dependencies. Runtime injects the active `AgentContext` into tool dependencies before tool calls.
- First-party SDK tools access environment handles through `ToolContext -> AgentContext -> EnvironmentHandle`.
- Instruction presets moved to prompt examples under `examples/prompts`.
- Tests cover typed schema generation, toolset instructions, runtime dependency injection, first-party bundles, and tool proxy behavior.

## Next Milestones

### N1 Agent SDK P0/P1 Foundation

Status: landed in the current workspace. See `memos/agent-sdk-foundation-plan.md` for merged evidence, API decisions, focused tests, docs touched, and validation commands.

Current landed substrate:

- `AgentBuilder`, `AgentApp`, `AgentSession`, and `AgentRunOptions` provide the primary reusable and run-scoped SDK surface.
- `AgentSpec` and `AgentSpecRegistry` cover app-profile fields for model selection, SDK policy presets, output profile, selected toolsets/subagents, skill config, host adapters, MCP servers, environment policy, and durability policy.
- SDK policy presets cover approval, retry, streaming, observability, environment, and durability.
- First-party bundle helpers cover filesystem, shell, task, host operations, tool proxy, skills, environment toolsets, process shell toolsets, and live MCP toolsets.
- Fileops-loaded skills are represented by `SkillPackage`, `SkillSourceScope`, `SkillRegistry`, `parse_skill_markdown`, and `skill_tools()` over `EnvironmentProvider` file operations.
- Subagent inheritance supports required, optional, denied, and auto-inherited tools, with approval metadata preservation and nested delegation guardrails.
- Host web/media/download tools have executable adapter seams: `HostSearchClient`, Brave Search fallback by env key, `HostScrapeClient`, Firecrawl/local scrape paths, text download through `EnvironmentProvider`, media URL classification, and fallback media understanding clients.
- Process shell support includes `ProcessShellProvider`, durable process snapshots, context attachment, and shell lifecycle tools against process-capable providers.
- Live MCP support includes `LiveMcpClient`, discovered server snapshots, and `live_mcp_toolset()` mapping to `McpToolset`.

Validation recorded for this slice:

```bash
make check
make test
make docs-check
make fmt-check
git diff --check
```

### N2 CLI-First Product, Shared Session/Stream Runtime, and Durable Service

This is the recommended active implementation milestone after the Agent SDK foundation work. The next product layer should start with the CLI because it provides the self-hosting surface: prompt-driven local runs, display-protocol stdio streams, session restore from persisted display messages, and a launcher/install path that users can adopt before service adapters deepen.

Target outcome:

- Added `starweaver-session` crate for input parts, `SessionStore`, session/run records, checkpoint refs, approvals, deferred records, resume snapshots, and compact trace projections.
- Added `starweaver-stream` crate for display messages, replay event logs, replay transports, stream archives, realtime compaction buffers, and protocol envelopes.
- CLI headless mode is landed through `sw cli -p <prompt>` with text, display JSONL, and silent output modes.
- `clap` command parsing, `clap_complete` completions, and retained TUI snapshot rendering are landed; full `ratatui + crossterm` interactivity remains a later renderer pass.
- Persisted `DisplayMessage` records are the session replay and TUI snapshot source for local CLI restore and future service UI flows.
- AGUI-compatible display adapter and replay compaction paths are landed based on Starweaver Claw behavior.
- `starweaver` launcher dispatch, `sw` alias, `starweaver-{command}` convention, GitHub release installer, update command, and CLI release smoke validation are landed.
- CLI configuration resolution is landed for global/project config roots, `config.toml`, `tools.toml`, `mcp.json`, `state.json`, layered skills/subagents, narrow environment overrides, setup UX, auth status/logout, and command-line flag precedence.
- CLI app-profile workflows over `AgentSpec` are landed for streamed runs, environment provider selection, display-message rendering, compact run trace projection, compact session commands for list/show/replay/trim, default first-party tool catalog assembly, configured MCP server validation, and skill/subagent catalog inspection.
- Claw-style session/run selectors for `-p/--prompt` are landed: `--session`, `--continue`, `--new-session`, `--run`, and `--branch-from`, where every prompt-backed invocation appends a run under a session.
- Headless HITL policy handling is landed for deny, defer, fail, and prompt policy selection, with persisted approval/deferred records for deferred workflows.
- CLI approval and deferred commands are landed for list/show/approve/reject/complete/fail control-flow management.
- CLI `resume` is landed as a continuation-run path over saved session state and persisted control-flow decisions.
- Local SQLite plus file-store persistence is landed in the CLI path for session/run/display indexes, checkpoint refs, raw evidence blobs, compact snapshots, approval/deferred records, and local trim.
- Current-session and all-sessions trim policies are landed for recent run count, age filters, active runs, latest successful run preservation, and bytes-reclaimed reporting.
- Service execution loop, cancellation/interruption, same-run approval/deferred resume endpoints, SSE replay, and compact run trace APIs use the same display/replay contracts in the Claw layer.
- Add runtime checkpoint reload APIs that hydrate from `AgentCheckpoint.state` and continue from safe execution nodes.
- Add idempotency metadata for external tool calls, host adapters, environment resources, and process handles.
- Add deployment metadata propagation into trace/session records: profile, workspace provider, build version, release, user id, and tags.

Focused implementation slices:

01. **CLI framework and parser:** add `clap` derive command schemas, `ValueEnum` types, command parsing tests, and `clap_complete` shell completion generation.
02. **CLI display contract:** define display-message restore semantics, headless replay envelopes, terminal markers, and renderer input contracts.
03. **CLI configuration resolver:** landed global/project config discovery, `config.toml`, `tools.toml`, `mcp.json`, `state.json`, layered skills/subagents, selected env overrides, setup command, and command-line precedence tests.
04. **CLI module split:** split `starweaver-cli` into args/config/commands/render/stream/session/storage modules while preserving current deterministic commands.
05. **Headless stdio runs:** add `-p/--prompt`, session selectors, run selectors, `--hitl deny|defer|fail`, `--output display-jsonl|silent`, and golden output tests.
06. **AGUI-compatible DisplayMessage protocol:** make `DisplayMessage` the AGUI-compatible Starweaver wire event for lifecycle, text, reasoning, tool call, tool result, custom, and terminal events with compaction tests.
07. **Local SQLite and file store:** implement SQLite-backed session/run/display indexes plus file-store blobs for raw stream records, checkpoint blobs, compact snapshots, archives, and attachments.
08. **Display-message persistence:** persist and replay display messages through `StreamArchive` for local runs and session restore tests.
09. **CLI session workflows:** add compact session list/show/replay/trim commands over the session/run model.
10. **Trim engine:** add current-session and all-sessions trim with dry-run reports, compaction-before-delete, preserved active/latest-success runs, age policies, recent-run policies, and orphan cleanup.
11. **Launcher and install path:** add `starweaver` launcher, `sw` alias install behavior, `starweaver-{command}` dispatch, GitHub release installer, update command, checksums, and package metadata.
12. **CLI profile/session workflows:** assemble CLI config, commands, environment/profile resolution, store/stream/transport selection, and renderers over shared session and stream contracts.
13. **Shared session records:** keep `InputPart`, session/run records, checkpoint refs, control records, compact projections, stream cursor refs, and serialization tests aligned with CLI restore needs.
14. **Shared stream protocols:** keep AGUI-compatible `DisplayMessage`, replay cursors/scopes, replay events, replay snapshots, stream archive records, protocol envelopes, and realtime compaction tests aligned with CLI, TUI, and Claw use.
15. **Service storage adapters:** lift SQLite-backed `SessionStore` and `StreamArchive` adapters into reusable Claw/service storage once the CLI-local schema stabilizes; add PostgreSQL after schema stability.
16. **TUI renderer:** retained text/JSON snapshot is landed for replay inspection; add `ratatui + crossterm` interactive views after headless replay and session restore contracts are stable.
17. **Approval/deferred UX and resume:** CLI approval/deferred commands and continuation-run `resume` are landed; Claw remains responsible for service-managed same-run checkpoint reload and service-side HITL endpoints.
18. **Release smoke and coverage:** `make cli-smoke` is landed for release-binary CLI validation; `make coverage-service` and `make coverage-ci` remain release-readiness gates for coverage.
19. **Service executor:** wrap runtime execution with persisted run records, cancellation tokens, approval/deferred state, shared replay transport, display-message projection, and resume snapshots.
20. **Checkpoint reload:** define continuation semantics for `RunStart`, `PrepareModelRequest`, `BeforeModelRequest`, `ModelResponse`, `ToolCall`, `ToolReturn`, `ValidateOutput`, `RunComplete`, and `RunFailed`.
21. **SSE and JSONL replay:** serve replay transport events with replay-after-cursor behavior and trace correlation.
22. **Redis replay adapter:** add Redis Stream replay event-log adapter after memory and SQLite contracts stabilize.
23. **Validation and docs:** add `starweaver-cli`, `starweaver-stream`, `starweaver-session`, and `starweaver-claw` tests, then document CLI durable app workflows.

### N2.5 Remaining SDK Deepening

These items can run alongside durable runtime work when their call sites are needed:

- Add binary/resource write extension traits to `starweaver-environment` and implement streaming binary downloads with checksums and resource metadata.
- Add concrete first-party fallback media model clients and parent-context usage accounting.
- Implement concrete `rmcp` stdio and streamable HTTP clients behind the `LiveMcpClient` seam.
- Add sandboxed shell providers with aligned filesystem/shell path spaces, workspace mounts, diagnostics, and state export.
- Add bundled first-party skill publishing and upgrade metadata after fileops-loaded skills stabilize through real application use.

### N3 SDK Documentation and Examples

- Deepen docs for `AgentSpec`, SDK presets, model settings/config presets, first-party tool bundles, environment providers, runtime tracing hooks, session helpers, subagents, and streaming helpers.
- Keep new examples runnable through `make docs-check`.

## Later Milestones

### Trace Export, Redaction, and Sampling

- Add feature-gated `tracing`, OpenTelemetry, OTLP, and Langfuse-friendly exporters.
- Add redaction policy for model content, tool arguments/results, HTTP headers, media refs, and debug raw events.
- Add sampling controls based on span name, level, provider, model, agent, conversation, and error status.
- Add snapshot tests for trace levels, span kinds, content export decisions, and Langfuse metadata.

### Checkpoint Reload and Resume Execution

- Add runtime APIs that can restart from a loaded `AgentCheckpoint` and continue from safe boundaries.
- Define resumable-node semantics for `RunStart`, `PrepareModelRequest`, `BeforeModelRequest`, `ModelResponse`, `ToolCall`, `ToolReturn`, `ValidateOutput`, `RunComplete`, and `RunFailed`.
- Persist stream cursor in checkpoints from service-managed stream observers or executor metadata.
- Add idempotency keys for external tool calls and environment resources.
- Add tests for reload from latest checkpoint plus stream replay-after-cursor.

### CLI Product Deepening

- Deepen retained TUI snapshots into interactive `ratatui + crossterm` views over the same persisted display messages.
- Expand CLI resume from continuation-run workflows toward service-managed same-run checkpoint reload through Claw APIs.
- Deepen live MCP execution from configured MCP metadata and SDK `LiveMcpClient` seams into concrete `rmcp` stdio and streamable HTTP clients when service call sites require them.
- Expand release smoke coverage as installer and packaging behavior grows.
- Keep app profile loading over `AgentApp`, environment providers, first-party bundles, `SessionStore`, SQLite local storage, file-store blobs, and trim policy aligned with docs and coverage gates.

### Durable Service Runtime Deepening

- Lift the CLI-local SQLite/file-store schema into reusable Claw storage adapters after the CLI display/restore/trim contract is stable, then add PostgreSQL after schema stabilizes.
- Add service execution loop, cancellation/interruption, approval/deferred resume endpoints, SSE replay, and compact run trace APIs using the same display/replay contracts.
- Add trace/session inspection surfaces shared by command-line and service layers.
- Add environment state persistence and restore factory hooks.
- Add deployment metadata propagation into trace/session records: profile, workspace provider, build version, release, user id, and tags.

### Subagents and Skills Beyond P1

- Add durable subagent polling extension after service runtime cancellation and resume endpoints land.
- Add bundled first-party skill publishing and upgrade metadata after the fileops-loaded skill bundle stabilizes.
- Add remote skill registry sync after local/project/global fileops discovery and pre-scan hooks are validated.

### Advanced Observability

- Add `starweaver.filter.all` debug-level tracing for all filter/capability input-output snapshots.
- Add provider raw streaming debug capture for SSE/chunked APIs before canonical normalization.
- Add compact trace projection tools for command-line/UI inspection with content previews and truncation flags.
- Add OTel semantic convention conformance tests and GenAI attribute mapping coverage.

### Advanced Provider Coverage

- Maintain replay coverage as providers evolve.
- Add new native tools, media parts, reasoning/thinking variants, raw streaming chunks, and gateway/audit routing fixtures when public APIs require them.
- Use debug raw LLM recorder output as a fixture capture path with scrub/import tooling.

### Embeddings, Evals, and Retrieval

- Add embeddings and retrieval APIs after core agent, environment, and service contracts stabilize.
- Add evaluation layer after SDK and command-line surfaces are stable enough for repeatable benchmark workflows.

### Platform Adapter Layer

- A2A adapter over service/session contracts.
- AGUI-compatible client transport over service/session/`DisplayMessage` contracts.
- Adapter conformance tests after core SDK and service runtime stabilize.

## Open Design Questions

- Exact extension-trait split for `EnvironmentProvider`: file/search/shell/process/resource/sandbox traits and default capability discovery.
- Sandboxed shell runtime selection across Linux bubblewrap/seccomp, macOS seatbelt, Windows restricted tokens, Docker/Podman, and remote microVM providers.
- Environment state domain schema for resources, background shell handles, sandbox mounts, output cursors, policy revisions, sandbox diagnostics, and workspace trust.
- Resume safety for already-started external resources, long-running shell processes, and deferred tool calls.
- Unified delegation schema for subagent selection, task metadata, inherited tools, and durable polling.
- Typed output ergonomics in Rust with manageable generic complexity.
- Skill pre-scan hook API and bundled-skill sync strategy across provider-visible roots.
- Trace redaction policy API and default sensitive-key list.
- Langfuse extension attribute names and release/session/user mapping.
- Compact run trace projection schema for model/tool/content previews across session tools, command-line workflows, and UI.
- command-line configuration format for model/profile/environment/session settings.

## Validation Matrix

| Area                       | Commands                                                                                                  |
| -------------------------- | --------------------------------------------------------------------------------------------------------- |
| Formatting                 | `make fmt-check`                                                                                          |
| Workspace compile and lint | `make check`                                                                                              |
| Tests                      | `make test`                                                                                               |
| Replay compatibility       | `make replay-check`                                                                                       |
| Repository automation      | `make scripts-check`                                                                                      |
| Docs examples              | `make docs-check`                                                                                         |
| Docs site                  | `make docs-build`                                                                                         |
| Coverage                   | `make coverage-core`, `make coverage-agent`, `make coverage-service`, `make coverage-ci`, `make coverage` |
| Release prep               | `make upversion VERSION=0.2.0`, `cargo run -p xtask --locked -- workspace-version`                        |

## Review Checklist

Before implementation resumes or public APIs graduate:

- Review the owning spec in `spec/core`, `spec/sdk`, or `spec/ops`.
- Confirm crate ownership and dependency direction.
- Add targeted tests before expanding docs examples.
- Update `README.md`, `CONTRIBUTING.md`, `AGENTS.md`, specs, docs, and workflow files when project structure or validation commands change.
- Keep new user-facing examples runnable through `make docs-check`.
