# JSON-RPC Host Protocol

The Starweaver host protocol is the local control plane for desktop clients, automation hosts, and future product surfaces that need durable sessions, run orchestration, replay, live stream subscriptions, HITL decisions, model profile selection, configuration reads, and diagnostics.

JSON-RPC 2.0 is the semantic request/response protocol. Stdio, HTTP, local sockets, named pipes, and WebSocket are transport profiles over the same typed Starweaver method and event contracts.

## Goals

- Define a Starweaver-owned host-control protocol instead of treating RPC as a CLI implementation detail.
- Keep TUI, CLI commands, and RPC as sibling product edges over shared runtime, session, and stream contracts.
- Keep the protocol Starweaver-native by making `ReplayScope`, `ReplayCursor`, `ReplayEvent`, `DisplayMessage`, session records, run records, approval records, and deferred records the canonical data.
- Make JSON-RPC framing transport-neutral enough to move from stdio to HTTP, local sockets, or WebSocket without changing method semantics.
- Make stream replay and live subscription semantics cursor-safe, scope-local, and reconnectable.
- Make all mutating methods idempotent when clients provide an idempotency key.
- Make errors structured enough for host clients to distinguish invalid params, missing records, active-run conflicts, closed subscriptions, timeouts, and failed runtime operations.
- Make projection into AGUI or other UI protocols an explicit edge concern, never the protocol core.
- Keep model profile selection as client state, separate from shared Starweaver config.

## Non-goals

- This protocol does not define provider model wire formats. Provider protocols remain in `starweaver-model`.
- This protocol does not define hosted multi-tenant auth. Hosted auth belongs to future platform/service layers.
- This protocol does not require the TUI to launch or speak JSON-RPC. TUI uses the same coordinator and stores in process.
- This protocol does not make AGUI the native event format. AGUI is one projection format.
- This protocol does not replace CLI command UX. CLI commands remain a shell-friendly subset over the same underlying handlers.

## Layering

```mermaid
flowchart TD
    host[Desktop or host client]
    transport[Transport profile: stdio, HTTP, socket, named pipe, WebSocket]
    rpc[starweaver.host JSON-RPC protocol]
    cli_server[starweaver-cli RPC server adapter]
    coordinator[CliRuntimeCoordinator]
    session[SessionStore]
    archive[StreamArchive]
    eventlog[ReplayEventLog]
    runtime[starweaver-runtime]
    stream[starweaver-stream records]
    platform[Future platform adapters]

    host --> transport
    transport --> rpc
    rpc --> cli_server
    cli_server --> coordinator
    cli_server --> session
    cli_server --> archive
    cli_server --> eventlog
    coordinator --> runtime
    runtime --> stream
    stream --> archive
    stream --> eventlog
    rpc --> platform
```

The protocol boundary is the `starweaver.host` method, event, and error contract. `starweaver-cli` provides the first local server adapter. Future platform adapters can implement the same protocol over different transports or bridge it into external protocols.

## Protocol Identity

The protocol name is `starweaver.host`.

Version shape:

| Field      | Meaning                                                  |
| ---------- | -------------------------------------------------------- |
| `name`     | Fixed string `starweaver.host`                           |
| `major`    | Breaking contract generation, starting at `1`            |
| `revision` | Date-like revision string for documentation and fixtures |
| `features` | Negotiated feature names returned by `initialize`        |

Example:

```json
{
  "name": "starweaver.host",
  "major": 1,
  "revision": "2026-06-23",
  "features": [
    "session.lifecycle",
    "run.lifecycle",
    "stream.replay",
    "stream.subscribe",
    "hitl.approvals",
    "hitl.deferred",
    "environment.attachments",
    "projection.agui"
  ]
}
```

Clients should key behavior off `major` and `features`, not off revision string ordering.

Feature names:

| Feature                      | Meaning                                |
| ---------------------------- | -------------------------------------- |
| `session.lifecycle`          | Session create/list/get/delete/current |
| `run.lifecycle`              | Run start/get/status/await/cancel      |
| `run.steering`               | Active run steering                    |
| `stream.replay`              | Retained replay by `ReplayScope`       |
| `stream.subscribe`           | Live stream subscriptions              |
| `stream.snapshot`            | Compact replay snapshots               |
| `hitl.approvals`             | Approval list/show/decide              |
| `hitl.deferred`              | Deferred tool list/show/complete/fail  |
| `environment.attachments`    | Host-managed environment attachments   |
| `client.model_selection`     | Frontend-local model selection         |
| `projection.display_message` | Starweaver display-message projection  |
| `projection.agui`            | AGUI projection                        |

## JSON-RPC Envelope Rules

Each request, response, and notification is JSON-RPC 2.0.

Request:

```json
{
  "jsonrpc": "2.0",
  "id": "req_1",
  "method": "run.start",
  "params": {}
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": "req_1",
  "result": {}
}
```

Error:

```json
{
  "jsonrpc": "2.0",
  "id": "req_1",
  "error": {
    "code": -32014,
    "message": "run is not active",
    "data": {
      "kind": "run_not_active",
      "retryable": false,
      "sessionId": "session_...",
      "runId": "run_..."
    }
  }
}
```

Notification:

```json
{
  "jsonrpc": "2.0",
  "method": "starweaver.event",
  "params": {}
}
```

Envelope requirements:

- `jsonrpc` must be `2.0`.
- Request `id` must be a string or integer. `null` ids are rejected for requests.
- Request `params` must be an object. Missing params are treated as `{}`.
- Server notifications have no `id`.
- Host protocol v1 defines no client-to-server notification methods. Client messages that mutate state or control subscriptions must carry an `id`.
- JSON-RPC batch arrays are not part of host protocol v1. A batch array receives `invalid_request`.
- Protocol-owned field names use `camelCase`.
- Embedded Starweaver records keep their crate-defined serde shape.

## Common Params

Most methods accept only their method-specific params. Mutating methods and long-running methods may also accept these common fields:

| Field            | Type   | Meaning                                                     |
| ---------------- | ------ | ----------------------------------------------------------- |
| `requestId`      | string | Client correlation id distinct from JSON-RPC `id`           |
| `idempotencyKey` | string | Client key for idempotent mutation semantics                |
| `traceContext`   | object | Optional W3C trace context fields such as `traceparent`     |
| `metadata`       | object | Method-specific durable metadata where the method allows it |

`requestId` is for logs and diagnostics. It does not imply idempotency. `idempotencyKey` is the only field that changes repeated mutation behavior.

## Transport Profiles

Transport profiles only define framing, process IO, and connection lifetime. They do not change methods, params, results, events, cursor semantics, idempotency, or error data.

| Profile      | Framing                                        | Primary use                         |
| ------------ | ---------------------------------------------- | ----------------------------------- |
| `stdio`      | UTF-8 newline-delimited JSON objects           | Desktop launches local CLI server   |
| `http`       | One JSON-RPC object per `POST /rpc` body       | Local request/response integrations |
| `local-sock` | One JSON-RPC object per length-delimited frame | Long-lived local host integrations  |
| `named-pipe` | Same as local socket                           | Windows local host integrations     |
| `websocket`  | One JSON-RPC object per text message           | Browser-backed local or platform UI |

Stdio profile:

- stdin carries client requests.
- stdout is reserved for JSON-RPC responses and notifications.
- stderr carries human-readable diagnostics, trace setup messages, and crash reports.
- Each non-empty stdin line must be one complete JSON object.
- Each stdout line is one complete JSON object.
- The server exits after a successful `shutdown` response is written or after stdin closes.

HTTP profile:

- `POST /rpc` carries one UTF-8 JSON-RPC request object as the HTTP request body.
- Successful JSON-RPC responses use HTTP `200 OK` with an `application/json` response body.
- JSON-RPC notifications without response ids use HTTP `204 No Content`.
- HTTP request parsing failures use HTTP `4xx` responses before JSON-RPC dispatch.
- `GET /health` and `GET /healthz` may expose a lightweight local health response.
- Unary HTTP does not carry live server notifications. HTTP clients use `run.await`, `run.status`, `stream.replay`, or a future long-connection profile for progress.
- Unary HTTP `initialize` responses must not advertise live subscription features such as `stream.subscribe` unless the server is also negotiating a long-connection notification profile.
- Unary HTTP does not support connection-scoped environment attachment leases.
  HTTP clients use session-scoped leases or inline run attachments.
