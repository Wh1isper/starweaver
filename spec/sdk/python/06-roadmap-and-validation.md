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
- `AgentRuntime` and `create_agent_runtime`
- durable `AgentRuntime` binding for Python and native `SessionStore`
- collected durable `AgentRuntime.run_stream`
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
- agent-level `approval_required_tools` mapping to native
  `AgentBuilder::approval_required_tools`
- capability bundles
- subagent registration and delegation
- stream raw record projection and lazy `StreamEvent` accessors
- stream interruption and recoverable state
- raw HITL/deferred resume
- typed HITL approval and deferred helpers
- full-state archive helpers for simple JSON persistence and restore
- `RuntimeConfig`
- `Toolset`, `ToolLibrary`, `ToolSearchToolset`, and `ToolProxyToolset`
- `AbstractToolset` and the `PythonDynamicToolset` native bridge
- `FunctionToolset` with `tool`, `tool_plain`, `add_function`, `add_tool`,
  static instructions, and dynamic instruction callbacks
- toolset wrapper facades for prefix, static include/exclude filtering,
  rename, approval-required, and deferred calls
- `ToolsetLifecyclePolicy` facade for Python dynamic toolsets, including
  enter/read/exit timeout policy and `with_lifecycle(...)`
- typed `McpToolset`, `McpTransport`, and MCP server spec dataclasses over
  Rust `McpToolsetConfig`
- Python `SessionStore`, `InMemorySessionStore`, `JsonSessionStore`, native
  `SqliteSessionStore` facades, and callback-backed native `PythonSessionStore`
  bridge over canonical record JSON
- native `SqliteReplayEventLog` and `SqliteStreamArchive` facades over
  canonical replay/archive JSON
- Rust-owned virtual and local `EnvironmentProvider` wrappers
- callback-backed `PythonEnvironmentProvider` bridge for Python-defined
  providers
- `SkillRegistry` and `SkillPackage` helpers backed by native skill parsing
- `BaseResource`, `ResumableResource`, `InstructableResource`, `ResourceRef`,
  `ResourceRegistry`, and `ResourceRegistryState`
- `MediaUploader`
- `StreamAdapter`
- typed `ProviderAuth` and provider routing convenience helpers

Known product gaps:

- a minimal Claw-like Python product runtime example now covers the SDK-backed
  product facade path for API auth, notification/run SSE replay, service
  readiness, workspace snapshots, sandbox status and TTL cleanup, async task
  tools, scheduler/workflow loops, memory, agency, bridge HITL, and UI replay
  evidence. Full production parity remains above `starweaver-py`: real FastAPI
  packaging, envd/Docker sandbox lifecycle, backend-specific TTL deletion,
  production async subagent policy, external bridge adapters, inbound bridge
  recovery, and web UI compatibility.

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
- Message-bus subscribers preserve independent cursors, target delivery,
  broadcast delivery, and unsubscribe/resubscribe tail semantics.
- Generic active message writes do not become steering unless they use the
  steering API or explicit steering metadata.
- Active message writes do not require taking the session busy lock.
- Terminal runs reject new control input with `StateError`.

Status: implemented for the Python SDK path. Python tests cover active
steering/control receipts and idle message-bus subscriber, target, broadcast,
idempotency, and unsubscribe semantics. CLI reuse of the shared Rust seam
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

Status: implemented for raw-state archive persistence, Python record/store
facades, native `SqliteSessionStore`, Python-to-Rust `SessionStore` trait
callback bridge, and `create_agent_runtime(..., session_store=...)` binding
into `AgentRuntimeBuilder`.

## Milestone C: Native SessionStore Facade

Goal: expose durable session-store records to Python and provide a callback
bridge for custom Python stores without moving restore semantics out of Rust.

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
- native `SqliteSessionStore` wrapping `starweaver-storage` migrations
- native `PythonSessionStore` bridge returned by `SessionStore.to_native()`
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
- Custom Python stores can be adapted into the native Rust `SessionStore` trait.

Status: implemented for Python record/store facades with `InMemorySessionStore`,
`JsonSessionStore`, native `SqliteSessionStore`, and callback-backed
`PythonSessionStore`.

