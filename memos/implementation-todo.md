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
- CLI foundations are landed: `version`, `run`, `diagnostics`, `session inspect`, and replay-check guidance with deterministic tests.
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

- SDK preset types: `ModelPreset`, `SdkPreset`, `text_output_preset`.
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
- Shell bundle with `shell_exec`, background execution envelopes, `shell_wait`, `shell_status`, `shell_input`, `shell_signal`, `shell_kill`, stdout/stderr/status evidence, and approval metadata.
- Shell execution state: foreground `shell_exec` executes through `EnvironmentProvider::run_shell`; background `shell_exec` and lifecycle tools emit durable operation envelopes pending a process-capable provider.
- Task bundle with `task_create`, `task_get`, `task_update`, and `task_list` operation envelopes.
- Host-operation bundle with web, image search, fetch/scrape/download, document conversion, media, summarize, note, thinking, and to-do operation envelopes.
- Core tool proxy foundation through fixed `search_tools` and `call_tool` via `ToolProxyToolset`, plus `PrefixedToolset`/`namespaced_toolset` for namespace-prefixed proxy surfaces or wrapped tools.
- Bundle APIs return `DynToolset` for direct `ToolRegistry` and `AgentBuilder` registration.
- Tests cover stable tool names, instructions, fake-backed execution, resource refs, background shell handles, context propagation, proxy search/call, namespacing, and agent builder registration.

### M4 Durable Session Runtime

Status: first implementation landed and refined.

- `starweaver-claw` crate with `SessionStore` and `InMemorySessionStore`.
- `SessionRecord`, `RunRecord`, `CompactRunTrace`, `SessionId`, and `SessionResumeSnapshot`.
- Session save/load, run append, checkpoint append/load/latest, stream record append, stream replay, stream replay after cursor, resume snapshot, and compact run projection.
- `SessionStoreExecutor` persists runtime checkpoints into any `SessionStore`.
- Compact projections include run id, checkpoint ids, latest checkpoint id, stream event count, stream cursor, and trace context.
- Tests cover session save/load, run append, checkpoint persistence/load/latest, runtime executor persistence, stream replay, replay after cursor, and compact projection.

### M5 Observability and CLI Inspection

Status: first implementation landed and refined.

- Runtime `TraceRecorder` abstraction with `SpanSpec`, `SpanKind`, `TraceLevel`, `SpanHandle`, `SpanEvent`, `SpanStatus`, `RecordedSpan`, `NoopTraceRecorder`, `InMemoryTraceRecorder`, and `AdapterTraceRecorder` exporter seam.
- Runtime loop spans for `gen_ai.invoke_agent`, `starweaver.loop.step`, `gen_ai.inference`, `gen_ai.execute_tool`, `starweaver.history.compaction`, and `starweaver.checkpoint`.
- Default model-layer canonical events for request, stream events, and response.
- Debug LLM-request recorder seam for raw provider request/response evidence through `ModelRequestContext` metadata.
- Tool spans record tool call arguments and tool return result events.
- Trace context propagation into model request contexts, tool contexts, checkpoints, and compact run projections.
- Nested span tests cover agent, loop step, model, tool, checkpoint spans, and the adapter seam in one trace.
- CLI commands cover local run, diagnostics, version, session inspect, and replay-check guidance with deterministic tests.

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

### N1 Agent SDK Foundation Hardening

This is the recommended next implementation milestone. See `memos/agent-sdk-foundation-plan.md` for the detailed execution plan.

Current substrate:

- `AgentBuilder`, `AgentApp`, and `AgentSession` provide the primary SDK surface.
- `AgentContext` typed dependencies, state export/restore, notes, message bus, usage, and trace context are available through session helpers.
- First-party bundles cover filesystem, shell, task, host operations, and tool proxy composition.
- `Toolset`, typed tools, registry instruction aggregation, `PrefixedToolset`, and `ToolProxyToolset` are landed in the core tool layer.
- Subagent config parsing, subagent registry foundations, and lifecycle events are landed.

Target outcome:

- Audit reference Agent SDK and pydantic-ai agent/toolset patterns carefully against the Starweaver SDK surface.
- Refine SDK API boundaries for reusable agent configuration, per-run/session overrides, environment composition, toolsets, and subagents.
- Add strong focused tests for public SDK contracts, composition order, override precedence, tool inheritance, approval metadata, and session/context behavior.
- Keep Claw, CLI, service orchestration, and platform adapters sequenced after foundational SDK/runtime/tool work is solid.

Proposed N1 implementation slices:

1. **Reference evidence table:** map reference patterns for agent construction, context deps, toolsets, environments, subagents, and streaming to Starweaver code targets.
2. **SDK API review:** audit `crates/starweaver-agent/src` and tests for awkward seams, then simplify public boundaries when the improvement is clear.
3. **Per-run composition:** evaluate and implement clean per-run additional/override toolsets and settings where the runtime surface already supports it.
4. **Environment composition:** add explicit SDK composition points for environment-provided and resource-provided toolsets when the seam stays small.
5. **Subagent foundation:** implement or deepen unified delegation, required/optional tool availability, inherited tool policy, and lifecycle/trace tests.
6. **Docs and examples:** update SDK docs only after API shape is stable, keeping examples runnable through `make docs-check`.