- The server stops accepting new HTTP requests after a successful `shutdown` response is written.

Local socket and named-pipe profile:

- Each frame starts with a 4-byte unsigned big-endian payload length.
- The payload is one UTF-8 JSON-RPC object.
- The length counts payload bytes only and excludes the 4-byte prefix.
- The peer closes the connection after `shutdown` or transport failure.

WebSocket profile:

- Each text message is one complete JSON-RPC object.
- Binary messages are rejected with `invalid_request`.
- Platform deployments must authenticate the WebSocket before accepting host protocol messages.

## Shared Data Contracts

The host protocol reuses shared crates as wire contracts:

| Data                   | Owner                | Protocol role                        |
| ---------------------- | -------------------- | ------------------------------------ |
| `SessionId`, `RunId`   | `starweaver-core`    | Stable durable identifiers           |
| `InputPart`            | `starweaver-session` | Structured run input                 |
| `SessionRecord`        | `starweaver-session` | Durable session state                |
| `RunRecord`            | `starweaver-session` | Durable run state                    |
| `RunStatus`            | `starweaver-session` | Run lifecycle status                 |
| `ApprovalRecord`       | `starweaver-session` | Approval workflow state              |
| `DeferredToolRecord`   | `starweaver-session` | Deferred tool workflow state         |
| `ReplayScope`          | `starweaver-stream`  | Stream replay namespace              |
| `ReplayCursor`         | `starweaver-stream`  | Resume point inside one replay scope |
| `ReplayEvent`          | `starweaver-stream`  | Canonical replay/live stream event   |
| `ReplaySnapshot`       | `starweaver-stream`  | Compact replay state                 |
| `DisplayMessage`       | `starweaver-stream`  | Starweaver-native UI/display event   |
| `StreamTerminalMarker` | `starweaver-stream`  | Terminal stream marker               |

Protocol-owned wrapper objects may add `subscriptionId`, `requestId`, `idempotencyKey`, pagination tokens, projection payloads, and client metadata around these shared records. They should not duplicate or reshape shared records unless the protocol explicitly defines a projection.

## Client Lifecycle

All clients begin with `initialize`.

```json
{
  "jsonrpc": "2.0",
  "id": "req_init",
  "method": "initialize",
  "params": {
    "clientInfo": {
      "name": "desktop",
      "version": "0.1.0"
    },
    "clientIdentity": {
      "clientId": "desktop:user-machine-install"
    },
    "clientStateScope": "desktop",
    "workspaceRoot": "/workspace/project",
    "requiredFeatures": ["run.lifecycle", "stream.subscribe"],
    "preferredProjectionFormats": ["starweaver.display_message", "agui"]
  }
}
```

Result:

```json
{
  "protocol": {
    "name": "starweaver.host",
    "major": 1,
    "revision": "2026-06-23",
    "features": [
      "run.lifecycle",
      "stream.replay",
      "stream.subscribe",
      "environment.attachments"
    ]
  },
  "serverInfo": {
    "name": "starweaver-cli",
    "version": "0.0.1"
  },
  "connectionId": "conn_...",
  "capabilities": {
    "sessions": true,
    "runs": true,
    "streams": true,
    "approvals": true,
    "deferredTools": true,
    "environmentAttachments": true,
    "clientModelSelection": true,
    "projections": ["starweaver.display_message", "agui"],
    "defaultProjectionFormat": "starweaver.display_message"
  },
  "limits": {
    "maxRequestBytes": 1048576,
    "maxResponseBytes": 8388608,
    "defaultPageLimit": 50,
    "maxPageLimit": 500,
    "maxReplayEvents": 1000,
    "maxSubscriptions": 32
  },
  "config": {
    "globalDir": "/home/user/.starweaver",
    "projectDir": "/workspace/project/.starweaver",
    "defaultProfile": "default_model"
  }
}
```

Lifecycle invariants:

- Calls other than `initialize` may be rejected until initialization succeeds.
- `requiredFeatures` is fail-closed. If any required feature is absent, initialize fails with `unsupported_feature`.
- `clientIdentity.clientId` is the preferred durable idempotency scope. If it is absent, idempotency is scoped to the connection only.
- `clientStateScope` selects frontend-local state such as model selection. It does not mutate shared config.
- A connection can own multiple stream subscriptions.
- Clients should respect advertised `limits`; servers reject oversized payloads or exhausted resources with structured errors.
- `shutdown` closes subscriptions, stops accepting new requests, and returns after terminal cleanup that is safe to run synchronously.
- Servers may process independent requests concurrently. Mutations for one session, run, approval, deferred record, or subscription must be serialized by that record key.

### `shutdown`

Requests graceful connection shutdown.

Params:

| Field              | Type    | Required | Meaning                                                                 |
| ------------------ | ------- | -------- | ----------------------------------------------------------------------- |
| `timeoutMs`        | number  | no       | Maximum cleanup wait                                                    |
| `cancelActiveRuns` | boolean | no       | Whether to request cancellation for active runs owned by the connection |

Result:

```json
{
  "status": "shutdown",
  "closedSubscriptions": 2,
  "cancelledRuns": []
}
```

Shutdown closes subscriptions owned by the connection. It does not cancel active runs unless `cancelActiveRuns` is true. The stdio profile exits the process after the shutdown response is written.

## Pagination and Limits

List and replay methods use explicit limits and opaque pagination or cursor tokens.

Pagination rules:

- `limit` defaults to `limits.defaultPageLimit` from `initialize`.
- `limit` greater than `limits.maxPageLimit` fails with `invalid_params`.
- `pageToken` is opaque, server-owned, and scoped to the method plus filter params that produced it.
- Changing filter params while reusing a page token fails with `invalid_params`.
- `nextPageToken: null` means the page is complete.
- Profile and model lists still expose `limit` and `pageToken`; local config implementations can return all entries in one page.

Replay limit rules:

- `stream.replay.limit` defaults to `limits.maxReplayEvents` unless the server advertises a smaller method-specific default.
- `stream.subscribe.replay.limit` defaults to `limits.maxReplayEvents`.
- Replay limits greater than `limits.maxReplayEvents` fail with `invalid_params`.
- A subscription that would need more retained events than its accepted replay limit fails with `replay_limit_exceeded`.

## Method Groups

The protocol uses domain-qualified method names.

| Group       | Methods                                                                                                                     |
| ----------- | --------------------------------------------------------------------------------------------------------------------------- |
| Lifecycle   | `initialize`, `shutdown`                                                                                                    |
| Session     | `session.create`, `session.list`, `session.get`, `session.current.get`, `session.current.set`, `session.delete`             |
| Run         | `run.start`, `run.get`, `run.status`, `run.await`, `run.cancel`, `run.steer`                                                |
| Environment | `environment.attach`, `environment.detach`, `environment.list`, `environment.health`                                        |
| Stream      | `stream.replay`, `stream.subscribe`, `stream.unsubscribe`, `stream.snapshot`, `stream.cursorRange`                          |
| HITL        | `approval.list`, `approval.show`, `approval.decide`, `deferred.list`, `deferred.show`, `deferred.complete`, `deferred.fail` |
| Model       | `model.list`, `model.current`, `model.select`                                                                               |
| Profile     | `profile.list`, `profile.get`                                                                                               |
| Config      | `config.get`                                                                                                                |
| Diagnostics | `diagnostics.get`                                                                                                           |

`stream.*` is the canonical stream API. Product-specific command shortcuts can call shared handlers, but protocol clients should not need separate methods for run attach, session output, and replay export.

## Session Methods

### `session.create`

Creates a durable session record.

Params:

| Field            | Type   | Required | Meaning                                       |
| ---------------- | ------ | -------- | --------------------------------------------- |
| `profile`        | string | no       | Agent/model profile for future runs           |
| `title`          | string | no       | Human-readable session title                  |
| `workspaceRoot`  | string | no       | Session workspace path                        |
| `metadata`       | object | no       | Durable session metadata                      |
| `idempotencyKey` | string | no       | Idempotent create key scoped to client/method |

Result:

```json
{
  "session": {}
}
```

### `session.list`

Lists durable sessions.

Params:

| Field       | Type   | Required | Meaning                                      |
| ----------- | ------ | -------- | -------------------------------------------- |
| `status`    | string | no       | `active`, `archived`, `failed`, or `deleted` |
| `profile`   | string | no       | Profile filter                               |
| `workspace` | string | no       | Workspace filter                             |
| `limit`     | number | no       | Maximum rows                                 |
| `pageToken` | string | no       | Opaque pagination token                      |

Result:

```json
{
  "sessions": [],
  "nextPageToken": null
}
```

### `session.get`

Loads a session and recent runs.

Params:

| Field       | Type   | Required | Meaning                                                  |
| ----------- | ------ | -------- | -------------------------------------------------------- |
| `sessionId` | string | yes      | Session id                                               |
| `runsLimit` | number | no       | Maximum recent runs                                      |
| `include`   | array  | no       | Optional sections such as `runs`, `trace`, `environment` |

Result:

```json
{
  "session": {},
  "runs": []
}
```

### Current Session Pointer

`session.current.get` and `session.current.set` operate on the project runtime state pointer. They do not change `SessionRecord` contents.

`session.current.get` params:

| Field           | Type   | Required | Meaning                                    |
| --------------- | ------ | -------- | ------------------------------------------ |
| `workspaceRoot` | string | no       | Project root; defaults to initialized root |

Result:

```json
{
  "workspaceRoot": "/workspace/project",
  "sessionId": "session_...",
  "session": {}
}
```

`sessionId` and `session` are `null` when no current pointer exists.

`session.current.set` params:

| Field            | Type           | Required | Meaning                                    |
| ---------------- | -------------- | -------- | ------------------------------------------ |
| `workspaceRoot`  | string         | no       | Project root; defaults to initialized root |
| `sessionId`      | string or null | yes      | Session id to set, or `null` to clear      |
| `idempotencyKey` | string         | no       | Idempotent pointer update key              |

Result:

```json
{
  "workspaceRoot": "/workspace/project",
  "sessionId": "session_...",
  "session": {}
}
```

`session.current.set` must verify the session exists before updating the pointer when `sessionId` is not `null`. Clearing an already empty pointer is successful.

### `session.delete`

Deletes or tombstones a session.

Params:

| Field            | Type    | Required | Meaning                                             |
| ---------------- | ------- | -------- | --------------------------------------------------- |
| `sessionId`      | string  | yes      | Session id                                          |
| `deleteEvidence` | boolean | no       | Delete run, stream, approval, and deferred evidence |
| `reason`         | string  | no       | Human-readable reason                               |
| `idempotencyKey` | string  | no       | Idempotent delete key                               |

Result:

```json
{
  "sessionId": "session_...",
  "deleted": true,
  "evidenceDeleted": false
}
```

Delete invariants:

- `deleteEvidence` defaults to `false`; retained run and stream evidence may remain as hidden audit data.
- If `deleteEvidence` is `true`, session records, run records, replay events, stream snapshots, approvals, and deferred records are removed as one durable operation or the method fails.
- Deleting the current project session clears the current pointer.
- Deleting a session with an active run returns `run_conflict` unless the product has first cancelled or completed the run.

## Run Methods

`run.start` is the core run creation method. It accepts structured input. Plain prompt text is represented as an `InputPart` with kind `text`.

Params:

| Field                    | Type   | Required | Meaning                                             |
| ------------------------ | ------ | -------- | --------------------------------------------------- |
| `input`                  | object | yes      | Run input object with `parts`                       |
| `session`                | object | no       | Session selection policy                            |
| `model`                  | object | no       | One-run model/profile override                      |
| `clientStateScope`       | string | no       | Frontend-local model selection scope                |
| `environment`            | object | no       | Workspace and environment selection hints           |
| `environmentAttachments` | array  | no       | Inline or pre-attached environments for the run     |
| `hitl`                   | object | no       | Approval/deferred policy overrides                  |
| `stream`                 | object | no       | Initial subscription and projection options         |
| `metadata`               | object | no       | Run metadata                                        |
| `idempotencyKey`         | string | no       | Idempotent run creation key scoped to client/method |

Input object:

```json
{
  "parts": [
    {
      "kind": "text",
      "text": "summarize this repository"
    }
  ]
}
```

Session selection:

```json
{
  "policy": "new"
}
```

| Policy              | Fields                          | Meaning                                                  |
| ------------------- | ------------------------------- | -------------------------------------------------------- |
| `selected`          | `sessionId`                     | Append under a specific session                          |
| `current`           | none                            | Use current project session pointer                      |
| `current_or_latest` | none                            | Use current project session pointer, then latest session |
| `latest`            | none                            | Use latest resumable session                             |
| `new`               | optional `title`, `profile`     | Create a new session                                     |
| `restore`           | `sessionId`, `restoreFromRunId` | Append after restoring a run snapshot                    |
| `branch`            | `sessionId`, `branchFromRunId`  | Append a branch run from historical run evidence         |

`current` fails with `not_found` when no current project session exists. `current_or_latest` is the protocol form of the CLI continue behavior.

Current CLI compatibility parameters map `sessionId` to `selected`,
`continueLatest=true` to `current_or_latest`, and `newSession=true` to `new`.
These session selectors are mutually exclusive. A request that supplies more
than one target selector is rejected before environment attachment
materialization.

Model options:

```json
{
  "profile": "coding",
  "settings": {}
}
```

| Field      | Type   | Required | Meaning                                    |
| ---------- | ------ | -------- | ------------------------------------------ |
| `profile`  | string | no       | Config-backed profile for this run         |
| `settings` | object | no       | Typed `ModelSettings` overlay for this run |

Model option invariants:

- `profile` must resolve through shared config.
- `settings` must validate against `starweaver-model` typed model settings for the resolved provider.
- Arbitrary provider HTTP headers are not accepted through generic RPC metadata.
- Provider routing affinity can only flow through typed provider routing settings and runtime context, not durable session ids.

Result:

```json
{
  "sessionId": "session_...",
  "runId": "run_...",
  "status": "running",
  "stream": {
    "subscriptionId": "sub_...",
    "scope": "run:run_...",
    "cursor": null,
    "replay": {
      "events": [],
      "nextCursor": null,
      "hasMore": false
    }
  }
}
```

Run invariants:

- `run.start` creates durable session/run records before returning.
- `run.start` returns after the run is accepted and active-run registration is complete.
- `stream` is `null` when the request does not ask for an initial subscription.
- Terminal completion is reported through stream events and can be read with `run.await` or `run.status`.
- If `stream.subscribe` options are supplied in `run.start`, the server creates the run subscription atomically with run registration.
- The server must not use durable session ids as provider routing headers. Provider routing affinity flows through context and typed model settings only.

Environment options:

```json
{
  "workspaceRoot": "/workspace/project",
  "provider": "local",
  "worktree": {
    "mode": "existing",
    "name": "feature"
  },
  "metadata": {}
}
```

Environment fields are hints resolved by the product host against Starweaver environment provider configuration. Unsupported providers, worktree modes, or workspace roots fail with `configuration_failed` or `invalid_params`; they must not silently widen file or shell policy.

Run environment attachments:

```json
{
  "environmentAttachments": [
    {
      "id": "workspace",
      "attachmentLeaseId": "envatt_workspace",
      "default": true
    },
    {
      "id": "data",
      "kind": "envd",
      "endpointRef": "http://127.0.0.1:8770/rpc",
      "environmentId": "dataset",
      "mode": "read_only"
    }
  ]
}
```

`environmentAttachments` entries have two allowed forms:

- A pre-attached lease ref with `attachmentLeaseId`, plus run-local fields such
  as `id`, `mode`, and `default`.
- An inline attachment source with `kind`, endpoint/source fields, `mode`, and
  `default`. The server resolves it through the same attachment manager used by
  `environment.attach`.

Attachment invariants:

- `id` is the run-local agent-facing mount identity. It is an ASCII slug and is
  exposed to tools as `/environment/{id}`.
- One attachment is the default for unqualified paths. Multiple attachments
  require exactly one `default: true`. A single attachment defaults to true when
  omitted.
- `attachmentLeaseId` is a host-control lease id, not an envd environment id and
  not visible to the model.
- A run-local `mode` on a lease ref may keep or narrow the leased mode. It must
  not widen a `read_only` lease to `read_write`.
- Inline envd attachments identify envd by `endpointRef` and `environmentId`.
  The initial local host accepts literal `http://...` endpoints. Named aliases
  are future host capabilities.
