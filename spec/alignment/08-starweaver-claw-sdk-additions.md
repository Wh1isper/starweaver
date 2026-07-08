# Starweaver Claw Layering and SDK Additions

This document records the best package placement for reusable Starweaver work discovered while designing `starweaver-claw` on top of `starweaver-python`.

The goal is not to keep all missing behavior in `starweaver-claw`. When a capability is already close to existing Starweaver contracts, or when a Rust built-in gives the best reuse, stability, and testability, it should move into the relevant Starweaver crate and be exposed through `starweaver-python`. `starweaver-claw` should keep product orchestration, reference API compatibility, business dispatchers, and external product adapters.

The product implementation plan lives in `../claw/02-python-implementation-plan.md`. No item here should block the first product phase unless that phase explicitly chooses to depend on the generalized abstraction.

## Placement Rules

Use these rules before adding a new API or package:

1. **Rust first for reusable execution contracts**: session records, stream records, environment descriptors, tool bundle schemas, live-control receipts, and HITL records should live in Rust crates when they are product-neutral.
2. **Python as an SDK binding, not a separate source of truth**: `starweaver-python` should expose the Rust contract and add Python ergonomics, adapters, and typing where needed.
3. **Claw owns product semantics**: HTTP routes, compatibility schemas, product DB rows, run queue policy, schedules, workflows, memory, agency, bridge/Lark behavior, notification policy, and migration aliases stay in `starweaver-claw`.
4. **Platform only after protocol generality is clear**: AGUI/A2A/external protocol adapters belong in a future platform layer only after stream/session contracts are stable.
5. **Do not encode Claw status names in SDK contracts**: shared crates should expose neutral records and mapping hooks; Claw maps them to reference-compatible public enums.

## Package Placement Map

| Area                                    | Best Starweaver owner                                                                             | Python exposure                                         | Claw-owned part                                                                                    | Rationale                                                                                                                           |
| --------------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------- | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Live run and durable control            | `starweaver-runtime`, `starweaver-session`, `starweaver-stream`, `starweaver-agent`               | Stable `AgentRun` / `LiveRunHandle` facade              | HTTP control routes, product receipts, queue policy, active-run lookup                             | Control state and terminal semantics are reusable execution contracts. Product routing and status projection are not.               |
| HITL approval/deferred resume           | `starweaver-runtime`, `starweaver-session`, `starweaver-agent`                                    | Typed approval/deferred helpers and durable resume APIs | Product interaction IDs, HITL batches, bridge card state, idempotent API responses                 | Approval/deferred records are runtime evidence; external workflows are product-specific.                                            |
| Typed run/tool context                  | `starweaver-core`, `starweaver-context`, `starweaver-tools`, `starweaver-runtime`                 | Typed attachments on run/tool context                   | Product metadata schema and values                                                                 | Generic JSON-compatible attachments are broadly useful and avoid custom context subclasses.                                         |
| Display, replay, and AGUI projection    | `starweaver-stream`, `starweaver-rpc-core`; future `starweaver-platform` for external protocols   | `StreamAdapter` and projection bindings                 | Product SSE endpoints, reference cursor compatibility, notification fanout                         | Display/replay records are shared; service transport ownership stays at product edge.                                               |
| Reusable sandbox lifecycle              | `starweaver-environment`, `starweaver-envd-core`, `starweaver-envd-client`, `starweaver-envd`     | Environment and sandbox provider bindings               | Workspace API, mount validation, Docker path compatibility, retention/TTL policy until generalized | Environment lifecycle and descriptors are reusable; Claw-specific workspace policy and reference status mapping stay product-owned. |
| First-party host tool bundles           | `starweaver-tools`, `starweaver-agent`, `starweaver-environment`, `starweaver-model`              | Default Python toolsets and capability builders         | Schedule, workflow, agency, bridge, and product async task tools                                   | Filesystem, shell, search, scrape, download, media, task, skill, MCP, and proxy bundles have best reuse as Rust/SDK built-ins.      |
| Storage adapters and migration status   | `starweaver-session`, `starweaver-stream`, `starweaver-storage`                                   | Store constructors and typed records                    | Product SQL schema and migrations                                                                  | Evidence stores are shared; orchestration tables are product truth.                                                                 |
| Capability hooks                        | `starweaver-runtime`, `starweaver-agent`                                                          | Typed hook registration                                 | Product lifecycle hooks and business policies                                                      | Runtime hook points should be stable and minimal; Claw composes product policies around them.                                       |
| Child-run linkage and lightweight tasks | `starweaver-session`, `starweaver-agent`, `starweaver-stream`                                     | Parent/child IDs and task/tool helpers                  | Durable async task records, wake policy, child-session lifecycle                                   | Generic linkage is reusable; product async semantics need validation in Claw first.                                                 |
| Service host helpers                    | `starweaver-stream`, `starweaver-rpc-core`, `starweaver-storage`; optional future support package | Small reusable helpers after validation                 | Execution supervisor, auth, FastAPI app, run claiming, notifications, pruning                      | Only extract narrow helpers proven by multiple product hosts.                                                                       |

