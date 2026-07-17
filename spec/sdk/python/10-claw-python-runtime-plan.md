# Claw Runtime Replication With Starweaver Python

This spec turns the source review of `ya-mono/packages/ya-claw` into an
implementation plan for a Claw-like Python product built on `starweaver-py`.

It is intentionally placed under the Python SDK specs because the product
runtime depends on the Python binding quality. The product code still belongs
above `starweaver-py`: service API, database schema, scheduler, workflow,
memory, agency, bridge, web UI, and Docker retention policy must not become
SDK behavior.

## Verdict

Current `starweaver-py` can replicate the agent execution kernel, but it cannot
replicate all of ya-claw by itself.

Directly usable today:

- `create_agent(...)`
- `create_agent_runtime(...)`
- `AgentRuntime` durable store binding for collected runs and by-id resume
- `Agent`, `AgentSession`, and `AgentRun`
- `AgentSession.run_stream(...)`
- `AgentRun.steer(...)`
- `AgentRun.send_message(...)`
- `AgentRun.interrupt(...)`
- `AgentRun.recoverable_state()`
- `AgentSession.export_full_state()`
- `Agent.session_from_state(...)`
- typed HITL approval and deferred-result helpers
- Python tools and first-party toolsets
- `AbstractToolset`, `PythonDynamicToolset`, `FunctionToolset`, `Toolset`,
  `ToolLibrary`, `ToolSearchToolset`, and `ToolProxyToolset`
- local, virtual, composite, envd-backed, and Python-defined
  `EnvironmentProvider`
- in-memory, JSON, and native SQLite session-store facades
- native SQLite replay-log and stream-archive facades

Required before a faithful product replica:

- product database schema and migrations;
- Python service coordinator and durable queue semantics;
- runtime instance claim and recovery semantics;
- Claw-compatible API and SSE event replay;
- workspace binding and sandbox lifecycle;
- profile resolver and runtime builder;
- product toolsets over a service self-client;
- memory, agency, schedule, workflow, heartbeat, bridge, and web UI product
  layers.

## Reviewed Source Map

The plan is based on the ya-claw source structure below. The implementation
does not copy code directly; it maps behavior to Starweaver-owned contracts.

| Ya-claw area          | Main source shape                                               | Starweaver mapping                                                                                                                               |
| --------------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| app lifecycle         | FastAPI app factory, lifespan, auth, static UI, dispatchers     | Python product service over `starweaver-py`                                                                                                      |
| settings              | Pydantic settings, data dirs, DB URL, workspace backend, bridge | product config, not SDK config                                                                                                                   |
| ORM schema            | profiles, sessions, runs, schedules, workflows, bridge, HITL    | product tables plus native session/stream blobs                                                                                                  |
| run store             | `state.json` and `message.json` artifacts                       | native full `ResumableState`, stream archive, replay log, plus compatibility projections if needed                                               |
| profile resolver      | DB/YAML profile to model, tools, MCP, subagents, policy         | Python resolver that builds `ProviderModel`, `Toolset`, `Subagent`, `EnvironmentProvider`                                                        |
| runtime builder       | `create_agent(...)`, context kwargs, self-client, system prompt | `create_agent_runtime(...)` for store-bound collected runs; `create_agent(...)` plus `AgentSession.run_stream(...)` for live coordinator streams |
| execution supervisor  | queued claim, active handles, startup recovery, shutdown        | Python product coordinator using `AgentSession`, `AgentRun`, `AgentRuntime`, and native store bindings                                           |
| run coordinator       | environment, restore, stream loop, checkpoint, HITL, commit     | Python coordinator over `AgentRun`, `recoverable_state`, HITL helpers, stream adapter                                                            |
| session controller    | idle/queued/running submit semantics and fork                   | product state machine over session/run records                                                                                                   |
| workspace provider    | local/docker binding, virtual paths, sandbox metadata           | product workspace model plus Rust environment providers                                                                                          |
| toolsets              | self-client, background, async tasks, session, schedule         | Python `AbstractToolset`/`PythonDynamicToolset` product toolsets and native wrappers                                                             |
| memory and agency     | internal sessions, fire queues, workspace memory files          | product state machines using Starweaver sessions                                                                                                 |
| schedule and workflow | timer dispatchers and DAG executor                              | product dispatchers and run orchestration                                                                                                        |
| bridge and web UI     | Lark adapter, HITL actions, web console                         | product adapters over the same API/event contract                                                                                                |