- A session-scoped `attachmentLeaseId` can only be used by a run bound to the
  same `sessionId`. If the run target session is not explicit or cannot be
  proven before materialization, the server rejects the lease ref.
- Session target selection is resolved before lease materialization and must not
  change after lease scope validation.
- Run preparation must resolve and probe all attachments before creating the
  runtime session. A failure in any required attachment fails `run.start` before
  active-run registration.
- The runtime receives one SDK `EnvironmentProvider`, normally a
  `CompositeEnvironmentProvider`; it does not receive a list of host-control
  attachments.

HITL options:

```json
{
  "policy": "prompt"
}
```

| Field    | Type   | Required | Meaning                                            |
| -------- | ------ | -------- | -------------------------------------------------- |
| `policy` | string | no       | `configured`, `prompt`, `deny`, `defer`, or `fail` |

HITL policy meanings:

| Policy       | Meaning                                                  |
| ------------ | -------------------------------------------------------- |
| `configured` | Use resolved product/profile configuration               |
| `prompt`     | Create approval records and wait for human decisions     |
| `deny`       | Deny approval-required actions                           |
| `defer`      | Persist deferred tool records for later completion       |
| `fail`       | Fail the run when an approval-required action is reached |

Run stream options:

```json
{
  "subscribe": true,
  "scope": "run",
  "cursor": null,
  "replay": {
    "enabled": true,
    "limit": 1000
  },
  "projection": {
    "formats": ["starweaver.display_message"]
  }
}
```

If `stream.subscribe` options are supplied in `run.start`, `scope` may be `run` or `session`; the server expands it to `run:<runId>` or `session:<sessionId>` after durable ids are allocated. The `stream` result follows `stream.subscribe` result semantics for the expanded scope.

### `run.get`

Loads one run record and optional compact trace.

Params:

| Field       | Type   | Required | Meaning                           |
| ----------- | ------ | -------- | --------------------------------- |
| `sessionId` | string | yes      | Session id                        |
| `runId`     | string | yes      | Run id                            |
| `include`   | array  | no       | Optional sections such as `trace` |

Result:

```json
{
  "run": {},
  "trace": null
}
```

### `run.status`

Returns the current run status from active-run state when present, otherwise from durable run state.

Params:

| Field       | Type   | Required | Meaning    |
| ----------- | ------ | -------- | ---------- |
| `sessionId` | string | yes      | Session id |
| `runId`     | string | yes      | Run id     |

Result:

```json
{
  "sessionId": "session_...",
  "runId": "run_...",
  "status": "running",
  "outputPreview": null,
  "error": null
}
```

### `run.await`

Waits for terminal run status.

Params:

| Field       | Type   | Required | Meaning               |
| ----------- | ------ | -------- | --------------------- |
| `sessionId` | string | yes      | Session id            |
| `runId`     | string | yes      | Run id                |
| `timeoutMs` | number | no       | Maximum wait duration |

Result:

```json
{
  "run": {},
  "status": "completed",
  "outputPreview": "..."
}
```

Await invariants:

- `run.await` returns only after the run reaches a terminal status.
- If `timeoutMs` expires first, the method returns a `timeout` error with `sessionId`, `runId`, and the current run status in `error.data`.
- Disconnecting a client cancels the pending await request, not the underlying run.

### `run.cancel`

Requests cooperative cancellation.

Params:

| Field            | Type   | Required | Meaning                     |
| ---------------- | ------ | -------- | --------------------------- |
| `runId`          | string | yes      | Active run id               |
| `reason`         | string | no       | Human-readable reason       |
| `idempotencyKey` | string | no       | Idempotent cancellation key |

Result:

```json
{
  "runId": "run_...",
  "accepted": true
}
```

### `run.steer`

Queues steering text for an active run.

Params:

| Field            | Type   | Required | Meaning                     |
| ---------------- | ------ | -------- | --------------------------- |
| `runId`          | string | yes      | Active run id               |
| `input`          | object | yes      | Steering input parts        |
| `steeringId`     | string | no       | Client-provided steering id |
| `idempotencyKey` | string | no       | Idempotent steering key     |

Result:

```json
{
  "runId": "run_...",
  "steeringId": "steer_...",
  "queued": true
}
```

## Environment Attachment Methods

The host owns an `EnvironmentAttachmentManager` between JSON-RPC handlers and
run preparation. It resolves host-control refs into SDK providers and leases.
Envd remains the environment data/effect plane; the host manager only owns
selection, probing, lifecycle, and run binding.

```mermaid
flowchart TD
    client[Host client]
    rpc[starweaver.host RPC]
    manager[EnvironmentAttachmentManager]
    registry[Literal endpoint resolver]
    probe[Readiness probe]
    lease_store[Attachment lease store]
    envd_client[EnvdRpcClient]
    local_provider[Local or virtual provider]
    envd_provider[EnvdEnvironmentProvider]
    composite[CompositeEnvironmentProvider]
    runtime[AgentSession]

    client --> rpc
    rpc --> manager
    manager --> registry
    manager --> probe
    manager --> lease_store
    registry --> envd_client
    registry --> local_provider
    envd_client --> envd_provider
    manager --> composite
    local_provider --> composite
    envd_provider --> composite
    composite --> runtime
```

Manager responsibilities:

- Validate attachment ids, default selection, access modes, and source shape.
- Resolve literal `endpointRef` transport refs supported by the advertised
  protocol revision. The initial local host supports literal `http://...` envd
  endpoints.
- Construct SDK providers such as local, virtual, sandbox, or envd-backed
  providers.
- Probe process liveness and environment readiness before a run uses an
  attachment.
- Track connection-scoped and session-scoped attachment leases.
- Materialize one run-scoped `RunEnvironmentBinding` from inline attachments
  and lease refs, then construct the SDK provider composition.
- Release manager-owned leases when their scope closes, while leaving envd
  service-owned environment state to envd.

Future manager capabilities:

- Resolve named `endpointRef` aliases to concrete local or remote transports.
- Launch host-owned envd daemons when a configured endpoint requires it.
- Support stdio, local socket, named pipe, and WebSocket envd transports.

Manager non-goals:

- It does not expose envd file, process, mount, or operation DTOs through
  `starweaver.host`.
- It does not store full envd state in Starweaver session storage.
- It does not mutate an already running AgentSession in host protocol v1.
  Active-run mount/unmount requires a future `environment.active_mounts`
  feature because it must update the runtime environment handle and reinject
  environment context safely.

### Attachment Source

Attachment source shape:

```json
{
  "id": "data",
  "kind": "envd",
  "endpointRef": "http://127.0.0.1:8770/rpc",
  "environmentId": "dataset",
  "mode": "read_only",
  "default": false,
  "metadata": {}
}
```

Fields:

| Field           | Type    | Required | Meaning                                                       |
| --------------- | ------- | -------- | ------------------------------------------------------------- |
| `id`            | string  | yes      | Agent-facing mount identity within the lease or run           |
| `kind`          | string  | no       | `local`, `virtual`, `sandbox`, or `envd`; defaults to `local` |
| `endpointRef`   | string  | for envd | Literal endpoint, initially `http://...` for envd             |
| `environmentId` | string  | no       | Concrete environment id inside the implementation             |
| `mode`          | string  | no       | `read_write` or `read_only`; defaults to `read_write`         |
| `default`       | boolean | no       | Default preference when materialized into a run               |
| `metadata`      | object  | no       | Host metadata, never model-visible by default                 |

Endpoint refs:

- Literal `http://...` endpoints connect through `EnvdRpcClient::http`.
- Named aliases, stdio, local socket, named pipe, WebSocket, and launched daemon
  transports are future host capabilities. Servers that do not advertise those
  capabilities reject them with `unsupported_feature`.
- Alias resolution failures return `configuration_failed` once alias resolvers
  are supported.
- Transport liveness failures return `environment_unavailable`.

### `environment.attach`

Creates or reuses an attachment lease for a connection or session.

Params:

```json
{
  "scope": {
    "kind": "session",
    "sessionId": "session_..."
  },
  "attachment": {
    "id": "data",
    "kind": "envd",
    "endpointRef": "http://127.0.0.1:8770/rpc",
    "environmentId": "dataset",
    "mode": "read_only"
  },
  "readiness": {
    "policy": "required",
    "timeoutMs": 5000
  },
  "idempotencyKey": "attach-data"
}
```