## Ranking Criteria

The proposals are ordered by a combined layering score:

1. **Existing Starweaver fit**: capabilities that extend existing Rust contracts or already appear in `spec/ops`, `spec/session`, `spec/stream`, `spec/environment`, or `spec/sdk` rank higher.
2. **Rust built-in reuse**: capabilities with stable schemas, deterministic tests, and broad CLI/SDK/service reuse rank higher.
3. **Claw product specificity**: route compatibility, business dispatch, bridge behavior, and product state machines rank lower as SDK additions and should stay in the Claw plan.
4. **Implementation risk**: small typed contracts and validation suites rank ahead of broad host frameworks.

## Proposal 1: Rust Live-Run and Durable Control Contract

### Best package placement

- `starweaver-runtime`: owns execution state transitions, interruption semantics, recoverable state, and terminal classification.
- `starweaver-session`: stores durable run/session status, checkpoint refs, approval/deferred refs, and resume snapshots.
- `starweaver-stream`: stores replay cursors, terminal markers, and stream archive linkage.
- `starweaver-agent`: exposes the ergonomic SDK `AgentRun` / session handle.
- `starweaver-python`: exposes a stable Python facade with typed receipts and errors.

### General Starweaver work

Freeze a service-safe live-control contract that covers:

- `recv`.
- `join`.
- `status`.
- `steer`.
- `send_message`.
- `interrupt`.
- `recoverable_state`.
- `detach`.
- HITL suspension and resume handoff.

Candidate guarantees:

- Terminal-state idempotency.
- Stable control receipt fields.
- Typed errors for already-finished, detached, no-active-run, receiver-closed, and unsupported-control cases.
- Explicit distinction between live in-process handles and durable resume by ID.
- Recoverable-state semantics on partial stream failure, interrupt, and HITL suspension.

### Claw-owned part

`starweaver-claw` owns HTTP routes, active-run lookup, public product receipts, `queued/running/completed/failed/cancelled` status mapping, queue merge/steer policy, and compatibility behavior for session/run endpoints.

## Proposal 2: HITL Durable Records and Resume Patterns

### Best package placement

- `starweaver-runtime`: emits approval/deferred suspension events and validates resume decisions.
- `starweaver-session`: owns `ApprovalRecord`, `DeferredToolRecord`, and resume snapshot linkage.
- `starweaver-agent`: owns ergonomic resume helpers.
- `starweaver-python`: exposes typed approval/deferred objects and by-ID resume helpers.

### General Starweaver work

Document and test service-safe HITL patterns:

- Mapping runtime pending approval/deferred state to durable records.
- Live in-process resume.
- Durable resume by session/run/interaction record IDs.
- Idempotent repeated response behavior.
- Error taxonomy for stale, duplicate, missing, incompatible, or already-resumed interactions.

### Claw-owned part

`starweaver-claw` owns reference-compatible interaction IDs, HITL batch rows, bridge HITL cards, external action correlation, response API shape, deferred input dedupe, and public interaction metadata.

## Proposal 3: Typed Run Attachments and Tool Context

### Best package placement

- `starweaver-core`: shared metadata key rules, JSON-compatible value constraints, and typed IDs.
- `starweaver-context`: attaches run/session metadata to `AgentContext` and typed dependencies.
- `starweaver-tools`: exposes attachments through tool execution context.
- `starweaver-runtime`: propagates attachments through run construction and hooks.
- `starweaver-python`: exposes Python typed mapping/dataclass helpers.

### General Starweaver work

Add a generic typed context attachment surface for tools and toolsets:

- JSON-compatible `RunAttachments` or `RunContextMetadata`.
- Tool-context access to the same attachment map.
- Toolset factory access to run/session attachments.
- Clear durability rules: JSON-compatible metadata may be persisted; live handles stay process-local typed dependencies.
- Redaction and provider-boundary rules so attachments are not accidentally forwarded to model/provider headers.

