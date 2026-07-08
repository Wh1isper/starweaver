# Python Stability And Known Boundaries

This page lists the boundaries that should stay explicit while the Python SDK
is still young. Treat them as refactor candidates, not as product polish.

## Stable Contracts

- Python tools are injected in-process as native Starweaver runtime tools.
- Rust remains the source of truth for model request preparation, tool
  scheduling, retries, usage, trace, and session state.
- `StreamEvent.raw` is the portable canonical stream record.
- `RunResult.raw_state` and `SessionArchive` carry raw Starweaver state for
  durability boundaries.
- Tool calls run in parallel by default unless the tool is marked
  `sequential=True` or duplicate same-name calls require ordered execution.

## Stable Top-Level Imports

The names below are the stable top-level imports for application code. This
index is checked against `spec/sdk/python/12-api-compatibility-checklist.md` and
`starweaver.__all__`, so docs, tests, and package exports move together.

<!-- stable-public-api:start -->

| Area                                 | Stable imports                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agent_session_runtime`              | `create_agent`, `create_agent_runtime`, `Agent`, `AgentRuntime`, `AgentSession`, `AgentRun`, `AgentStream`, `RunResult`, `StreamRunResult`, `RunStatusSnapshot`, `StreamEvent`, `SessionArchive`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `active_control_message_hitl`        | `BusMessage`, `MessageBus`, `MessageDelivery`, `ControlReceipt`, `HitlSnapshot`, `PendingApproval`, `PendingDeferred`, `ApprovalDecision`, `DeferredResult`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `tools_toolsets_mcp`                 | `tool`, `Tool`, `BaseTool`, `ToolContext`, `ToolConfig`, `ToolResult`, `Toolset`, `ToolsetContext`, `ToolsetPreparation`, `AbstractToolset`, `PythonDynamicToolset`, `FunctionToolset`, `ToolsetFactory`, `toolset_factory`, `ToolLibrary`, `ToolSearchToolset`, `ToolProxyToolset`, `ToolsetIdentity`, `ToolsetIdIssue`, `ToolsetIdValidation`, `validate_toolset_ids`, `validate_toolsets_for_durability`, `ToolsetLifecyclePolicy`, `ToolsetLifecycleReport`, `ToolsetLifecycleState`, `core_toolsets`, `filesystem_toolset`, `shell_toolset`, `environment_toolsets`, `host_io_toolset`, `task_toolset`, `context_toolset`, `McpTransport`, `McpToolset`, `McpToolSpec`, `McpResourceSpec`, `McpPromptSpec`, `McpSamplingSpec`, `McpSubscriptionSpec` |
| `models_output_runtime_composition`  | `ProviderModel`, `ProviderAuth`, `ModelSettings`, `RequestParams`, `RuntimeConfig`, `SecurityConfig`, `ShellReviewConfig`, `CapabilityBundle`, `PythonCapability`, `OutputSchema`, `OutputPolicy`, `OutputContext`, `OutputFunction`, `OutputValidator`, `OutputValue`, `output_validator`                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `environment_resources_skills_media` | `Environment`, `EnvironmentLifecycleSnapshot`, `EnvironmentProvider`, `EnvdEnvironment`, `DockerEnvironmentProvider`, `PythonEnvironmentProvider`, `LocalEnvironment`, `VirtualEnvironment`, `FileOperator`, `Shell`, `ShellProcess`, `WorkspaceBinding`, `VirtualMount`, `VirtualPath`, `BaseResource`, `ResumableResource`, `InstructableResource`, `ResourceRef`, `ResourceRegistry`, `ResourceRegistryState`, `RESOURCE_REF_KIND_KEY`, `SkillRegistry`, `SkillPackage`, `SkillSourceScope`, `MediaUploader`, `MediaUploadRequest`                                                                                                                                                                                                                     |
| `storage_stream_observability`       | `SessionStore`, `InMemorySessionStore`, `JsonSessionStore`, `SqliteSessionStore`, `SqliteReplayEventLog`, `SqliteStreamArchive`, `InputPart`, `SessionStatus`, `RunStatus`, `ExecutionStatus`, `SessionRecord`, `RunRecord`, `StreamRecord`, `CheckpointRef`, `ApprovalRecord`, `DeferredToolRecord`, `SessionResumeSnapshot`, `StreamAdapter`, `Usage`, `UsageAgentTotal`, `UsageSnapshot`, `UsageSnapshotEntry`, `PricingEstimate`, `TraceMetadata`                                                                                                                                                                                                                                                                                                     |
| `subagents_testing_errors_version`   | `Subagent`, `TestModel`, `FunctionModel`, `StarweaverError`, `AgentError`, `ToolError`, `ModelError`, `StateError`, `StreamError`, `OutputError`, `InvalidArguments`, `Feedback`, `UserError`, `ApprovalRequired`, `CallDeferred`, `Cancelled`, `Timeout`, `ModelRetry`, `OutputRetry`, `OutputValidationFailed`, `__version__`, `version`                                                                                                                                                                                                                                                                                                                                                                                                                |

<!-- stable-public-api:end -->

## Current Facades

`run_stream()` currently returns `AgentRun`, which is both an async iterator over
canonical stream records and a facade for `recv()`, `join()`, `result()`,
`status()`, `recoverable_state()`, `close_receiver()`, `detach()`,
`interrupt()`, active messages, steering, and streamed HITL helpers. The
canonical records are the stable evidence surface. The Python live-control shape
should remain easy to revise until the Rust live-stream and interrupt contract
is intentionally frozen for Python.

`AgentStream` is a compatibility alias for `AgentRun`; new examples use
`AgentRun`.

## Known Boundaries

- Streamed HITL `resume(...)` returns a live continuation `AgentRun` only for an
  in-process session that is still alive. Durable recovery uses
  `session_id`/`run_id` through the runtime/store APIs. `resume_collected(...)`
  remains the compatibility path for applications that want a collected
  `RunResult`; in that mode `run.join().events` remains the original suspended
  stream records and does not include post-resume records.
- `AgentRuntime.stream(...)` returns a live `AgentRun` owner and persists through
  the bound runtime on join. `AgentRuntime.run_stream(...)` remains the collected
  durable `StreamRunResult` path for callers that do not need live control.
- `SessionArchive` serializes Starweaver state, not Python callables, provider
  connections, media upload callbacks, or live environment handles.
- `EnvironmentProvider` handles are process-local and must be reattached after
  restore.
- `SkillRegistry` loads native Starweaver skill packages; there is no separate
  Python skill authoring DSL.
- `PythonCapability` currently exposes a narrow hook-level contract for
  run-start state callbacks. Broader provider-message, request, tool, and output
  mutation hooks need typed Python contracts before becoming public API.
- `MediaUploader` adapts a callback into the native filter, but the host still
  owns resource lifecycle, storage, and access control.
- `StreamAdapter` projects collected or replayed records only. It is not a
  stream owner and cannot interrupt, steer, resume, or continue a run.

## UX Rules For New Python APIs

- Prefer explicit scope over global mutation: agent defaults, per-run overrides,
  and session state should stay visibly separate.
- Prefer canonical Rust records plus typed Python helpers over Python-only
  hidden state.
- Do not serialize process-local Python objects into durable session archives.
- Do not make subprocess or MCP-style bridges the default for Python tools.
- Do not document live-control ergonomics as stable until the underlying Rust
  contract is intentionally finalized for Python.