Params fields:

| Field            | Type   | Required | Meaning                                           |
| ---------------- | ------ | -------- | ------------------------------------------------- |
| `scope`          | object | no       | `connection` or `session`; defaults to connection |
| `attachment`     | object | yes      | Attachment source                                 |
| `readiness`      | object | no       | Probe policy and timeout                          |
| `idempotencyKey` | string | no       | Idempotent attach key                             |

Scope shape:

```json
{
  "kind": "connection"
}
```

```json
{
  "kind": "session",
  "sessionId": "session_..."
}
```

Readiness policy:

| Policy        | Meaning                                                              |
| ------------- | -------------------------------------------------------------------- |
| `required`    | Attach fails if liveness or environment readiness cannot be proven   |
| `best_effort` | Attach succeeds but reports degraded readiness                       |
| `skip`        | Construct the lease without probing; run materialization may reprobe |

Result:

```json
{
  "attachment": {
    "attachmentLeaseId": "envatt_...",
    "scope": {
      "kind": "session",
      "sessionId": "session_..."
    },
    "id": "data",
    "kind": "envd",
    "endpointRef": "http://127.0.0.1:8770/rpc",
    "environmentId": "dataset",
    "mode": "read_only",
    "default": false,
    "mountRoot": "/environment/data",
    "status": "ready",
    "readiness": {
      "transport": "ready",
      "environment": "ready",
      "capabilities": {
        "files": ["read", "list", "stat", "glob", "grep"],
        "command": [],
        "process": []
      }
    }
  }
}
```

Attach invariants:

- A repeated idempotent attach returns the original lease.
- Reusing the same `id` in the same scope with a different source returns
  `already_exists` unless a future replace mode is specified.
- Session-scoped leases may be reused by future runs for that session.
- Connection-scoped leases require a stateful connection profile such as stdio,
  local socket, named pipe, or WebSocket. Unary HTTP rejects connection-scoped
  environment leases.
- Connection-scoped leases are released on `shutdown` or connection loss.
- The manager may share one transport client between compatible leases, but
  lease ids remain distinct host-control records.

Attachment status values:

| Status        | Meaning                                        |
| ------------- | ---------------------------------------------- |
| `ready`       | Probe succeeded and the provider can be used   |
| `degraded`    | Lease exists but readiness is only best-effort |
| `unavailable` | Probe failed or transport is unreachable       |
| `detached`    | Lease was released                             |

### `environment.detach`

Releases an attachment lease.

Params:

```json
{
  "attachmentLeaseId": "envatt_...",
  "force": false,
  "idempotencyKey": "detach-data"
}
```

Params fields:

| Field               | Type    | Required | Meaning                                      |
| ------------------- | ------- | -------- | -------------------------------------------- |
| `attachmentLeaseId` | string  | yes      | Lease id returned by `environment.attach`    |
| `force`             | boolean | no       | Reserved for future force-after-run policies |
| `idempotencyKey`    | string  | no       | Idempotent detach key                        |

Result:

```json
{
  "attachmentLeaseId": "envatt_...",
  "detached": true
}
```

Detach invariants:

- Detaching a lease used by an active run returns `run_conflict` unless a future
  force policy explicitly detaches only after the active run finishes.
- Detach releases host manager resources and host-launched processes when no
  remaining lease uses them.
- Detach does not imply envd `environment.close` or `environment.unload`.
  Concrete envd lifecycle remains envd-owned.

### `environment.list`

Lists attachment leases visible to the connection.

Params:

```json
{
  "scope": {
    "kind": "session",
    "sessionId": "session_..."
  },
  "status": "ready",
  "limit": 50,
  "pageToken": null
}
```

Result:

```json
{
  "attachments": [],
  "nextPageToken": null
}
```

The result uses the same attachment lease shape returned by
`environment.attach`, but secrets and launch credentials are always redacted.

### `environment.health`

Probes one existing lease or one inline attachment source.

Params:

```json
{
  "attachmentLeaseId": "envatt_...",
  "timeoutMs": 5000
}
```

Inline probe:

```json
{
  "attachment": {
    "id": "data",
    "kind": "envd",
    "endpointRef": "http://127.0.0.1:8770/rpc",
    "environmentId": "dataset"
  },
  "timeoutMs": 5000
}
```

Result:

```json
{
  "status": "ready",
  "readiness": {
    "transport": "ready",
    "environment": "ready",
    "capabilities": {
      "files": ["read", "write", "list", "stat", "glob", "grep"],
      "command": ["run"],
      "process": ["start", "wait", "input", "signal", "kill"]
    }
  }
}
```

Health invariants:

- `environment.health` is diagnostic. A ready result is not a permanent
  guarantee; tool calls still handle per-operation failures.
- For envd, HTTP `/health` or transport initialization proves process liveness.
  `initialize` plus `environment.open` or `environment.state` proves
  environment readiness.
- Health results must not include provider credentials, raw shell output, or
  host filesystem paths beyond declared agent-facing mount roots.

### Run Materialization

Before `run.start` creates an active run, the server materializes environment
attachments:

```mermaid
sequenceDiagram
    participant Client
    participant RPC as starweaver.host RPC
    participant Manager as EnvironmentAttachmentManager
    participant Envd as EnvdRpcClient or LocalEnvd
    participant SDK as CompositeEnvironmentProvider
    participant Runtime as AgentSession

    Client->>RPC: environment.attach(optional)
    RPC->>Manager: create lease and probe
    Manager->>Envd: initialize/open/state
    Envd-->>Manager: descriptor and readiness
    Manager-->>RPC: attachmentLeaseId
    Client->>RPC: run.start(environmentAttachments)
    RPC->>Manager: materialize run attachments
    Manager->>Manager: validate ids/default/modes
    Manager->>Manager: build RunEnvironmentBinding
    Manager->>SDK: construct CompositeEnvironmentProvider
    RPC->>Runtime: start run with one provider
```

Materialization rules:

- Inline attachments and `attachmentLeaseId` refs can be mixed in one run.
- `RunEnvironmentBinding` is a host-side run-local value:
  `mounts`, `defaultMountId`, `defaultShellMountId`, readiness summary, and
  safe lease refs. It is not a general graph abstraction.
- The run-local `id` determines the agent-facing path even when a lease has a
  different source identity.
- The materialized `RunEnvironmentBinding` is immutable for the run in host
  protocol v1.
- The run record should store safe attachment refs, lease ids when applicable,
  readiness summary, and start/end state versions when providers expose them.
- Failed required probes fail `run.start` before durable active-run
  registration. If the server has already created a durable run record, it must
  mark that run failed instead of leaving an active orphan.

## Stream Methods

Streams use `ReplayScope` and `ReplayCursor`.

Scope examples:

| Scope                 | Meaning                                  |
| --------------------- | ---------------------------------------- |
| `session:session_...` | Ordered display feed across session runs |
| `run:run_...`         | Ordered display feed for one run         |

Cursor rule:

- A cursor is valid only inside its exact scope.
- A replay after cursor `N` returns events with sequence greater than `N`.
- A missing cursor means replay from the first retained event.
- Sequence values are monotonic inside a scope.
- Gaps can exist after retention trimming; servers report retained range through `stream.cursorRange`.

Stream item shape:

```json
{
  "cursor": {
    "scope": "run:run_...",
    "sequence": 3
  },
  "event": {},
  "projections": []
}
```

`event` is the canonical `ReplayEvent`. `projections` contains zero or more projection result entries requested by the client. `stream.replay`, `stream.subscribe` replay results, and `starweaver.event` notifications all use this stream item shape for canonical stream events.

### `stream.replay`

Replays retained events without opening a live subscription.

Params:

| Field        | Type   | Required | Meaning                        |
| ------------ | ------ | -------- | ------------------------------ |
| `scope`      | string | yes      | Replay scope                   |
| `cursor`     | object | no       | Last cursor observed by client |
| `limit`      | number | no       | Maximum events                 |
| `projection` | object | no       | Projection options             |

Result:

```json
{
  "scope": "run:run_...",
  "events": [],
  "nextCursor": null,
  "retainedRange": {
    "first": null,
    "last": null
  },
  "hasMore": false
}
```

`nextCursor` is the cursor for the last returned event. If no events are returned, it is the request cursor when present, otherwise `null`.

### `stream.subscribe`

