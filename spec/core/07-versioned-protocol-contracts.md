# Versioned Protocol and Durable Contracts

Status: normative

This specification defines the compatibility boundary for Starweaver-owned durable JSON and local wire protocols. It replaces implicit compatibility through additive Serde defaults with named schemas, integer schema versions, explicit legacy readers, and conformance fixtures.

## Ownership

Stable product-neutral protocol primitives live in lower owning crates:

| Contract                                                                      | Owner                  | Role                                                                              |
| ----------------------------------------------------------------------------- | ---------------------- | --------------------------------------------------------------------------------- |
| `ProtocolIdentity`                                                            | `starweaver-core`      | Common protocol name, major, revision, and feature shape                          |
| `VersionedEnvelope<T>` and `VersionedRecord`                                  | `starweaver-core`      | Durable JSON envelope and schema declaration                                      |
| `AgentEvent`                                                                  | `starweaver-core`      | Product-neutral sideband event record shared by context and streams               |
| `AgentExecutionNode` and `RunLifecycle`                                       | `starweaver-core`      | Execution-boundary and lifecycle vocabularies                                     |
| `AgentRunState`                                                               | `starweaver-context`   | Checkpointable model/tool loop state shared by runtime and durable consumers      |
| `AgentCheckpoint`, resume DTOs, and `AgentExecutor`                           | `starweaver-context`   | Versioned checkpoint evidence and the persistence/suspension callback contract    |
| `ContentPart`                                                                 | `starweaver-model`     | Canonical model-visible input/content AST                                         |
| `InputPart`                                                                   | `starweaver-session`   | Versioned durable wire AST with exhaustive conversion to and from `ContentPart`   |
| `DurableRunStatus`                                                            | `starweaver-session`   | Admission state composed with `RunLifecycle`                                      |
| `AgentStreamEvent`, `AgentStreamRecord`, source attribution, and stream sinks | `starweaver-stream`    | Typed raw execution stream protocol consumed by runtime, archives, and projectors |
| `ReplayScope`, `ReplayCursorFamily`, and `ReplayCursor`                       | `starweaver-stream`    | Canonical family-aware replay namespace and ordered resume position               |
| host identity and feature constants                                           | `starweaver-rpc-core`  | `starweaver.host` major-version contract                                          |
| envd identity and feature constants                                           | `starweaver-envd-core` | `starweaver.envd` major-version contract                                          |

CLI/TUI and standalone RPC remain independent products. Versioned lower contracts do not create a shared product coordinator, configuration type, handler layer, or dependency between the two products.

## Durable Envelope

New durable JSON records use this envelope:

```json
{
  "schema": "starweaver.session.run_record",
  "version": 1,
  "payload": {}
}
```

Rules:

- `schema` is a stable, globally unique Starweaver schema id.
- `version` is a positive integer. It changes only when the payload requires migration.
- `payload` is the owning type's serialized JSON value.
- Writers always emit the current envelope.
- Readers accept the current version and explicitly registered older versions.
- Readers reject a different schema, version zero, and an unknown newer version. They never guess.
- Release fixtures preserve at least the previous released bare or enveloped shape.
- A legacy bare JSON object is version `0` only at a codec boundary that explicitly opts into the legacy reader. For the initial v1 transition, the owning type's fixture-gated v0-compatible Serde reader decodes that shape; future incompatible versions must introduce an explicit previous-version DTO/migration before the current DTO changes. The next successful write emits the current envelope.
- A value that has envelope marker fields but is malformed is an error, not a legacy payload.

The envelope is applied at persistence and transport-record boundaries rather than embedded into mutable runtime structs. This keeps runtime construction ergonomic and prevents schema bookkeeping from becoming mutable execution state.

Initial durable schema ids:

| Schema id                                 | Current version | Legacy compatibility                                              |
| ----------------------------------------- | --------------: | ----------------------------------------------------------------- |
| `starweaver.context.resumable_state`      |               1 | bare JSON v0                                                      |
| `starweaver.runtime.checkpoint`           |               1 | bare JSON v0 plus the existing explicit `CheckpointRef` migration |
| `starweaver.runtime.stream_record`        |               1 | bare JSON v0                                                      |
| `starweaver.session.session_record`       |               1 | bare JSON v0                                                      |
| `starweaver.session.run_record`           |               1 | bare JSON v0                                                      |
| `starweaver.session.approval_record`      |               1 | bare JSON v0                                                      |
| `starweaver.session.deferred_tool_record` |               1 | bare JSON v0                                                      |
| `starweaver.stream.replay_event`          |               1 | bare JSON v0 and legacy display-row migration                     |
| `starweaver.stream.replay_snapshot`       |               1 | bare JSON v0                                                      |

`DisplayMessage` retains its embedded `starweaver.display.v1` display-protocol marker. It is not a root `VersionedRecord`: durable display rows store it inside a versioned `ReplayEvent::DisplayMessage`. This prevents the embedded `schema` field from being mistaken for a malformed durable envelope and keeps display projection compatibility separate from persistence-record migration.

SQLite schema migrations and JSON record migrations are separate. SQL migrations own tables and indexes; versioned codecs own JSON payload evolution.

## Canonical Input

`ContentPart` is the canonical ordered model-visible content AST. `InputPart` is deliberately separate because a durable run record carries per-input metadata and must remain readable independently of model request preparation.

The durable v1 AST has one explicit variant for every `ContentPart` variant:

- cache point
- text
- image URL
- file URL with media type
- inline binary with media type
- resource reference with media type, resource type, and resource metadata
- data URL with media type

Conversion requirements:

- `From<ContentPart> for InputPart` is exhaustive and lossless.
- `TryFrom<InputPart> for ContentPart` is exhaustive. It fails only for a legacy product-edge input that cannot be model content.
- Per-input durable metadata is not silently injected into provider-visible content metadata.
- `mode: "content_part"` is a legacy read-only encoding. New writers never emit it.
- Previous `url`, `file`, and binary-reference forms remain explicit legacy read variants and have documented conversion behavior.
- Slash commands and planning modes are parsed and executed at CLI, TUI, RPC-client, or application edges. They are not new canonical session input variants. Existing v0 command/mode JSON remains readable as legacy evidence but is never used to smuggle canonical content.

## Run Lifecycle

`RunLifecycle` is the single execution vocabulary:

```text
starting, running, waiting, completed, failed, cancelled
```

Runtime state and runtime stream records use `RunLifecycle` directly. The session layer composes admission with lifecycle:

```text
DurableRunStatus = queued | lifecycle(RunLifecycle)
```

Its stable JSON representation remains the flat strings `queued`, `starting`, `running`, `waiting`, `completed`, `failed`, and `cancelled` for compatibility. Conversion between runtime lifecycle and durable status is centralized and exhaustive. `queued` has no conversion to an executing runtime lifecycle until admission occurs.

Terminal classification is owned by the vocabulary, not duplicated in CLI, RPC, storage, and Python match tables.

## Cursor Vocabulary

`ReplayScope`, `ReplayCursorFamily`, and `ReplayCursor` are canonical for retained streams. A cursor is `(family, scope, sequence, optional backend cursor)` and is valid only for its exact family and scope. The stable families are `raw_runtime`, `display`, and `replay_event`; a cursor from one family must never enter another family's API even when the scope and sequence happen to match.

`StreamCursorRef` remains a durable index over multiple stream families, but it composes one typed `ReplayCursor` instead of copying family, scope, sequence, and backend cursor fields. Compatibility deserialization accepts the v0 flat shape and new serialization emits the composed v1 shape.

Runtime-internal resume counters such as model attempt, tool batch, message index, and output validation attempt are not replay cursors. They remain in runtime checkpoint evidence and must not be accepted by stream replay APIs.

Display archive rows and typed replay-event rows use separate persistence families. `display_message_records` stores the display projection while `replay_events` stores RPC/product replay events. Each family has its own monotonic sequence. A `ReplayEvent::DisplayMessage` therefore carries an event-family sequence outside a nested `DisplayMessage` that retains its display-family sequence.

## Protocol Identity

All Starweaver-owned local protocols use:

```json
{
  "name": "starweaver.host",
  "major": 1,
  "revision": "2026-07-11",
  "features": []
}
```

Rules:

- `name` selects the protocol family.
- `major` is the compatibility gate.
- `revision` identifies documentation and fixtures but is not ordered by clients.
- `features` is the only capability-negotiation mechanism.
- Clients reject an unexpected name or unsupported major.
- Implementations must not expose a second date/version string with conflicting semantics.

The current identities are `starweaver.host` major 1 and `starweaver.envd` major 1. Their implemented feature lists come from typed constants in their protocol-core crates.

## Wire Compatibility

JSON-RPC 2.0 remains the framing protocol. The initialized Starweaver protocol identity versions method and embedded DTO semantics. Embedded durable records either:

1. use their owning versioned envelope when transferred as opaque durable evidence; or
2. use a method DTO fixed by the negotiated protocol major when projected into a typed result.

RPC handlers must not serialize mutable internal runtime state as an undocumented wire contract. Unknown host/envd major versions fail initialization. Unknown durable schema versions fail decoding with a safe schema/version error.

## Fixtures and Release Gate

Versioned owner fixtures live with each crate under `tests/fixtures/contracts/`. The shared cross-surface raw/display/replay corpus lives at `spec/fixtures/stream/raw-display-replay-v1.json`; it is consumed directly by `starweaver-stream`, Python, CLI, and RPC tests so those surfaces cannot drift behind duplicated copies.

The fixture set includes:

- previous bare v0 input, session, run, context, replay event, replay snapshot, checkpoint, and stream records;
- current v1 envelopes;
- unknown-version and wrong-schema negative fixtures;
- canonical input coverage for every content variant;
- all run lifecycle and durable admission states;
- flat-v0 and composed-v1 cursor records;
- host and envd initialize identities, including wrong-name and wrong-major rejection;
- ordered raw records, exact display payloads and terminal kind, nested source attribution, and replay sequences, scope, and terminal marker in the shared corpus.

The fixture directories are registered as capability evidence in `spec/capabilities.toml`. `make capability-check` verifies registry schema/version, required entries, workspace owners, and the existence of normative specs, implementation paths, and contract-test evidence.

Every fixture test proves old-read/current-write behavior. Current writes must decode to an equal typed value. A release that changes one of these payloads must add a migration and retain the prior fixture.

Validation:

```bash
cargo test -p starweaver-core --locked
cargo test -p starweaver-session --locked
cargo test -p starweaver-stream --locked
cargo test -p starweaver-storage --locked
cargo test -p starweaver-rpc-core --locked
cargo test -p starweaver-rpc --locked
cargo test -p starweaver-envd-core --locked
cargo test -p starweaver-envd --locked
make capability-check
```
