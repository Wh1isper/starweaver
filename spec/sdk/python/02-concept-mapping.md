# Public Python API Contract

This spec defines the Python-facing concepts and maps them to Starweaver Rust
contracts. The goal is a Python API that feels natural without creating a
second runtime model.

## API Layers

The Python package should expose three layers:

1. Declarative definitions: tools, output policies, capability bundles,
   subagents, models, and run options.
2. Runtime handles: `Agent`, `AgentSession`, and `AgentRun`.
3. Evidence values: results, stream events, pending HITL items, state snapshots,
   usage snapshots, and raw records.

The public API should make the common path concise while keeping raw evidence
available for applications that need forward compatibility.

## Preferred User Shape

```python
from pydantic import BaseModel

from starweaver import ToolContext, ToolResult, create_agent, tool


class LookupArgs(BaseModel):
    query: str


@tool
async def lookup(ctx: ToolContext, args: LookupArgs) -> ToolResult:
    return ToolResult({"value": args.query, "run_id": ctx.run_id})


async def main() -> None:
    async with create_agent(model=model, tools=[lookup]) as agent:
        async with agent.session() as session:
            result = await session.run("Say ready")
            assert result.output
```

Advanced control should stay explicit:

```python
async with agent.session() as session:
    async with session.run_stream("Investigate") as run:
        await run.steer("Prioritize implementation evidence.")
        async for event in run:
            ...
```

## Concept Map

| Python concept      | Rust owner                                      | Status              | Contract                                                                                                                        |
| ------------------- | ----------------------------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `create_agent(...)` | `starweaver-agent::AgentBuilder`                | current             | Build a reusable agent facade from model, instructions, tools, toolsets, output policy, subagents, bundles, and runtime config. |
| `RuntimeConfig`     | runtime/context config seams                    | current             | Runtime and context knobs stay separate from provider `ModelSettings`.                                                          |
| `Agent`             | `starweaver-runtime::Agent` plus SDK facade     | current             | Reusable configuration and resource owner.                                                                                      |
| `Agent.session()`   | `starweaver-agent::AgentSession`                | current             | Pythonic alias for new or restored sessions.                                                                                    |
| `AgentSession`      | `AgentSession`                                  | current             | Stateful conversation, export/restore, HITL resume, session streams, and active session control.                                |
| `AgentRun`          | `AgentStreamHandle` plus control seam           | current             | Live run handle for events, result, interrupt, steer, messages, HITL, and recovery.                                             |
| `AgentStream`       | `AgentRun` compatibility alias                  | current             | Compatibility name for the live run handle.                                                                                     |
| `RunOptions`        | `AgentRunOptions`                               | current plus target | Per-run instructions, tools, toolsets, model settings, request params, output policy, trace metadata, and approval policy.      |
| `@tool`             | `starweaver-tools::Tool`                        | current             | Python callable adapter registered as a native tool.                                                                            |
| `BaseTool`          | `Tool`                                          | current             | Subclass-friendly tool definition.                                                                                              |
| `ToolContext`       | `ToolContext`                                   | current             | Run ids, retry, metadata, approval, deferred result, cancellation.                                                              |
| `ToolResult`        | `ToolResult`                                    | current             | Tool content, app value, user/model content, metadata, private metadata.                                                        |
| tool exceptions     | `ToolError`                                     | current             | Python control exceptions map into native tool control flow.                                                                    |
| `Toolset`           | `starweaver-tools` and SDK capability contracts | current             | Group static tools, instructions, metadata, and per-run composition without changing the tool loop.                             |
| `ToolLibrary`       | Starweaver tool metadata and session state      | current             | Index tools/namespaces and expose search/proxy facades while persisting IDs, not Python objects.                                |
| output policies     | `OutputPolicy`                                  | current             | Structured output, validators, output functions, retry budget.                                                                  |
| stream events       | `AgentStreamRecord`                             | current             | Python `StreamEvent.kind`, lazy accessors, and raw JSON.                                                                        |
| stream adapters     | `starweaver-stream`                             | current             | Projection helpers over canonical stream records; live control remains on `AgentRun`.                                           |
| HITL result helpers | `AgentResult` helpers                           | current             | Pending approval/deferred records have typed helper objects plus raw dicts.                                                     |
| message bus facade  | `AgentContext.messages`                         | current             | MQ-like send, peek, consume, steering, and idempotency.                                                                         |
| active control      | neutral Rust SDK seam                           | current             | Live steering, message writes, interruption, recovery.                                                                          |
| `SessionArchive`    | `ResumableState`                                | current             | Full-state JSON/file persistence helper.                                                                                        |
| `SessionStore`      | `starweaver-session` and `starweaver-storage`   | partial             | Python record/store facades preserve canonical JSON; native SQLite and Rust trait callback bridges remain future work.          |
| `CapabilityBundle`  | SDK capability bundle                           | current             | Static composition of instructions, tools, model/request overlays, output callbacks.                                            |
| `Subagent`          | `SubagentSpec` and registry                     | current             | Register child agents through Starweaver delegation tools.                                                                      |
| `SkillRegistry`     | Starweaver skill specs and bundles              | current             | List/load/inspect skills and attach skill toolsets or bundles through native skill parsing.                                     |
| provider models     | `starweaver-model` adapters                     | current             | Provider helper constructors backed by Rust transports/profiles plus typed OAuth/routing helpers.                               |
| environments        | `EnvironmentProvider`                           | partial             | Python wrappers over Rust-owned local and virtual providers; envd and Python-defined providers remain future work.              |
| resources           | environment resource refs                       | current             | Resource refs, registries, and environment-owned resource lifecycle.                                                            |
| media               | media filters and resource refs                 | current             | Upload/config adapters without embedding large binary state.                                                                    |
| observability       | trace context and usage                         | partial             | Expose ids now; usage/trace helpers should become typed.                                                                        |