Creates a live subscription, optionally replaying retained events before live events.

This method is only valid on a connection or transport profile that advertised live subscription support. Unary request/response HTTP clients use `stream.replay`, `run.status`, or `run.await` instead.

Params:

| Field        | Type   | Required | Meaning              |
| ------------ | ------ | -------- | -------------------- |
| `scope`      | string | yes      | Replay/live scope    |
| `cursor`     | object | no       | Last observed cursor |
| `replay`     | object | no       | Replay options       |
| `projection` | object | no       | Projection options   |

Replay options:

```json
{
  "enabled": true,
  "limit": 1000
}
```

Result:

```json
{
  "subscriptionId": "sub_...",
  "scope": "run:run_...",
  "cursor": null,
  "active": true,
  "replay": {
    "events": [],
    "nextCursor": null,
    "retainedRange": {
      "first": null,
      "last": null
    },
    "hasMore": false
  }
}
```

Subscription invariants:

- The server must guarantee no event gap between replayed retained events and live events observed by the subscription.
- If replay is enabled, retained events are returned in the `stream.subscribe` result. Live notifications for that subscription start only after the response is written.
- If retained replay after the requested cursor exceeds the requested `replay.limit`, the server returns `replay_limit_exceeded` and does not create the live subscription. The client should page with `stream.replay`, then subscribe from the latest cursor.
- A `subscription.ready` event is the first notification for a successful subscription and records the cursor from which live tail begins.
- A terminal stream event does not automatically delete retained replay evidence.
- `stream.unsubscribe` is idempotent for an existing or already closed subscription id.

### `stream.unsubscribe`

Closes a subscription owned by the connection.

Params:

| Field            | Type   | Required | Meaning                   |
| ---------------- | ------ | -------- | ------------------------- |
| `subscriptionId` | string | yes      | Subscription id           |
| `reason`         | string | no       | Client reason for closing |

Result:

```json
{
  "subscriptionId": "sub_...",
  "closed": true
}
```

### `stream.snapshot`

Loads the latest compact replay snapshot for a scope.

Params:

| Field   | Type   | Required | Meaning      |
| ------- | ------ | -------- | ------------ |
| `scope` | string | yes      | Replay scope |

Result:

```json
{
  "scope": "session:session_...",
  "snapshot": null
}
```

### `stream.cursorRange`

Returns the retained cursor range.

Params:

| Field   | Type   | Required | Meaning      |
| ------- | ------ | -------- | ------------ |
| `scope` | string | yes      | Replay scope |

Result:

```json
{
  "scope": "run:run_...",
  "first": {
    "scope": "run:run_...",
    "sequence": 0
  },
  "last": {
    "scope": "run:run_...",
    "sequence": 42
  }
}
```

## Event Notifications

The server emits one notification method for protocol events: `starweaver.event`.

Event envelope:

```json
{
  "jsonrpc": "2.0",
  "method": "starweaver.event",
  "params": {
    "eventId": "evt_...",
    "kind": "stream.event",
    "subscriptionId": "sub_...",
    "scope": "run:run_...",
    "cursor": {
      "scope": "run:run_...",
      "sequence": 3
    },
    "item": {
      "cursor": {
        "scope": "run:run_...",
        "sequence": 3
      },
      "event": {},
      "projections": []
    }
  }
}
```

Event kinds:

| Kind                  | Meaning                                                            |
| --------------------- | ------------------------------------------------------------------ |
| `stream.event`        | One canonical stream item containing `ReplayEvent` and projections |
| `subscription.ready`  | Replay phase is complete; live tail is attached                    |
| `subscription.closed` | Subscription closed by client, server, or terminal policy          |
| `run.started`         | Durable run accepted and active registration done                  |
| `run.status`          | Run status changed                                                 |
| `approval.changed`    | Approval record changed                                            |
| `deferred.changed`    | Deferred tool record changed                                       |
| `diagnostic`          | Non-fatal server diagnostic                                        |

Event payload rules:

| Kind                  | Required params fields                         |
| --------------------- | ---------------------------------------------- |
| `stream.event`        | `subscriptionId`, `scope`, `cursor`, `item`    |
| `subscription.ready`  | `subscriptionId`, `scope`, `cursor`            |
| `subscription.closed` | `subscriptionId`, `scope`, `reason`            |
| `run.started`         | `sessionId`, `runId`, `status`                 |
| `run.status`          | `sessionId`, `runId`, `status`, optional `run` |
| `approval.changed`    | `approval`                                     |
| `deferred.changed`    | `deferred`                                     |
| `diagnostic`          | `level`, `message`, optional `details`         |

Ordering guarantees:

- Events for one subscription are emitted in cursor order.
- `subscription.ready` for a subscription is emitted before live stream events for that subscription.
- `run.status` can be emitted independently of stream events, but terminal run status should be reflected in retained run records.
- Cross-subscription ordering is not meaningful.
- `eventId` is unique within one connection and suitable for client log correlation.

## Projections

The canonical stream payload is always `ReplayEvent`.

Projection request:

```json
{
  "formats": ["starweaver.display_message", "agui"],
  "includeCanonicalEvent": true
}
```

Projection formats:

| Format                       | Meaning                                                |
| ---------------------------- | ------------------------------------------------------ |
| `starweaver.display_message` | Extracted `DisplayMessage` when the event contains one |
| `agui`                       | AGUI top-level event mapped from `DisplayMessage`      |

Projection result entry:

```json
{
  "format": "agui",
  "payload": {
    "type": "TEXT_MESSAGE_CHUNK",
    "delta": "hello"
  }
}
```

Projection invariants:

- Projection is optional. Clients can consume `ReplayEvent` directly.
- Projection failure must not drop the canonical event.
- The default projection format is `starweaver.display_message`.
- Multiple projections can be returned when requested.
- Projection adapters live at protocol/product edges, not in `CliRuntimeCoordinator`.

## HITL Methods

Approval methods use `ApprovalRecord` as the canonical record.

`approval.list` params:

| Field       | Type   | Required | Meaning                 |
| ----------- | ------ | -------- | ----------------------- |
| `sessionId` | string | no       | Session filter          |
| `runId`     | string | no       | Run filter              |
| `status`    | string | no       | Approval status filter  |
| `limit`     | number | no       | Maximum rows            |
| `pageToken` | string | no       | Opaque pagination token |

Result:

```json
{
  "approvals": [],
  "nextPageToken": null
}
```

`approval.show` params:

| Field        | Type   | Required | Meaning     |
| ------------ | ------ | -------- | ----------- |
| `approvalId` | string | yes      | Approval id |

Result:

```json
{
  "approval": {}
}
```

`approval.decide` params:

| Field            | Type   | Required | Meaning                 |
| ---------------- | ------ | -------- | ----------------------- |
| `approvalId`     | string | yes      | Approval id             |
| `decision`       | string | yes      | `approved` or `denied`  |
| `reason`         | string | no       | Human-readable reason   |
| `idempotencyKey` | string | no       | Idempotent decision key |

Result:

```json
{
  "approval": {}
}
```

Deferred methods use `DeferredToolRecord` as the canonical record.

`deferred.list` params:

| Field       | Type   | Required | Meaning                 |
| ----------- | ------ | -------- | ----------------------- |
| `sessionId` | string | no       | Session filter          |
| `runId`     | string | no       | Run filter              |
| `status`    | string | no       | Deferred status filter  |
| `limit`     | number | no       | Maximum rows            |
| `pageToken` | string | no       | Opaque pagination token |

Result:

```json
{
  "deferred": [],
  "nextPageToken": null
}
```

`deferred.show` params:

| Field        | Type   | Required | Meaning            |
| ------------ | ------ | -------- | ------------------ |
| `deferredId` | string | yes      | Deferred record id |

Result:

```json
{
  "deferred": {}
}
```

`deferred.complete` params:

| Field            | Type   | Required | Meaning                   |
| ---------------- | ------ | -------- | ------------------------- |
| `deferredId`     | string | yes      | Deferred record id        |
| `result`         | any    | yes      | Tool result payload       |
| `idempotencyKey` | string | no       | Idempotent completion key |

Result:

```json
{
  "deferred": {}
}
```

`deferred.fail` params:

| Field            | Type   | Required | Meaning                |
| ---------------- | ------ | -------- | ---------------------- |
| `deferredId`     | string | yes      | Deferred record id     |
| `error`          | string | yes      | Failure message        |
| `idempotencyKey` | string | no       | Idempotent failure key |

Result:

```json
{
  "deferred": {}
}
```

HITL invariants:

- Decisions and deferred completions must persist before returning success.
- A repeated idempotent decision returns the persisted record.
- A conflicting repeated decision returns `idempotency_conflict`.
- HITL updates should emit `approval.changed` or `deferred.changed` for active subscriptions whose scope is affected.

## Model and Profile Methods

Shared configuration owns available model profiles. Frontend-local client state owns the selected profile for a client.

`profile.list` reads config-backed model profiles.

Params:

| Field       | Type   | Required | Meaning                 |
| ----------- | ------ | -------- | ----------------------- |
| `limit`     | number | no       | Maximum rows            |
| `pageToken` | string | no       | Opaque pagination token |

Result:

```json
{
  "profiles": [],
  "nextPageToken": null
}
```

`profile.get` reads one config-backed profile.

Params:

| Field  | Type   | Required | Meaning      |
| ------ | ------ | -------- | ------------ |
| `name` | string | yes      | Profile name |

Result:

```json
{
  "profile": {}
}
```

`model.list` returns profile summaries and the selected profile for one client state scope.

Params:

| Field              | Type   | Required | Meaning                                       |
| ------------------ | ------ | -------- | --------------------------------------------- |
| `clientStateScope` | string | no       | `tui`, `desktop`, or another registered scope |
| `limit`            | number | no       | Maximum rows                                  |
| `pageToken`        | string | no       | Opaque pagination token                       |

Result:

```json
{
  "profiles": [],
  "selectedProfile": "default_model",
  "nextPageToken": null
}
```

`model.current` reads the selected profile for one client state scope without listing all profiles.

Params:

| Field              | Type   | Required | Meaning                                       |
| ------------------ | ------ | -------- | --------------------------------------------- |
| `clientStateScope` | string | yes      | `tui`, `desktop`, or another registered scope |

Result:

```json
{
  "clientStateScope": "desktop",
  "selectedProfile": "default_model",
  "resolvedProfile": {}
}
```

`model.select` params:

| Field              | Type   | Required | Meaning                                       |
| ------------------ | ------ | -------- | --------------------------------------------- |
| `clientStateScope` | string | yes      | `tui`, `desktop`, or another registered scope |
| `profile`          | string | yes      | Config-backed model profile name              |

Result:

```json
{
  "clientStateScope": "desktop",
  "selectedProfile": "default_model",
  "resolvedProfile": {}
}
```

`model.select` must not mutate shared `~/.starweaver/config.toml`.

Run model selection priority:

1. Explicit `run.start.model.profile`.
2. Selected profile for `clientStateScope`.
3. Resolved config default profile.

## Config and Diagnostics

`config.get` reads selected resolved config values through an allowlist. It must not expose secrets by default.

`config.get` params:

```json
{
  "keys": ["general.default_profile", "storage.database_path"]
}
```

`config.get` result:

```json
{
  "values": {
    "general.default_profile": "default_model"
  }
}
```

`diagnostics.get` returns operational state safe for clients.

Params:

| Field     | Type  | Required | Meaning                                             |
| --------- | ----- | -------- | --------------------------------------------------- |
| `include` | array | no       | Optional sections such as `storage`, `runs`, `logs` |

Result:

```json
{
  "serverInfo": {},
  "protocol": {},
  "storage": {},
  "activeRuns": [],
  "clientStateScopes": [],
  "profiles": [],
  "environmentPolicies": []
}
```

Diagnostic sections may include:

- server version
- protocol identity
- storage paths
- active run counts
- environment attachment summaries and readiness
- selected client state roots
- configured model profile names
- environment policy summaries

Diagnostics must not include OAuth tokens, API keys, provider request bodies, or raw shell output.

## Idempotency

Mutating methods accept `idempotencyKey`.

Recommended methods:

- `session.create`
- `run.start`
- `run.cancel`
- `run.steer`
- `environment.attach`
- `environment.detach`
- `approval.decide`
- `deferred.complete`
- `deferred.fail`
- `session.current.set`
- `session.delete`

Semantics:

- Idempotency scope is `(clientIdentity.clientId or connectionId, method, idempotencyKey)`.
- The server stores a digest of canonical params and the result record reference.
- Repeating the same method with the same key and same params returns the original result.
- Repeating the same method with the same key and different params returns `idempotency_conflict`.
- For `run.start`, idempotency must prevent duplicate durable runs.
- For process-local stdio servers, in-memory idempotency is acceptable for methods that do not create durable state. Durable run creation should persist enough metadata to survive a client reconnect when practical.

## Error Model

JSON-RPC standard codes:

| Code     | Meaning          | Use                                                |
| -------- | ---------------- | -------------------------------------------------- |
| `-32700` | Parse error      | Invalid JSON                                       |
| `-32600` | Invalid request  | Envelope is not a valid host protocol request      |
| `-32601` | Method not found | Unknown method                                     |
| `-32602` | Invalid params   | Params shape, type, enum, or field validation fail |
| `-32603` | Internal error   | Unexpected server failure                          |

Starweaver domain codes:

| Code     | Kind                      | Meaning                                                   |
| -------- | ------------------------- | --------------------------------------------------------- |
| `-32001` | `not_initialized`         | Method requires successful initialize                     |
| `-32002` | `unsupported_feature`     | Required feature is unavailable                           |
| `-32010` | `not_found`               | Session, run, approval, deferred, or subscription missing |
| `-32011` | `already_exists`          | Create conflicts with existing record                     |
| `-32012` | `idempotency_conflict`    | Same key used with different params                       |
| `-32013` | `run_conflict`            | Run state does not allow requested action                 |
| `-32014` | `run_not_active`          | Active-run-only action requested for inactive run         |
| `-32015` | `stream_cursor_invalid`   | Cursor scope or sequence is invalid                       |
| `-32016` | `stream_trimmed`          | Requested cursor is before retained range                 |
| `-32017` | `subscription_closed`     | Subscription no longer accepts events                     |
| `-32018` | `timeout`                 | Operation timed out                                       |
| `-32019` | `cancelled`               | Operation was cancelled                                   |
| `-32020` | `approval_conflict`       | Approval has already reached a terminal decision          |
| `-32021` | `deferred_conflict`       | Deferred record has already reached a terminal state      |
| `-32022` | `replay_limit_exceeded`   | Subscribe replay would exceed requested limit             |
| `-32030` | `execution_failed`        | Runtime or tool execution failed                          |
| `-32031` | `environment_unavailable` | Environment transport or readiness probe failed           |
| `-32040` | `storage_failed`          | Durable storage operation failed                          |
| `-32050` | `configuration_failed`    | Config/profile resolution failed                          |
| `-32060` | `payload_too_large`       | Request, response, or frame exceeds advertised limits     |
| `-32061` | `resource_exhausted`      | Subscription, page, or server resource limit reached      |

Error data shape:

```json
{
  "kind": "stream_cursor_invalid",
  "retryable": false,
  "field": "cursor.scope",
  "expected": "run:run_...",
  "received": "session:session_...",
  "sessionId": "session_...",
  "runId": "run_..."
}
```

`replay_limit_exceeded` error data should include `scope`, `requestedLimit`, `retainedRange`, and `nextPageCursor` when known.

Error invariants:

- `error.message` is concise and safe to show.
- `error.data.kind` is stable and machine-readable.
- `error.data.retryable` tells clients whether immediate retry is meaningful.
- Validation errors include a field path when possible.
- Storage and runtime failures should preserve a safe failure category without exposing secrets.
- Environment availability failures should include safe fields such as
  `attachmentLeaseId`, `id`, `endpointRef`, `environmentId`, `phase`, and
  `retryable`, but not credentials, command output, or daemon stderr.
- Oversized requests fail with `payload_too_large` when the server can still emit a JSON-RPC error; otherwise the transport may close.
- Exhausted subscription slots or other advertised resource limits fail with `resource_exhausted`.

## Retention and Reconnect

Reconnect flow:

1. Client stores the last `ReplayCursor` per scope.
2. Client reconnects and calls `initialize`.
3. Client calls `stream.replay` or `stream.subscribe` with the stored cursor.
4. Server returns retained events after the cursor or reports `stream_trimmed`.
5. If trimmed, client can load `stream.snapshot` and continue from the retained range.