## Target Architecture

```mermaid
flowchart TD
    ui["Web UI / bridge clients"]
    api["Python product API"]
    service["Service controllers"]
    coord["ExecutionSupervisor and RunCoordinator"]
    profile["ProfileResolver and RuntimeBuilder"]
    workspace["WorkspaceProvider and sandbox policy"]
    tools["Product toolsets and self-client"]
    py["starweaver Python SDK"]
    native["starweaver._native"]
    rust["Rust runtime/session/stream/environment/tools"]
    db["Product DB"]
    archive["Session/stream/replay storage"]

    ui --> api
    api --> service
    service --> coord
    service --> db
    coord --> profile
    coord --> workspace
    profile --> tools
    profile --> py
    workspace --> py
    tools --> api
    py --> native
    native --> rust
    coord --> archive
    archive --> rust
```

The product layer owns:

- HTTP routes and DTOs;
- SQLAlchemy or equivalent product tables;
- service authentication and CORS;
- runtime instance records;
- scheduler loops;
- workflow, memory, agency, and bridge behavior;
- workspace policy, Docker retention, and UI-specific event projections.

`starweaver-py` owns:

- Python facade objects;
- callback registration;
- native tool/toolset conversion;
- native session and stream storage bindings;
- typed state, stream, HITL, and environment helper objects;
- adapters that project canonical Starweaver evidence without changing it.

Rust owns:

- model request preparation;
- tool scheduling and retries;
- tool approval/deferred control flow;
- session state serialization;
- stream and replay record contracts;
- environment policy enforcement;
- usage and trace primitives.

## Required Rust To Python Bindings

### Storage And Replay

Expose native storage as Python facade classes:

```python
SqliteSessionStore.migrate("claw.db")
store = SqliteSessionStore.open("claw.db")
replay = SqliteReplayEventLog("claw.db")
archive = SqliteStreamArchive("claw.db")
```

Required APIs:

- open by path or URL;
- run migrations and report migration status;
- save/load sessions and runs;
- append/load checkpoints;
- append/replay stream records by cursor;
- append/replay display messages;
- append/load approvals and deferred tool records;
- expose raw record dictionaries for forward compatibility;
- map storage errors to stable Python exceptions.

Rules:

- Python does not duplicate Rust migrations.
- Python can add product tables in its own Alembic migration chain.
- Native session/stream records remain canonical evidence.
- AG-UI compatibility, if required, is a projection over canonical records.

### Durable Runtime Binding

`create_agent_runtime(...)` binds Python or native stores into Rust
`AgentRuntimeBuilder`:

```python
runtime = create_agent_runtime(
    model=profile.model,
    instructions=profile.instructions,
    toolsets=profile.toolsets,
    session_store=store,
    stream_archive=archive,
    replay_event_log=replay,
    durable_session_id=session.session_id,
    state=restore_state,
)
```

This is the SDK-owned durable execution path for collected runs, persisted run
evidence, and by-id HITL resume. A Claw-like live SSE coordinator still owns
active `AgentRun` streaming, cursor fanout, notification delivery, and product
state transitions. It should use the same profile/toolset/environment builder
inputs and append canonical stream records to native stores instead of creating
a parallel runtime loop.

### Session Records And Input Parts

Expose typed wrappers around Rust-owned records:

- `InputPart`
- `SessionRecord`
- `RunRecord`
- `CheckpointRef`
- `ApprovalRecord`
- `DeferredToolRecord`
- `SessionResumeSnapshot`
- session/run status enums

