# Runtime, Sessions, Streaming, And HITL

This spec covers the Python-facing agent runtime API after Python tools can be
registered in process.

## Agent Construction

P0 should support deterministic tests before production providers:

- `TestModel` with fixed text.
- `FunctionModel` for scripted model behavior and tool-call tests.
- A registry-resolved model handle for real providers once the provider factory
  boundary is extracted from CLI-only code, if needed.

The Python model field should not become an untyped provider escape hatch.
Acceptable P0 forms:

- a native Python wrapper around a Rust `ModelAdapter`
- a registry name resolved by `AgentSpecRegistry`
- a test model helper from `starweaver.testing`

Future forms:

- Python model config helpers for Starweaver model presets
- gateway/profile resolution
- Python-defined `ModelAdapter` only after tool injection, session, streaming,
  and HITL are stable

## Agent And Session API

The Python `Agent` should be an async context manager:

```python
async with create_agent(model=model, tools=[lookup]) as agent:
    result = await agent.run("Say ready")
```

Sessions should map directly to `AgentSession`.

P0 session API:

- `agent.new_session()`
- `agent.session_from_state(state)`
- `session.run(input, **run_options)`
- `session.run_stream(input, **run_options)`
- `session.export_state(mode="curated" | "full")`
- `session.inject_hitl_results(...)`
- `session.resume_after_hitl(...)`
- `session.metadata`
- `session.state`

Run options should map to `AgentRunOptions`:

- per-run instructions
- model settings
- request params
- extra tools
- extra toolsets
- replace-tools flag
- output policy when supported
- trace metadata when supported

## State Export And Restore

Run state and resumable state should remain JSON-compatible and versioned by
the Rust state schema. Python Pydantic models can validate and document the
shape, but Rust owns the canonical export format.

Python should expose both:

- raw JSON state for persistence and forward compatibility
- typed helper models for application validation and discoverability

Restore rules:

- Serializable context state restores from `ResumableState`.
- Process-local dependencies must be rehydrated by the Python application.
- Python callable handles are not serialized.
- Tool and toolset registration must happen again before a restored session can
  run tools that depend on process-local Python objects.

## HITL Mapping

| Python API                   | Rust contract                                             |
| ---------------------------- | --------------------------------------------------------- |
| `ApprovalRequired` exception | approval-required tool control flow                       |
| `CallDeferred` exception     | deferred tool control flow                                |
| `PendingApproval` dataclass  | approval records exposed from session/run result          |
| `DeferredCall` dataclass     | deferred records exposed from session/run result          |
| `resume_after_hitl(...)`     | `AgentHitlResults` into `AgentSession::resume_after_hitl` |

Python run result helpers should map to Rust `AgentResult::has_pending_hitl()`,
`pending_approvals()`, and `pending_deferred_tools()` instead of parsing raw
state fields.

HITL should preserve Starweaver control flow:

```python
run = await session.run("Deploy api")
if run.needs_approval:
    resumed = await session.resume_after_hitl(
        approvals={run.pending_approvals[0].id: {"approved": True}}
    )
```

P0 can expose simple dictionaries for approval decisions. Later phases can add
typed decision classes that mirror `ToolApprovalDecision`.

## Streaming

Python streaming should expose typed events while preserving raw records for
forward compatibility.

P0:

- `agent.run_stream(...) -> AsyncIterator[StreamEvent]`
- `session.run_stream(...) -> AsyncIterator[StreamEvent]`
- `StreamEvent.kind`
- event dataclasses for message deltas, tool calls, tool results, approvals,
  deferred calls, usage snapshots, lifecycle markers, and final result
- `event.raw` backed by `AgentStreamRecord::to_raw_json()` for unrecognized
  Starweaver records and forward-compatible extensions
- `stream.interrupt()` or async context-managed stream handles

Later:

- backpressure options
- replay cursor support
- child stream source attribution
- stream archive persistence
- callback hooks for lifecycle events
- event filters for UI surfaces

The Python API should not invent a separate event protocol. It should project
Starweaver stream records into Python-friendly classes.

## Output And Validation

P0:

- text output
- JSON schema output
- Pydantic output model validation
- output validators mapped to Starweaver output validation hooks

The Python API can accept Pydantic model classes for ergonomics, but Rust owns
the output retry loop and structured output parsing behavior.

Output validator shape:

```python
async def validate_answer(ctx, output: Answer) -> None:
    if not output.value:
        raise ModelRetry("return a non-empty answer")
```

The validator exception should map into the same retry behavior used by Rust
output validators.

## Error Model

Python exceptions should mirror public Starweaver control flow without leaking
Rust implementation details.

P0 exception classes:

- `StarweaverError`
- `AgentError`
- `ModelError`
- `ToolError`
- `InvalidArguments`
- `ModelRetry`
- `ApprovalRequired`
- `CallDeferred`
- `Cancelled`
- `Timeout`
- `StateError`
- `StreamError`

Rust errors should convert into these Python exceptions at API boundaries.
Python tool exceptions should convert back into `ToolError` while inside the
runtime tool loop.

## Runtime Validation

P0 runtime validation should prove:

- agent run returns deterministic output
- session run preserves message history
- state export and restore work
- a restored session can run after Python tools are re-registered
- run options can add per-run tools
- Python exceptions produce predictable Python API errors

Streaming and HITL validation should land in the next phase:

- final result through async stream
- tool-call events through async stream
- stream interruption cancels Python tools
- approval resume
- deferred resume
- usage snapshot projection