## Milestone D: Toolsets And Dynamic Tool Composition

Goal: add advanced tool composition without changing the Rust runtime loop.

Deliverables:

- `RuntimeConfig`
- Python `Toolset`
- per-run `toolsets=[...]`
- per-run `trace_metadata={...}`
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

Status: implemented for runtime config, static toolsets, dynamic
`AbstractToolset` preparation, `FunctionToolset`, tool libraries, direct tool
search, tool proxy, typed MCP toolset config construction, per-agent toolsets,
per-run toolsets, and per-run trace metadata that enters model request tracing
and result evidence without persisting into session metadata.

## Milestone D1: Python-Native Toolset Builder

Goal: make Python toolset authoring feel like a native Python library while the
Rust runtime remains the execution authority.

Deliverables:

- `AbstractToolset`
- `PythonDynamicToolset`
- `FunctionToolset`
- `@toolset.tool`
- `@toolset.tool_plain`
- `toolset.add_function(...)`
- `toolset.add_tool(...)`
- static toolset instructions
- toolset-level defaults for retry, timeout, strict, sequential, and metadata
- native wrapper facades for prefix, rename, static filter, approval-required,
  deferred, metadata, and lifecycle policy
- read-only `ToolsetContext` for dynamic instructions and factories
- dynamic factory bridge through callback scheduling, timeout, cancellation, and
  lifecycle reporting
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

Status: implemented. Current Python exposes `AbstractToolset`,
`FunctionToolset`, decorator registration, Python dynamic preparation,
toolset-level defaults, dynamic instructions, and native wrappers for prefix,
rename, static include/exclude filtering, predicate filtering, prepared
callbacks, metadata merging,
approval-required, and deferred control flow. Python also exposes
`ToolsetFactory` and `toolset_factory()` so agent-level and per-run toolsets can
be built from `ToolsetContext`, including run-scoped cached factories and
factories that return `ToolsetPreparation(toolsets=[...])`. Rust wrappers now
preserve context-aware `prepare_with_context`, and approved HITL execution
re-prepares context-aware toolsets before running the approved call. Typed MCP
constructors build Rust `McpToolsetConfig` values and expose native deferred MCP
tool calls. `ToolsetLifecyclePolicy` is exposed to Python and passed through
`PythonDynamicToolset`, so Rust enforces enter/read/exit timeouts and lifecycle
toggles. `ToolsetLifecycleReport` and `ToolsetLifecycleState` project Rust
lifecycle sideband events through `StreamEvent` and `StreamAdapter`.
`validate_toolset_ids()` and `ToolLibrary.validate_ids()` validate durable
Python/native toolset identities without materializing dynamic toolsets.
Session archives record required Python profile toolset IDs and
`session_from_archive(...)` validates them against the currently registered
toolsets before restoring; Python callable objects remain process-local and are
not serialized.
`AbstractToolset.refresh()` is called when a Python dynamic toolset is prepared
again for the same run, such as HITL resume, and the bridge emits a refreshed
lifecycle report.
Agent construction also exposes `approval_required_tools=[...]` for
profile-level approval policy over registered native toolsets.

## Milestone E: Environment, Resources, Skills, Media, And Adapters

Goal: expose the surrounding SDK features Python products need without
duplicating Starweaver contracts.

Deliverables:

- `SkillRegistry`
- skill list/load helpers
- Rust-owned and Python-defined environment provider wrappers
- `Environment`, `VirtualEnvironment`, `LocalEnvironment`, and
  `EnvdEnvironment`
- `EnvironmentProvider.virtual(...)` and `EnvironmentProvider.local(...)`
- `EnvironmentProvider.render_context()` for model-facing environment context
  preview
- `EnvironmentProvider.envd_local(...)`, `envd_http(...)`, and
  `envd_stdio(...)`
- `PythonEnvironmentProvider` and `EnvironmentProvider.from_python(...)`
- `FileOperator` and foreground `Shell.execute(...)` facades over the current
  provider
- `Shell` background process snapshot methods over `ProcessShellProvider`
- `WorkspaceBinding`, `VirtualPath`, and `VirtualMount`
- `BaseResource`, `ResumableResource`, `InstructableResource`,
  `ResourceRegistry`, `ResourceRegistryState`, `ResourceRef`, and resumable
  resource wrappers
