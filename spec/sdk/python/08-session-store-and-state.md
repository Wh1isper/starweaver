# Session Store And State

This spec defines the durable Python state contract for `starweaver-py`. It
inherits the source-backed advanced design and makes it part of the root Python
SDK contract.

## Verdict

Python can support `SessionStore`-class behavior in the current Starweaver
architecture, but only at the correct boundary:

- Python stores JSON-compatible Starweaver records and full `ResumableState`.
- Rust owns `AgentContext` restore semantics.
- Python callables, Python dependencies, live environment handles, local process
  handles, provider connections, and live OAuth token sources are re-registered
  or rebound by the current process.
- Runtime policy such as approval, sandboxing, provider routing, and security
  comes from the current profile unless a trusted administrative path
  explicitly restores it.

The boundary is not "pickle `AgentContext`". It is typed JSON records plus
Rust-owned restore.

## Rust Evidence

| Area                    | Rust owner                                                                       | Python implication                                                                                   |
| ----------------------- | -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Message serialization   | `starweaver-model::ModelMessage`                                                 | Persist the serde JSON shape; do not invent Python message formats.                                  |
| Context state           | `starweaver-context::ResumableState`                                             | Use full state for durable restore; curated state is not enough for recovery.                        |
| Context restore         | `AgentContext::from_state`, `export_state`, `export_full_state`, `restore_state` | Python restore calls Rust instead of reconstructing a live context.                                  |
| Durable records         | `starweaver-session::SessionStore`                                               | Python store facades mirror native session, run, checkpoint, stream, approval, and deferred records. |
| Stream replay           | `starweaver-stream`                                                              | Python adapters project stream records; raw records remain canonical.                                |
| Storage implementations | `starweaver-storage`                                                             | Python may expose native-backed stores when the Rust store exists.                                   |

## Export Modes

| Mode      | Intended use                                                                                                                                                    | Store implication                                                                     |
| --------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `curated` | Lightweight portable context snapshots. Runtime extensions such as message history, message bus state, usage, trace, and internal state domains may be omitted. | Not sufficient as the only durable store snapshot.                                    |
| `full`    | Product persistence, recovery, debugging, service boundaries, pending HITL restore, and replay evidence.                                                        | Required for `SessionArchive` default persistence and all session-store save helpers. |

`AgentSession.export_state()` may remain curated for portable application
snapshots. Durable helpers must either call `export_full_state()` or make full
state explicit in their names.

## Serializable Boundary

| Domain                           | Persist from Python?           | Restore owner                | Rule                                                                   |
| -------------------------------- | ------------------------------ | ---------------------------- | ---------------------------------------------------------------------- |
| `ModelMessage` history           | Yes                            | Rust                         | Persist the serde-backed message history from full `ResumableState`.   |
| `ResumableState`                 | Yes                            | Rust                         | Preferred context restoration record.                                  |
| session and run records          | Yes                            | Rust or app store            | Python wrappers preserve raw JSON.                                     |
| stream records                   | Yes                            | Rust or app store            | Required for replay, UI adapters, and resume evidence.                 |
| checkpoints                      | Yes                            | Rust runtime                 | Preserve native executor evidence and references.                      |
| approvals and deferred tools     | Yes                            | Rust HITL/session layer      | Python helpers build decisions; IDs stay canonical.                    |
| environment state refs           | Yes                            | Environment provider         | Persist refs, revisions, and metadata, not live provider objects.      |
| resource state                   | Only when explicitly resumable | Environment/resource factory | Restore through provider/factory semantics.                            |
| Python callables                 | No                             | Application                  | Re-register tools, validators, output functions, and hooks.            |
| Python dependencies              | No                             | Application                  | Rehydrate process-local dependencies after restore.                    |
| live process handles             | No                             | Environment provider         | Snapshots can describe processes; live reattachment is backend policy. |
| provider credentials/connections | No generic persistence         | Provider/OAuth layer         | Use typed provider settings and OAuth stores.                          |
| security and approval policy     | Usually no                     | Current profile              | Old archives must not silently weaken current policy.                  |

## Public API Contract

### Raw State

The low-level API remains:

```python
state = session.export_full_state()
restored = agent.session_from_state(state)
```

Rules:

- raw dict state remains available;
- unknown fields are preserved when they come from Rust records;
- invalid state raises a typed Python exception;
- restoring requires tools, toolsets, bundles, dependencies, and provider
  constructors to be supplied again in the current process.

### SessionArchive

`SessionArchive` is the simple JSON/file persistence helper:

```python
archive = SessionArchive.from_session(session)
archive.save("session.json")

restored_archive = SessionArchive.load("session.json")
restored = agent.session_from_archive(restored_archive)
```

Rules:

- default mode is `full`;
- `mode="curated"` is allowed only for portable snapshots and must omit
  `last_run_state`;
- archive format and schema version are explicit;
- raw state remains accessible;
- JSON round trips must not require Python callables;
- malformed archive fields fail early instead of being silently skipped.

### SessionStore Facade And Native Bridge

Python exposes native record shapes and a callback-backed bridge for custom
Python backends:

```python
class SessionStore:
    def to_native(self) -> _native.PythonSessionStore: ...

    async def save_session(self, record: SessionRecord) -> None: ...
    async def load_session(self, session_id: str) -> SessionRecord: ...
    async def list_sessions(self, filter: Mapping[str, object] | None = None) -> list[SessionRecord]: ...
    async def update_session_status(self, session_id: str, status: str) -> None: ...
    async def save_context_state(
        self,
        session_id: str,
        state: Mapping[str, object],
    ) -> None: ...
    async def save_environment_state(
        self,
        session_id: str,
        environment_state: Mapping[str, object],
    ) -> None: ...
    async def append_run(self, record: RunRecord) -> None: ...
    async def load_run(self, session_id: str, run_id: str) -> RunRecord: ...
    async def list_runs(self, session_id: str) -> list[RunRecord]: ...
    async def update_run_status(
        self,
        session_id: str,
        run_id: str,
        status: str,
        output_preview: str | None = None,
    ) -> None: ...
    async def append_checkpoint(self, session_id: str, checkpoint: Mapping[str, object]) -> None: ...
    async def load_checkpoints(self, session_id: str, run_id: str) -> list[Mapping[str, object]]: ...
    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord],
    ) -> None: ...
    async def replay_stream_records(
        self,
        session_id: str,
        run_id: str,
        after_sequence: int | None = None,
    ) -> list[StreamRecord]: ...
    async def save_stream_cursor(
        self,
        session_id: str,
        run_id: str,
        cursor: Mapping[str, object],
    ) -> None: ...
    async def resume_snapshot(
        self,
        session_id: str,
        run_id: str | None = None,
    ) -> SessionResumeSnapshot: ...
    async def compact_run_trace(self, session_id: str, run_id: str) -> Mapping[str, object]: ...
    async def compact_session_trace(self, session_id: str) -> Mapping[str, object]: ...
```

Required wrappers:

- `SessionRecord`
- `RunRecord`
- `StreamRecord`
- `CheckpointRef`
- `ApprovalRecord`
- `DeferredToolRecord`
- `SessionResumeSnapshot`

Required concrete stores:

- `InMemorySessionStore` for deterministic tests and examples.
- `JsonSessionStore` for simple local file persistence.
- `SqliteSessionStore` wrapping the native storage crate without weakening Rust
  migration semantics.

Store rules:

- record JSON shape is the Rust shape;
- raw dict escape hatches remain available;
- `save_current_session(...)` captures full state by construction;
- store APIs do not mutate product services by side effect;
- store errors map to stable Python exceptions;
- stream records remain ordered by sequence;
- approvals and deferred records preserve canonical IDs;
- loaded dynamic tool state stores IDs/namespaces, not Python object references.

### Python-Implemented Store Backend

`SessionStore.to_native()` adapts a Python store into the Rust `SessionStore`
trait through `_native.PythonSessionStore`. The bridge:

- schedules Rust trait calls back onto the Python event loop that created the
  native handle;
- accepts sync or async Python store methods;
- normalizes Python wrapper objects through `to_dict()`;
- validates returned records against Rust session types;
- maps missing records to `SessionStoreError::NotFound` where possible;
- cancels pending Python futures if the Rust caller drops the operation.

`SqliteSessionStore.to_native()` returns its native SQLite handle directly.
`create_agent_runtime(..., session_store=...)` binds either Python callback
stores or native SQLite stores into `AgentRuntimeBuilder`, so durable Rust
execution writes session, run, checkpoint, stream, approval, and deferred
evidence through the same `SessionStore` contract. The remaining integration
work is product-level coordination above the SDK: queueing, ownership claims,
API state, replay cursors, workspace policy, and recovery semantics.

Current Python package tests exercise the callback-backed `_native.PythonSessionStore`
handle across the full native bridge surface: session status and context state,
environment state, runs, run status, checkpoints, stream records and cursors,
approval records, deferred tool records, resume snapshots, and compact traces.
Resume snapshots preserve the full latest `AgentCheckpoint` record because that
is the Rust `SessionResumeSnapshot` contract; `CheckpointRef` remains the
run-record reference shape.

## Acceptance Checks

A Python session-store implementation is correct only if:

- full state round trips through JSON and `agent.session_from_state(...)`;
- `message_history` survives the round trip;
- message bus state, state domains, usage, trace metadata, and loaded dynamic
  tool state survive in full mode;
- curated state omits full runtime extensions;
- pending approvals and deferred tools keep canonical IDs;
- stream replay records remain ordered and raw;
- `save_current_session(...)` captures full state without a stringly typed mode;
- Python tools, callbacks, dependencies, and live environment handles are not
  serialized;
- current security and approval policy cannot be weakened by stale state;
- restored sessions use the current agent profile approval policy and current
  environment provider bindings instead of archived process-local objects;
- raw record dictionaries remain available for fields added by future Rust
  versions.
- Python `SessionStore.to_native()` round trips through the native Rust trait
  bridge without bypassing record validation.
- `create_agent_runtime(session_store=..., durable_session_id=...)` persists
  durable run evidence through the bound native `SessionStore`.

Validation commands:

```bash
cargo test -p starweaver-session --locked
cargo test -p starweaver-storage --locked
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```
