# Python SDK

The `starweaver` Python package is an in-process SDK facade over the Starweaver
Rust runtime. Python owns ergonomics and callback adaptation; Rust remains the
source of truth for model requests, tool scheduling, retries, HITL state,
sessions, stream records, usage, and trace boundaries.

Use the Python SDK when application code is Python but you still want the same
runtime contract as the Rust SDK. Python tools are native Starweaver tools:
they are called in-process through PyO3 and do not use MCP, stdio, or a sidecar
binary protocol.

## Install From A Checkout

Local development uses `uv` from the repository root. The repository default
interpreter is Python 3.13 through `.python-version`; supported package targets
are CPython 3.11 through 3.13.

```bash
make py-sync
make py-test
```

## Create An Agent

`create_agent()` builds a reusable agent facade. `TestModel` is deterministic
and does not call an external provider.

```python
import asyncio

from starweaver import create_agent
from starweaver.testing import TestModel


async def main() -> None:
    agent = create_agent(model=TestModel.text("ready"))
    result = await agent.run("Say ready")
    assert result.output == "ready"


asyncio.run(main())
```

## Add Python Tools

Use `@tool` for function tools. Typed parameters are converted into JSON Schema
for the model-facing tool definition. Tool bodies can be sync or async.

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


asyncio.run(main())
```

Independent tool calls run in parallel by default. Set `sequential=True` when a
tool must run in model-returned order. The runtime also automatically falls
back to sequential execution when the same tool name appears more than once in
one model response.

## Class Tools And Pydantic Args

Use `BaseTool` for subclass-style tools. Pydantic models can be used as the
single argument object and are validated before the user function runs.

```python
from pydantic import BaseModel

from starweaver import BaseTool, ToolContext, tool


class TicketArgs(BaseModel):
    id: str


@tool
async def fetch_ticket(args: TicketArgs) -> dict[str, str]:
    return {"id": args.id, "priority": "high"}


