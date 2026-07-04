# Roadmap And Validation

This spec defines the current baseline, next milestones, and validation gates
for treating `starweaver-py` as application-ready.

## Current Baseline

The repository already has a working Python package under
`packages/starweaver-py` with PyO3/maturin packaging and tests.

Implemented baseline:

- package name `starweaver`
- extension module `starweaver._native`
- CPython 3.11 through 3.13 package metadata
- deterministic `TestModel`
- callback-backed `FunctionModel`
- provider model helpers
- `create_agent`
- `Agent.run`
- `Agent.run_stream`
- `Agent.new_session`
- `Agent.session`
- `Agent.session_from_state`
- `AgentSession.run`
- `AgentSession.run_stream`
- `AgentSession` async context manager
- `AgentSession.export_state`
- `AgentSession.export_full_state`
- `AgentSession.resume_after_hitl`
- `SessionArchive`
- `Agent.session_from_archive`
- `AgentRun` public live-run facade with `AgentStream` compatibility alias
- `AgentRun.steer`
- `AgentRun.send_message`
- `AgentRun.messages`
- `AgentSession.steer`
- `AgentSession.messages`
- optional guarded `Agent.steer`
- `BusMessage`, `MessageDelivery`, `MessageBus`, and `ControlReceipt`
- neutral Rust active-control handle and drain capability for live runs
- stream evidence for submitted and received steering
- Python tool injection
- Pydantic/type-hint schema extraction
- `ToolContext`
- `ToolResult`
- Python exception mapping
- default parallel tool execution
- duplicate tool-name sequential fallback
- `sequential=True`
- output policies, validators, and functions
- capability bundles
- subagent registration and delegation
- stream raw record projection and lazy `StreamEvent` accessors
- stream interruption and recoverable state
- raw HITL/deferred resume
- typed HITL approval and deferred helpers
- full-state archive helpers for simple JSON persistence and restore
- `RuntimeConfig`
- `Toolset`, `ToolLibrary`, `ToolSearchToolset`, and `ToolProxyToolset`
- Python `SessionStore`, `InMemorySessionStore`, and `JsonSessionStore`
  facades over canonical record JSON
- Rust-owned virtual and local `EnvironmentProvider` wrappers
- `SkillRegistry` and `SkillPackage` helpers backed by native skill parsing
- `ResourceRef` and `ResourceRegistry`
- `MediaUploader`
- `StreamAdapter`
- typed `ProviderAuth` and provider routing convenience helpers

Known product gaps:

- no native `SqliteSessionStore` Python facade yet
- no Python implementation of the Rust `SessionStore` trait callback bridge
- no envd-backed Python environment constructor yet
- no separate `FileOperator`, `Shell`, `WorkspaceBinding`, `VirtualPath`, or
  `VirtualMount` facade yet
- no typed usage snapshot or trace metadata helper facade yet
- no Python-defined environment provider trait bridge yet

## Milestone A: Polish The Current Python Surface

Goal: make the already implemented SDK feel deliberate and stable before adding
new control primitives.

Deliverables:

- `Agent.session(state=None)` alias
- `AgentSession.__aenter__` and `AgentSession.__aexit__`
- `AgentRun` Python facade over current `AgentStream`
- keep `AgentStream` as alias or low-level compatibility name
- typed `RunStatusSnapshot`
- lazy `StreamEvent` convenience accessors over raw records
- typed `PendingApproval`, `PendingDeferred`, `ApprovalDecision`, and
  `DeferredResult`
- `RunResult.approvals`, `RunResult.deferred`, and `RunResult.hitl`
- raw dict result fields preserved
- `.pyi` signatures updated
- docs updated only for implemented stable APIs

Validation:

```bash
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```

Exit criteria:

- Existing user code keeps working.
- New context-manager/session/run names have tests.
- Typed HITL helpers can resume approval and deferred flows.
- Raw result and raw stream escape hatches remain available.

Status: implemented with Python tests in `packages/starweaver-py/tests`.

## Milestone B: Pythonic Active-Run Control

Goal: make live steering, interruption, and message-bus writes available through
Python SDK objects.

Deliverables:

- neutral Rust `AgentControlHandle` or equivalent in `starweaver-agent`
- pending control queue for active runs
- drain capability that injects queued `BusMessage` values into the active
  `AgentContext`
- `AgentRun.steer(...)`
- `AgentRun.send_message(...)`
- `AgentRun.messages`
- `AgentSession.steer(...)`
- `AgentSession.messages`
- optional guarded `Agent.steer(...)`
- `BusMessage`
- `MessageDelivery`
- `MessageBus`
- `ControlReceipt`
- stream evidence for submitted/received steering
- migration path for CLI steering to reuse the shared Rust seam when practical

Validation:

```bash
cargo test -p starweaver-agent --locked
cargo test -p starweaver-runtime --test context --locked
uv run pytest packages/starweaver-py/tests
make py-check
make fmt-check
make check
make test
git diff --check
```

Exit criteria:

- Python can steer an active stream from another task.
- The steering message reaches the active runtime context.
- Late steering triggers the existing steering guard path.
- Message-bus ids remain idempotent after drain.
- Generic active message writes do not become steering unless they use the
  steering API or explicit steering metadata.
- Active message writes do not require taking the session busy lock.
- Terminal runs reject new control input with `StateError`.

Status: implemented for the Python SDK path. CLI reuse of the shared Rust seam
remains a follow-up migration item.

## Milestone B1: Full-State Archive Helpers

Goal: make durable Python persistence explicit without introducing a custom
store backend.

Deliverables:

- `AgentSession.export_full_state()`
- `SessionArchive.from_session(session)` with full state by default
- `SessionArchive.to_dict()`, `from_dict()`, `to_json()`, `from_json()`,
  `save()`, and `load()`
- `Agent.session_from_archive(...)`
- tests proving curated export omits full runtime extensions while full export
  preserves message history and message-bus state

Validation:

```bash
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```

Status: implemented for raw-state archive persistence and Python record/store
facades. Native `SqliteSessionStore` and Python-to-Rust `SessionStore` trait
bridges remain future work.

## Milestone C: Native SessionStore Facade

Goal: expose durable session-store records to Python without adding a custom
Python-to-Rust backend first.

Deliverables:

- `SessionRecord`
- `RunRecord`
- `StreamRecord`
- `CheckpointRef`
- `ApprovalRecord`
- `DeferredToolRecord`
- `SessionResumeSnapshot`
- `SessionStore` base facade
- `InMemorySessionStore`
- `JsonSessionStore`
- optional native `SqliteSessionStore` when storage migrations can be exposed
  safely
- `save_current_session(session, store=...)` captures full state

Validation:

```bash
cargo test -p starweaver-session --locked
cargo test -p starweaver-storage --locked
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```

Exit criteria:

- Python can save/load session and run records with raw JSON preserved.
- Full context state round trips through a store helper.
- Stream records remain ordered.
- Pending HITL records preserve canonical IDs.
- Python callables and live objects are not serialized.

Status: implemented for Python record/store facades with `InMemorySessionStore`
and `JsonSessionStore`. Native SQLite wrapping remains open.

## Milestone D: Toolsets And Dynamic Tool Composition

Goal: add advanced tool composition without changing the Rust runtime loop.

Deliverables:

- `RuntimeConfig`
- Python `Toolset`
- per-run `toolsets=[...]`
- `ToolLibrary`
- `ToolSearchToolset`
- persisted loaded tool/namespace IDs
- typed tool-search initialization sideband events
- `ToolProxyToolset` with fixed `search_tools` and `call_tool` surface

Validation:

```bash
cargo test -p starweaver-tools --locked
cargo test -p starweaver-agent --locked
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```

Exit criteria:

- Python can compose agents and runs with named toolsets.
- Toolset instructions enter the native instruction path.
- Search and proxy state persists serializable IDs/namespaces.
- Search and proxy remain distinct APIs.

Status: implemented for runtime config, static toolsets, tool libraries, direct
tool search, tool proxy, per-agent toolsets, and per-run toolsets.

## Milestone D1: Python-Native Toolset Builder