The Python product may add fields such as `session_type`, `source_session_id`,
`trigger_type`, `profile_name`, and `workspace_snapshot` in product tables. It
must not require those fields to become generic SDK session fields.

### Stream Adapters

Expose adapters over `starweaver-stream`:

- raw stream record adapter;
- display-message adapter;
- SSE cursor adapter;
- AG-UI-style adapter;
- replay buffer helper.

Current SDK status: Python `StreamAdapter` exposes raw, display-message,
AG-UI, SSE-frame, replay-window, cursor-range, and replay-buffer projections
over collected or replayed Starweaver stream records. Product code still owns
HTTP auth, live SSE fanout, notification cursors, UI session state, and web
console compatibility.

Rules:

- raw records are always available;
- cursor order is stable and monotonic;
- unknown record kinds pass through;
- adapters never invent alternate run status.

### Environment Providers

Expose enough environment constructors for Claw-style workspace binding:

- `EnvironmentProvider.local(...)` already exists;
- `EnvironmentProvider.virtual(...)` already exists;
- `EnvironmentProvider.envd_local(...)`, `envd_http(...)`, and
  `envd_stdio(...)` wrap Rust envd services and clients;
- `EnvironmentProvider.composite(...)` wraps Rust composite providers;
- `PythonEnvironmentProvider` plus `EnvironmentProvider.from_python(...)`
  adapts product-owned workspace/resource APIs into the Rust environment trait;
- add switchable providers when Rust supports the policy;
- use `WorkspaceBinding`, `VirtualPath`, and `VirtualMount` Python value
  objects over Rust composite providers;
- expose environment state export/import.

Docker container creation and TTL policy remain product code. The environment
provider binding only gives the product a safe executable environment.

### Toolset And MCP Bindings

Available Rust toolset wrappers and constructors through Python:

- prefix;
- filter;
- rename;
- approval-required;
- deferred;
- metadata;
- lazy or dynamic inventory;
- dynamic factories;
- predicate filtering;
- prepared callbacks;
- refresh-specific callbacks;
- lifecycle policy;
- lifecycle reports;
- MCP toolset construction from typed config.

Python product toolsets can be written in Python, but the inventory and
execution path should still be registered as native Starweaver toolsets.

Claw replication must treat product toolsets as first-class profile artifacts,
not as ad hoc per-run `tools=[...]` lists. The product-level interface is:

- use `AbstractToolset` for Starweaver-native dynamic product toolsets;
- use `PythonDynamicToolset` when the code should make the ya-agent-sdk-style
  dynamic-toolset intent explicit;
- give every durable product toolset a stable `name` and `id`;
- return `ToolsetPreparation` from `prepare(ctx)` or `refresh(ctx)`;
- derive availability from `ToolsetContext` and product bindings at prepare
  time, instead of exposing a mutable live runtime context;
- return nested `Toolset`, `FunctionToolset`, MCP, search, or proxy toolsets
  through `ToolsetPreparation.toolsets` when the product surface is composed;
- keep Python callable objects process-local and re-register current toolsets
  from profile IDs on service startup and restore.

Ya-agent-sdk `BaseToolset` and `BaseTool` behavior maps into this contract as
follows:

- `BaseTool.is_available(ctx)` becomes either `prepare(ctx)` inventory
  selection or a `prepared(...)`/`filtered(...)` wrapper;
- `BaseTool.get_instruction(ctx)` becomes
  `ToolsetPreparation.instructions`;
- tool class registration becomes `BaseTool` instances, decorated `Tool`
  values, or `FunctionToolset` members returned from `prepare(ctx)`;
- pre/post hook style behavior should be expressed through Starweaver typed
  wrappers, capability hooks, prepared callbacks, approval wrappers, deferred
  wrappers, or product service code, rather than a Python-only middleware
  stack;
- `get_approval_metadata()` and `process_user_input()` should map to typed HITL
  metadata and approval/deferred result helpers, not Pydantic-AI-specific
  control-flow objects.