class DeployTool(BaseTool):
    name = "deploy"

    def __init__(self) -> None:
        super().__init__(
            parameters_schema={"type": "object", "properties": {}, "additionalProperties": False}
        )

    async def call(self, ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        return {"ok": not ctx.is_cancelled()}
```

Raw Python callables can also be passed to `create_agent(tools=[...])`; they are
wrapped with the same schema inference path as `@tool`.

## Tool Control Flow

Python tool exceptions map onto runtime tool control flow:

- `InvalidArguments` asks the model to retry with corrected arguments.
- `ModelRetry` asks the model to retry the tool call with the provided message.
- `ApprovalRequired` suspends the run until approval is supplied.
- `CallDeferred` suspends the run until an external deferred result is supplied.
- `Cancelled` and `Timeout` map to canonical runtime cancellation and timeout errors.

Approval resume uses canonical pending IDs from the run result:

```python
from starweaver import ApprovalRequired, ToolContext, create_agent, tool


@tool(parameters_schema={"type": "object", "properties": {}})
async def deploy(ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
    if ctx.approval is None:
        raise ApprovalRequired("deploy production", metadata={"risk": "high"})
    return {"approved": True}


async def run_with_approval(model) -> None:
    session = create_agent(model=model, tools=[deploy]).new_session()
    waiting = await session.run("deploy")
    assert waiting.status == "waiting"
    approval_id = str(waiting.pending_approvals[0]["approval_id"])
    result = await session.resume_after_hitl(approvals={approval_id: {"approved": True}})
    assert result.output
```

Deferred resume passes `DeferredToolResults` JSON through
`resume_after_hitl(deferred_results=...)`. Use
`waiting.pending_deferred[0]["deferred_id"]` as the stable request id.

## Sessions

Use an `AgentSession` when an application needs reusable context, message
history, pending HITL state, or resumable state export.

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def run_session() -> None:
    agent = create_agent(model=TestModel.responses([{"text": "first"}, {"text": "second"}]))
    session = agent.new_session()
    first = await session.run("first")
    state = session.export_state()
    restored = agent.session_from_state(state)
    second = await restored.run("second")
    assert (first.output, second.output) == ("first", "second")
```

## Streams

`run_stream()` returns a live `AgentStream` handle over canonical `StreamEvent`
records. The handle is also an async iterator, so `async for` works directly.
Use `recv()` for one record, `join()` for the stream result plus events,
`result()` for the final run result, `interrupt()` for cooperative cancellation,
and `recoverable_state()` to export the latest observed session state after an
interruption.

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def stream() -> None:
    agent = create_agent(model=TestModel.text("streamed"))
    async for event in agent.run_stream("stream"):
        print(event.kind, event.raw)
```

Cancelling a Python task waiting on `recv()`, `join()`, or `result()` interrupts
the underlying Starweaver run. If the run is inside a Python async tool, the
tool coroutine is cancelled on the application event loop.

## Per-Run Options

`run()` and `run_stream()` support the same option family used at agent
construction:

- `instructions=[...]` appends instructions for this run.
- `tools=[...]` injects extra tools for this run only.
- `replace_tools=True` uses the per-run tools as the complete tool registry for
  this run.
- `model_settings=ModelSettings(...)` overrides provider-neutral generation
  settings such as `temperature`, `max_tokens`, `timeout_ms`,
  `provider_settings`, `extra_headers`, and `extra_body`.
- `request_params=RequestParams(...)` forwards provider-neutral request
  parameters such as native tools, request metadata, HTTP overrides, and extra
  body fields.
- `output_schema=OutputSchema(...)` enables structured output parsing.
- `output_policy=OutputPolicy...` selects structured output mode, retry budget,
  output validators/functions, and text/image output allowances.

Unknown run options are rejected instead of silently ignored.

```python
from pydantic import BaseModel

from starweaver import ModelSettings, OutputPolicy, OutputSchema, create_agent
from starweaver.testing import TestModel


class Answer(BaseModel):
    ok: bool


async def run_json() -> None:
    agent = create_agent(model=TestModel.text('{"ok": true}'))
    result = await agent.run(
        "return JSON",
        model_settings=ModelSettings(temperature=0.1),
        output_policy=OutputPolicy.tool_or_text(OutputSchema.from_pydantic(Answer)),
    )
    assert result.structured_output == {"ok": True}
```

## Output Validators And Functions

Use output validators when the model should retry a final answer that parses but
does not satisfy application rules. Raise `OutputRetry` to ask for another model
turn within the policy retry budget.

```python
from pydantic import BaseModel

from starweaver import OutputPolicy, OutputRetry, OutputSchema, create_agent
from starweaver.testing import TestModel


class Answer(BaseModel):
    answer: str


def require_paris(output: dict[str, object]) -> None:
    if output["answer"] != "Paris":
        raise OutputRetry("answer must be Paris")


async def validate_answer() -> None:
    policy = (
        OutputPolicy.structured(OutputSchema.from_pydantic(Answer))
        .with_validator(require_paris)
        .with_retries(1)
    )
    agent = create_agent(
        model=TestModel.responses(
            [{"text": '{"answer":"London"}'}, {"text": '{"answer":"Paris"}'}]
        ),
        output_policy=policy,
    )
    result = await agent.run("answer")
    assert result.structured_output == {"answer": "Paris"}
```

`OutputFunction` exposes a final-output function to the model. The callback can
return a string for text output, a JSON-serializable value for structured output,
or `OutputValue.text()` / `OutputValue.json()` when the boundary must be
explicit.

```python
from starweaver import OutputFunction, OutputPolicy, create_agent
from starweaver.testing import TestModel


def final_answer(args: dict[str, object]) -> dict[str, object]:
    return {"answer": args["answer"]}


async def run_output_function() -> None:
    final = OutputFunction(
        "final_answer",
        {
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
        },
        final_answer,
    )
    agent = create_agent(
        model=TestModel.responses(
            [
                TestModel.tool_call_response(
                    [
                        {
                            "id": "call_final",
                            "name": "final_answer",
                            "arguments": {"answer": "Paris"},
                        }
                    ]
                )
            ]
        ),
        output_policy=OutputPolicy().with_function(final),
    )
    result = await agent.run("answer")
    assert result.structured_output == {"answer": "Paris"}
```

## Provider Models

`ProviderModel` creates production model adapters using the Rust provider
transport and profile code. API keys can be passed directly or loaded from the
provider default environment variable.

```python
from starweaver import ModelSettings, ProviderModel, create_agent


async def run_provider() -> None:
    model = ProviderModel.from_model_id(
        "openai_responses:gpt-5-mini",
        model_settings=ModelSettings(timeout_ms=30_000),
    )
    result = await create_agent(model=model).run("Write one sentence.")
    print(result.output)
```

Available helpers are `openai_responses()`, `openai_chat()`, `anthropic()`, and
`gemini()`. `from_model_id()` accepts `openai:`, `openai_responses:`,
`openai_chat:`, `anthropic:`, `gemini:`, and `oauth@codex:` model IDs. API-key
providers accept `api_key`, `api_key_env`, `model_config_preset`,
`model_settings`, `base_url`, and `endpoint_path`; `oauth@codex:` uses the
Starweaver OAuth store and does not require an API key.

## Capability Bundles

Use `CapabilityBundle` for static SDK composition: instructions, Python tools,
model settings, request parameters, output validators, and output functions can
be packaged once and reused across agents.

```python
from starweaver import CapabilityBundle, create_agent, tool
from starweaver.testing import TestModel


@tool
async def audit(value: str) -> dict[str, str]:
    return {"value": value}


bundle = CapabilityBundle(
    "audit-bundle",
    instructions=["Prefer concise audit notes."],
    tools=[audit],
)
agent = create_agent(model=TestModel.text("ready"), capability_bundles=[bundle])
```

The Python capability API is bundle-oriented. Hook-level capabilities should use
a typed Python hook contract before becoming public API; raw Rust
`AgentCapability` callbacks remain a Rust-side extension point.

## Subagents

`Subagent` registers another Python-created agent as an SDK subagent. The parent
agent receives Starweaver's native `delegate` and `subagent_info` tools. Use
`subagent_delegation_mode="async"` or `"blocking_and_async"` when background
delegation should be exposed.

```python
from starweaver import Subagent, create_agent
from starweaver.testing import TestModel


worker = create_agent(
    model=TestModel.text("worker done"),
    instructions=["Handle bounded tasks."],
)
parent = create_agent(
    model=TestModel.responses(
        [
            TestModel.tool_call_response(
                [
                    {
                        "id": "call_delegate",
                        "name": "delegate",
                        "arguments": {"subagent_name": "worker", "prompt": "do work"},
                    }
                ]
            ),
            {"text": "parent done"},
        ]
    ),
    subagents=[Subagent("worker", worker, description="Bounded worker")],
)
```

## FunctionModel

`FunctionModel` is a deterministic test model backed by a Python callback. The
callback receives canonical message history plus an info object containing the
runtime-prepared request parameters, merged model settings, and request context.

```python
from starweaver import create_agent, tool
from starweaver.testing import FunctionModel


@tool
async def echo(value: str) -> dict[str, str]:
    return {"value": value}


def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
    if len(messages) == 1:
        return {"tool_calls": [{"id": "call_echo", "name": "echo", "arguments": {"value": "hi"}}]}
    return {"text": "done"}


async def run_function_model() -> None:
    result = await create_agent(model=FunctionModel(respond), tools=[echo]).run("use echo")
    assert result.output == "done"
```

## Local Gates

Run the Python package gate before sending Python SDK changes:

```bash
make py-check
```

For repository-wide changes, also run:

```bash
make fmt-check
make check
make test
```
