# Python Tools And Callback Runtime

Python tool injection is the central binding feature. A Python callable or
class must become a real Starweaver `Tool` implementation in the same process.

The runtime should see Python tools exactly like Rust tools: same schema,
scheduling, retries, approval, deferred execution, cancellation, stream events,
private metadata, and durable evidence.

## Current Baseline

The current package provides:

- `@tool` decorator
- `Tool` wrapper
- subclassable `BaseTool`
- raw callable registration through `ensure_tool`
- Pydantic model schema extraction
- simple type-hint schema extraction
- explicit JSON schema override
- `ToolContext`
- `ToolResult`
- sync and async callback support
- `sequential=True`
- timeout and retry metadata
- Python exception to `ToolError` mapping
- traceback capture into private metadata
- cancellation of the Python future when the Rust call is dropped

This is already enough to prove the in-process tool architecture. The next work
is polish, typed helper objects, more negative tests, and better integration
with live run control.

## Adapter Contract

`PythonTool` should implement `starweaver_tools::Tool` and store:

- stable name
- optional description
- parameters JSON schema
- optional return JSON schema
- metadata
- strict schema flag
- sequential scheduling flag
- timeout metadata
- retry metadata
- Python callable handle
- Python event loop handle
- callback dispatcher state

It should not store hidden application state that claims to be resumable. If a
tool depends on process-local Python objects, the application must re-register
the tool before restored sessions can use it.

## Registration Flow

```mermaid
sequenceDiagram
    participant App as Python app
    participant Decorator as tool decorator
    participant Native as starweaver._native
    participant Builder as AgentBuilder
    participant Runtime as Starweaver runtime
    participant Loop as Python event loop

    App->>Decorator: define callable
    Decorator->>Native: create PythonTool
    Native->>Builder: register DynTool
    Runtime->>Native: Tool::call(ctx, args)
    Native->>Loop: schedule callable
    Loop-->>Native: value or exception
    Native-->>Runtime: ToolResult or ToolError
```

## Supported Definition Styles

Preferred:

```python
from pydantic import BaseModel
from starweaver import ToolContext, ToolResult, tool


class SearchArgs(BaseModel):
    query: str


@tool
async def search(ctx: ToolContext, args: SearchArgs) -> ToolResult:
    return ToolResult({"query": args.query})
```

Also supported:

```python
@tool
def add(left: int, right: int) -> dict[str, int]:
    return {"total": left + right}
```

Subclass form:

```python
class DeployTool(BaseTool):
    name = "deploy"

    async def call(self, ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        return {"ok": not ctx.is_cancelled()}
```

## Schema Extraction

Schema extraction order:

1. Explicit `parameters_schema`.
2. Pydantic `BaseModel` argument.
3. Simple Python type hints for keyword parameters.
4. Rejection with a clear error.

Supported shapes:

- `(ctx: ToolContext, args: PydanticModel)`
- `(args: PydanticModel)`
- `(ctx: ToolContext, args: dict[str, object])`
- `(args: dict[str, object])`
- typed keyword parameters such as `(query: str, limit: int = 10)`
- `**kwargs` only when an explicit schema is supplied
- `BaseTool.call(ctx, args)`

`ToolContext` injection is explicit: a parameter annotated as `ToolContext` is
the context parameter, and an unannotated parameter named `ctx` is accepted as a
short convenience. A business parameter named `context` is not special unless
it is annotated as `ToolContext`.

Rejected shapes:

- untyped `*args`
- untyped `**kwargs` without explicit schema
- arbitrary classes without Pydantic schema
- docstring-only schema inference
- ambiguous mixtures of Pydantic model and unrelated positional fields

The P0 behavior should prefer explicit failure over lossy inference.

## Return Conversion

Return conversion order:

1. `ToolResult`
2. Pydantic model
3. JSON-serializable value
4. `ToolError::Execution` for non-serializable values

`ToolResult` maps to native Starweaver result fields:

- `content`
- `metadata`
- `app_value`
- `model_content`
- `user_content`
- `private_metadata`

Model-visible content and application/debug content must remain separate.
Tracebacks and local exception details belong in private metadata.

## Exception Mapping

| Python exception          | Rust `ToolError`   |
| ------------------------- | ------------------ |
| `InvalidArguments`        | `InvalidArguments` |
| Pydantic validation error | `InvalidArguments` |
| `ModelRetry`              | `ModelRetry`       |
| `ApprovalRequired`        | `ApprovalRequired` |
| `CallDeferred`            | `CallDeferred`     |
| `asyncio.CancelledError`  | `Cancelled`        |
| `Cancelled`               | `Cancelled`        |
| `TimeoutError`            | `Timeout`          |
| `Timeout`                 | `Timeout`          |
| other `Exception`         | `Execution`        |

The mapping must work in both directions:

- Python API boundary: Rust errors become Python exceptions.
- Tool-loop boundary: Python exceptions become Starweaver `ToolError` values.

User-defined exceptions with the same class name as Starweaver exceptions must
not be misclassified. Use module/class identity, not name-only matching.