Goal: make Python toolset authoring feel like a native Python library while the
Rust runtime remains the execution authority.

Deliverables:

- `FunctionToolset`
- `@toolset.tool`
- `@toolset.tool_plain`
- `toolset.add_function(...)`
- `toolset.add_tool(...)`
- static toolset instructions
- toolset-level defaults for retry, timeout, strict, sequential, and metadata
- native wrapper facades for prefix, rename, static filter, approval-required,
  deferred, metadata, and lifecycle policy
- read-only `ToolsetContext` for later dynamic instructions and factories
- dynamic factory bridge only after callback scheduling, timeout, cancellation,
  and lifecycle reporting are implemented
- durable toolset ID validation helpers

Validation:

```bash
cargo test -p starweaver-tools --locked
cargo test -p starweaver-agent --locked
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```

Exit criteria:

- Python authors can define grouped tools with decorators.
- Toolset wrappers chain through Rust-owned wrappers.
- Toolset IDs are stable enough for durable products.
- Dynamic callbacks do not create a Python-only preparation pipeline.
- Approval, deferred, retry, timeout, cancellation, and stream evidence remain
  native Starweaver behavior.

Status: specified in `11-python-native-toolsets.md`; implementation pending.

## Milestone E: Environment, Resources, Skills, Media, And Adapters

Goal: expose the surrounding SDK features Python products need without
duplicating Starweaver contracts.

Deliverables:

- `SkillRegistry`
- skill list/load helpers
- Rust-owned environment provider wrappers
- `EnvironmentProvider.virtual(...)` and `EnvironmentProvider.local(...)`
- future `EnvdEnvironment` constructor
- `FileOperator`, `Shell`, `WorkspaceBinding`, `VirtualPath`, `VirtualMount`
- `ResourceRegistry`, `ResourceRef`, and resumable resource wrappers
- `MediaUploader`
- stream adapter helpers over `starweaver-stream`
- provider auth/model constructors over Rust provider and OAuth contracts
- resource refs in Python inputs and tool results
- usage snapshot helpers
- trace metadata helpers
- Python logging/OTel convenience only after redaction policy is clear

Validation:

```bash
uv run pytest packages/starweaver-py/tests
make py-check
make docs-check
make fmt-check
make check
make test
git diff --check
```

Exit criteria:

- Python can compose agents with tools, bundles, toolsets, subagents, skills,
  and environment-backed resources through Starweaver-owned contracts.
- Usage and trace evidence can be inspected without raw JSON path walking.
- Environment/resource helpers respect Starweaver policy and restore rules.

Status: partially implemented. Current Python exposes Rust-owned virtual/local
environment providers, native skill registry scan/activate, resource refs,
media upload adapter, stream adapter projections, provider auth/routing helpers,
and environment attachment at agent/session/run scope. Envd construction,
separate file/shell operator facades, typed usage/trace helpers, and
Python-defined provider bridges remain open.

## Milestone F: Application And Release Readiness

Goal: make the package reliable for applications and releasable through the
workspace release flow.

Deliverables:

- public `docs/python-sdk.md` updated for stable APIs
- migration guide for older provisional Python APIs when needed
- wheel smoke tests for supported platforms
- Claw integration example or test app
- release workflow validation for Python distributions
- API compatibility checklist for public Python names

Validation:

```bash
make py-check
make docs-check
make fmt-check
make check
make test
make ci
```

Exit criteria:

- Python package can be built and smoke tested from release artifacts.
- Public docs describe implemented stable behavior.
- Claw or an equivalent Python app can run the intended library path without
  MCP, JSON-RPC, or a sidecar binary.

## Milestone G: Claw-Like Python Product Runtime

Goal: prove that a Claw-like product can be built on `starweaver-py` without
moving product policy into the SDK.

Deliverables:

- native SQLite session/stream/replay bindings exposed to Python;
- product database schema and migrations above `starweaver-py`;
- Python service app lifecycle, auth, ready/doctor, and notification hub;
- profile resolver and runtime builder over `create_agent(...)`;
- durable execution supervisor and run coordinator;
- session submit state machine for idle, queued, running, HITL, restore, and
  fork cases;
