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

The compact set below is the stable top-level surface for application code. The broader
compatibility facade remains importable but is classified as provisional. Use
`starweaver.api_stability(name)`, `starweaver.STABLE_API`, and
`starweaver.PROVISIONAL_API` to inspect the tier. `make py-api-check` compares every top-level
export and tier with a checked snapshot.

<!-- stable-public-api:start -->

| Area                    | Stable imports                                                                                                          |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `agent_runtime`         | `create_agent`, `create_agent_runtime`, `Agent`, `AgentRuntime`, `AgentSession`, `AgentRun`, `AgentStream`, `RunResult` |
| `models_output`         | `FunctionModel`, `TestModel`, `ModelSettings`, `OutputSchema`, `OutputValue`, `output_validator`                        |
| `tools`                 | `tool`, `Tool`, `ToolContext`, `ToolResult`                                                                             |
| `stream_errors_version` | `StreamAdapter`, `StarweaverError`, `AgentError`, `ToolError`, `__version__`, `version`                                 |
| `api_stability`         | `API_STABILITY`, `PROVISIONAL_API`, `STABLE_API`, `api_stability`                                                       |

<!-- stable-public-api:end -->

All other names in `starweaver.__all__` are provisional compatibility exports. Prefer their
owning modules for advanced use, and review release notes before upgrading across minor versions.

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
- `StreamAdapter` projects canonical records through the Rust
  `DefaultDisplayMessageProjector`; its Python fallback is limited to unknown extension records.
  It is not a stream owner and cannot interrupt, steer, resume, or continue a run.

## UX Rules For New Python APIs

- Prefer explicit scope over global mutation: agent defaults, per-run overrides,
  and session state should stay visibly separate.
- Prefer canonical Rust records plus typed Python helpers over Python-only
  hidden state.
- Do not serialize process-local Python objects into durable session archives.
- Do not make subprocess or MCP-style bridges the default for Python tools.
- Do not document live-control ergonomics as stable until the underlying Rust
  contract is intentionally finalized for Python.