## HITL And Deferred Flow

Python tools should request control flow through public exceptions:

```python
from starweaver import ApprovalRequired, CallDeferred


async def deploy(ctx, args):
    raise ApprovalRequired("production deploy", metadata={"service": args["service"]})


async def slow_job(ctx, args):
    raise CallDeferred("queued", metadata={"queue": "deploy"})
```

Rust remains the source of truth for pending approval and deferred records.
Python should not keep a parallel pending store.

Typed Python HITL helpers should build decisions and deferred results, then pass
those values through `AgentSession.resume_after_hitl(...)`.

## Async Runtime And GIL Strategy

Required constraints:

- Python callers use `await`, `async for`, and `async with`.
- Rust runtime/model/network work must not hold the GIL.
- Python callbacks run on the Python event loop that registered them.
- Starweaver cancellation cancels the corresponding Python task.
- Callback tracebacks are captured for debugging without leaking to model
  content.

Implementation rules:

1. The native extension owns or attaches to a Tokio runtime.
2. Python APIs return awaitables backed by Rust futures.
3. Each Python callback captures the event loop used at registration.
4. Native code schedules the coroutine with `asyncio.run_coroutine_threadsafe`
   or the selected async bridge.
5. GIL sections are limited to conversion, scheduling, and result extraction.
6. Dropping or cancelling the Rust-side future cancels the Python future.

`pyo3-async-runtimes` may be used where it fits, but keep an explicit
Starweaver callback dispatcher abstraction because cancellation, traceback
capture, tool metadata, and event-loop ownership are product-specific.

## Cancellation

Python tool cancellation must be observable:

- `ctx.is_cancelled()`
- `await ctx.cancelled()`
- cancellation of the Python `asyncio.Task`
- `asyncio.CancelledError` mapped to native cancellation
- recoverable state repaired for dangling tool calls after interruption

The stream interruption tests should cover a Python tool blocked on an
`asyncio.Event` and prove that `stream.interrupt()` cancels the coroutine.

## Concurrency Policy

Default policy:

- Independent tool calls run in parallel.
- Python tools default to `sequential=False`.
- Stateful or non-reentrant Python tools opt into `sequential=True`.
- Duplicate calls to the same tool name in one model response fall back to
  sequential execution.
- Mixed Rust and Python tool scheduling uses the same runtime scheduler.

This keeps Python behavior aligned with the Starweaver tool contract instead of
creating a Python-only scheduler.

## Tool Context

Current `ToolContext` exposes:

- `run_id`
- `conversation_id`
- `run_step`
- `retry`
- `max_retries`
- `metadata`
- `approval`
- `deferred_result`
- `is_cancelled()`
- `await cancelled()`

Future additions should be controlled facades:

- dependencies
- message bus access
- resource handles
- environment handle

Do not expose a mutable raw `AgentContext` to tools unless the mutation path is
explicitly part of a stable Starweaver contract.

## Validation Matrix

Tests should cover:

- async Python tool success
- sync Python tool success
- raw callable registration
- `BaseTool` subclass registration
- Pydantic argument validation
- explicit JSON schema registration
- invalid schema rejection
- non-serializable return handling
- `ToolResult` conversion
- private metadata preservation
- ordinary Python exception to `ToolError::Execution`
- Starweaver control-flow exception identity checks
- `ModelRetry`
- `ApprovalRequired`
- `CallDeferred`
- timeout
- cancellation
- parallel default scheduling
- duplicate-name sequential fallback
- explicit `sequential=True`
- traceback capture in private metadata

Current Python package tests include end-to-end coverage for `ToolResult`
layering from Python callbacks into native tool returns: `model_content` becomes
the tool-return content, `app_value` and `user_content` are preserved as
separate evidence, and `private_metadata` is retained without entering public
tool-return `content` or `metadata`. Ordinary Python exceptions are also covered:
tracebacks are captured in `private_metadata` while model-facing error content
receives only the canonical tool error payload. Explicit parameter schemas are
covered in both directions: valid object schemas reach the provider-neutral tool
definition unchanged, while malformed explicit schemas fail at registration
before they can enter the native runtime. Python tool timeout is covered by an
end-to-end test that verifies the canonical timeout tool-return metadata and the
corresponding Python coroutine cancellation. Pydantic argument validation is
covered for both successful model construction and validation failure:
`ValidationError` becomes a canonical `invalid_arguments` tool return with
private Python traceback evidence. Public `InvalidArguments`, `Cancelled`, and
`Timeout` exceptions plus standard `asyncio.CancelledError` and `TimeoutError`
are covered with canonical tool-return metadata.
Cancellation coverage proves Python task cancellation, `ToolContext.is_cancelled()`
visibility, and `await ToolContext.cancelled()` visibility from the cancelled
callback.
Control-flow exception identity is covered for `ModelRetry`, `ApprovalRequired`,
and `CallDeferred`: user-defined exceptions with those class names remain
ordinary execution errors unless they are instances of the public
`starweaver.errors` classes.

Validation commands:

```bash
uv run pytest packages/starweaver-py/tests
make py-check
```
