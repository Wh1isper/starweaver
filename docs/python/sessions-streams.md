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
re-register those before restoring. When an agent profile has durable toolsets,
the archive records their required IDs and `session_from_archive(...)` validates
them against the current agent before restore. Restore always rebinds those
process-local objects from the current agent/session profile, so stale archives
cannot weaken the current approval policy or keep an old environment provider.

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
`usage_record`, `usage_snapshot`, `approval`, `deferred`, `text_delta`,
`sideband_kind`, and `sideband_payload`.

Use typed observability helpers when application code should not walk raw JSON
paths:

```python
from starweaver import TraceMetadata, UsageSnapshot


async def inspect_evidence(agent) -> None:
    result = await agent.run("Summarize", trace_metadata={"audit_id": "run-1"})
    assert result.usage.total_tokens >= 0
    snapshot: UsageSnapshot = result.usage_snapshot
    trace: TraceMetadata = result.trace_metadata
    assert trace.metadata["audit_id"] == "run-1"

    async with agent.run_stream("Stream") as run:
        async for event in run:
            if event.usage_snapshot is not None:
                assert event.usage_snapshot.run_id
```

`RunResult.usage`, `RunResult.usage_snapshot`, and
`RunResult.trace_metadata` are typed facades over `raw_state`. Per-run
`trace_metadata={...}` is copied to the result evidence and model request
context, but it is not persisted as reusable session metadata. `StreamEvent`
keeps `usage` as the raw model-response usage dict for compatibility and adds
`usage_record` plus `usage_snapshot` for typed access.

Use `StreamAdapter` only for already-collected or replayed records:

```python
from starweaver import StreamAdapter


async def text_from_stream(agent) -> str:
    async with agent.run_stream("Research") as run:
        joined = await run.join()
    return StreamAdapter(joined.events).text()
```

`StreamAdapter` can also build deterministic projections for stored or replayed
records:

```python
adapter = StreamAdapter(joined.events)
display_messages = adapter.display_messages(session_id="session_app")
sse_frames = adapter.sse_frames(scope="run:run_app")
agui_events = adapter.agui_events(session_id="session_app")
buffer = adapter.replay_buffer(session_id="session_app")
```

These helpers keep `raw_records` available, preserve ordered cursors, pass
unknown records through as host display events, and never invent alternate run
state. `StreamAdapter` does not own interruption, steering, live continuation,
or SSE fanout.

## Message Bus

`session.messages` exposes Starweaver's message bus as coordination state. It
is MQ-like state owned by the session, not a UI event stream. Message IDs are
idempotent, `target=None` broadcasts to all subscribers, and targeted messages
are delivered only to the matching subscriber:

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def coordinate_agents() -> None:
    session = create_agent(model=TestModel.text("ok")).session()
    session.messages.subscribe("main")
    session.messages.subscribe("debugger")

    await session.messages.send("broadcast", id="msg-1", topic="notice")
    await session.messages.send("debug", id="msg-2", target="debugger")

    assert [message.id for message in session.messages.consume("main")] == ["msg-1"]
    assert [message.id for message in session.messages.consume("debugger")] == [
        "msg-1",
        "msg-2",
    ]
```

New subscribers start at the current tail, so they do not receive historical
messages. `unsubscribe(agent_id)` removes the subscriber cursor; subscribing the
same ID again also starts from the current tail. Python exposes the canonical
Rust bounded-retention behavior rather than adding another retention layer.

## Session Stores

Use `SessionStore` facades when an application needs durable records instead
of only an exported state blob. Python preserves the Rust record JSON shape for
sessions, runs, stream records, approvals, deferred tools, checkpoints, and
resume snapshots.

`InMemorySessionStore` is the deterministic test store. `JsonSessionStore` is a
single-file local development store. `SqliteSessionStore`,
`SqliteReplayEventLog`, and `SqliteStreamArchive` are backed by the native
`starweaver-storage` SQLite migrations:

```python
from starweaver import SqliteReplayEventLog, SqliteSessionStore, SqliteStreamArchive


async def persist_with_sqlite(session) -> None:
    path = "sessions.db"
    SqliteSessionStore.migrate(path)
    status = SqliteSessionStore.migration_status(path)
    assert status["pending"] == []

    store = SqliteSessionStore.open(path)
    session_record = await store.save_current_session(session)
    snapshot = await store.resume_snapshot(session_record.session_id)
    assert snapshot.session.session_id == session_record.session_id

    scope = f"session:{session_record.session_id}"
    replay = SqliteReplayEventLog.open(path)
    archive = SqliteStreamArchive.open(path)
    await replay.save_snapshot(scope, {"revision": 0})
    archived = await archive.latest_snapshot(scope)
    assert archived is not None and archived["revision"] == 0
