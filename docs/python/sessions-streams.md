# Python Sessions And Streams

Sessions keep state across runs. Streams expose canonical records while a run
is active. Python wraps both surfaces without changing the Rust runtime state
machine.

## Session State

`AgentSession.export_state()` returns the curated portable snapshot by default.
Use `export_full_state()` or `SessionArchive.from_session(session)` when a
durable application needs full runtime state.

```python
from starweaver import SessionArchive, create_agent
from starweaver.testing import TestModel


async def persist_session() -> None:
    agent = create_agent(model=TestModel.responses([{"text": "first"}, {"text": "second"}]))
    async with agent.session() as session:
        first = await session.run("first")
        archive = SessionArchive.from_session(session)

    restored = agent.session_from_archive(archive)
    second = await restored.run("second")
    assert (first.output, second.output) == ("first", "second")
```

`SessionArchive` is a JSON-compatible envelope with a checked archive format and
schema version. It does not serialize Python callables, live environment
handles, provider connections, or process-local dependencies; applications must
re-register those before restoring.

Full archives preserve collected pending HITL run state. Curated archives are
portable context snapshots and intentionally omit `last_run_state`.

## Stream Records

`run_stream()` returns an `AgentRun` facade. The facade is an async iterator over
`StreamEvent` objects:

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def collect_events() -> None:
    agent = create_agent(model=TestModel.text("streamed"))
    events = []
    async with agent.run_stream("stream") as run:
        async for event in run:
            events.append(event.raw)

    assert events[0]["event"]["kind"] == "run_start"
```

`StreamEvent.raw` is the canonical escape hatch. Common fields are exposed as
lazy accessors: `run_id`, `step`, `tool_call`, `tool_return`, `usage`,
`approval`, `deferred`, `text_delta`, `sideband_kind`, and `sideband_payload`.

Use `StreamAdapter` only for already-collected or replayed records:

```python
from starweaver import StreamAdapter


async def text_from_stream(agent) -> str:
    async with agent.run_stream("Research") as run:
        joined = await run.join()
    return StreamAdapter(joined.events).text()
```

`StreamAdapter` does not own interruption, steering, or live continuation.

## Active Control

`AgentRun.steer(...)`, `AgentSession.steer(...)`, and
`run.messages.steer(...)` queue user steering messages into the active Rust
run. The queue is drained into `AgentContext`, then the runtime's existing
steering logic adds a model-visible user prompt named `steering`.

```python
from starweaver import create_agent, tool


@tool(parameters_schema={"type": "object", "properties": {}})
async def wait(args: dict[str, object]) -> dict[str, bool]:
    return {"ok": True}


async def steer_run(model) -> None:
    async with create_agent(model=model, tools=[wait]) as agent:
        async with agent.session() as session:
            async with session.run_stream("deploy") as run:
                await run.steer("Use the safe rollout path.", id="ui-1")
                async for event in run:
                    print(event.kind)
```

The returned `ControlReceipt` means the control input was accepted for the
active run. Terminal, suspended, and finalizing runs reject new messages with
`StateError`.

`session.messages.send(...)` always returns `MessageDelivery`. For idle
sessions, `delivery.message` is the stored `BusMessage` and `delivery.receipt`
is `None`. During an active run, message writes are routed through the active
control handle and `delivery.receipt` is the `ControlReceipt`.

Generic `messages.send(...)` defaults to `source="application"` and never
becomes model-visible steering by source alone. Use `messages.steer(...)` or
topic `steering` for that path.

## Interruption

Cancelling a Python task waiting on `recv()`, `join()`, or `result()` interrupts
the underlying Starweaver run. If the run is inside a Python async tool, the
tool coroutine is cancelled on the application event loop.

```python
import asyncio


async def cancel_run(agent) -> None:
    run = agent.run_stream("long task")
    task = asyncio.create_task(run.result())
    task.cancel()
```

Call `recoverable_state()` after an interrupted run when the application needs
the latest observed state for storage.

## Streamed HITL

For streamed HITL, wait for a `suspended` event before calling
`await run.hitl().snapshot()`:

```python
async def stream_hitl(session) -> None:
    async with session.run_stream("deploy") as run:
        async for event in run:
            if event.kind != "suspended":
                continue
            snapshot = await run.hitl().snapshot()
            decision = snapshot.approvals[0].approve(decided_by="ui")
            result = await run.hitl().resume_collected(approvals=[decision])
            assert result.output
            break
```

`resume_collected(...)` resumes through the owning session and returns a
collected `RunResult`. It is not a live continuation handle. After collected
resume, `run.result()` and `run.join().result` expose the final resumed result;
`run.join().events` remains the original suspended stream records and does not
include post-resume replay records.
