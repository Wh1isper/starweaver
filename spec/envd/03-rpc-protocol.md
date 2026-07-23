# Envd RPC Protocol

Status: implemented normative profile

Revision: 2026-07-23

Envd RPC exposes `EnvdService` over JSON-RPC. Stdio and HTTP are initial
transports. The protocol must match the service semantics exactly.

## Protocol Identity

```json
{
  "name": "starweaver.envd",
  "major": 2,
  "revision": "2026-07-23",
  "features": [
    "environment.lifecycle",
    "files",
    "commands",
    "processes"
  ]
}
```

`InitializeEnvdRequest.protocol` carries this required typed identity. Services and clients validate the exact name and major through `starweaver-envd-core`; revision is fixture/documentation identity rather than an ordered compatibility gate. `InitializeEnvdResult.protocol` is the only protocol-version field, and implementations must not emit a duplicate `protocolVersion` or `protocol_version` string.

Missing identities, wrong names, and unsupported majors fail initialization as invalid requests. The release fixture under `crates/starweaver-envd-core/tests/fixtures/contracts/` fixes the identity, feature vocabulary, and rejection behavior.

## Transport Profiles

| Profile | Framing                                  | Use                                  |
| ------- | ---------------------------------------- | ------------------------------------ |
| `stdio` | one JSON-RPC object per UTF-8 line       | host-launched envd process           |
| `http`  | one JSON-RPC object per `POST /rpc` body | local automation and service clients |

Future profiles can add local sockets, named pipes, or WebSocket. Method
semantics must not change across transports.

## Stdio Rules

- stdin carries requests.
- stdout carries JSON-RPC responses and notifications only.
- stderr carries diagnostics.
- each non-empty stdin line is one JSON object.
- each stdout line is one JSON object.
- server exits after successful `shutdown` or stdin close.

## HTTP Rules

- `POST /rpc` carries one JSON-RPC request.
- successful JSON-RPC response uses HTTP `200`.
- parsing failures can use HTTP `4xx` before JSON-RPC dispatch.
- HTTP mode requires a non-empty bearer token configured when the daemon starts.
- Every HTTP request, including `GET /health` and `GET /healthz`, must include
  `Authorization: Bearer <token>` or the server returns HTTP `401` before
  JSON-RPC dispatch.
- `GET /health` and `GET /healthz` can expose lightweight health after
  authorization succeeds.
- unary HTTP does not carry live notifications unless a future long-connection
  profile is negotiated.

Bearer tokens are transport credentials. They are not JSON-RPC params, must not
be included in envd operation records, and must not be echoed by host-control
attachment results.

## Health and Readiness

Envd health has two layers:

- transport health: the daemon process is reachable and can parse requests.
- environment readiness: a specific `environmentId` is open, its mounts are
  ready, and advertised capabilities are currently usable.

The current HTTP transport may expose lightweight `GET /health` and
`GET /healthz` for process liveness. Transport-neutral clients should probe by
calling `initialize`, then `environment.open` or `environment.state` for each
environment they intend to use.

A future typed method can expose the same readiness shape over every transport:

```json
{
  "status": "ready",
  "protocol": {
    "name": "starweaver.envd",
    "major": 2,
    "revision": "2026-07-23"
  },
  "environments": [{
    "environmentId": "env_cli_default",
    "status": "open",
    "stateVersion": 3,
    "mounts": [{
      "mountId": "workspace",
      "status": "ready",
      "mode": "read_write"
    }]
  }]
}
```

Host integrations should treat readiness failures as attachment-resolution
errors before starting a run. Runtime tools should still handle per-operation
errors because readiness can change after a run begins.

## Method Groups

| Group       | Methods                                                                                                                                                         |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Lifecycle   | `initialize`, `shutdown`                                                                                                                                        |
| Environment | `environment.open`, `environment.state`, `snapshot.export`                                                                                                      |
| File        | `file.read`, `file.write`, `file.create_dir`, `file.delete`, `file.move`, `file.copy`, `file.write_scratch`, `file.list`, `file.stat`, `file.glob`, `file.grep` |
| Command     | `command.run`                                                                                                                                                   |
| Process     | `process.start`, `process.wait`, `process.list`, `process.input`, `process.signal`, `process.kill`                                                              |
| Context     | `context.render`, `shell.review_context`                                                                                                                        |