### Runtime Context And Lifecycle

Claw uses context extensions and run lifecycle hooks. The Python binding needs
safe equivalents:

- read-only `ToolsetContext` or `AgentContextView`;
- run/session/profile/source metadata;
- injected instruction tags;
- lifecycle callbacks for run start, model request, HITL suspension, run
  commit, failure, and checkpoint;
- resource reference access;
- current environment access;
- usage and trace snapshots.

The context object must not be a mutable live `AgentContext` escape hatch. It
should expose deliberate operations that map to Rust-owned contracts.

## Python Product Modules

A Claw-like Python product can use this module layout:

```text
starweaver_claw/
  app.py
  config.py
  api/
  controller/
  execution/
  orm/
  workspace/
  toolsets/
  memory/
  agency/
  bridge/
  web/
```

`starweaver-py` should not import this package. The product imports
`starweaver`.

## Service Startup Contract

The product app should provide:

- FastAPI application factory;
- lifespan startup and shutdown;
- API token middleware;
- CORS policy;
- static web fallback;
- database engine/session factory;
- migration command;
- ready/doctor payloads;
- notification hub;
- supervisor startup and shutdown;
- startup recovery before accepting new runs;
- optional bridge supervisor.

Startup order:

01. Load settings and ensure data directories.
02. Open product database and run migrations when configured.
03. Open native session/stream/replay stores.
04. Build in-memory runtime state and notification hub.
05. Build workspace provider and environment factory.
06. Build profile resolver and runtime builder.
07. Register runtime instance.
08. Start execution supervisor and run startup recovery.
09. Start schedule, workflow, heartbeat, memory, agency, and bridge dispatchers.
10. Mark service ready.

Shutdown order reverses startup and interrupts or drains active runs according
to product policy.

## Product Database Contract

Product tables should stay product-owned:

- profiles;
- sessions with `session_type`, profile, source, and workspace metadata;
- runs with trigger type, dispatch mode, status, restore source, claim fields;
- runtime instances;
- HITL batches and interactions;
- async tasks;
- memory state;
- agency fires;
- schedules and schedule fires;
- workflow definitions, runs, node runs, and events;
- heartbeat fires;
- bridge conversations, bridge events, and bridge HITL messages.

Native storage should hold:

- full `ResumableState`;
- stream records;
- display messages;
- replay events;
- approval and deferred records;
- checkpoint references.

The product DB can keep denormalized summaries for listing and filtering, but
canonical replay and restore evidence should stay in Starweaver record shapes.

## Runtime Builder Mapping

The product runtime builder resolves a profile into Starweaver objects.

```python
profile = await resolver.resolve(profile_name)
environment = await workspace_factory.environment_for(session, run, profile)
runtime = create_agent_runtime(
    model=profile.model,
    instructions=profile.instructions,
    model_settings=profile.model_settings,
    request_params=profile.request_params,
    runtime_config=profile.runtime_config,
    tools=profile.inline_tools,
    toolsets=profile.toolsets,
    approval_required_tools=profile.approval_required_tools,
    subagents=profile.subagents,
    skills=profile.skills,
    environment=environment,
    session_store=native_store,
    stream_archive=stream_archive,
    replay_event_log=replay_log,
    durable_session_id=session.session_id,
    state=restore_state,
)
```

For live UI/SSE streaming, build an `Agent` with the same resolved values and
run an `AgentSession.run_stream(...)` coordinator. The durable runtime binding
does not replace the product's active-run ownership, notification hub, or
cursor replay contract.

Profile resolution owns:

- model constructor;
- model settings and request params;
- runtime config;
- system and dynamic instructions;
- built-in toolset selection;
- product toolset selection;
- MCP toolsets;
- subagents;
- approval and deferred policy;
- workspace backend hint;
- stream resume policy;
- source-kind policy for schedule, workflow, memory, agency, and bridge runs.

## Execution Coordinator

The coordinator is product code. It wraps `AgentSession` and `AgentRun` for
live streams and may use `AgentRuntime` for collected durable commands or
by-id resume paths.