### Claw-owned part

`starweaver-claw` owns the product metadata schema: product session/run ID, profile name, trigger/source kind, source metadata, workspace binding snapshot, async task context, schedule metadata, workflow metadata, and bridge metadata.

## Proposal 4: Display, Replay, and AGUI Projection Parity

### Best package placement

- `starweaver-stream`: owns `DisplayMessage`, replay events, stream archives, replay cursors, terminal markers, compaction buffers, sanitizers, and projection traits.
- `starweaver-rpc-core`: owns protocol envelopes and shared host-control projection helpers.
- Future `starweaver-platform`: owns external protocol adapters such as AGUI/A2A when those protocols are generalized beyond one product.
- `starweaver-python`: exposes collected/replayed projection helpers.

### General Starweaver work

Add a display/AGUI parity suite over shared stream contracts:

- Golden raw runtime stream records.
- Expected display messages.
- Expected AGUI events where AGUI is enabled.
- Terminal/error/suspended/HITL/tool-call coverage.
- Cursor and sequence ordering cases.
- Sanitization/redaction snapshots.

Expose missing projection fields in `starweaver-stream`, `starweaver-rpc-core`, and `starweaver-python` if the audit finds product-neutral gaps.

### Claw-owned part

`starweaver-claw` owns live SSE routes, `Last-Event-ID` compatibility, notification fanout, reference event names, and any product-specific stream envelope required by existing clients. `StreamAdapter` remains a projection helper, not the live SSE owner.

## Proposal 5: Reusable Environment Sandbox Lifecycle

### Best package placement

- `starweaver-environment`: owns provider descriptors, capability flags, resource refs, mount descriptors, lifecycle traits, and environment snapshots.
- `starweaver-envd-core`: owns runtime-neutral service DTOs for reusable environments, process state, mount state, operation records, and lifecycle methods.
- `starweaver-envd-client`: exposes client transport for remote/local envd services.
- `starweaver-envd`: provides local reusable implementations.
- `starweaver-python`: exposes provider factories and lifecycle bindings.

### General Starweaver work

Promote the generic part of reusable sandboxes into Rust instead of leaving it as a Claw-only Python wrapper:

- Provider identity and descriptors.
- Mount descriptors with host-visible path, backend-visible path, environment path, and access mode.
- Lifecycle categories: session, run, ephemeral.
- Neutral lifecycle states such as `pending`, `preparing`, `ready`, `running`, `idle`, `stopped`, and `failed`.
- `prepare()`, `stop()`, `inspect()`, and `cleanup_idle()` where supported.
- Capability-driven fallback when a provider does not support explicit lifecycle controls.
- State snapshots that can be stored in session records without serializing daemon-private handles.

### Claw-owned part

`starweaver-claw` owns workspace API compatibility, maximum mount count, duplicate validation, default mount rules, CWD rules, reference public sandbox statuses, Docker image choice, Docker host path compatibility, service-container-to-Docker-daemon mapping, UID/GID policy, retention policy, and TTL cleanup scheduling until a generic implementation exactly covers those needs.

## Proposal 6: First-Party Host Tool Bundles

### Best package placement

- `starweaver-tools`: owns tool schemas, toolset combinators, metadata, approval/deferred control-flow metadata, and proxy/search wrappers.
- `starweaver-agent`: owns built-in SDK bundle registration, policy presets, and tool instructions.
- `starweaver-environment`: backs filesystem and shell tools.
- `starweaver-model`: owns provider-native media/web/search pass-through behavior and request mapping.
- `starweaver-python`: exposes the built-in bundles as Python toolsets.

### General Starweaver work

Actively move product-neutral host tools into Rust/SDK built-ins where reuse is highest:

- Filesystem and shell bundles over `EnvironmentProvider`.
- Search and scrape bundles with injectable clients and deterministic fakes.
- Download and media bundles with bounded HTTP, resource refs, model media capability detection, and fallback media-understanding hooks.
- Task and skill bundles over `AgentContext` and provider-visible skill directories.
- MCP and tool-proxy bundles over shared toolset contracts.
- Session trace/read tools over `SessionStore`, `StreamArchive`, and display projections if the schema is product-neutral.

### Claw-owned part

`starweaver-claw` owns schedule tools, workflow tools, agency tools, bridge tools, and product async task tools because their semantics depend on product controllers and product DB state. Claw may wrap Starweaver built-ins with stable reference-compatible toolset IDs when preserving API/tool schema parity.

