import asyncio

import pytest
import starweaver
from pydantic import BaseModel
from starweaver import (
    ApprovalRequired,
    CallDeferred,
    CapabilityBundle,
    ModelError,
    ModelRetry,
    ModelSettings,
    OutputFunction,
    OutputPolicy,
    OutputRetry,
    OutputSchema,
    ProviderModel,
    RequestParams,
    StreamError,
    Subagent,
    ToolError,
    ToolResult,
    create_agent,
    tool,
)
from starweaver.testing import FunctionModel, sleep_echo
from starweaver.testing import TestModel as StarweaverTestModel


def test_version_matches_native_extension() -> None:
    assert starweaver.__version__ == starweaver.version()


def test_native_awaitable_bridge() -> None:
    async def run() -> None:
        assert await sleep_echo({"ok": True}, delay_ms=1) == {"ok": True}

    asyncio.run(run())


def test_agent_run_returns_text() -> None:
    async def run() -> None:
        async with create_agent(model=StarweaverTestModel.text("ready")) as agent:
            result = await agent.run("Say ready")
        assert result.output == "ready"
        assert result.messages
        assert result.raw_state["status"] == "completed"

    asyncio.run(run())


def test_function_model_uses_python_callback_and_request_params() -> None:
    calls: list[dict[str, object]] = []

    @tool
    async def echo(value: str) -> dict[str, str]:
        return {"value": value}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        calls.append(info)
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        tool_def = tools[0]
        assert isinstance(tool_def, dict)
        assert tool_def["name"] == "echo"
        if len(messages) == 1:
            return {
                "tool_calls": [{"id": "call_echo", "name": "echo", "arguments": {"value": "hi"}}]
            }
        return {"text": "done"}

    async def run() -> None:
        model = FunctionModel(respond)
        result = await create_agent(model=model, tools=[echo]).run("use echo")
        assert result.output == "done"
        assert calls
        assert model.captured_messages()

    asyncio.run(run())


def test_model_errors_raise_specific_python_exception_class() -> None:
    def fail(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        raise RuntimeError("model boom")

    async def run() -> None:
        with pytest.raises(ModelError, match="model boom"):
            await create_agent(model=FunctionModel(fail)).run("fail")

    asyncio.run(run())


def test_tool_boundary_errors_raise_specific_python_exception_class() -> None:
    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_missing", "name": "missing", "arguments": {}}]
            )
        ]
    )

    async def run() -> None:
        with pytest.raises(ToolError, match="tool calls require"):
            await create_agent(model=model).run("missing tool")

    asyncio.run(run())


def test_per_run_tools_are_injected_without_mutating_agent_defaults() -> None:
    @tool
    async def once(value: str) -> dict[str, str]:
        return {"value": value}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_once", "name": "once", "arguments": {"value": "ok"}}]
            ),
            {"text": "done"},
            StarweaverTestModel.tool_call_response(
                [{"id": "call_once_again", "name": "once", "arguments": {"value": "missing"}}]
            ),
        ]
    )

    async def run() -> None:
        agent = create_agent(model=model)
        injected = await agent.run("with tool", tools=[once])
        assert injected.output == "done"
        with pytest.raises(ToolError, match="tool calls require"):
            await agent.run("without tool")

    asyncio.run(run())


def test_unknown_run_options_are_rejected_instead_of_ignored() -> None:
    agent = create_agent(model=StarweaverTestModel.text("ok"))
    with pytest.raises(TypeError):
        agent.run_stream("x", temperature=0.1)  # type: ignore[call-arg]


def test_agent_model_settings_and_request_params_are_forwarded() -> None:
    seen: list[dict[str, object]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, str]:
        seen.append(info)
        assert info["settings"] == {"temperature": 0.25}
        params = info["params"]
        assert isinstance(params, dict)
        assert params["metadata"] == {"purpose": "test"}
        return {"text": "configured"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            model_settings=ModelSettings(temperature=0.25),
            request_params=RequestParams(metadata={"purpose": "test"}),
        ).run("configured")
        assert result.output == "configured"
        assert seen

    asyncio.run(run())