- `MediaUploader`
- stream adapter helpers over `starweaver-stream`
- provider auth/model constructors over Rust provider and OAuth contracts
- resource refs in Python inputs and tool results
- typed usage snapshot helpers
- typed trace metadata helpers
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

Status: implemented for the current SDK boundary. Current Python exposes
Rust-owned virtual/local environment providers through both the
`EnvironmentProvider` factories and named facades: `Environment`,
`VirtualEnvironment`, `LocalEnvironment`, and `EnvdEnvironment`. It also
exposes native skill registry scan/activate, resource refs, media upload
adapter, stream adapter projections, provider auth/routing helpers, typed
usage/trace helpers, separate file/shell operator facades, and background
process snapshot methods through `EnvironmentProvider.shell`, plus
`WorkspaceBinding`, `VirtualMount`, and `VirtualPath` wrappers over Rust
composite providers. The Python SDK also exposes envd-backed constructors for
in-process `LocalEnvd`, HTTP envd endpoints, stdio child envd processes, and
callback-backed `PythonEnvironmentProvider` bridges for Python-defined
providers. Python tests cover `EnvironmentProvider.render_context()` for
Python-defined providers and local providers where `allowed_paths` grants file
authority while `context_file_tree_roots` narrows the model-facing file tree.
Resource tests cover environment-attached references, dynamic toolset context
registry access, resource base classes, explicit resource state export/restore,
`ResourceRegistryState` round trips and legacy list-state restore, and
`InputPart.file(...)` / `InputPart.binary(...)` accepting `ResourceRef` values
while preserving canonical session input JSON. Media uploader tests cover
successful resource-ref replacement, preflight evidence passed to Python
callbacks, and upload failure metadata without leaking adapter-private URLs into
model-visible content.

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

Status: implemented for the SDK/library path. `examples/python/claw_like_runtime.py`
and the Python package smoke test exercise an equivalent product library path
with `AbstractToolset`, active steering, typed HITL approval resume, native
SQLite session storage, raw stream archive, and replay log without `sw`,
`starweaver-rpc`, MCP, or an external provider. The public Python API
compatibility checklist is captured in `12-api-compatibility-checklist.md` and
validated by the Python package tests. `make py-wheel-smoke` installs the built
wheel into a clean virtual environment and runs deterministic SDK, Claw-like
library-path, and minimal Claw-like product runtime smoke checks against the
installed artifact. The full repository `make ci` gate also validates this
path; full Claw-like product parity remains tracked separately by Milestone G.

## Milestone G: Claw-Like Python Product Runtime

Goal: prove that a Claw-like product can be built on `starweaver-py` without
moving product policy into the SDK.

Deliverables:

- native SQLite session-store, stream-archive, and replay-log bindings exposed
  to Python;
- product database schema and migrations above `starweaver-py`;
- Python service app lifecycle, auth, ready/doctor, and notification hub;
- profile resolver and runtime builder over `create_agent_runtime(...)` for
  store-bound paths and `AgentSession.run_stream(...)` for live coordinator
  streams;
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

