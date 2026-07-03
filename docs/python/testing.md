# Python Testing

The Python SDK is designed so application tests can run without network access.
Use deterministic model helpers and Python tool callbacks to cover runtime
composition, tool loops, output validation, HITL, sessions, and streams.

## TestModel

`TestModel.text(...)` returns one final text response:

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def test_ready() -> None:
    result = await create_agent(model=TestModel.text("ready")).run("status")
    assert result.output == "ready"
```

Use response scripts for multi-turn loops:

```python
from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def echo(value: str) -> dict[str, str]:
    return {"value": value}


async def test_tool_loop() -> None:
    model = TestModel.responses(
        [
            TestModel.tool_call_response(
                [{"id": "call_echo", "name": "echo", "arguments": {"value": "hi"}}]
            ),
            {"text": "done"},
        ]
    )
    result = await create_agent(model=model, tools=[echo]).run("use echo")
    assert result.output == "done"
```

## FunctionModel

Use `FunctionModel` when the test needs to inspect prepared request params or
branch on message history:

```python
from starweaver import create_agent, tool
from starweaver.testing import FunctionModel


@tool
async def echo(value: str) -> dict[str, str]:
    return {"value": value}


def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
    params = info["params"]
    assert isinstance(params, dict)
    assert params["tools"][0]["name"] == "echo"
    if len(messages) == 1:
        return {"tool_calls": [{"id": "call_echo", "name": "echo", "arguments": {"value": "hi"}}]}
    return {"text": "done"}


async def test_function_model() -> None:
    model = FunctionModel(respond)
    result = await create_agent(model=model, tools=[echo]).run("use echo")
    assert result.output == "done"
    assert model.captured_messages()
```

## Streams In Tests

Stream tests can assert record order without a provider:

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def test_stream_records() -> None:
    agent = create_agent(model=TestModel.text("streamed"))
    events = [event async for event in agent.run_stream("stream")]
    assert events[0].kind == "run_start"
    assert events[-1].kind == "run_complete"
```

Use `StreamAdapter` to project collected events into text, tool events, usage
snapshots, or sideband records.

## Local Gates

Run the full Python package gate from the repository root:

```bash
make py-check
```

For focused iteration:

```bash
make py-lint
make py-rust-check
make py-test
```

For docs changes:

```bash
make docs-check
make docs-build
```