def test_per_run_output_schema_and_model_settings_are_applied() -> None:
    class Answer(BaseModel):
        ok: bool

    seen_settings: list[object] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, str]:
        seen_settings.append(info["settings"])
        params = info["params"]
        assert isinstance(params, dict)
        assert params["output_schema"]["name"] == "Answer"
        assert params["output_mode"] == "tool_or_text"
        return {"text": '{"ok": true}'}

    async def run() -> None:
        schema = OutputSchema.from_pydantic(Answer)
        result = await create_agent(model=FunctionModel(respond)).run(
            "json",
            model_settings={"temperature": 0.1},
            output_policy=OutputPolicy.tool_or_text(schema).with_retries(2),
        )
        assert result.structured_output == {"ok": True}
        assert seen_settings == [{"temperature": 0.1}]

    asyncio.run(run())


def test_output_validator_retries_and_accepts_next_output() -> None:
    class Answer(BaseModel):
        answer: str

    attempts: list[object] = []

    async def require_ok(ctx: starweaver.OutputContext, output: dict[str, object]) -> None:
        attempts.append((ctx.run_id, output))
        if output["answer"] != "ok":
            raise OutputRetry("answer must be ok")

    async def run() -> None:
        policy = (
            OutputPolicy.structured(OutputSchema.from_pydantic(Answer))
            .with_validator(require_ok)
            .with_retries(1)
        )
        result = await create_agent(
            model=StarweaverTestModel.responses(
                [{"text": '{"answer":"bad"}'}, {"text": '{"answer":"ok"}'}]
            ),
            output_policy=policy,
        ).run("answer")
        assert result.structured_output == {"answer": "ok"}
        assert len(attempts) == 2

    asyncio.run(run())


def test_output_function_finishes_run_with_structured_value() -> None:
    calls: list[dict[str, object]] = []

    def final_answer(ctx: starweaver.OutputContext, args: dict[str, object]) -> dict[str, object]:
        calls.append({"run_id": ctx.run_id, **args})
        return {"answer": args["answer"]}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_final",
                        "name": "final_answer",
                        "arguments": {"answer": "Paris"},
                    }
                ]
            )
        ]
    )

    async def run() -> None:
        output_function = OutputFunction(
            "final_answer",
            {
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"],
            },
            final_answer,
        )
        result = await create_agent(
            model=model,
            output_policy=OutputPolicy().with_function(output_function),
        ).run("answer")
        assert result.structured_output == {"answer": "Paris"}
        assert calls and calls[0]["answer"] == "Paris"
        assert model.captured_params()[0]["tools"][0]["name"] == "final_answer"

    asyncio.run(run())


def test_capability_bundle_contributes_output_components() -> None:
    def require_bundle(output: dict[str, object]) -> None:
        if output["source"] != "bundle":
            raise OutputRetry("source must be bundle")

    async def final(args: dict[str, object]) -> dict[str, object]:
        return {"source": args["source"]}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_final_bad",
                        "name": "bundle_final",
                        "arguments": {"source": "wrong"},
                    }
                ]
            ),
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_final_good",
                        "name": "bundle_final",
                        "arguments": {"source": "bundle"},
                    }
                ]
            ),
        ]
    )

    async def run() -> None:
        bundle = CapabilityBundle(
            "output-bundle",
            output_validators=[require_bundle],
            output_functions=[
                OutputFunction(
                    "bundle_final",
                    {
                        "type": "object",
                        "properties": {"source": {"type": "string"}},
                        "required": ["source"],
                    },
                    final,
                )
            ],
        )
        result = await create_agent(
            model=model,
            output_policy=OutputPolicy().with_retries(1),
            capability_bundles=[bundle],
        ).run("bundle output")
        assert result.structured_output == {"source": "bundle"}

    asyncio.run(run())


def test_capability_bundle_contributes_tools() -> None:
    @tool
    async def bundled_echo(value: str) -> dict[str, str]:
        return {"value": value}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_bundle",
                        "name": "bundled_echo",
                        "arguments": {"value": "from bundle"},
                    }
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        bundle = CapabilityBundle("bundle", tools=[bundled_echo])
        result = await create_agent(model=model, capability_bundles=[bundle]).run("use bundle")
        assert result.output == "done"

    asyncio.run(run())


def test_registered_subagent_delegate_tool_runs_child_agent() -> None:
    worker = create_agent(
        name="worker",
        model=StarweaverTestModel.text("worker done"),
        instructions=["You are the worker."],
    )
    parent_model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
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
    )

    async def run() -> None:
        parent = create_agent(
            model=parent_model,
            subagents=[Subagent("worker", worker, description="Worker subagent")],
        )
        result = await parent.run("delegate")
        assert result.output == "parent done"
        assert worker._native is not None

    asyncio.run(run())