Protocol major 2 at revision `2026-07-23` replaces the temporary-file vocabulary
atomically with `FileWriteScratchRequest`, `FileWriteScratchResult`, and
`file.write_scratch`. There is no `file.write_tmp` fallback or dual DTO surface;
major-1 peers are rejected during initialization.
The returned path is owned by the target environment and must remain usable by
its ordinary file operations and shell context.

Interactive terminal state is not part of envd v2. Execution capabilities are
exposed as foreground commands and background process handles.

## Common Params

The implemented v2 minimum uses method-specific params plus `environmentId`
where an environment method needs it. The richer request metadata below is a
planned protocol extension for remote, audited, or retry-heavy envd
deployments, not a requirement for the current local CLI path.

```json
{
  "environmentId": "env_cli_default",
  "requestId": "req_123",
  "idempotencyKey": "run_1:tool_2",
  "traceContext": {},
  "actor": {
    "actorId": "cli",
    "kind": "host"
  },
  "metadata": {}
}
```

Rules:

- `environmentId` is required for environment methods unless the service has one
  implicit environment.
- `idempotencyKey` should become required for mutating methods where retry can
  duplicate effects once envd supports typed idempotency records.
- `traceContext` propagates host/runtime traces.
- `actor` is required when policy or audit is enabled.

## File Method Examples

`file.read`:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "file.read",
  "params": {
    "environmentId": "env_cli_default",
    "path": "/README.md"
  }
}
```

Result:

```json
{
  "environmentId": "env_cli_default",
  "stateVersion": "sv_3",
  "path": "/README.md",
  "content": "hello",
  "contentType": "text/markdown"
}
```

`file.write`:

```json
{
  "environmentId": "env_cli_default",
  "path": "/README.md",
  "content": "hello",
  "baseVersion": "sv_2",
  "idempotencyKey": "run_1:tool_4"
}
```

## Process Method Examples

`process.start`:

```json
{
  "environmentId": "env_cli_default",
  "command": {
    "command": "cargo test",
    "cwd": "/",
    "timeoutSeconds": 120,
    "environment": {}
  },
  "idempotencyKey": "run_1:tool_5"
}
```

Result:

```json
{
  "processId": "proc_1",
  "status": "running",
  "stdout": "",
  "stderr": "",
  "returnCode": null
}
```

## Error Codes

Use JSON-RPC standard errors plus envd server errors.

| Code     | Kind                    | Meaning                             |
| -------- | ----------------------- | ----------------------------------- |
| `-32700` | `parse_error`           | invalid JSON                        |
| `-32600` | `invalid_request`       | invalid JSON-RPC request            |
| `-32601` | `method_not_found`      | unknown method                      |
| `-32602` | `invalid_params`        | invalid params                      |
| `-32001` | `not_initialized`       | initialize required                 |
| `-32002` | `unsupported_feature`   | method or transport feature missing |
| `-32010` | `not_found`             | environment/path/process not found  |
| `-32011` | `permission_denied`     | policy denied                       |
| `-32012` | `state_conflict`        | base version mismatch               |
| `-32013` | `resource_conflict`     | idempotency/process/mount conflict  |
| `-32020` | `execution_failed`      | command or process failed           |
| `-32021` | `execution_unavailable` | execution backend unavailable       |
| `-32030` | `payload_too_large`     | request or response too large       |
| `-32050` | `internal`              | server failure                      |

Error data should include:

- `kind`
- `retryable`
- `environmentId`
- resource ids when relevant
- policy or state version details when relevant

## Idempotency

Mutating methods use `(environmentId, actorId, method, idempotencyKey)` as the
retry scope. Repeating a completed mutation returns the original result when the
payload matches. Repeating with a different payload returns `resource_conflict`.

## Streaming

Initial stdio/http can be request/response only. Streaming can be added in two
ways:

- polling methods such as `process.wait` and `process.list`
- future subscription methods over notification-capable transports

Do not block envd v2 on streaming. Process output can be returned in snapshots
until output cursors are added.