Retention invariants:

- Servers should expose retained cursor range.
- Trimmed evidence must be explicit. Silent replay from the earliest retained event after an out-of-range cursor is not allowed.
- Compact snapshots should include the cursor they summarize.
- Clients should treat snapshots as read models, not as exact event logs.

## Security and Locality

The stdio profile is local-process IPC. It inherits the user's local OS identity and filesystem permissions.

Security rules:

- Remote exposure is outside stdio profile scope.
- Local sockets and named pipes must use same-user filesystem or OS permissions.
- WebSocket profile must be protected by a platform-owned auth layer before remote use.
- `config.get` must use an allowlist and redact secrets.
- Diagnostics must never expose provider credentials, OAuth refresh tokens, raw request bodies, or shell output.
- Paths returned to clients should be limited to operational paths necessary for diagnostics.
- Runtime/tool approvals and deferred completions remain explicit methods; the protocol must not silently auto-approve actions.

## Crate and Implementation Shape

The current implementation includes `starweaver-rpc` as the standalone process entrypoint and `starweaver-rpc-core` as the shared protocol helper crate. The process entrypoint owns its argv parser and calls the shared RPC server API so Desktop and local clients can launch a dedicated host binary now. The remaining extraction target is to move typed params/results, transport-independent service wiring, and golden fixtures out of `starweaver-cli` and into the RPC crates.

Current `starweaver-rpc-core` ownership:

- JSON-RPC frame parsing and request validation
- JSON-RPC request/error/notification envelopes
- stream payload format parsing
- replay cursor parsing and scope validation
- replay result and environment attachment result helpers
- `DisplayMessage` and AGUI output projection helpers

Target `starweaver-rpc-core` ownership:

- method constants or `RpcMethod`
- params/result structs
- environment attachment params/result structs and validation helpers
- event envelope structs
- error code/data structs
- protocol identity and feature names
- serde round-trip tests
- optional JSON Schema generation for host clients
- golden JSON fixture tests

Target `starweaver-rpc` ownership:

- standalone `starweaver-rpc` binary
- process lifecycle and local transport selection

Current `starweaver-cli` ownership during extraction:

- stdio and HTTP transport profiles
- local server adapter
- router wiring to `CliRuntimeCoordinator`
- local `EnvironmentAttachmentManager` implementation
- local session/current-state/profile/config handlers
- projection adapters for AGUI and native display messages
- local process lifecycle and shutdown

`starweaver-storage` owns concrete SQLite `SessionStore`, `StreamArchive`, and `ReplayEventLog` adapters once storage convergence lands. The CLI server should depend on shared store/archive traits at the protocol handler boundary.

Target module shape:

```mermaid
flowchart TD
    rpc_crate[starweaver-rpc]
    stdio[cli rpc stdio transport]
    http[cli rpc HTTP transport]
    server[cli rpc server]
    router[method router]
    session_handlers[session handlers]
    run_handlers[run handlers]
    stream_handlers[stream handlers]
    hitl_handlers[hitl handlers]
    env_handlers[environment attachment handlers]
    env_manager[EnvironmentAttachmentManager]
    model_handlers[model and profile handlers]
    config_handlers[config and diagnostics handlers]
    coordinator[CliRuntimeCoordinator]
    stores[SessionStore, StreamArchive, ReplayEventLog adapters]

    rpc_crate --> server
    stdio --> server
    http --> server
    server --> router
    router --> session_handlers
    router --> run_handlers
    router --> env_handlers
    router --> stream_handlers
    router --> hitl_handlers
    router --> model_handlers
    router --> config_handlers
    env_handlers --> env_manager
    run_handlers --> env_manager
    run_handlers --> coordinator
    stream_handlers --> coordinator
    stream_handlers --> stores
    session_handlers --> stores
```

## Transition From Current CLI RPC Surface

The current CLI RPC implementation already proves local stdio and HTTP framing, model selection, session management, non-blocking runs, active control, approvals, deferred tools, replay, and AGUI projection. The target protocol tightens that surface in these ways:

| Current shape                                                              | Target shape                                                                 |
| -------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Shared frame/projection helpers but CLI-owned method handlers              | Public typed `starweaver-rpc-core` protocol/service crate                    |
| `payloadFormat` centered on AGUI/default output                            | Canonical `ReplayEvent` with optional projections                            |
| `session.replay`, `session.output`, `run.attach` split by product behavior | `stream.replay` and notification-capable `stream.subscribe` by `ReplayScope` |
| Ad hoc params via `serde_json::Value`                                      | Typed params/results with serde fixture tests                                |
| Mostly string errors                                                       | JSON-RPC errors with stable `error.data.kind`                                |
| `prompt` field as primary run input                                        | `InputPart` list as primary input                                            |
| No idempotency contract                                                    | Idempotency keys on mutating methods                                         |
| Per-method live notification names                                         | Unified `starweaver.event` envelope                                          |

This transition should be done as a protocol cleanup before Desktop depends on the current method surface.

## Review Checklist

Architecture review:

- RPC is a host-control protocol, not a CLI helper module.
- TUI is not routed through RPC.
- CLI commands are not the protocol source of truth.
- Environment attachment management is host-control; envd remains the
  environment data/effect plane.
- Runtime checkpoint and stream record types remain durable upstream evidence, not a new protocol crate.
- Session and stream persistence are accessed through shared contracts.
- Projection stays at protocol/product edges.

Wire review:

- Every request and response has a typed schema.
- Every mutating request has an idempotency story.
- Every replay/live event carries a scope and cursor.
- Every list method has a pagination story.
- Every error has a machine-readable kind.
- Environment attachment ids, default selection, readiness, and lease scope have
  typed validation.
- Every notification kind has required payload fields.
- Advertised limits have defined failure behavior.
- Embedded shared records keep their existing serde shape.

Streaming review:

- Subscribe has no replay/live gap.
- Cursor semantics are scope-local.
- Retention trimming is explicit.
- Snapshot and replay roles are distinct.
- Terminal events do not erase durable replay evidence.
- Cross-subscription ordering is not promised.

Operational review:

- Stdio stdout is protocol-only.
- Stderr is diagnostics-only.
- Config reads are allowlisted.
- Secrets are redacted.
- Shutdown closes subscriptions predictably.
- Attachment manager leases have explicit scope and release behavior.
- Transport profiles do not change method semantics.

Future-surface review:

- Local socket and WebSocket can reuse the same protocol crate.
- Desktop can consume the protocol without importing CLI internals.
- Platform adapters can bridge events without adopting AGUI as the core.
- JSON Schema and golden fixtures can be generated from typed protocol structs.

## Acceptance Gates

Before this protocol is accepted as implemented:

```bash
cargo test -p starweaver-rpc --locked
cargo test -p starweaver-cli --locked rpc -- --nocapture
make fmt-check
make check
make test
git diff --check
```

Required tests:

- JSON-RPC envelope parse, invalid request, notification, shutdown, and method-not-found fixtures.
- `initialize` feature negotiation fixtures.
- Typed serde round trips for every params/result/event/error type.
- Error fixtures for invalid params, not found, stream cursor invalid, stream trimmed, timeout, run not active, and idempotency conflict.
- Error fixture for `replay_limit_exceeded` on `stream.subscribe`.
- Error fixtures for `payload_too_large` and `resource_exhausted`.
- `run.start` idempotency fixture proving no duplicate run.
- `run.start` model override and HITL policy validation fixtures.
- `environment.attach` idempotency fixture proving no duplicate lease.
- `environment.detach` conflict fixture for an active run attachment.
- `environment.health` fixture covering ready, unavailable, and redacted error data.
- `run.start` multi-attachment fixture proving default selection and `/environment/{id}` materialization.
- Current-session pointer set, clear, and missing-session fixtures.
- `stream.replay` run scope and session scope fixtures.
- `stream.subscribe` replay-before-live no-gap fixture.
- Projection fixture proving canonical event remains present when AGUI projection is requested.
- HITL approval/deferred decision idempotency fixtures.
- Profile/model selection fixtures proving client-local selection does not mutate shared config.
- Stdio transport fixture proving stdout contains only protocol frames.
- HTTP transport fixture proving `POST /rpc` dispatches through the same RPC service and `shutdown` stops the accept loop.