```

`save_current_session(session)` always captures full runtime state. Use
`append_run(...)`, `append_stream_records(...)`, `append_approval(...)`, and
`append_deferred_tool(...)` when the product stores run evidence separately
from the current session snapshot.
`replay_stream_records(..., after_sequence=...)` returns canonical stream
records in sequence order.

Durable run input uses the same JSON shape as `starweaver-session::InputPart`.
Use `InputPart` helpers and status enums when writing store records directly:

```python
from starweaver import InputPart, ResourceRef, RunStatus, SessionStatus


artifact = ResourceRef.typed(
    "resource://workspace/spec.md",
    kind="document",
    metadata={"media_type": "text/markdown", "name": "spec.md"},
)


input_parts = [
    InputPart.text("deploy the service", metadata={"source": "api"}),
    InputPart.url("https://example.com/spec.md"),
    InputPart.file(artifact),
    InputPart.command("plan", ["--fast"]),
]

run_input = [part.to_dict() for part in input_parts]
assert run_input[0]["kind"] == "text"
assert SessionStatus.ACTIVE.value == "active"
assert RunStatus.WAITING.value == "waiting"
```

Custom Python stores can participate in the native `SessionStore` trait through
`to_native()`:

```python
from starweaver import InMemorySessionStore, SessionRecord


async def use_python_store_backend() -> None:
    store = InMemorySessionStore()
    native = store.to_native()
    record = SessionRecord.from_state(
        {
            "agent_id": "main",
            "session_id": "session_local",
            "conversation_id": "conversation_local",
        }
    )
    await native.save_session(record)
    assert (await native.load_session(record.session_id))["session_id"] == record.session_id
```

The bridge schedules Rust trait calls back onto the Python event loop and
validates records against Rust session types. The native callback handle exposes
the same session, run, checkpoint, stream, approval, deferred, resume, and
compact-trace methods as the SQLite store. Resume snapshots preserve the full
latest checkpoint record, not only the run-level checkpoint reference.
`SqliteSessionStore.to_native()` returns the native SQLite handle directly.

`SqliteStreamArchive` stores raw runtime stream records, projected display
messages, compact replay snapshots, and cursor ranges. `SqliteReplayEventLog`
stores ordered replay events and compact snapshots for product transports such
as SSE or AG-UI adapters. Both facades use raw canonical dicts so product code
can project them without forking the Rust stream protocol.

`SqliteSessionStore.in_memory()`, `SqliteReplayEventLog.in_memory()`, and
`SqliteStreamArchive.in_memory()` are useful for integration tests that should
exercise the native schema without writing a file. Python does not recreate the
SQLite schema or migration policy; that remains owned by `starweaver-storage`.

## Durable Runtime

Use `create_agent_runtime(...)` when the application wants Rust-owned durable
execution to write through a bound session store during runs:

```python
from starweaver import InMemorySessionStore, create_agent_runtime
from starweaver.testing import TestModel


async def run_with_store() -> None:
    store = InMemorySessionStore()
    runtime = create_agent_runtime(
        model=TestModel.responses([{"text": "saved"}, {"text": "streamed"}]),
        session_store=store,
        durable_session_id="session_app",
    )

    result = await runtime.run("persist this")
    assert result.output == "saved"

    runs = await store.list_runs("session_app")
    assert runs[0].to_dict()["output_preview"] == "saved"

    stream = await runtime.run_stream("persist stream")
    assert stream.result.output == "streamed"
```

`session_store` may be an `InMemorySessionStore`, `JsonSessionStore`,
`SqliteSessionStore`, `_native.PythonSessionStore`, or another object that
adapts through `to_native()`. `stream_archive` and `replay_event_log` accept the
native SQLite archive/log facades when the product wants separate replay
surfaces.

`AgentRuntime.run_stream(...)` is a collected durable run result. It persists
canonical stream records through the runtime but does not return the live
`AgentRun` control facade. Use `Agent.run_stream(...)` or
`AgentSession.run_stream(...)` when the application needs live steering,
interruption, or streamed HITL control.

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
active run. `AgentSession.steer(...)` raises `StateError` when no run is active
unless the caller passes `when_idle="queue"`, in which case the steering message
is stored in the idle session message bus and the returned receipt has no
`run_id`. Terminal, suspended, and finalizing runs reject new messages with
`StateError`.

`session.messages.send(...)` always returns `MessageDelivery`. For idle
sessions, `delivery.message` is the stored `BusMessage` and `delivery.receipt`
is `None`. During an active run, message writes are routed through the active
control handle and `delivery.receipt` is the `ControlReceipt`.

Generic `messages.send(...)` defaults to `source="application"` and never
becomes model-visible steering by source alone. Use `messages.steer(...)` or
topic `steering` for that path.

## Receiver Close And Detach

Use `close_receiver()` when an application wants to stop reading live stream
records but still intends to collect the final result:

```python
async def close_ui_stream(agent) -> None:
    run = agent.run_stream("summarize")
    run.close_receiver()
    result = await run.result()
    assert result.output
```

Use `detach()` only for explicit fire-and-observe work. It closes the receiver
and lets the native run finish in the background; after detach, the Python
`AgentRun` handle can no longer be joined or inspected for recoverable state.

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
