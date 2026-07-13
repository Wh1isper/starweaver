# JSON-RPC Host Protocol v1

Status: implemented normative profile

Revision: 2026-07-11

The Starweaver host protocol is implemented by the standalone `starweaver-rpc` product. It is a local control plane for durable sessions, non-blocking runs, replay, HITL records, RPC-owned model profiles, and RPC-owned environment attachments.

This document describes only implemented v1 behavior. Proposed long-connection subscriptions, additional authorization roles, pagination tokens, richer idempotency, sockets, WebSocket, and hosted deployment semantics live in `rfcs/host-protocol-future.md` and are not part of v1 conformance.

## Product Boundary

- `starweaver-rpc` and `starweaver-cli` are independent products.
- Neither crate may depend on the other directly or transitively.
- CLI/TUI is not an RPC frontend and does not reuse RPC handlers, configuration, coordinator state, or attachment leases.
- Both products may independently use lower runtime, storage, stream, environment, and envd abstractions.
- `starweaver-rpc-core` owns typed host wire contracts and projections.
- `starweaver-rpc` owns configuration, model materialization, handlers, active runs, environment leases, and transports.

The permanent `make architecture-check` gate enforces the dependency boundary.

## Protocol Identity

Initialization returns the shared identity shape:

```json
{
  "protocol": {
    "name": "starweaver.host",
    "major": 1,
    "revision": "2026-07-11",
    "features": [
      "sessions",
      "runs",
      "stream.replay",
      "environment.attachments",
      "environment.active_mounts",
      "hitl"
    ]
  },
  "serverInfo": {
    "name": "starweaver-rpc",
    "version": "X.Y.Z"
  }
}
```

Identity constants are owned by `starweaver-rpc-core`. Clients validate `name` and `major`, then use `features`. They must not compare revision strings for ordering. The previous top-level `protocolVersion` date field is accepted as legacy response evidence but is no longer emitted.

`stream.subscribe` is not advertised. Calling `stream.subscribe` or `stream.unsubscribe` returns `unsupported_feature` on current stdio and unary HTTP transports.

## RPC-owned Configuration

RPC resolves `$STARWEAVER_CONFIG_DIR/rpc.toml` (default `~/.starweaver/rpc.toml`) or `STARWEAVER_RPC_CONFIG`. It never reads CLI `config.toml` through CLI types.

```toml
[server]
default_profile = "default"
database_path = "starweaver.sqlite3"
workspace_root = "."

[server.http_auth]
token_env = "STARWEAVER_RPC_TOKEN"
# token_file = "secrets/rpc-token" # relative to rpc.toml; mode 0600 on Unix
scopes = ["read", "run", "approval", "admin", "shutdown"]
# allowed_origins = ["https://trusted-desktop.example"]
# allowed_hosts = ["rpc.internal.example:8765"]

[profiles.default]
model_id = "openai-responses:gpt-5"
toolsets = ["filesystem"]

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
```

At run start RPC validates the profile/toolsets, resolves `protocol:model` or `provider@protocol:model`, reads the configured API-key environment variable, builds a production `ProtocolModelClient`, projects an `AgentSpec`, and constructs its own runtime. Deterministic models are private test fixtures and are not production management profiles.

## JSON-RPC Envelope

Requests and responses use JSON-RPC 2.0. Requests require object params; omitted params are treated as an empty object. Scalar, `null`, and positional array params fail closed with `invalid request`; batch arrays are rejected. A missing request id is a notification. Present ids may be strings, integers, or explicit `null` (discouraged by JSON-RPC 2.0 but valid); booleans, fractional numbers, arrays, and objects are rejected. Server stdout in stdio mode contains protocol frames only.

Request:

```json
{"jsonrpc":"2.0","id":"req_1","method":"session.list","params":{}}
```

Success:

```json
{"jsonrpc":"2.0","id":"req_1","result":{}}
```

Error:

```json
{"jsonrpc":"2.0","id":"req_1","error":{"code":-32602,"message":"invalid params: missing sessionId"}}
```

Current domain codes:

|     Code | Meaning                 |
| -------: | ----------------------- |
| `-32700` | parse error             |
| `-32600` | invalid request         |
| `-32601` | method not found        |
| `-32602` | invalid params          |
| `-32000` | internal/server failure |
| `-32002` | unsupported feature     |
| `-32011` | already exists          |
| `-32012` | idempotency conflict    |
| `-32013` | run conflict            |
| `-32031` | environment unavailable |
| `-32050` | configuration failure   |

