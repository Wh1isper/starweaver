# Python Agents

`create_agent()` is the main Python entry point. It builds a Python facade over
a Rust `starweaver-runtime` agent and accepts the same runtime concepts as the
Rust SDK: model, instructions, tools, toolsets, output policy, capability
bundles, subagents, runtime config, skills, environment providers, and media
uploaders.

## Create An Agent

```python
import asyncio

from starweaver import create_agent
from starweaver.testing import TestModel


async def main() -> None:
    agent = create_agent(
        model=TestModel.text("ready"),
        instructions=["Answer with one short sentence."],
    )
    result = await agent.run("Say ready")
    assert result.output == "ready"


asyncio.run(main())
```

`agent.run(...)` collects the final `RunResult`. Use it for request/response
workflows, tests, and server endpoints that do not need incremental stream
records.

## Per-Run Overrides

Run options are additive by default and do not mutate agent defaults:

- `instructions=[...]` appends instructions for this run.
- `tools=[...]` injects extra tools for this run only.
- `replace_tools=True` makes per-run tools the complete registry.
- `model_settings=ModelSettings(...)` overrides generation settings.
- `request_params=RequestParams(...)` forwards provider-neutral request data.
- `output_schema=OutputSchema(...)` or `output_policy=OutputPolicy(...)`
  controls final output.
- `trace_metadata={...}` adds low-cardinality run evidence to model request
  tracing and the returned `RunResult` without persisting it as session
  metadata.
- `toolsets=[...]` attaches grouped tools and instructions for one run.
- `environment=EnvironmentProvider(...)` attaches a process-local provider for
  one run.

Unknown options are rejected instead of being silently ignored.

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

## Sessions

Use `agent.session()` when an application needs reusable message history,
message-bus state, pending HITL records, environment state, or resumable
runtime evidence.

```python
from starweaver import SessionArchive, create_agent
from starweaver.testing import TestModel


async def run_session() -> None:
    agent = create_agent(model=TestModel.responses([{"text": "first"}, {"text": "second"}]))
    async with agent.session() as session:
        first = await session.run("first")
        archive = SessionArchive.from_session(session)

    restored = agent.session_from_archive(archive)
    second = await restored.run("second")
    assert (first.output, second.output) == ("first", "second")
```

Use `session.export_state()` for a curated portable snapshot. Use
`session.export_full_state()` or `SessionArchive.from_session(session)` when a
durable product needs raw runtime state and collected HITL state.

## Streams

`run_stream()` returns the current Python `AgentRun` facade. The stable evidence
surface is the canonical stream record sequence:

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def stream() -> None:
    agent = create_agent(model=TestModel.text("streamed"))
    async with agent.run_stream("stream") as run:
        async for event in run:
            print(event.kind, event.raw)

    result = await run.result()
    assert result.output == "streamed"
```

The facade also exposes `recv()`, `join()`, `result()`, `status()`,
`recoverable_state()`, `interrupt()`, and active message helpers. Treat those
as the Python package's current lifecycle API. The Rust canonical stream
records remain the portable contract for replay, storage, and UI projection.

## Async Context Managers

Both `Agent` and `AgentSession` are async context managers. If an exception
leaves the context while a run is active, the active run is interrupted before
the context exits. If there is no exception, unjoined active runs are joined so
session state is returned to a consistent idle state.

```python
async def scoped_run() -> None:
    async with create_agent(model=TestModel.text("done")) as agent:
        async with agent.session() as session:
            run = session.run_stream("finish")
        assert run.is_finished
```

## Subagents

`Subagent` registers another Python-created agent behind Starweaver's native
delegation tools. Use explicit inheritance policy when a child agent should
reuse parent tools or capability bundles.

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

Use `subagent_delegation_mode="async"` or `"blocking_and_async"` when
background delegation should be exposed.