### N2 Tool, Environment, and Subagent Deepening

- Split rich environment operations into optional capability traits after call sites stabilize: file ops, search ops, shell ops, process ops, resource ops, sandbox ops.
- Keep provider-scoped `glob` and `grep` backed by native Rust matchers (`globset`, `grep-regex`, `grep-matcher`) and provider traversal (`ignore` for local files) as the baseline search operator.
- Align first-party bundle instructions around one compact instruction group per bundle, concrete tool selection guidance, stable deduplication keys, and prompt text that can migrate into SDK presets.
- Deepen media/search/document host-operation bundles with host-backed execution adapters.
- Add a skill bundle and skill-contributed toolsets.
- Add richer tool proxy execution evidence, including search result ranking tests and namespace-description tests.
- Add background shell lifecycle handles, stdin/signal/status/output cursor tools, and resumable process state through a process-capable provider.
- Add `SandboxedShellProvider` design and implementation: local file operator plus sandboxed shell runtime, workspace mounts, policy profiles, diagnostics, and environment state export.
- Add sandbox mount mapping and environment state-domain restore helpers.

### N3 Documentation and Examples

- Deepen docs for `AgentSpec`, SDK presets, first-party tool bundles, environment providers, runtime tracing, durable sessions, checkpoint reload, streaming persistence, and future CLI workflows.
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

### Durable Service Runtime Deepening

- Add SQLite storage adapter first, then PostgreSQL after schema stabilizes.
- Add service execution loop, cancellation/interruption, approval/deferred resume endpoints, SSE replay, and compact run trace APIs.
- Add trace/session inspection surfaces shared by CLI and service layers.
- Add environment state persistence and restore factory hooks.

### Application Surfaces: CLI and Service Runtime

- Define app profile loading over `AgentApp`, environment providers, first-party bundles, and `SessionStore`.
- Add CLI session create/list/resume/inspect with compact trace projection and stream replay.
- Define service coordinator span, run records, session state storage, SSE replay, approval endpoints, and workspace provider factories.
- Add local-first and service-backed command parity so CLI and service share persistence and checkpoint semantics.
- Add deployment metadata propagation into trace/session records: profile, workspace provider, build version, release, user id, and tags.

### Subagents and Skills

- Complete `SubagentSpec` frontmatter fields.
- Add subagent factory and builtin registry.
- Implement unified delegation tool and inherited tool policy.
- Add lifecycle event propagation, nested delegation guardrails, trace parent propagation, and durable subagent polling extension.
- Add skill parser, registry, precedence rules, and skill-contributed toolsets.

### Advanced Observability

- Add `starweaver.filter.all` debug-level tracing for all filter/capability input-output snapshots.
- Add provider raw streaming debug capture for SSE/chunked APIs before canonical normalization.
- Add compact trace projection tools for CLI/UI inspection with content previews and truncation flags.
- Add OTel semantic convention conformance tests and GenAI attribute mapping coverage.

### Advanced Provider Coverage

- Maintain replay coverage as providers evolve.
- Add new native tools, media parts, reasoning/thinking variants, raw streaming chunks, and gateway/audit routing fixtures when public APIs require them.
- Use debug raw LLM recorder output as a fixture capture path with scrub/import tooling.

### Embeddings, Evals, and Retrieval

- Add embeddings and retrieval APIs after core agent, environment, and service contracts stabilize.
- Add evaluation layer after SDK and CLI surfaces are stable enough for repeatable benchmark workflows.

### Platform Adapter Layer

- A2A adapter over service/session contracts.
- AGUI adapter over service/session/event contracts.
- Adapter conformance tests after core SDK and service runtime stabilize.

## Open Design Questions

- Exact extension-trait split for `EnvironmentProvider`: file/search/shell/process/resource/sandbox traits and default capability discovery.
- Sandboxed shell runtime selection across Linux bubblewrap/seccomp, macOS seatbelt, Windows restricted tokens, Docker/Podman, and remote microVM providers.
- Environment state domain schema for resources, background shell handles, sandbox mounts, output cursors, policy revisions, sandbox diagnostics, and workspace trust.
- Resume safety for already-started external resources, long-running shell processes, and deferred tool calls.
- Unified delegation schema for subagent selection, task metadata, inherited tools, and durable polling.
- Typed output ergonomics in Rust with manageable generic complexity.
- Skill package format and precedence across project, global, and builtin scopes.
- Trace redaction policy API and default sensitive-key list.
- Langfuse extension attribute names and release/session/user mapping.
- Compact run trace projection schema for model/tool/content previews across session tools, CLI, and UI.
- CLI configuration format for model/profile/environment/session settings.

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