Messages are safe for client display and must not include credentials, provider request bodies, raw shell output, or unredacted endpoint launch data.

## Transport Profiles

### Stdio

- One UTF-8 JSON object per non-empty input line.
- stdin carries requests; stdout carries responses; stderr carries diagnostics.
- The process exits after successful `shutdown` response or stdin close.
- Live display notifications may be emitted only by the stdio service mode that explicitly enables them.

### Unary HTTP

- `POST /rpc` carries one JSON-RPC object and requires `Content-Type: application/json` (an optional UTF-8 charset is accepted). Browser-simple `text/plain` writes are rejected.
- `GET /health` and `GET /healthz` provide authenticated local liveness.
- Every endpoint requires `Authorization: Bearer <token>`. If the configured token environment variable is absent, startup creates `$state_dir/http-token` atomically with mode 0600 and prints only its path.
- HTTP remains loopback-only until TLS or an authenticated reverse-proxy deployment profile is implemented.
- `Host` must match the listener or an explicit allowlist. Requests carrying `Origin` are rejected unless that exact origin is allowlisted, preventing DNS-rebinding and browser blind-write paths.
- Bearer comparison is constant-time. Token files must be regular private files and tokens must contain at least 32 non-whitespace bytes.
- Request size, header size, connection count, read deadline, and total `run.await` duration are bounded.
- Unary HTTP does not advertise live subscription capability.
- Successful `shutdown` stops the accept loop.

## Implemented Methods

| Group                 | Methods                                                                                                                                                                   |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Lifecycle             | `initialize`, `shutdown`                                                                                                                                                  |
| Diagnostics           | `diagnostics.get`, `config.get`                                                                                                                                           |
| Profiles              | `profile.list`, `profile.get`, `model.list`, `model.current`, `model.select`                                                                                              |
| Sessions              | `session.create`, `session.list`, `session.get`, `session.current.get`, `session.current.set`, `session.delete`                                                           |
| Runs                  | `run.start`, `run.prompt`, `run.status`, `run.await`, `run.cancel`, `run.steer`, `run.attach`                                                                             |
| Streams               | `stream.replay`                                                                                                                                                           |
| Compatibility aliases | `session.output`, `session.replay`                                                                                                                                        |
| HITL                  | `approval.list`, `approval.show`, `approval.decide`, `deferred.list`, `deferred.show`, `deferred.complete`, `deferred.fail`                                               |
| Environments          | `environment.attach`, `environment.detach`, `environment.list`, `environment.health`, `environment.active_mount`, `environment.active_unmount`, `environment.active_list` |

A method not in this table is not part of implemented v1. Compatibility aliases are provisional and may be removed in the next host protocol major after clients migrate.

## Durable Record Compatibility

Shared durable records follow `spec/core/07-versioned-protocol-contracts.md`.

- Method DTOs are fixed by host protocol major 1.
- Opaque durable evidence uses `{schema, version, payload}`.
- Embedded session/run records use their typed v1 projection in method results.
- The server can read previous bare v0 SQLite JSON and writes current v1 envelopes.
- Unknown durable schema versions fail; they are never coerced through generic metadata.

## Run Input

`run.start` accepts canonical durable input:

```json
{
  "input": {
    "parts": [
      {"kind":"text","text":"summarize this repository"}
    ]
  },
  "profile": "default"
}
```

Each part is `starweaver-session::InputPart`. Every canonical model `ContentPart` has an explicit lossless durable variant. New requests must not encode content through `kind: "mode", mode: "content_part"`.

For compatibility, `prompt` remains accepted when `input` is absent and is converted to one text part. Supplying both is invalid. `run.prompt` remains a blocking compatibility method over the same input preparation path.

Run creation resolves session selection and environment attachments before active registration. The created run record persists the exact durable input parts. Runtime conversion is exhaustive and errors before model execution if legacy product-edge evidence is not model content.

## Run Lifecycle

Wire status values use the shared lifecycle contract:

```text
queued, starting, running, waiting, completed, failed, cancelled
```

`queued` is durable admission state. The remaining values are shared `RunLifecycle` values. RPC does not maintain a separate status enum.