Main algorithm:

01. Open a DB transaction and atomically claim a queued run.
02. Register an active run handle in process memory.
03. Resolve profile, workspace, trigger source, and restore source.
04. Load native resume state or create a new Starweaver session.
05. Build the Starweaver agent and attach the environment.
06. Start `session.run_stream(...)`.
07. For each stream event:
    - append raw record to native replay/archive;
    - append display or AG-UI projection if required;
    - publish SSE notification;
    - checkpoint recoverable state at model boundaries and on suspension.
08. On HITL suspension:
    - persist pending approvals/deferred tools;
    - expose interactions through API;
    - resume with typed decisions or deferred results.
09. On steering:
    - use `AgentRun.steer(...)` or `AgentRun.send_message(...)`;
    - record accepted receipts;
    - let stream evidence prove runtime consumption.
10. On terminal completion:
    - save full state;
    - update product run/session summaries;
    - dispatch memory, agency, async-task, schedule, or workflow follow-ups.
11. On failure or interruption:
    - save recoverable state when available;
    - update terminal status without erasing prior interrupt/cancel reason.
12. Clear the active handle and notify subscribers.

The service must not mutate exported state dictionaries to fake runtime
progress. Runtime progress enters through Starweaver APIs.

## Session Submit State Machine

The product session controller should preserve the ya-claw behavior:

| Session state               | Submit behavior                                     |
| --------------------------- | --------------------------------------------------- |
| no active run               | create a queued run                                 |
| active queued run           | merge input parts and metadata into that queued run |
| active running run          | append input as steering through the active run     |
| active waiting-for-HITL run | create deferred input or HITL response per endpoint |
| terminal run with restore   | create new queued run from selected restore point   |
| fork request                | create child session with explicit restore source   |

The session lock is a product lock. It prevents concurrent submit decisions
from creating two active runs for the same conversation.

## Workspace Contract

Workspace binding is a product model over environment providers.

Required value objects:

- workspace binding spec;
- mount spec;
- mount binding;
- virtual path;
- default cwd;
- read-only/read-write policy;
- backend hint;
- sandbox state snapshot;
- generation/fingerprint.

Rules:

- mount IDs are stable;
- default cwd is within a mounted virtual path;
- model-facing paths are virtual POSIX paths;
- host paths do not enter provider requests or durable model semantics;
- workspace snapshots persist on each run;
- Docker container IDs and TTL metadata are product runtime metadata;
- run-scoped containers are used for schedule, workflow, heartbeat, memory, and
  agency internal runs when product policy requires isolation;
- session-scoped containers are used for interactive sessions when policy
  allows reuse.

## Product Toolsets

Each product toolset should be a Python-native dynamic toolset registered with
Starweaver. Product code should subclass `AbstractToolset` for the normal public
contract or `PythonDynamicToolset` when the explicit dynamic-toolset name makes
the product boundary clearer. Both enter the runtime through the same native
`PythonDynamicToolset` bridge; Claw-like code should not flatten product
toolsets into static tool lists except for simple leaf bundles.

Authoring requirement: Claw parity must keep a ya-agent-sdk-style toolset
interface. Product features are accepted only when they are expressed as
`AbstractToolset` or `PythonDynamicToolset` subclasses with stable durable IDs,
context-aware `prepare(ctx)`/`refresh(ctx)`, and lifecycle hooks where resources
or clients are opened. A plain `tools=[...]` list is acceptable only for small
leaf bundles that have no product context, no lifecycle, and no durable restore
requirements.

Target product toolset classes:

| Ya-claw source family | Target Starweaver Python class                                   | Notes                                                                 |
| --------------------- | ---------------------------------------------------------------- | --------------------------------------------------------------------- |
| `session.py`          | `ClawSessionToolset(PythonDynamicToolset)`                       | Current-session turns, run trace, compacting, and self-client checks. |
| `async_subagent.py`   | `ClawAsyncSubagentToolset(PythonDynamicToolset)`                 | Durable child sessions, parent wake policy, steering, and cancel.     |
| `background.py`       | `ClawBackgroundDelegateToolset(PythonDynamicToolset)`            | Run-scoped background monitor and message-bus delivery.               |
| `schedule.py`         | `ClawScheduleToolset(PythonDynamicToolset)`                      | Schedule CRUD, once/cron triggers, profile/session inheritance.       |
| `workflow.py`         | `ClawWorkflowToolset(PythonDynamicToolset)`                      | Workflow definitions, runs, node steering, and preset lookup.         |
| `agency.py`           | `ClawAgencyToolset(PythonDynamicToolset)`                        | Agency-only availability, source-session reads, proactive submit.     |
| memory/bridge tools   | `ClawMemoryToolset` and `ClawBridgeHitlToolset(AbstractToolset)` | Product-owned state and typed HITL resume over canonical IDs.         |

- self-client toolset;
- session trace and turn tools;
- background delegate tools;
- durable async subagent tools;
- schedule tools;
- workflow tools;
- agency handoff tools;
- memory tools;
- bridge-aware HITL tools where needed.

Rules:

- product tools call product controllers through an in-process client when
  running in the same service;
- HTTP self-client is only required when the toolset crosses a process boundary;
- `prepare(ctx)` owns context-aware inventory selection and may return
  `ToolsetPreparation` with nested Starweaver toolsets;
- `enter(ctx)`, `refresh(ctx)`, and `exit(ctx)` are the lifecycle hooks for
  service clients, run-scoped resources, and HITL resume preparation;
- ya-agent-sdk-style pre/post call hooks and `_call_tool_func`-style execution
  overrides must map to typed Starweaver native capability/runtime hooks before
  becoming public product API;
- product toolsets must not execute tools outside the Starweaver registry to
  simulate hooks, retries, approval, or deferred control flow;
- tool call IDs, approval IDs, and deferred IDs remain canonical;
- product toolsets expose stable IDs for durable execution;
- product toolsets avoid importing web UI concepts.

## Feature Implementation Order

### Phase 0: Binding Prerequisites

Implement and test the missing `starweaver-py` bindings:

1. typed session/run/input/stream records;
2. stream/display/SSE/AG-UI adapters;
3. Python-native `AbstractToolset`/`PythonDynamicToolset` dynamic bridge,
   function toolset builder, and wrappers;
4. Python-native `PythonEnvironmentProvider` bridge for product-owned
   workspace/resource boundaries;
5. durable `AgentRuntime` binding for Python/native `SessionStore`,
   `StreamArchive`, and `ReplayEventLog` handles;
6. lifecycle/context view binding;
7. usage and trace evidence helpers.

### Phase 1: Minimal Service Runtime

Build the service with:

- settings;
- database;
- app lifecycle;
- auth;
- profiles;
- sessions;
- runs;
- runtime builder;
- execution supervisor;
- stream API;
- HITL API;
- local workspace.

Exit criteria: one interactive session can start, stream, steer, suspend for
approval, resume, complete, and restore after process restart.

Current evidence: `examples/python/claw_product_runtime.py` implements this
minimal product runtime as an example instead of an SDK module. It keeps product
tables and native Starweaver storage in separate SQLite databases, starts and
stops a product runtime instance, seeds a profile, creates sessions and runs,
merges queued input, steers a running `AgentRun`, suspends for typed approval,
restarts the product runtime, resumes from saved HITL state, writes canonical
Starweaver stream/archive/replay records, rebuilds UI-visible state from stored
records, exposes a product API facade with bearer auth plus notification and
run SSE-style replay projections, reports service ready/doctor payloads with
startup recovery, store, supervisor, workspace, notification, and bridge status,
exposes a product service app facade with migration, lifespan, CORS, static
fallback, route, and middleware descriptors, rejects new product work before
readiness, and recovers orphan running runs on startup.

### Phase 2: Workspace And Storage Parity

Add:

- native store integration;
- replay archive;
- workspace binding snapshots;
- envd or Docker-backed execution;
- sandbox status endpoints;
- TTL cleanup;
- startup recovery.

Exit criteria: queued and running runs recover deterministically after service
restart, and workspace state is visible in run details.

Current evidence: the product runtime example now persists a product-owned
virtual workspace snapshot on each run, validates that the default cwd stays
inside mounted virtual paths, builds a Starweaver `WorkspaceBinding` over
virtual environment providers, injects the resulting environment into live and
collected runtimes, records exported environment state in the run snapshot, and
stores sandbox status transitions in run details. The product API facade now
exposes sandbox status and TTL cleanup endpoints; TTL cleanup marks terminal
sandbox records as cleaned without deleting canonical run/session evidence.
Native store integration, replay archive, and startup recovery are already
covered by Phase 1 evidence. Remaining Phase 2 work is envd/Docker-backed
execution and backend-specific TTL resource deletion.

### Phase 3: Product Toolsets And Async Tasks

Add:

- self-client;
- session and trace tools;
- background delegate tools;
- durable async subagent sessions;
- parent wake policy;
- product toolset tests.

Exit criteria: an agent can spawn, inspect, steer, and cancel a durable async
subagent through Starweaver tool calls, with each product tool family expressed
as an `AbstractToolset` or `PythonDynamicToolset` subclass carrying a stable
durable ID.

Current evidence: the product runtime example now splits product service tools
async task tools, and session trace tools into separate Python `AbstractToolset`
families with stable IDs. A deterministic Starweaver run invokes
`spawn_async_task`, `inspect_async_task`, `steer_async_task`, and
`cancel_async_task` through the normal native tool loop, and the tool calls
persist a durable `product_async_tasks` record that survives runtime restart.
Another deterministic run invokes `inspect_session`, `list_session_runs`, and
`inspect_run_trace`; the trace summary reports both product run status and the
canonical stream terminal from stored replay evidence. Product toolsets call the
controller through an in-process `ProductSelfClient`, so the example keeps the
self-client seam above the SDK. The example now also spawns a durable background
task through a Starweaver tool call, dispatches a worker run through the shared
coordinator, records the worker run on the task, completes the task with worker
output, and emits a parent wake notification. Remaining Phase 3 work is richer
background subagent policy and cross-process self-client behavior.

### Phase 4: Schedule, Workflow, Heartbeat

Add:

- schedules and fire records;
- heartbeat runs;
- workflow definitions and DAG executor;
- workflow toolset;
- run/session orchestration policy.

Exit criteria: scheduled and workflow-triggered runs use the same run
coordinator as interactive sessions.

Current evidence: the product runtime example now persists schedule, schedule
fire, heartbeat fire, workflow, workflow run, and workflow node run records in
product-owned tables. Schedule fires, heartbeat fires, and a two-node workflow
dispatch product runs through the same `submit`/`run_next_queued` coordinator
path as interactive sessions, and a `ScheduleWorkflowToolset` lets Starweaver
tool calls inspect schedule fire, heartbeat fire, and workflow run records.
It also provides a product `ProductDispatcher` with deterministic `run_once()`
execution and a start/stop background loop; dispatcher ticks fire active
schedules and heartbeat runs through the same coordinator path. Remaining Phase
4 work is advanced scheduler policy, workflow orchestration, retries, branching,
and larger DAG semantics.

### Phase 5: Memory, Agency, Bridge, Web

Add:

- workspace memory store;
- memory extraction and summary sessions;
- agency singleton session and fire queue;
- bridge controller and Lark adapter;
- web console compatibility.

Exit criteria: UI and bridge clients observe the same canonical run state and
stream evidence as API clients.