def test_provider_model_requires_api_key_before_network() -> None:
    with pytest.raises(ValueError, match="missing STARWEAVER_TEST_API_KEY"):
        ProviderModel.openai_responses(
            "gpt-test",
            api_key_env="STARWEAVER_TEST_API_KEY",
        )


def test_provider_model_from_model_id_dispatches_prefixes() -> None:
    with pytest.raises(ValueError, match="missing STARWEAVER_TEST_API_KEY"):
        ProviderModel.from_model_id(
            "openai_responses:gpt-test",
            api_key_env="STARWEAVER_TEST_API_KEY",
        )

    with pytest.raises(ValueError, match="unsupported model provider prefix"):
        ProviderModel.from_model_id("unknown:model")

    oauth_model = ProviderModel.from_model_id("oauth@codex:gpt-5.5")
    assert oauth_model.to_native() is not None

    codex_model = ProviderModel.codex_oauth("gpt-5.5")
    assert codex_model.to_native() is not None


def test_async_and_sync_python_tools_execute_in_runtime_loop() -> None:
    @tool
    async def alpha(value: int) -> dict[str, int]:
        await asyncio.sleep(0)
        return {"value": value + 1}

    @tool
    def beta(value: int) -> ToolResult:
        return ToolResult({"value": value + 2}, metadata={"source": "beta"})

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {"id": "call_alpha", "name": "alpha", "arguments": {"value": 1}},
                    {"id": "call_beta", "name": "beta", "arguments": {"value": 2}},
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[alpha, beta]).run("call tools")
        assert result.output == "done"
        second_request = model.captured_messages()[1]
        assert "'name': 'alpha'" in str(second_request)
        assert "'name': 'beta'" in str(second_request)

    asyncio.run(run())


def test_base_tool_subclass_and_raw_callable_are_registered() -> None:
    class MultiplyTool(starweaver.BaseTool):
        name = "multiply"

        def __init__(self) -> None:
            super().__init__(
                parameters_schema={
                    "type": "object",
                    "properties": {"value": {"type": "integer"}},
                    "required": ["value"],
                }
            )

        async def call(
            self, ctx: starweaver.ToolContext, args: dict[str, object]
        ) -> dict[str, object]:
            assert ctx.run_id
            value = args["value"]
            assert isinstance(value, int)
            return {"product": value * 2}

    def add_one(value: int) -> dict[str, int]:
        return {"value": value + 1}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {"id": "call_multiply", "name": "multiply", "arguments": {"value": 3}},
                    {"id": "call_add", "name": "add_one", "arguments": {"value": 4}},
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[MultiplyTool(), add_one]).run("tools")
        assert result.output == "done"
        second_request = str(model.captured_messages()[1])
        assert "'name': 'multiply'" in second_request
        assert "'name': 'add_one'" in second_request

    asyncio.run(run())


def test_python_tools_run_in_parallel_by_default() -> None:
    current = 0
    max_seen = 0
    both_started = asyncio.Event()

    async def enter() -> None:
        nonlocal current, max_seen
        current += 1
        max_seen = max(max_seen, current)
        if current == 2:
            both_started.set()
        await both_started.wait()
        await asyncio.sleep(0.01)
        current -= 1

    @tool
    async def alpha(args: dict[str, object]) -> dict[str, str]:
        await enter()
        return {"tool": "alpha"}

    @tool
    async def beta(args: dict[str, object]) -> dict[str, str]:
        await enter()
        return {"tool": "beta"}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {"id": "call_alpha", "name": "alpha", "arguments": {}},
                    {"id": "call_beta", "name": "beta", "arguments": {}},
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[alpha, beta]).run("parallel")
        assert result.output == "done"
        assert max_seen == 2

    asyncio.run(run())


def test_duplicate_python_tool_calls_fall_back_to_sequential_execution() -> None:
    current = 0
    max_seen = 0

    async def enter() -> None:
        nonlocal current, max_seen
        current += 1
        max_seen = max(max_seen, current)
        await asyncio.sleep(0.01)
        current -= 1

    @tool
    async def alpha(args: dict[str, object]) -> dict[str, str]:
        await enter()
        return {"tool": str(args.get("id"))}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {"id": "call_alpha_1", "name": "alpha", "arguments": {"id": "one"}},
                    {"id": "call_alpha_2", "name": "alpha", "arguments": {"id": "two"}},
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[alpha]).run("duplicate")
        assert result.output == "done"
        assert max_seen == 1

    asyncio.run(run())


