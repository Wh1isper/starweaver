# Reference Provider and Host Integration

This page describes Starweaver as one envd consumer. It is a reference
integration, not a requirement that envd only work with Starweaver.

Envd becomes useful to Starweaver through two integration points:

1. `EnvironmentProvider` adapts envd service calls for tools.
2. Host services and RPC select or open envd environments for runs.

## EnvironmentProvider Adapter

`EnvdEnvironmentProvider` wraps `Arc<dyn EnvdService>`. Other runtimes can build
their own adapter over the same service interface.

```mermaid
flowchart TD
    tool[Tool call]
    provider[EnvdEnvironmentProvider]
    service[EnvdService]
    local[LocalEnvd]
    rpc_client[EnvdRpcClient]

    tool --> provider
    provider --> service
    service --> local
    service --> rpc_client
```

Method mapping:

| `EnvironmentProvider` method | `EnvdService` method        |
| ---------------------------- | --------------------------- |
| `read_text`                  | `file_read`                 |
| `read_bytes`                 | `file_read` with byte range |
| `write_text`                 | `file_write`                |
| `create_dir`                 | file mutation method        |
| `delete_path`                | file mutation method        |
| `move_path`                  | file mutation method        |
| `copy_path`                  | file mutation method        |
| `write_tmp_file`             | scratch/tmp write method    |
| `stat`                       | `file_stat`                 |
| `list`                       | `file_list`                 |
| `glob`                       | `file_glob`                 |
| `grep`                       | `file_grep`                 |
| `run_shell`                  | `command_run`               |
| `export_state`               | `export_snapshot`           |

`ProcessShellProvider` mapping:

| `ProcessShellProvider` method | `EnvdService` method |
| ----------------------------- | -------------------- |
| `start_process`               | `process_start`      |
| `wait_process`                | `process_wait`       |
| `list_processes`              | `process_list`       |
| `input_process`               | `process_input`      |
| `signal_process`              | `process_signal`     |
| `kill_process`                | `process_kill`       |

## CLI Direct Mode

CLI direct mode should construct `LocalEnvd` and wrap it with
`EnvdEnvironmentProvider`.

```mermaid
sequenceDiagram
    participant CLI
    participant Service as HeadlessHostService
    participant Envd as LocalEnvd
    participant Provider as EnvdEnvironmentProvider
    participant Runtime

    CLI->>Service: prepare run
    Service->>Envd: create/open implicit environment
    Service->>Provider: wrap EnvdService
    Service->>Runtime: run AgentSession
    Runtime->>Provider: tool file/process call
    Provider->>Envd: direct service call
    Envd-->>Provider: result
```

This is the desired special case: no RPC, one env, direct code path.

## RPC Host Mode

Host RPC can select one or more envd environments and pass them into run
preparation. The dynamic host-control contract is defined in
`../ops/06-json-rpc-host-protocol.md` as the Environment Attachment Manager.
This page only records how Starweaver uses envd after the host has selected an
attachment.

```mermaid
sequenceDiagram
    participant Client
    participant HostRPC as starweaver.host RPC
    participant Manager as EnvironmentAttachmentManager
    participant EnvdClient as EnvdRpcClient
    participant Provider as EnvdEnvironmentProvider
    participant Composite as CompositeEnvironmentProvider
    participant Runtime

    Client->>HostRPC: environment.attach or run.start(environmentAttachments)
    HostRPC->>Manager: resolve envd endpoint and lease
    Manager->>EnvdClient: initialize/open/state
    EnvdClient-->>Manager: descriptor and readiness
    Manager->>Provider: wrap EnvdRpcClient
    Manager->>Manager: build RunEnvironmentBinding
    Manager->>Composite: construct composite provider
    HostRPC->>Runtime: run AgentSession with one provider
    Runtime->>Composite: tool call
    Composite->>Provider: routed call
    Provider->>EnvdClient: envd service method
```

Host RPC remains the agent-control plane. Envd RPC is the environment
data/effect plane. The attachment manager owns literal endpoint validation,
liveness/readiness probes, lease scope, and run materialization. Named endpoint
aliases and host-launched envd daemons are future host capabilities. Envd owns
environment state and operation effects behind the selected service boundary.

## Run Environment Reference

Run params should reference envd without embedding envd file, process, or mount
DTOs in the host-control protocol.

```json
{
  "environmentAttachments": [{
    "id": "workspace",
    "kind": "envd",
    "endpointRef": "http://127.0.0.1:8766/rpc",
    "environmentId": "env_cli_default",
    "mode": "read_write"
  }]
}
```

In multi-environment runs, `id` is also the agent-facing mount identity. The SDK
composite provider exposes each attachment at `/environment/{id}` and chooses
one attachment as the default for unqualified relative paths. Exactly one
attachment should set `default: true`; if omitted for a single attachment, that
attachment is the default.

```json
{
  "environmentAttachments": [
    {
      "id": "workspace",
      "kind": "envd",
      "endpointRef": "http://127.0.0.1:8766/rpc",
      "environmentId": "env_cli_default",
      "mode": "read_write",
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

The same attachment can also be prepared through `environment.attach` and then
referenced from `run.start` by `attachmentLeaseId`. That lease id is a
Starweaver host-control handle and is not part of the envd protocol.

CLI direct mode can omit endpoint:

```json
{
  "environment": {
    "kind": "envd",
    "environmentId": "env_cli_default",
    "store": "ephemeral"
  }
}
```

## Session and Replay Metadata

Session/run records store environment refs:

```json
{
  "environment": {
    "kind": "envd",
    "environmentId": "env_123",
    "endpointRef": "http://127.0.0.1:8766/rpc",
    "startStateVersion": "sv_10",
    "endStateVersion": "sv_15",
    "operationIds": ["op_1", "op_2"]
  }
}
```

Session storage does not store full envd state.

## Approval and Policy Flow

Envd can deny, allow, or request approval. Host HITL handles user-facing
approval.

```mermaid
sequenceDiagram
    participant Tool
    participant Provider
    participant Envd
    participant Host
    participant Human

    Tool->>Provider: write or command
    Provider->>Envd: service call
    Envd-->>Provider: approval_required
    Provider-->>Host: approval metadata
    Host->>Human: approval request
    Human-->>Host: decision
    Host->>Envd: approval decision
    Envd-->>Provider: operation result
```

The first slice can map approval-required to deferred/approval records through
existing HITL paths after the adapter is in place.

## Dependency Boundary

Allowed:

```text
starweaver-agent -> starweaver-environment
starweaver-environment -> starweaver-envd-core
starweaver-rpc -> host service -> envd service/client
starweaver-cli -> host service -> envd service
```

Avoid:

```text
starweaver-runtime -> envd RPC DTOs
starweaver-rpc-core -> envd file/process DTOs
starweaver-storage -> full envd state schema
```

`starweaver-storage` can store refs. Envd owns environment state.
