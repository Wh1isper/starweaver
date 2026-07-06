import asyncio
import tempfile
from pathlib import Path
from typing import Any, cast

from starweaver import (
    AbstractToolset,
    ApprovalRequired,
    RunRecord,
    SqliteReplayEventLog,
    SqliteSessionStore,
    SqliteStreamArchive,
    Tool,
    ToolContext,
    ToolsetContext,
    ToolsetPreparation,
    create_agent,
    validate_toolset_ids,
)
from starweaver.testing import FunctionModel


class ProductToolset(AbstractToolset):
    name = "claw.product"
    id = "claw.product"

    def __init__(self, release: asyncio.Event) -> None:
        super().__init__()
        self._release = release

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[
                Tool(
                    self.wait_for_operator,
                    name="wait_for_operator",
                    description="Wait for operator steering before continuing.",
                    parameters_schema={"type": "object", "properties": {}},
                    sequential=True,
                ),
                Tool(
                    self.deploy_service,
                    name="deploy_service",
                    description="Deploy a service after canonical approval.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"service": {"type": "string"}},
                        "required": ["service"],
                    },
                    sequential=True,
                ),
            ],
            instructions=[f"Use product tools for session {ctx.session_id}."],
        )

    async def wait_for_operator(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> dict[str, bool]:
        del ctx, args
        await self._release.wait()
        return {"released": True}

    async def deploy_service(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> dict[str, object]:
        service = str(args["service"])
        if ctx.approval is None:
            raise ApprovalRequired(
                f"deploy {service}",
                metadata={"service": service, "risk": "medium"},
            )
        return {"service": service, "status": "deployed"}


async def run_claw_like_smoke(database_path: str | Path) -> dict[str, Any]:
    database = Path(database_path)
    SqliteSessionStore.migrate(database)
    store = SqliteSessionStore.open(database)
    archive = SqliteStreamArchive.open(database)
    replay = SqliteReplayEventLog.open(database)
    release = asyncio.Event()
    toolset = ProductToolset(release)
    validate_toolset_ids([toolset]).raise_for_errors()

    captured_messages: list[list[object]] = []

    def model_callback(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        params = cast(dict[str, Any], info["params"])
        tools = cast(list[dict[str, Any]], params["tools"])
        tool_names = {tool["name"] for tool in tools}
        if not {"wait_for_operator", "deploy_service"}.issubset(tool_names):
            raise RuntimeError(f"missing product tools: {sorted(tool_names)}")
        if len(captured_messages) == 1:
            return {
                "tool_calls": [
                    {
                        "id": "call_wait",
                        "name": "wait_for_operator",
                        "arguments": {},
                    }
                ]
            }
        if len(captured_messages) == 2:
            rendered = str(messages)
            if "Steering update from the user" not in rendered:
                raise RuntimeError("steering was not delivered to the active run")
            if "Use staged rollout." not in rendered:
                raise RuntimeError("operator steering text was not delivered")
            return {
                "tool_calls": [
                    {
                        "id": "call_deploy",
                        "name": "deploy_service",
                        "arguments": {"service": "api"},
                    }
                ]
            }
        return {"text": "deployment complete"}

    agent = create_agent(
        model=FunctionModel(model_callback),
        toolsets=[toolset],
    )
    async with agent.session() as session:
        async with session.run_stream("Deploy api") as run:
            async for event in run:
                tool_call = event.tool_call or {}
                call = tool_call.get("call", tool_call)
                if (
                    event.kind == "tool_call"
                    and isinstance(call, dict)
                    and call.get("name") == "wait_for_operator"
                ):
                    await run.steer("Use staged rollout.", id="operator-steer")
                    release.set()
                if event.kind == "suspended":
                    snapshot = await run.hitl().snapshot()
                    decision = snapshot.approvals[0].approve(decided_by="operator")
                    await run.hitl().resume_collected(approvals=[decision])
                    break
            stream_result = await run.join()

        session_record = await store.save_current_session(session)
        run_record = RunRecord.from_result(
            session_record.session_id,
            stream_result.result,
            sequence_no=1,
        )
        await store.append_run(run_record)
        await store.append_stream_records(
            session_record.session_id,
            run_record.run_id,
            [event.raw for event in stream_result.events],
        )
        scope = f"run:{run_record.run_id}"
        created_at = session_record.to_dict()["created_at"]
        await archive.append_raw_records(
            session_record.session_id,
            run_record.run_id,
            [event.raw for event in stream_result.events],
        )
        await replay.append(
            scope,
            {
                "scope": scope,
                "sequence": 1,
                "timestamp": created_at,
                "event": {"kind": "heartbeat"},
            },
        )

    restored = agent.session_from_archive(await store.load_archive(session_record.session_id))
    restored_state = restored.export_full_state()
    replayed_records = await store.replay_stream_records(
        session_record.session_id,
        run_record.run_id,
    )
    archived_records = await archive.replay_raw_after(
        session_record.session_id,
        run_record.run_id,
    )
    replay_events = await replay.replay_after(scope)

    return {
        "output": stream_result.result.output,
        "session_id": session_record.session_id,
        "run_id": run_record.run_id,
        "raw_stream_records": len(replayed_records),
        "archived_records": len(archived_records),
        "replay_events": len(replay_events),
        "restored_messages": len(restored_state["message_history"]),
        "steering_seen": "Use staged rollout." in str(captured_messages[1]),
    }


async def main() -> None:
    with tempfile.TemporaryDirectory() as directory:
        result = await run_claw_like_smoke(Path(directory) / "claw-like.sqlite3")
    print(result)


if __name__ == "__main__":
    asyncio.run(main())