- `run.start` returns after durable creation and active registration.
- `run.status` prefers active state and falls back to durable state.
- `run.await` uses one absolute deadline and returns only terminal state or timeout.
- Client disconnect cancels the await request, not the run.
- `run.cancel` requests cooperative cancellation.
- `run.steer` accepts active-run steering text and returns its steering id.

## Stream Replay

Canonical replay contracts are `ReplayScope`, `ReplayCursor`, and `ReplayEvent`.

```json
{
  "scope": "run:run_...",
  "cursor": {
    "family": "replay_event",
    "scope": "run:run_...",
    "sequence": 3
  }
}
```

Rules:

- A cursor is valid only for its exact family and scope. `stream.replay` accepts only `replay_event` cursors.
- Replay returns events with sequence greater than the supplied event-family cursor.
- Event sequence is monotonic within one scope; retention gaps are explicit.
- Display-message sequence and replay-event sequence are independent. A display replay event preserves the nested display-family sequence while the outer event uses RPC's event-family sequence.
- RPC owns append, live publication, and persistence for its replay-event projection. Agent/runtime display persistence remains a separate lower-level archive family.
- RPC projections preserve the canonical replay event and can add display-message or AGUI payloads.
- Environment lifecycle events remain typed replay events and project to `HOST_EVENT`; they are not text chunks.

`run.attach`, `session.output`, and `session.replay` are compatibility surfaces over retained replay. `stream.subscribe` is reserved but not implemented.

## Environment Attachments

RPC owns an `EnvironmentAttachmentManager` independent of CLI/TUI. It resolves local and literal envd sources into one SDK environment provider for a run.

Implemented source rules:

- `id: "local"` is reserved for `kind: "local"`.
- envd sources use literal loopback `http://...` with request-only bearer token or trusted-local `stdio://...` launch refs.
- URL userinfo, credential-like query parameters, fragments, and embedded tokens are rejected.
- session leases can only attach to runs in that session.
- connection leases are available only to stateful stdio connections, not unary HTTP.
- read-only leases cannot be widened.
- required readiness is probed before run registration.
- tokens, launch arguments, and undeclared host paths never appear in results, replay, diagnostics, or model context.

Active mount/unmount operations are serialized per run and use monotonic `bindingVersion`. Successful mutations append typed environment lifecycle replay before acknowledging success. Context injection occurs through steering after the lifecycle event; an injection failure leaves the mount active and returns a safe warning.

Envd remains the environment data/effect plane. Host attachment leases do not transfer envd lifecycle ownership to RPC.

## HITL

Approval and deferred records are canonical `starweaver-session` durable records. Decisions persist before success is returned. Terminal conflicts fail rather than overwrite prior evidence.

Current v1 does not promise a general cross-method idempotency store. Active environment mutation uses operation-specific idempotency to prevent duplicate binding versions, lifecycle events, and steering injection. Richer method-wide idempotency remains an RFC.

## Security

- Stdio inherits the local OS process identity and does not use HTTP bearer credentials.
- HTTP is bearer-authenticated by default and loopback-only. Authentication is evaluated before JSON-RPC dispatch, including health requests.
- HTTP methods require one explicit scope: `read` for queries/initialize, `run` for session/run/environment effects, `approval` for HITL decisions, `admin` for administrative mutations, and `shutdown` for process termination. Scopes do not imply one another.
- Missing/invalid credentials return HTTP 401; valid credentials lacking the method scope return HTTP 403.
- Config reads are allowlisted.
- RPC profile credentials are loaded at run start and are never returned.
- Environment bearer tokens are request-only.
- Provider routing headers come from typed model routing settings, never durable session/run metadata.
- Shutdown, execution, approval, and mutation methods must not become remotely exposed without an explicit authenticated policy.

## Conformance

Required gates:

```bash
cargo test -p starweaver-rpc-core --locked
cargo test -p starweaver-rpc --all-targets --locked
cargo run -p xtask --locked -- check-architecture
make capability-check
git diff --check
```

Fixtures cover protocol identity, feature negotiation, JSON-RPC id/params framing, typed input, all lifecycle values, replay cursor validation, projections, loopback HTTP policy, bearer/scope/Host/Origin/content-type rejection, absolute await timeout, RPC-owned profile materialization, local/envd attachments, active binding mutations, redaction, stdio stdout purity, authenticated HTTP shutdown, and the CLI/RPC dependency prohibition. Stdio and HTTP consume one shared method-group/error conformance vector set so transport dispatch cannot drift.