## Proposal 7: Storage Backend Adapters and Migration Status

### Best package placement

- `starweaver-session`: owns session/run/checkpoint/approval/deferred record contracts and `SessionStore` traits.
- `starweaver-stream`: owns stream archive, replay log, display/replay record contracts, and cursor semantics.
- `starweaver-storage`: owns concrete SQLite adapters today and future PostgreSQL adapters when a product requires them.
- `starweaver-python`: exposes store constructors and migration status helpers.

### General Starweaver work

Keep evidence persistence backend-neutral and add production adapters where they serve multiple hosts:

- Preserve `SessionStore`, `ReplayEventLog`, and `StreamArchive` as backend-neutral traits.
- Add migration status/reporting helpers for evidence stores.
- Add PostgreSQL adapters in `starweaver-storage` when a concrete product requires unified deployment storage.
- Keep append idempotency, replay cursor behavior, and schema migrations covered by Rust contract tests.

### Claw-owned part

`starweaver-claw` owns product orchestration tables, product migrations, public query indexes, database compatibility decisions, and links from product rows to Starweaver evidence IDs/cursor ranges. Product DB tables must remain separate from runtime evidence tables even if both use one PostgreSQL database.

## Proposal 8: Runtime Capability Hook Expansion

### Best package placement

- `starweaver-runtime`: owns hook points in the agent loop and their mutability/retry/durability semantics.
- `starweaver-agent`: owns ergonomic capability builders and policy presets.
- `starweaver-python`: exposes typed hook registration where the Rust contract is stable.

### General Starweaver work

Expand hooks only where the boundary is stable and product-neutral:

- Before prompt/instruction assembly.
- After prompt/instruction assembly.
- Before model request.
- After model response.
- Before tool execution.
- After tool execution.
- Before output validation.
- After run commit.
- Before/after compaction or summary when those operations become shared runtime/session behaviors.

Every hook must define mutability, retry semantics, cancellation behavior, persistence guarantees, and whether it can affect provider-bound requests.

### Claw-owned part

`starweaver-claw` owns product lifecycle hooks around profile resolution, memory injection, agency observation, bridge state updates, schedule/workflow dispatch, terminal product commit, and product DB side effects. These should not become generic runtime hooks until multiple products need the same contract.

## Proposal 9: Shared Child-Run Linkage and Lightweight Task Primitives

### Best package placement

- `starweaver-session`: owns optional parent/child session and run linkage fields when product-neutral.
- `starweaver-agent`: owns lightweight task helpers and SDK subagent ergonomics.
- `starweaver-stream`: links child stream/replay scopes to parent scopes.
- `starweaver-python`: exposes child-run linkage and task helper objects.

### General Starweaver work

Add only the reusable substrate first:

- Parent-child session/run linkage helpers.
- Child stream/event linkage to parent scopes.
- Optional lightweight task record shape for SDK task tools where it does not imply product orchestration.
- Result handoff references that point to run/session/stream evidence.
- Cancellation/steering wrappers that delegate to existing live-control contracts.

This should not force native subagents into durable background mode. Blocking native subagents and product-managed child runs remain distinct.

### Claw-owned part

`starweaver-claw` owns durable async task records, wake policies, unique `(parent_session_id, name)` behavior, task statuses, spawn/list/get/steer/cancel APIs, terminal task status updates, and product-specific result handoff behavior.

## Proposal 10: Service Host Extraction Candidates

### Best package placement

- `starweaver-stream`: owns replay buffers, replay cursors, terminal markers, and compaction helpers.
- `starweaver-rpc-core`: owns host-control protocol envelopes and typed method/error/event contracts.
- `starweaver-storage`: owns reusable local evidence storage adapters.
- A future optional support package may own narrow host helpers only after at least one product host validates them.

### General Starweaver work

Do not create a broad service framework up front. Extract only narrow helpers when their shape is proven:

- `LiveStreamBuffer` over `ReplayEventLog` / `StreamArchive`.
- SSE/WebSocket replay cursor helpers if they stay protocol-neutral.
- Graceful shutdown groups.
- Runtime instance descriptors if multiple hosts need the same heartbeat contract.
- Run supervisor building blocks only if they do not encode product queue or API semantics.

### Claw-owned part

