# Envd Service Interface and State

Envd starts as a service interface that can be called directly in process or
through an RPC transport. The interface is the canonical envd boundary for
environment state and side effects.

## Service Trait

Target shape:

```rust
#[async_trait]
pub trait EnvdService: Send + Sync {
    async fn initialize(&self, request: InitializeEnvdRequest) -> EnvdResult<InitializeEnvdResult>;
    async fn open_environment(&self, request: OpenEnvironmentRequest) -> EnvdResult<EnvironmentDescriptor>;
    async fn environment_state(&self, request: EnvironmentRequest) -> EnvdResult<EnvironmentStateSnapshot>;
    async fn file_read(&self, request: FileReadRequest) -> EnvdResult<FileReadResult>;
    async fn file_write(&self, request: FileWriteRequest) -> EnvdResult<FileWriteResult>;
    async fn file_create_dir(&self, request: FileCreateDirRequest) -> EnvdResult<MutationResult>;
    async fn file_delete(&self, request: FileDeleteRequest) -> EnvdResult<MutationResult>;
    async fn file_move(&self, request: FileMoveRequest) -> EnvdResult<MutationResult>;
    async fn file_copy(&self, request: FileCopyRequest) -> EnvdResult<MutationResult>;
    async fn file_write_tmp(&self, request: FileWriteTmpRequest) -> EnvdResult<FileWriteTmpResult>;
    async fn file_list(&self, request: FileListRequest) -> EnvdResult<FileListResult>;
    async fn file_stat(&self, request: FileStatRequest) -> EnvdResult<FileStat>;
    async fn file_glob(&self, request: FileGlobRequest) -> EnvdResult<Vec<FileGlobMatch>>;
    async fn file_grep(&self, request: FileGrepRequest) -> EnvdResult<Vec<FileGrepMatch>>;
    async fn command_run(&self, request: CommandRunRequest) -> EnvdResult<CommandRunResult>;
    async fn process_start(&self, request: ProcessStartRequest) -> EnvdResult<ProcessSnapshot>;
    async fn process_wait(&self, request: ProcessWaitRequest) -> EnvdResult<ProcessSnapshot>;
    async fn process_list(&self, request: EnvironmentRequest) -> EnvdResult<ProcessListResult>;
    async fn process_input(&self, request: ProcessInputRequest) -> EnvdResult<ProcessSnapshot>;
    async fn process_signal(&self, request: ProcessSignalRequest) -> EnvdResult<ProcessSnapshot>;
    async fn process_kill(&self, request: ProcessKillRequest) -> EnvdResult<ProcessSnapshot>;
    async fn render_environment_context(&self, request: EnvironmentContextRequest) -> EnvdResult<EnvironmentContextResult>;
    async fn shell_review_context(&self, request: ShellReviewContextRequest) -> EnvdResult<ShellReviewContextResult>;
    async fn export_snapshot(&self, request: EnvironmentRequest) -> EnvdResult<EnvironmentStateSnapshot>;
}
```

The important decision is that direct CLI mode and RPC mode call this same
interface. New envd capabilities should be added here first, then exposed over
RPC and through adapters only when a concrete implementation or caller needs
them.

## Environment Identity

Envd manages one or more environments.

```json
{
  "environmentId": "env_123",
  "kind": "local",
  "store": "ephemeral",
  "status": "open",
  "stateVersion": "sv_0001",
  "policyRevision": "pol_1",
  "capabilities": {},
  "metadata": {}
}
```

`environmentId` is stable within the envd service. In CLI local ephemeral mode
there can be one implicit environment id such as `env_cli_default`.

## State Model

Envd maintains Environment state as service-owned data.

Minimum state:

```text
EnvironmentState
  environment_id
  kind
  status
  state_version
  mounts
  files or file_backend_ref
  resources
  processes
  shell_sessions
  operations
  effects
  policy_revision
  metadata
```

This state is richer than Starweaver's portable `EnvironmentState` exported by
`EnvironmentProvider`. A Starweaver provider snapshot can be derived from envd
state, but envd state is the owner when envd is selected.

## Mount State

Mounts define the path namespace used by file and execution methods.

```json
{
  "mountId": "workspace",
  "root": "/workspace",
  "mode": "read_write",
  "backend": {
    "kind": "memory"
  },
  "generation": 1,
  "status": "ready"
}
```

In CLI local ephemeral mode the default environment can have one implicit mount:

```text
mount_id = "workspace"
root = "/"
backend = in-memory file tree or local workspace backend
```

File and command/process methods must resolve paths through the same mount
state.

## File State

The first implementation can use the current local and virtual provider file
logic behind envd.

Backends:

| Backend                | Meaning                                                                      |
| ---------------------- | ---------------------------------------------------------------------------- |
| `memory`               | in-memory file tree for tests and CLI local ephemeral mode                   |
| `local`                | host filesystem rooted at workspace root                                     |
| `implementation_store` | implementation-owned state backend, such as a service database or blob store |
| `composite`            | route paths to child backends                                                |

File state should expose:

- text reads
- byte reads
- writes
- create/delete/move/copy
- stat/list
- glob/grep
- tmp/scratch writes

## Operation and Effect Records

Envd should record operations and effects even when `LocalEnvd` uses an
ephemeral memory store.

Operation:

```json
{
  "operationId": "op_123",
  "environmentId": "env_123",
  "actorId": "actor_cli",
  "baseVersion": "sv_0001",
  "newVersion": "sv_0002",
  "kind": "file.write",
  "summary": "write /workspace/README.md",
  "status": "committed"
}
```

Effect:

```json
{
  "effectId": "eff_123",
  "operationId": "op_123",
  "kind": "process.start",
  "idempotencyKey": "run_1:tool_2",
  "status": "completed",
  "resultRef": "process:proc_123"
}
```

Operation/effect records support replay, conflict debugging, and future
multi-agent coordination.

## Process State

Process state is service-owned.

```json
{
  "processId": "proc_123",
  "environmentId": "env_123",
  "mountId": "workspace",
  "command": "cargo test",
  "status": "running",
  "stdoutCursor": "out_10",
  "stderrCursor": "err_3",
  "returnCode": null,
  "metadata": {}
}
```

The `EnvironmentProvider` adapter can translate this into
`ShellProcessSnapshot` for current tools.

## Shell Session State

Durable shell sessions are not required in the first slice, but the state model
must leave room for them.

```json
{
  "shellSessionId": "sh_123",
  "environmentId": "env_123",
  "mountId": "workspace",
  "status": "attached",
  "transcriptCursor": "term_55",
  "rows": 40,
  "cols": 120
}
```

Shell session support should be a capability, not part of the minimum service.

## Capabilities

Every environment descriptor includes capabilities.

```json
{
  "files": ["read", "write", "list", "stat", "glob", "grep"],
  "command": ["run"],
  "process": ["start", "wait", "input", "signal", "kill"],
  "shellSession": [],
  "resources": [],
  "policy": ["static"]
}
```

Tools and host diagnostics should depend on capabilities, not concrete service
implementations.

## Error Model

Envd errors should be structured.

```json
{
  "kind": "permission_denied",
  "message": "write denied by file policy",
  "environmentId": "env_123",
  "retryable": false,
  "metadata": {}
}
```

Common kinds:

- `not_found`
- `invalid_request`
- `permission_denied`
- `state_conflict`
- `policy_conflict`
- `execution_failed`
- `execution_unavailable`
- `payload_too_large`
- `internal`

`EnvironmentProvider` maps these into `EnvironmentError` while preserving useful
metadata where possible.
