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

### SessionStore Facade

Python must expose native record shapes before adding custom Python backends:

```python
class SessionStore:
    async def save_session(self, record: SessionRecord) -> None: ...
    async def load_session(self, session_id: str) -> SessionRecord: ...
    async def save_context_state(
        self,
        session_id: str,
        state: Mapping[str, object],
    ) -> None: ...
    async def append_run(self, record: RunRecord) -> None: ...
    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord],
    ) -> None: ...
    async def resume_snapshot(
        self,
        session_id: str,
        run_id: str | None = None,
    ) -> SessionResumeSnapshot: ...
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
- `SqliteSessionStore` only when it can wrap the native storage crate without
  weakening Rust migration semantics.

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

A Python implementation of the Rust `SessionStore` trait is a later bridge. It
requires async Rust-to-Python callback scheduling, bidirectional error mapping,
backpressure, cancellation semantics, GIL boundaries, and versioned record
validation. It should be added only after native store facades are stable and a
product needs a custom Python database/service store to participate directly in
Rust durable execution.

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
- raw record dictionaries remain available for fields added by future Rust
  versions.

Validation commands:

```bash
cargo test -p starweaver-session --locked
cargo test -p starweaver-storage --locked
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```