- stream/SSE replay and optional AG-UI projection over canonical records;
- HITL controller using typed approval/deferred helpers;
- workspace binding model and local/envd/Docker-backed environment factory;
- product toolsets for self-client, sessions, async tasks, schedule, workflow,
  memory, agency, and bridge;
- startup recovery for queued and orphan running runs.

Validation:

```bash
cargo test -p starweaver-session --locked
cargo test -p starweaver-stream --locked
cargo test -p starweaver-storage --locked
uv run pytest packages/starweaver-py/tests
make py-check
```

Exit criteria:

- A service process can start, create a session, stream a run, steer it, suspend
  for HITL, resume it, complete it, persist full state, and recover after
  restart.
- Product APIs rebuild UI-visible state from stored stream/display records.
- Product tables hold product policy while Starweaver records remain canonical
  restore and replay evidence.
- Schedules, workflows, memory, agency, and bridges reuse the same execution
  coordinator instead of creating parallel runtime loops.

Status: specified in `10-claw-python-runtime-plan.md`; implementation pending.

## Acceptance Gates

Before the Python SDK is considered application-ready:

01. Python tools execute in process as native Starweaver tools.
02. The core Python agent/tool/session path does not shell out to Starweaver
    binaries.
03. The core Python tool path does not use MCP.
04. Tool schema, result, retry, approval, deferred, cancellation, timeout, and
    private metadata behavior round trip through native Starweaver contracts.
05. Python callback dispatch does not hold the GIL across Rust runtime awaits.
06. Cancellation propagates into running Python tools.
07. Session state export and restore work from Python.
08. Streaming yields Python events backed by Starweaver stream records.
09. Raw stream records and raw state remain available.
10. HITL can be resumed from Python without parsing hidden Rust internals.
11. Typed HITL helpers preserve canonical ids and raw escape hatches.
12. Active steering reaches the active run context, not a stale snapshot.
13. Message bus helpers preserve idempotency, source, target, topic, template,
    metadata, and subscriber semantics.
14. Output validators/functions participate in the native output retry loop.
15. Provider routing uses typed model/provider settings, not generic metadata.
16. Any FFI or unsafe lint exception is local to `packages/starweaver-py`.
17. Public docs cover only implemented stable APIs.
18. Deterministic tests do not require live provider credentials.

## Open Decisions

| Decision                     | Recommendation                                                                                             |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Public live handle name      | Add `AgentRun`; keep `AgentStream` as alias or lower-level compatibility name.                             |
| `Agent.session()`            | Add it as the preferred Python name.                                                                       |
| `agent.steer(...)`           | Provide only for exactly one direct active run; prefer `run.steer(...)`.                                   |
| Idle `session.steer(...)`    | Raise by default; optionally support `when_idle="queue"`.                                                  |
| Stream event typing          | Start with lazy accessors over one class; split dataclasses only when needed.                              |
| HITL helper mutability       | Helper methods build decisions; explicit resume remains visible.                                           |
| Message bus topic field      | Python can expose `topic`; Rust stores it in `starweaver.topic` metadata unless the core contract changes. |
| Python model adapters        | Not before active control and composition APIs are stable.                                                 |
| Python environment providers | Start with Rust-owned providers; add Python-defined providers later.                                       |
| Public docs timing           | Update docs after implementation and tests, not while APIs are speculative.                                |

## Review Checklist

- Does the change preserve Starweaver-native ownership boundaries?
- Does the Python surface feel natural to Python application authors?
- Does it avoid RPC/MCP/binary paths for the core library flow?
- Is every Python convenience mapped to a Rust-owned contract?
- Does live control enter the active runtime loop?
- Are raw records and ids still visible?
- Are process-local Python dependencies separated from resumable state?
- Is callback cancellation testable?
- Does the message bus stay MQ-like rather than becoming a UI event stream?
- Are docs/specs updated in the right layer?

## Validation Commands

For Python package changes:

```bash
make py-check
```

For spec-only changes:

```bash
git diff --check
```

For docs example changes:

```bash
make docs-check
```

For repository-wide behavior changes:

```bash
make fmt-check
make check
make test
```