Status: partially implemented as a product-layer example.
`examples/python/claw_product_runtime.py` proves the minimal service runtime
shape without importing product policy into `starweaver-py`: product-owned
SQLite tables for sessions, runs, runtime instances, and notifications; a
separate native Starweaver SQLite store for canonical session, stream, archive,
and replay evidence; profile resolution; runtime builder paths over
`create_agent_runtime(...)` and `AgentSession.run_stream(...)`; queued input
merge; active-run steering; HITL suspension; restart restore; typed approval
resume; product-owned bridge conversations, bridge events, and bridge HITL
messages that publish pending approvals from canonical approval IDs and resume
through the typed approval path; product-owned virtual workspace snapshots;
sandbox status in run details; product API sandbox status and TTL cleanup
endpoints that mark terminal sandbox records as cleaned without deleting
canonical run/session evidence; durable async task tools expressed as stable
Python `AbstractToolset` families; background async task worker execution and
parent wake notifications through product-owned task records; session trace
tools over product run details and canonical replay evidence; scheduled,
heartbeat, and workflow-triggered runs through the same coordinator;
product-owned memory extraction and agency-fire runs with a stable
`AbstractToolset` inspection surface; product API facade coverage for bearer
auth, session creation, submit, notification SSE replay, and run SSE replay; UI
replay from stored stream/display records; a product dispatcher loop that fires
active schedules and heartbeat runs through the same coordinator; a product
service app facade for migration, lifespan startup/shutdown, auth middleware
descriptors, CORS policy, static fallback, route descriptors, pre-start
rejection, structured doctor payloads, and dispatcher supervisor lifecycle; and
orphan-running startup recovery.
Remaining Claw parity is still product-layer work: real FastAPI packaging,
envd/Docker sandbox lifecycle and backend-specific TTL resource deletion, real
background async subagent policy, cross-process self-client behavior, advanced
scheduler policy, advanced workflow planning, production memory policies,
external bridge adapters, inbound bridge recovery, and web UI.

## Acceptance Gates

Before the Python SDK is considered application-ready:

01. Python tools execute in process as native Starweaver tools. Current Python
    tests assert sync and async Python tool callbacks run in the current process,
    appear as provider-neutral native tool definitions, and return native
    `ToolReturnPart` records into the next model request.
02. The core Python agent/tool/session path does not shell out to Starweaver
    binaries. Current Python tests statically inspect the core Python facade and
    FFI modules for subprocess and Starweaver CLI shortcuts.
03. The core Python tool path does not use MCP.
    Current Python tests keep MCP references out of the core agent/tool/session
    files while leaving MCP as the explicit `McpToolset` integration surface.
04. Tool schema, result, retry, approval, deferred, cancellation, timeout, and
    private metadata behavior round trip through native Starweaver contracts.
    Current Python tests cover Pydantic and explicit JSON schemas, invalid schema
    rejection, `ToolResult` content/metadata/app/user/private layers, model retry
    re-entry, HITL approval, deferred tool resume, canonical public exception
    mapping, timeout metadata plus coroutine cancellation, stream interruption,
    and private traceback preservation without model-visible leakage.
05. Python callback dispatch does not hold the GIL across Rust runtime awaits.
    Current Python tests statically inspect the PyO3 callback bridges for tools,
    dynamic toolsets, output callbacks, environment providers, media uploaders,
    and Python session stores. The bridges must submit Python coroutines through
    `run_coroutine_threadsafe`, poll from Rust with short `Python::attach(...)`
    sections, avoid `.await` inside those GIL-attached sections, and cancel the
    Python future when the Rust future is dropped.
06. Cancellation propagates into running Python tools. Current Python tests cover
    timeout-driven coroutine cancellation, explicit `stream.interrupt()`,
    `ToolContext.is_cancelled()` visibility, `await ToolContext.cancelled()`
    visibility inside the cancelled callback, cancellation of tasks awaiting
    `recv()`, `join()`, and `result()`, context manager cleanup for unjoined
    active runs, and recoverable state repair after session interruption.
07. Session state export and restore work from Python. Current Python tests cover
    curated versus full state boundaries, stable session IDs, full message history
    and message-bus state, state domains, trace snapshots, metadata,
    `SessionArchive` JSON round trips, `session_from_state(...)`,
    `session_from_archive(...)`, required toolset ID preservation through
    archive JSON and session stores, restore rejection when the current profile is
    missing a required toolset ID, restored sessions continuing with
    process-local callables re-registered by the application, restored runs using
    the current profile approval policy, and restored runs rebinding the current
    environment provider.
08. Streaming yields Python events backed by Starweaver stream records. Current
    Python tests run a real native stream and assert each `StreamEvent.kind`
    comes from its canonical `raw["event"]["kind"]`, with ordered sequences
    preserved through `StreamAdapter`.
09. Raw stream records and raw state remain available. Current Python tests
    assert `StreamAdapter.records()`, replay windows, replay buffers, and SSE
    frame data all preserve the same raw records, while `RunResult.raw_state`
    remains available after joining the stream.