Current evidence: the product runtime example now persists product-owned memory
entries, agency singleton sessions, and agency fire records. Memory extraction
and agency fires dispatch internal product runs through the same
`run_next_queued` coordinator as interactive, scheduled, heartbeat, and workflow
runs. A `MemoryAgencyToolset` exposes stable `AbstractToolset` tools for
inspecting memory entries and agency fire records through normal Starweaver tool
calls. It also persists product-owned bridge conversations, bridge events, and
bridge HITL messages; `ProductBridge` publishes pending HITL from canonical
approval IDs and resumes through the typed approval path while preserving the
canonical approval ID. Remaining Phase 5 work is production memory retention
and summarization policy, agency fire queue scheduling, external bridge
adapters, inbound bridge recovery, and web console compatibility.

## Validation Plan

Port ya-claw tests by behavior, not by implementation detail.

Initial tests:

- config and app startup;
- profile seed/resolve;
- input part normalization;
- run queue state machine;
- run store and restore;
- execution success/failure/interruption;
- startup recovery;
- stream/SSE replay;
- steering and terminal steering guard;
- HITL approval/deferred resume;
- session submit merge/steer/create decisions;
- workspace binding validation;
- local environment execution.

Current coverage: `test_claw_product_runtime_example_covers_service_state_machine`
imports the product runtime example and covers service startup, product/native
storage split, profile resolution, queued merge, running steering, HITL
suspension, bridge HITL request/approval projection, restart approval resume,
canonical stream/display/replay replay, doctor/ready payloads,
orphan-running recovery, product-owned workspace
snapshot validation, sandbox status persistence, environment state visibility
in run details, product API sandbox status and TTL cleanup behavior, stable
product toolset IDs, durable async task spawn/inspect/steer/cancel through
Starweaver tool calls, background async worker execution with parent wake
notification, session/run/trace inspection tools over product and canonical
replay evidence, scheduled and heartbeat fire
dispatch through the shared coordinator, workflow node dispatch through the
shared coordinator, schedule/workflow tool inspection, and construction of a
collected `AgentRuntime` through `create_agent_runtime(...)`; product-owned
memory extraction and agency-fire dispatch through the shared coordinator; and
memory/agency tool inspection; product API facade coverage for bearer auth,
session creation, submit, notification SSE replay, and run SSE replay; a product
service app facade for migration, lifespan startup/shutdown, auth middleware,
CORS policy, static fallback, route descriptors, pre-start rejection, structured
doctor payloads, and dispatcher supervisor startup/shutdown; and a product
dispatcher loop for active schedule and heartbeat firing.

Later tests:

- Docker/envd sandbox lifecycle;
- backend-specific TTL resource deletion;
- richer background subagent policy;
- advanced scheduler policy;
- advanced workflow DAG execution;
- production memory retention and summarization;
- agency fire queue scheduling;
- FastAPI packaging around the product API facade;
- external bridge adapters and inbound bridge recovery;
- web build and API client compatibility.

Required Starweaver gates for binding changes:

```bash
cargo test -p starweaver-session --locked
cargo test -p starweaver-stream --locked
cargo test -p starweaver-storage --locked
cargo test -p starweaver-tools --locked
cargo test -p starweaver-agent --locked
uv run pytest packages/starweaver-py/tests
make py-check
```

Spec-only changes should use:

```bash
uv run --with mdformat==1.0.0 --with mdformat-gfm --with mdformat-front-matters --with mdformat-footnote mdformat spec/sdk/python
git diff --check -- spec/sdk/python
```

## Completion Checklist

A Claw-like Python product is not complete until:

- service startup and shutdown are deterministic;
- product migrations are repeatable;
- native Starweaver state and stream records are canonical;
- session submit behavior matches idle, queued, running, HITL, restore, and
  fork cases;
- active steering reaches the running Starweaver context;
- HITL decisions preserve canonical IDs;
- startup recovery handles queued and orphan running runs;
- workspace snapshots are persisted and enforced;
- product toolsets call product controllers without bypassing Starweaver tool
  control flow;
- schedules, workflows, memory, agency, and bridges reuse the same run
  coordinator;
- UI replay can be rebuilt from stored stream/display records;
- no Claw product policy leaks into core `starweaver-py`.
