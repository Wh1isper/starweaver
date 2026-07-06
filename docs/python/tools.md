# Python Tools

Python tools are native Starweaver runtime tools. They are called in-process
through PyO3 and participate in the same Rust tool scheduling, retry, timeout,
approval, deferred-result, stream, and trace flow as Rust tools.

## Function Tools

Use `@tool` for Python callables. Parameters are converted into JSON Schema for
the model-facing tool definition.

```python
import asyncio

from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def add(left: int, right: int) -> dict[str, int]:
    await asyncio.sleep(0)
    return {"total": left + right}


async def main() -> None:
    model = TestModel.responses(
        [
            TestModel.tool_call_response(
                [{"id": "call_add", "name": "add", "arguments": {"left": 2, "right": 3}}]
            ),
            {"text": "done"},
        ]
    )
    result = await create_agent(model=model, tools=[add]).run("Add two numbers")
    assert result.output == "done"
```

Raw Python callables can also be passed to `create_agent(tools=[...])`; they
are wrapped through the same schema inference path as `@tool`.

## Argument Styles

Use normal typed keyword parameters for simple tools:

```python
@tool
async def lookup(ticket_id: str, include_history: bool = False) -> dict[str, object]:
    return {"ticket_id": ticket_id, "include_history": include_history}
```

Use a single Pydantic model argument when validation should live in one object:

```python
from pydantic import BaseModel

from starweaver import tool


class TicketArgs(BaseModel):
    ticket_id: str
    include_history: bool = False


@tool
async def fetch_ticket(args: TicketArgs) -> dict[str, object]:
    return {"ticket_id": args.ticket_id, "include_history": args.include_history}
```

Use `args: dict[str, object]` or an explicit `parameters_schema` when the shape
is dynamic:

```python
@tool(parameters_schema={"type": "object", "additionalProperties": True})
async def inspect_payload(args: dict[str, object]) -> dict[str, object]:
    return {"keys": sorted(args)}
```

Explicit parameter schemas must be JSON-compatible object schemas. Starweaver
rejects schemas with non-object top-level types, malformed `properties`,
malformed `required` lists, invalid `additionalProperties` values, or values
that cannot be serialized to JSON.

## Tool Context

Add a first `ToolContext` parameter when the tool needs runtime state,
dependencies, approval context, or cancellation checks:

```python
from starweaver import ToolContext, tool


@tool(parameters_schema={"type": "object", "properties": {}})
async def deploy(ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
    return {"ok": not ctx.is_cancelled()}
```

`ToolContext` must precede business arguments. A business parameter named
`context` is allowed when it is not typed as `ToolContext`. Use
`ctx.is_cancelled()` for a synchronous check and `await ctx.cancelled()` to wait
until Starweaver requests cooperative cancellation.

## Class Tools

Use `BaseTool` when a class is more natural than a function:

```python
from starweaver import BaseTool, ToolContext


class DeployTool(BaseTool):
    name = "deploy"
    description = "Deploy the current release."

    def __init__(self) -> None:
        super().__init__(
            parameters_schema={"type": "object", "properties": {}, "additionalProperties": False}
        )

    async def call(self, ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        return {"ok": not ctx.is_cancelled()}
```

## Parallel Execution

Independent tool calls run in parallel by default. Set `sequential=True` when a
tool must run in model-returned order:

```python
@tool(sequential=True)
async def append_audit_line(line: str) -> dict[str, bool]:
    return {"written": True}
```

The runtime automatically falls back to sequential execution when the same tool
name appears more than once in one model response. That keeps duplicate writes
predictable without making unrelated tools slower.

Set `strict=True` when the provider should receive a strict tool definition. In
strict mode, Starweaver requires every Python tool to have an explicit
`description` or a function docstring at registration time.

## Structured Results

Return `ToolResult` when a tool needs separate application, model, user, and
debug surfaces:

```python
from starweaver import ToolResult, tool


@tool(parameters_schema={"type": "object", "properties": {}})
async def inspect_release(args: dict[str, object]) -> ToolResult:
    return ToolResult(
        {"raw": "application record"},
        metadata={"audit": "release"},
        app_value={"release_id": 42},
        model_content={"summary": "release is ready"},
        user_content={"markdown": "**Ready**"},
        private_metadata={"trace_id": "host-only"},
    )
```

`model_content` becomes the tool-return content used for the next model
request. `metadata` is public tool-return metadata. `app_value` and
`user_content` remain available to application and UI code. `private_metadata`
is preserved in canonical stream/session evidence for debugging, but it must not
be treated as model-facing content. Python exception tracebacks are captured in
private metadata and are kept out of public tool-return `content` and
`metadata`.

## Control Flow

Tool exceptions map onto runtime tool control flow:

- `InvalidArguments` asks the model to retry with corrected arguments.
- `ModelRetry` asks the model to retry the tool call with the provided message.
- `ApprovalRequired` suspends the run until approval is supplied.
- `CallDeferred` suspends the run until an external deferred result is supplied.
- `Cancelled` and `Timeout` map to canonical runtime cancellation and timeout
  errors.

Approval resume can use raw canonical IDs or typed helper objects:

```python
from starweaver import ApprovalRequired, ToolContext, create_agent, tool


@tool(parameters_schema={"type": "object", "properties": {}})
async def deploy(ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
    if ctx.approval is None:
        raise ApprovalRequired("deploy production", metadata={"risk": "high"})
    return {"approved": True}


async def run_with_approval(model) -> None:
    async with create_agent(model=model, tools=[deploy]) as agent:
        async with agent.session() as session:
            waiting = await session.run("deploy")
            assert waiting.status == "waiting"
            decision = waiting.hitl.approvals[0].approve(decided_by="ui")
            result = await session.hitl.resume(approvals=[decision])
    assert result.output
```

Deferred helpers mirror approvals: use `waiting.hitl.deferred[0].complete(...)`
or pass canonical deferred-result JSON to
`resume_after_hitl(deferred_results=...)`.

When approval policy is profile-level rather than tool-implementation-level,
pass `approval_required_tools=[...]` to `create_agent(...)` or
`create_agent_runtime(...)`. Entries match native Starweaver approval policy:
tool name, toolset name/id, metadata bundle, or `*`.

## Metadata, Retry, And Timeout

Tool definitions can carry model-facing metadata and runtime policy:

```python
@tool(
    metadata={"risk": "network"},
    timeout_ms=5_000,
    max_retries=1,
)
async def fetch_status(url: str) -> dict[str, str]:
    return {"status": "ok", "url": url}
```

Per-tool retry and timeout are enforced by the Rust runtime. Python callbacks
should still cooperate with cancellation by awaiting normally and not blocking
the event loop.
