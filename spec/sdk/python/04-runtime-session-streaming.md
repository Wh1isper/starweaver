# Runtime, Sessions, Streams, State, And HITL

This spec defines the Python runtime handles after tools are registered:
agents, sessions, live runs, streams, output, state restore, and HITL resume.

## Current Baseline

The package currently exposes:

- `Agent.run(...)`
- `Agent.run_stream(...)`
- `Agent.session(...)`
- `Agent.new_session()`
- `Agent.session_from_state(...)`
- `AgentSession.run(...)`
- `AgentSession.run_stream(...)`
- `AgentSession` async context manager
- `AgentSession.export_state(...)`
- `AgentSession.resume_after_hitl(...)`
- `AgentRun` with `AgentStream` as compatibility alias
- `AgentRun.recv()`
- `AgentRun.join()`
- `AgentRun.result()`
- `AgentRun.interrupt()`
- `AgentRun.steer()`
- `AgentRun.send_message()`
- `AgentRun.messages`
- `AgentRun.hitl()`
- `AgentRun.recoverable_state()`
- `AgentRun.status()`
- `AgentSession.steer(...)`
- `AgentSession.messages`

Current behavior already validates:

- deterministic runs
- session history continuation
- state export and restore
- per-run tools and options
- stream records with raw JSON
- stream interruption
- Python task cancellation while waiting for stream result
- cancellation propagation into Python tools
- raw approval/deferred resume
- typed approval/deferred helpers
- session busy protection while a session stream is active
- active steering without taking the session busy lock

`AgentSession` and `AgentRun` are now the primary Python lifecycle objects.

## Agent Construction

`create_agent(...)` should accept:

- `model`
- `tools`
- `instructions`
- `name`
- `model_settings`
- `request_params`
- `output_schema`
- `output_policy`
- `subagents`
- `subagent_delegation_mode`
- `capability_bundles`

Model inputs should be Starweaver-backed:

- deterministic `TestModel`
- callback-backed `FunctionModel`
- `ProviderModel` helpers over Rust provider adapters
- future registry/profile helpers

Python-defined model adapters are not a priority until tool, session, stream,
HITL, and active control behavior are stable.

## Session Lifecycle

Preferred future shape:

```python
async with create_agent(model=model) as agent:
    async with agent.session() as session:
        first = await session.run("Remember Starweaver")
        state = session.export_state()

    restored = agent.session(state)
    second = await restored.run("What did I mention?")
```

Rules:

- Sessions are explicit stateful conversation objects.
- One active operation per session remains the default.
- Exported state is JSON-compatible and Rust-versioned.
- Process-local Python callables and dependencies are not serialized.
- Tools and bundles must be re-registered before a restored session can use
  them.
- `session.export_state("curated")` is the application default.
- `session.export_state("full")` is for recovery, debugging, and internal
  service boundaries.

## Live Run Lifecycle

The current `AgentStream` should graduate into a public `AgentRun` concept.

`AgentRun` owns:

- event iteration
- final result
- interruption
- status
- recoverable state
- active steering
- message-bus writes
- bound HITL helpers

Context-manager behavior:

```python
async with session.run_stream("Research") as run:
    async for event in run:
        ...
```

- Normal exit joins the run.
- Exceptional exit interrupts the run and preserves recoverable state.
- Cancelling a Python task that awaits `recv`, `join`, or `result` interrupts
  the native run and re-raises `asyncio.CancelledError`.
- Dropping the native stream currently interrupts; public Python APIs should
  prefer explicit context-manager semantics.

## Streaming

`StreamEvent.raw` remains the forward-compatible source of truth.

Current `StreamEvent`:

- `kind`
- `raw`

Target accessors:

- `run_id`
- `step`
- `sideband`
- `sideband_kind`
- `sideband_payload`
- `text_delta`
- `tool_call`
- `tool_return`
- `usage`
- `approval`
- `deferred`
- `is_terminal`

The first typed implementation can use lazy accessors over a single
`StreamEvent` class. Splitting into multiple dataclasses is optional and should
wait for evidence that applications need pattern matching over concrete event
types.