`starweaver-claw` owns the FastAPI app, auth middleware, run queue claiming, runtime instance heartbeat rows, startup recovery policy, product notifications, pruning, dispatcher startup/shutdown, and reference-compatible HTTP/SSE behavior.

## Claw Plan Items Not to Promote Prematurely

The following must remain in `../claw/02-python-implementation-plan.md` unless later evidence shows a clean reusable contract:

- Reference-compatible HTTP route inventory, request/response schemas, enums, and API tokens.
- Product SQL schema, migrations, DB compatibility views, and query indexes.
- `ExecutionSupervisor`, run claiming, queued-run merge policy, active-run selection, orphan recovery, and terminal product commit ordering.
- Live SSE endpoints, notification hub, and reference cursor behavior.
- Workspace manager compatibility rules, Docker image policy, service-to-Docker-daemon path mapping, public sandbox statuses, and TTL scheduling.
- Schedules, heartbeat, workflows, memory, agency, bridge/Lark adapters, and their dispatch records.
- Product async task records and wake policies.
- Product toolsets that call product controllers: schedule, workflow, agency, bridge, background, and product session tools.
- `STARWEAVER_CLAW_*` settings, field-specific `YA_CLAW_*` migration aliases, and deployment defaults.

## Prioritization

| Rank | Priority | Proposal                                                 | Best owner                                                            | Claw owns                                            | Reason                                                                                   |
| ---- | -------- | -------------------------------------------------------- | --------------------------------------------------------------------- | ---------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| 1    | P0       | Rust live-run and durable control contract               | `runtime` + `session` + `stream` + `agent` + Python binding           | HTTP control, queue policy, public receipts          | Foundational and already aligned with durable service runtime specs.                     |
| 2    | P0       | HITL durable records and resume patterns                 | `runtime` + `session` + `agent` + Python binding                      | Interaction API, bridge cards, product IDs           | Approval/deferred state is runtime evidence; external workflow remains product-specific. |
| 3    | P0       | Typed run attachments and tool context                   | `core` + `context` + `tools` + `runtime` + Python binding             | Product metadata schema                              | Small, elegant, generic contract that avoids custom context subclasses.                  |
| 4    | P1       | Display/replay and AGUI projection parity                | `stream` + `rpc-core` + Python binding; future `platform`             | SSE and reference cursor behavior                    | Shared stream semantics are reusable across CLI, service hosts, and platform adapters.   |
| 5    | P1       | Reusable environment sandbox lifecycle                   | `environment` + `envd-core` + `envd-client` + `envd` + Python binding | Workspace API, Docker policy, compatibility statuses | Generic lifecycle/descriptors belong in Rust; product workspace policy stays in Claw.    |
| 6    | P1       | First-party host tool bundles                            | `tools` + `agent` + `environment` + `model` + Python binding          | Schedule/workflow/agency/bridge/background tools     | Product-neutral tools have best reuse as Rust/SDK built-ins.                             |
| 7    | P1       | Storage backend adapters and migration status            | `session` + `stream` + `storage` + Python binding                     | Product DB and migrations                            | Evidence storage is shared; orchestration storage is product truth.                      |
| 8    | P2       | Runtime capability hook expansion                        | `runtime` + `agent` + Python binding                                  | Product lifecycle hooks                              | Useful, but must follow stable Rust hook contracts and avoid leaky seams.                |
| 9    | P2       | Shared child-run linkage and lightweight task primitives | `session` + `agent` + `stream` + Python binding                       | Durable async task records and wake policy           | Reusable linkage is safe; product async orchestration should be validated in Claw.       |
| 10   | P3       | Service host extraction candidates                       | `stream` + `rpc-core` + `storage`; possible future support package    | Supervisor, auth, app, notifications, pruning        | Extract narrow helpers only after Claw proves common host shapes.                        |

## Recommended Execution Order

- **P0**: stabilize reusable execution contracts: live-run control, HITL durable resume, and typed run/tool attachments.
- **P1**: deepen Rust built-ins with broad reuse: display/replay projections, reusable environment lifecycle, host tool bundles, and storage adapters.
- **P2**: add advanced extension surfaces after concrete gaps are proven: capability hooks and child-run/task primitives.
- **P3**: extract service host helpers only after `starweaver-claw` validates the common shapes.

## Acceptance Principle

Generalize when the shared contract is Starweaver-native, product-neutral, and testable in Rust. Keep behavior in `starweaver-claw` when it is reference API compatibility, product orchestration, deployment policy, external adapter behavior, or a business workflow.