10. HITL can be resumed from Python without parsing hidden Rust internals.
    Current Python tests resume approvals and deferred results through
    `SessionHitl.resume(...)`, `RunHitl.resume_collected(...)`, and raw
    `resume_after_hitl(...)` payloads, and assert typed approval metadata reaches
    the approved tool context.
11. Typed HITL helpers preserve canonical ids and raw escape hatches. Current
    Python tests validate `PendingApproval`, `PendingDeferred`, `HitlSnapshot`,
    `ApprovalDecision`, and `DeferredResult` directly: canonical approval and
    deferred IDs enter resume payloads, metadata is merged predictably, and raw
    pending approval/deferred dicts remain available beside typed helpers.
12. Active steering reaches the active run context, not a stale snapshot. Current
    Python tests steer while a native stream is blocked in a tool call and while
    output validators are paused, assert the next model request sees exactly one
    steering message, and validate control receipts carry the same active
    `run_id` and `session_id` through `AgentSession.steer(...)` and
    `AgentRun.steer(...)`. They also validate idle
    `AgentSession.steer(..., when_idle="queue")` storage and `Agent.steer(...)`
    behavior for zero, one, and ambiguous direct active runs.
13. Message bus helpers preserve idempotency, source, target, topic, template,
    metadata, and subscriber semantics. Current Python tests cover idle
    send/peek/consume, duplicate-ID idempotency, subscriber target routing and
    unsubscribe/resubscribe behavior, topic conflict rejection, active
    non-steering message submission, and canonical storage of source, target,
    template, topic metadata, and custom metadata.
14. Output validators/functions participate in the native output retry loop.
    Current Python tests cover structured-output validator retry, Python
    `OutputFunction` success, Python `OutputFunction` raising `OutputRetry` and
    re-entering the native output loop, output retry stream records, and
    capability bundles contributing output validators/functions to the same
    policy.
15. Provider routing uses typed model/provider settings, not generic metadata.
    Current Python tests cover typed provider overlays and the negative case
    where request/trace metadata keys such as `session_id`, `thread_id`, and
    `x-client-request-id` remain generic metadata instead of becoming
    `provider_settings`.
16. Any FFI or unsafe lint exception is local to `packages/starweaver-py`.
    Current Python tests parse the relevant Cargo manifests and fail if any
    non-PyO3 crate allows unsafe code.
17. Public docs cover only implemented stable APIs. Current Python tests compare
    the stable top-level import index in `docs/python/stability.md` against
    `12-api-compatibility-checklist.md` and `starweaver.__all__`, so documented
    stable names, checklist names, and package exports move together.
18. Deterministic tests do not require live provider credentials. Current
    Python tests statically verify that package tests, wheel smoke, and
    Claw-like deterministic smoke examples do not depend on live provider env
    keys or provider-backed models; `provider_smoke.py` remains an opt-in live
    provider example outside the default validation path. The root pyright gate
    also includes `examples/python` and `scripts/python_wheel_smoke.py`, so the
    executable product-path examples stay type-checked with the package surface.

## Open Decisions

| Decision                     | Recommendation                                                                                                                    |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Public live handle name      | Add `AgentRun`; keep `AgentStream` as alias or lower-level compatibility name.                                                    |
| `Agent.session()`            | Add it as the preferred Python name.                                                                                              |
| `agent.steer(...)`           | Provide only for exactly one direct active run; prefer `run.steer(...)`.                                                          |
| Idle `session.steer(...)`    | Raise by default; `when_idle="queue"` stores steering in the idle session message bus.                                            |
| Stream event typing          | Start with lazy accessors over one class; split dataclasses only when needed.                                                     |
| HITL helper mutability       | Helper methods build decisions; explicit resume remains visible.                                                                  |
| Message bus topic field      | Python can expose `topic`; Rust stores it in `starweaver.topic` metadata unless the core contract changes.                        |
| Python model adapters        | Not before active control and composition APIs are stable.                                                                        |
| Python environment providers | Rust-owned providers plus callback-backed `PythonEnvironmentProvider`; background process extension remains native-provider only. |
| Public docs timing           | Update docs after implementation and tests, not while APIs are speculative.                                                       |

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
make py-wheel-smoke
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