## HITL And Deferred Work

Current public API exposes raw lists:

- `RunResult.pending_approvals`
- `RunResult.pending_deferred`
- `RunResult.pending_deferred_tools`
- `RunResult.needs_approval`
- `RunResult.is_waiting`

Target typed helpers:

```python
waiting = await session.run("Deploy production")
decision = waiting.approvals[0].approve(decided_by="alice")
result = await session.resume_after_hitl(approvals=[decision])
```

Typed objects:

- `PendingApproval`
- `ApprovalDecision`
- `PendingDeferred`
- `DeferredResult`
- `HitlSnapshot`
- `SessionHitl`
- `RunHitl`

Rules:

- Raw dict resume remains available as an escape hatch.
- Typed helpers build canonical Starweaver decisions.
- Approval ids and deferred ids remain visible.
- Live `run.hitl().snapshot()` is valid after a `suspended` event. It may join
  that already-suspended stream to obtain the canonical waiting result.
- `run.hitl().resume_collected(...)` resumes through the owning session and
  returns a collected `RunResult`; it is not a live continuation handle.
- Python does not maintain a second pending-HITL store.

## Output And Validation

Current Python output features include:

- `OutputSchema`
- Pydantic schema helpers
- `OutputPolicy`
- structured output modes
- output validators
- output functions
- `OutputContext`
- `OutputValue`
- output retry exceptions

Rules:

- Rust owns the output retry loop.
- Python validators and output functions are callbacks inside that loop.
- Output callbacks follow the same GIL, event-loop, cancellation, and private
  metadata rules as tools.
- Pydantic should document and validate schemas; it should not replace native
  runtime parsing semantics.

## State Export And Restore

Python should expose two views:

- raw JSON state for persistence and compatibility
- typed helper wrappers for application discoverability

Restore rules:

- Serializable context state restores from `ResumableState`.
- Message history, pending HITL, message bus state, usage state, and run ids are
  owned by the Rust state schema.
- Process-local Python dependencies must be rehydrated by the application.
- Python callables are not serialized.
- Restored sessions must use newly registered tools/toolsets/bundles.
- Full `SessionArchive` payloads may carry the collected pending HITL
  `last_run_state`; curated archives must stay portable and omit that field.

See `08-session-store-and-state.md` for the durable store contract. The short
rule is that application persistence uses full `ResumableState` and native
session/run/stream/HITL records. Python callables, dependencies, live provider
connections, and environment handles are supplied again by the current process.

## Interruption And Recovery

Interruption must preserve a recoverable session state:

- request cooperative cancellation
- stop model streaming or tool execution at the next supported boundary
- cancel Python tool futures when applicable
- repair dangling tool calls
- publish cancellation evidence
- expose `recoverable_state()`

`StreamError` is appropriate when a caller awaits a run that was deliberately
interrupted. The caller can still inspect status and recoverable state.

## Error Categories

`AgentError`, `ModelError`, `ToolError`, `OutputError`, `StateError`, and
`StreamError` should be stable Python API categories.

Specific rules:

- Invalid state transitions raise `StateError`.
- Stream producer failure, receiver closure, and join failure raise
  `StreamError`.
- Runtime model failures raise `ModelError`.
- Tool-loop failures visible at the API boundary raise `ToolError`.
- Output parsing/validator/final-function failures raise `OutputError`.
- Python task cancellation re-raises `asyncio.CancelledError` after requesting
  native interruption.

## Runtime Validation

Required tests for this layer:

- agent run returns deterministic output
- session run preserves message history
- state export and restore continue a session
- restored session can use re-registered Python tools
- per-run tools do not mutate agent defaults
- unknown run options are rejected
- stream yields raw records before completion
- stream context-manager normal exit joins
- exceptional stream context exit interrupts
- interruption cancels running Python tool
- task cancellation while awaiting stream result interrupts
- approval resume succeeds
- deferred resume succeeds
- output validator retry participates in runtime loop
- output function can produce final structured output