## Agent

`Agent` is reusable configuration. It should not become a hidden global
conversation object.

Current shape:

```python
async with create_agent(model=model, tools=[lookup]) as agent:
    result = await agent.run("Say ready")
    stream = agent.run_stream("Stream")
```

Target additions:

```python
class Agent:
    def session(self, state: dict[str, object] | None = None) -> AgentSession: ...
    def new_session(self) -> AgentSession: ...
    def session_from_state(self, state: dict[str, object]) -> AgentSession: ...

    async def run(self, prompt: str, **options) -> RunResult: ...
    def run_stream(self, prompt: str, **options) -> AgentRun: ...

    async def steer(self, text: str, **options) -> ControlReceipt: ...
```

Rules:

- `session()` should be the preferred Python name.
- `new_session()` and `session_from_state()` remain explicit aliases.
- `run()` and `run_stream()` may create ephemeral sessions for one-off use.
- `agent.steer(...)` is optional convenience and must reject zero or multiple
  direct active runs.

## AgentSession

`AgentSession` is the primary stateful conversation object.

Target surface:

```python
class AgentSession:
    async def __aenter__(self) -> AgentSession: ...
    async def __aexit__(self, exc_type, exc, tb) -> None: ...

    async def run(self, prompt: str, **options) -> RunResult: ...
    def run_stream(self, prompt: str, **options) -> AgentRun: ...

    async def steer(self, text: str, **options) -> ControlReceipt: ...
    def interrupt(self, reason: str | None = None) -> None: ...

    @property
    def messages(self) -> MessageBus: ...
    @property
    def hitl(self) -> SessionHitl: ...

    def export_state(self, mode: str = "curated") -> dict[str, object]: ...
    async def resume_after_hitl(
        self,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult: ...
```

Rules:

- One active run per session remains the default.
- Concurrent conversations should use multiple sessions.
- Idle message-bus writes mutate stored session state.
- Active message-bus writes must go through the active control handle.
- State export must not serialize Python callable objects or process-local
  dependencies.

## AgentRun

`AgentRun` is the public live handle. It should own the behavior currently split
between `AgentStream`, raw result dicts, and future control APIs.

Target surface:

```python
class AgentRun:
    async def __aenter__(self) -> AgentRun: ...
    async def __aexit__(self, exc_type, exc, tb) -> None: ...

    def __aiter__(self) -> AsyncIterator[StreamEvent]: ...
    async def recv(self) -> StreamEvent | None: ...
    async def join(self) -> StreamRunResult: ...
    async def result(self) -> RunResult: ...

    async def steer(self, text: str, **options) -> ControlReceipt: ...
    async def send_message(self, message: BusMessage | dict[str, object]) -> ControlReceipt: ...
    def interrupt(self, reason: str | None = None) -> None: ...

    def status(self) -> RunStatusSnapshot: ...
    async def recoverable_state(self) -> dict[str, object]: ...

    @property
    def messages(self) -> MessageBus: ...
    def hitl(self) -> RunHitl: ...
```

Context-manager rules:

- Normal exit waits for completion.
- Exceptional exit interrupts, joins or completes, and preserves recoverable
  state.
- Python task cancellation while awaiting stream methods interrupts the Rust
  run and re-raises `asyncio.CancelledError`.
- Fire-and-observe behavior must use an explicit detach/receiver-close API.

## Run Options

`run()` and `run_stream()` should accept the same option family at the agent and
session level:

- `instructions`
- `tools`
- `replace_tools`
- `model_settings`
- `request_params`
- `output_schema`
- `output_policy`
- future `toolsets`
- future `runtime_config`
- future `trace_metadata`
- future `approval_policy`

Unknown run options should raise `TypeError` instead of being ignored.

## Results And Evidence

`RunResult` should expose:

- `output`
- `structured_output`
- `messages`
- `raw_state`
- `status`
- `is_waiting`
- `needs_approval`
- `pending_approvals`
- `pending_deferred`
- future `approvals`
- future `deferred`
- future `usage`
- future `trace`

Raw fields stay available. Typed helpers should sit on top of them.

`StreamEvent` should expose:

- `kind`
- `raw`
- future `run_id`
- future `step`
- future `text_delta`
- future `tool_call`
- future `tool_return`
- future `usage`
- future `approval`
- future `deferred`
- future `sideband`
- future `is_terminal`

Do not fork the stream protocol. Unknown records must remain accessible through
`raw`.

## Error Model

Public Python exceptions should mirror Starweaver control flow:

- `StarweaverError`
- `AgentError`
- `ModelError`
- `ToolError`
- `OutputError`
- `InvalidArguments`
- `ModelRetry`
- `OutputRetry`
- `OutputValidationFailed`
- `ApprovalRequired`
- `CallDeferred`
- `Cancelled`
- `Timeout`
- `StateError`
- `StreamError`

Control methods should return receipts when accepted or raise typed exceptions.
They should not return `False` for important failures.

## Mapping Principles

- Python convenience is a facade over Rust-owned contracts.
- Python decorators produce explicit handles, not hidden global runtime state.
- Pydantic helps build schemas, but Starweaver owns runtime validation.
- Python exceptions map into public Starweaver control flow.
- Raw evidence remains available for compatibility.
- Sessions should be explicit; hidden global conversation state should be
  avoided.
- Live control must target active runs, not exported snapshots.
- Message bus APIs should read like Python while preserving MQ semantics.
- Durable store APIs persist full Starweaver state and native records, not
  process-local Python objects.
- Tool search/proxy state persists serializable IDs and namespaces.
- Environment and resource facades configure Rust-owned providers; they do not
  bypass provider enforcement.