def test_python_tool_sequential_flag_forces_model_order_execution() -> None:
    current = 0
    max_seen = 0

    async def enter() -> None:
        nonlocal current, max_seen
        current += 1
        max_seen = max(max_seen, current)
        await asyncio.sleep(0.01)
        current -= 1

    @tool(sequential=True)
    async def alpha(args: dict[str, object]) -> dict[str, str]:
        await enter()
        return {"tool": "alpha"}

    @tool
    async def beta(args: dict[str, object]) -> dict[str, str]:
        await enter()
        return {"tool": "beta"}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {"id": "call_alpha", "name": "alpha", "arguments": {}},
                    {"id": "call_beta", "name": "beta", "arguments": {}},
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[alpha, beta]).run("sequential")
        assert result.output == "done"
        assert max_seen == 1

    asyncio.run(run())


def test_python_model_retry_exception_reenters_tool_with_retry_context() -> None:
    retries: list[int] = []

    @tool(max_retries=2)
    async def unstable(ctx: starweaver.ToolContext, value: int) -> dict[str, int]:
        retries.append(ctx.retry)
        if ctx.retry == 0:
            raise ModelRetry("try again with a safer value")
        return {"value": value}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_unstable_1", "name": "unstable", "arguments": {"value": 1}}]
            ),
            StarweaverTestModel.tool_call_response(
                [{"id": "call_unstable_2", "name": "unstable", "arguments": {"value": 2}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[unstable]).run("retry")
        assert result.output == "done"
        assert retries == [0, 1]

    asyncio.run(run())


def test_user_exception_with_control_flow_name_is_not_misclassified() -> None:
    class ModelRetry(Exception):
        pass

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def fake_retry(args: dict[str, object]) -> dict[str, bool]:
        raise ModelRetry("not starweaver control flow")

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_fake", "name": "fake_retry", "arguments": {}}]
            ),
            {"text": "handled"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[fake_retry]).run("fake retry")
        assert result.output == "handled"
        second_request = str(model.captured_messages()[1])
        assert "not starweaver control flow" in second_request
        assert "requested model retry" not in second_request

    asyncio.run(run())


def test_pydantic_tool_arguments_are_validated() -> None:
    class AddArgs(BaseModel):
        left: int
        right: int

    @tool
    async def add(args: AddArgs) -> dict[str, int]:
        return {"total": args.left + args.right}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_add", "name": "add", "arguments": {"left": 2, "right": 3}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[add]).run("add")
        assert result.output == "done"
        assert add.parameters_schema["properties"]["left"]["type"] == "integer"

    asyncio.run(run())


def test_tool_schema_inference_rejects_untyped_kwargs_without_explicit_schema() -> None:
    def loose(**kwargs: object) -> dict[str, object]:
        return kwargs

    with pytest.raises(ValueError, match="explicit parameters_schema"):
        tool(loose)


def test_non_json_serializable_tool_return_is_reported_to_model() -> None:
    @tool(parameters_schema={"type": "object", "properties": {}})
    async def bad_return(args: dict[str, object]) -> object:
        return object()

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_bad", "name": "bad_return", "arguments": {}}]
            ),
            {"text": "handled"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[bad_return]).run("bad return")
        assert result.output == "handled"
        second_request = str(model.captured_messages()[1])
        assert "bad_return" in second_request
        assert "JSON serializable" in second_request

    asyncio.run(run())


def test_hitl_result_helpers_and_resume_after_approval() -> None:
    @tool(parameters_schema={"type": "object", "properties": {}})
    async def deploy(ctx: starweaver.ToolContext, args: dict[str, object]) -> dict[str, bool]:
        if ctx.approval is None:
            raise ApprovalRequired("deploy production", metadata={"risk": "high"})
        return {"approved": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_deploy", "name": "deploy", "arguments": {}}]
            ),
            {"text": "deployed"},
        ]
    )

    async def run() -> None:
        session = create_agent(model=model, tools=[deploy]).new_session()
        waiting = await session.run("deploy")
        assert waiting.needs_approval
        assert waiting.status == "waiting"
        assert waiting.is_waiting
        approval_id = str(waiting.pending_approvals[0]["approval_id"])
        assert waiting.pending_approvals[0]["name"] == "deploy"
        resumed = await session.resume_after_hitl(approvals={approval_id: {"approved": True}})
        assert resumed.output == "deployed"

    asyncio.run(run())


