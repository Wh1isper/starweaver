# Starweaver Python

`starweaver` is the in-process Python SDK and binding package for Starweaver.
It keeps the Rust runtime as the canonical agent loop while exposing Python
ergonomics for agents, tools, sessions, streaming records, HITL control flow,
and deterministic tests.

Python tools are injected directly as native Starweaver runtime tools. They do
not use MCP, stdio, or another binary protocol.

Supported Python versions are CPython 3.11 through 3.13. Local development and
single-version CI jobs default to Python 3.13.

## Quickstart

```python
import asyncio

from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def add(left: int, right: int) -> dict[str, int]:
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
    agent = create_agent(model=model, tools=[add])
    result = await agent.run("Add two numbers")
    print(result.output)


asyncio.run(main())
```

## Tool Forms

Tools can be registered with `@tool`, as raw callables, or as `BaseTool`
subclasses. Typed function parameters produce JSON Schema automatically.
Pydantic models are supported for argument validation.

```python
from pydantic import BaseModel
from starweaver import BaseTool, ToolContext, tool


class SearchArgs(BaseModel):
    query: str


@tool
async def search(args: SearchArgs) -> dict[str, str]:
    return {"result": args.query}


class DeployTool(BaseTool):
    name = "deploy"

    def __init__(self) -> None:
        super().__init__(
            parameters_schema={"type": "object", "properties": {}, "additionalProperties": False}
        )

    async def call(self, ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        return {"ok": not ctx.is_cancelled()}
```

The runtime executes independent tool calls in parallel by default. A tool can
set `sequential=True`, and repeated calls to the same tool name in one model
response automatically run in model order.

## Streams And HITL

`run_stream()` returns a live `AgentStream` handle. It can be used as an async
iterator, or through `recv()`, `join()`, `result()`, `interrupt()`, and
`recoverable_state()`.

Run results expose `status`, `is_waiting`, `pending_approvals`, and
`pending_deferred`. Approval entries include `approval_id`; deferred entries
include `deferred_id`, and those IDs can be passed back to
`resume_after_hitl()`.

## Per-Run Options

`run()` and `run_stream()` support `instructions`, per-run `tools`,
`replace_tools`, `model_settings`, `request_params`, `output_schema`, and
`output_policy`. Unknown run options are rejected instead of ignored.

`ProviderModel` exposes provider-backed models for OpenAI Responses, OpenAI Chat,
Anthropic Messages, Gemini, and Codex OAuth through `from_model_id()` prefixes.
`OutputPolicy` packages structured output modes, retry budgets, validators, and
final-output functions. `CapabilityBundle` packages instructions, Python tools,
model settings, request parameters, output validators, and output functions.
`Subagent` registers another Python-created agent behind Starweaver's native
delegation tools.

## Validation

From the repository root:

```bash
make py-sync
make py-lint
make py-rust-check
make py-test
make py-check
```