def test_deferred_tool_result_resume() -> None:
    @tool(parameters_schema={"type": "object", "properties": {}})
    async def collect(args: dict[str, object]) -> dict[str, bool]:
        raise CallDeferred("waiting for worker", metadata={"queue": "default"})

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_collect", "name": "collect", "arguments": {}}]
            ),
            {"text": "collected"},
        ]
    )

    async def run() -> None:
        session = create_agent(model=model, tools=[collect]).new_session()
        waiting = await session.run("collect")
        assert waiting.needs_approval
        assert waiting.status == "waiting"
        assert waiting.is_waiting
        deferred_id = str(waiting.pending_deferred[0]["deferred_id"])
        resumed = await session.resume_after_hitl(
            deferred_results={
                "results": [
                    {
                        "deferred_id": deferred_id,
                        "status": "completed",
                        "response": {"ok": True},
                    }
                ]
            }
        )
        assert resumed.output == "collected"

    asyncio.run(run())


def test_stream_events_preserve_raw_records() -> None:
    async def run() -> None:
        agent = create_agent(model=StarweaverTestModel.text("streamed"))
        events = [event async for event in agent.run_stream("stream")]
        assert events[0].kind == "run_start"
        assert events[0].raw["event"]["kind"] == "run_start"
        assert events[-1].kind == "run_complete"

    asyncio.run(run())


def test_live_stream_yields_events_before_tool_finishes() -> None:
    release = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait_for_release(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"released": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_wait", "name": "wait_for_release", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        stream = create_agent(model=model, tools=[wait_for_release]).run_stream("live")
        first = await asyncio.wait_for(stream.recv(), timeout=1)
        assert first is not None
        assert first.kind == "run_start"
        assert stream.status()["run_status"] == "running"
        release.set()
        result = await stream.result()
        assert result.output == "done"

    asyncio.run(run())


def test_stream_interrupt_cancels_running_python_tool() -> None:
    started = asyncio.Event()
    cancelled = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def slow(args: dict[str, object]) -> dict[str, bool]:
        started.set()
        try:
            await asyncio.sleep(60)
        except asyncio.CancelledError:
            cancelled.set()
            raise
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_slow", "name": "slow", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        stream = create_agent(model=model, tools=[slow]).run_stream("interrupt")
        await asyncio.wait_for(started.wait(), timeout=1)
        stream.interrupt()
        with pytest.raises(StreamError):
            await stream.join()
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        state = await stream.recoverable_state()
        assert isinstance(state, dict)
        assert state["run_id"]
        assert state["message_history"]

    asyncio.run(run())


def test_cancelling_stream_result_interrupts_running_python_tool() -> None:
    started = asyncio.Event()
    cancelled = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def slow(args: dict[str, object]) -> dict[str, bool]:
        started.set()
        try:
            await asyncio.sleep(60)
        except asyncio.CancelledError:
            cancelled.set()
            raise
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_slow", "name": "slow", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        stream = create_agent(model=model, tools=[slow]).run_stream("cancel")
        task = asyncio.create_task(stream.result())
        await asyncio.wait_for(started.wait(), timeout=1)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        assert stream.status()["cancel_requested"]

    asyncio.run(run())


def test_session_stream_blocks_concurrent_operations_until_join() -> None:
    started = asyncio.Event()
    release = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        started.set()
        await release.wait()
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_wait", "name": "wait", "arguments": {}}]
            ),
            {"text": "first"},
            {"text": "second"},
        ]
    )

    async def run() -> None:
        session = create_agent(model=model, tools=[wait]).new_session()
        stream = session.run_stream("first")
        await asyncio.wait_for(started.wait(), timeout=1)
        with pytest.raises(RuntimeError, match="session is busy"):
            session.export_state()
        with pytest.raises(RuntimeError, match="session is busy"):
            session.run_stream("blocked")
        release.set()
        first = await stream.result()
        assert first.output == "first"
        second = await session.run("second")
        assert second.output == "second"

    asyncio.run(run())


def test_session_state_exports_and_restores() -> None:
    async def run() -> None:
        agent = create_agent(
            model=StarweaverTestModel.responses([{"text": "first"}, {"text": "second"}])
        )
        session = agent.new_session()
        first = await session.run("first")
        assert first.output == "first"
        state = session.export_state()
        restored = agent.session_from_state(state)
        second = await restored.run("second")
        assert second.output == "second"

    asyncio.run(run())
