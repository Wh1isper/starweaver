import asyncio
from pathlib import Path

import pytest
import starweaver
from pydantic import BaseModel
from starweaver import (
    ApprovalRequired,
    CallDeferred,
    CapabilityBundle,
    EnvironmentProvider,
    MediaUploader,
    ModelError,
    ModelRetry,
    ModelSettings,
    OutputFunction,
    OutputPolicy,
    OutputRetry,
    OutputSchema,
    ProviderAuth,
    ProviderModel,
    RequestParams,
    ResourceRef,
    RuntimeConfig,
    SkillPackage,
    SkillRegistry,
    SkillSourceScope,
    StateError,
    StreamAdapter,
    StreamError,
    Subagent,
    ToolError,
    ToolProxyToolset,
    ToolResult,
    ToolSearchToolset,
    Toolset,
    create_agent,
    environment_toolsets,
    filesystem_toolset,
    shell_toolset,
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


def test_python_tool_accepts_business_parameter_named_context() -> None:
    @tool
    async def echo_context(context: str) -> dict[str, str]:
        return {"context": context}

    assert echo_context.parameters_schema["properties"]["context"] == {"type": "string"}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_context",
                        "name": "echo_context",
                        "arguments": {"context": "business"},
                    }
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[echo_context]).run("echo")
        assert result.output == "done"

    asyncio.run(run())


def test_python_tool_can_return_pydantic_model() -> None:
    class Payload(BaseModel):
        value: str

    @tool
    async def build_payload(value: str) -> Payload:
        return Payload(value=value)

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_payload",
                        "name": "build_payload",
                        "arguments": {"value": "ok"},
                    }
                ]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[build_payload]).run("payload")
        assert result.output == "done"

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


def test_structured_output_failures_raise_output_error() -> None:
    class Answer(BaseModel):
        ok: bool

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, str]:
        return {"text": "not-json"}

    async def run() -> None:
        with pytest.raises(starweaver.OutputError, match="output"):
            await create_agent(
                model=FunctionModel(respond),
                output_policy=OutputPolicy.structured(OutputSchema.from_pydantic(Answer)),
            ).run("json")

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


def test_static_toolset_contributes_tools_and_instructions() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    seen_messages: list[object] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        seen_messages.append(messages)
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        assert tools[0]["name"] == "lookup"
        if len(messages) == 1:
            return {
                "tool_calls": [
                    {
                        "id": "call_lookup",
                        "name": "lookup",
                        "arguments": {"value": "ok"},
                    }
                ]
            }
        return {"text": "done"}

    async def run() -> None:
        toolset = Toolset(
            "workspace",
            tools=[lookup],
            instructions=["Use workspace paths exactly."],
        )
        assert toolset.tool_definitions()[0]["name"] == "lookup"
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset],
        ).run("use workspace")
        assert result.output == "done"
        assert "Use workspace paths exactly." in str(seen_messages[0])

    asyncio.run(run())


def test_per_run_toolset_does_not_mutate_agent_defaults() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_lookup",
                        "name": "lookup",
                        "arguments": {"value": "ok"},
                    }
                ]
            ),
            {"text": "done"},
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_lookup_missing",
                        "name": "lookup",
                        "arguments": {"value": "missing"},
                    }
                ]
            ),
        ]
    )

    async def run() -> None:
        agent = create_agent(model=model)
        toolset = Toolset("workspace", tools=[lookup])
        injected = await agent.run("with toolset", toolsets=[toolset])
        assert injected.output == "done"
        with pytest.raises(ToolError, match="tool calls require"):
            await agent.run("without toolset")

    asyncio.run(run())


def test_tool_search_and_proxy_toolsets_wrap_python_tools() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    async def run_search() -> None:
        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_search",
                            "name": "tool_search",
                            "arguments": {"query": "lookup"},
                        }
                    ]
                ),
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_lookup",
                            "name": "lookup",
                            "arguments": {"value": "ok"},
                        }
                    ]
                ),
                {"text": "done"},
            ]
        )
        library = [Toolset("workspace", tools=[lookup])]
        result = await create_agent(
            model=model,
            toolsets=[ToolSearchToolset(library, max_results=5)],
        ).run("search lookup")
        assert result.output == "done"

    async def run_proxy() -> None:
        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_proxy_search",
                            "name": "search_tools",
                            "arguments": {"query": "lookup"},
                        }
                    ]
                ),
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_proxy_lookup",
                            "name": "call_tool",
                            "arguments": {
                                "name": "lookup",
                                "arguments": {"value": "ok"},
                            },
                        }
                    ]
                ),
                {"text": "done"},
            ]
        )
        library = [Toolset("workspace", tools=[lookup])]
        result = await create_agent(
            model=model,
            toolsets=[ToolProxyToolset(library)],
        ).run("proxy lookup")
        assert result.output == "done"

    asyncio.run(run_search())
    asyncio.run(run_proxy())


def test_runtime_config_enters_full_state_without_provider_settings() -> None:
    async def run() -> None:
        async with create_agent(
            model=StarweaverTestModel.text("configured"),
            runtime_config=RuntimeConfig(
                context_window=1234,
                compact_threshold=0.75,
                cold_start_trim_seconds=10,
                stream_resume=True,
            ),
        ).session() as session:
            result = await session.run("configured")
            assert result.output == "configured"
            state = session.export_full_state()
        assert state["model_config"]["context_window"] == 1234
        assert state["model_config"]["compact_threshold"]["per_thousand"] == 750
        assert state["model_config"]["cold_start_trim_seconds"] == 10
        assert state["model_config"]["stream_resume_on_error"] is True

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


def test_provider_auth_helpers_build_typed_provider_overlays() -> None:
    codex_model = ProviderModel.codex_oauth(
        "gpt-5.5",
        auth=ProviderAuth.codex_oauth(),
        session_id="session-123",
        thread_id="thread-456",
        stream_transport="websocket",
    )
    assert codex_model.to_native() is not None

    with pytest.raises(ValueError, match="stream_transport"):
        ProviderModel.codex_oauth("gpt-5.5", stream_transport="pipe")

    settings = ModelSettings(
        {
            "provider_settings": {
                "codex": {"session_id": "base"},
                "openai_responses": {"stream_transport": "http"},
            }
        }
    )
    merged = starweaver.model._with_provider_settings(  # pyright: ignore[reportPrivateUsage]
        settings,
        codex={"thread_id": "thread"},
        openai_responses={"stream_transport": "web_socket"},
    )
    assert isinstance(merged, ModelSettings)
    payload = merged.to_dict()
    assert payload["provider_settings"]["codex"] == {
        "session_id": "base",
        "thread_id": "thread",
    }
    assert payload["provider_settings"]["openai_responses"]["stream_transport"] == "web_socket"


def test_environment_provider_and_skill_registry_use_native_provider() -> None:
    skill_markdown = """---
name: research
description: Research workflow
---
Read primary sources before answering.
"""

    async def run() -> None:
        environment = EnvironmentProvider.virtual(
            id="skills",
            files={
                "README.md": "workspace readme",
                "skills/research/SKILL.md": skill_markdown,
            },
            resources=[ResourceRef.typed("resource://artifact", kind="media")],
            shell_outputs={"echo ok": "ok\n"},
            tmp_namespace="pytest",
        )

        assert await environment.read_text("README.md") == "workspace readme"
        listing = await environment.list("")
        assert "README.md" in listing
        shell_output = await environment.run_shell("echo ok")
        assert shell_output["status"] == 0
        assert shell_output["stdout"] == "ok\n"
        assert shell_output["stderr"] == ""
        grep = await environment.grep("workspace", include="**/*.md")
        assert grep[0]["path"] == "README.md"
        state = await environment.export_state()
        assert state["provider_id"] == "skills"
        assert state["resources"][0]["metadata"]["resource_kind"] == "media"

        parsed = SkillPackage.parse("skills/research/SKILL.md", skill_markdown)
        assert parsed.name == "research"

        registry = await SkillRegistry.scan(
            environment,
            scopes=[SkillSourceScope(root="", directories=["skills"])],
        )
        package = registry.get("research")
        assert package is not None
        assert package.description == "Research workflow"

        activated = await SkillRegistry.activate(environment, package.path)
        assert "primary sources" in (activated.body or "")
        toolset = registry.toolset()
        instructions = toolset.instruction_records()
        assert "Available fileops-loaded skills" in str(instructions)

    asyncio.run(run())


def test_first_party_environment_toolsets_are_exposed() -> None:
    filesystem = filesystem_toolset()
    shell = shell_toolset()
    bundled = environment_toolsets()

    assert filesystem.name == "filesystem"
    assert shell.name == "shell"
    assert [toolset.name for toolset in bundled] == ["filesystem", "shell"]
    definitions = filesystem.tool_definitions() + shell.tool_definitions()
    names = {definition["name"] for definition in definitions}
    assert {"view", "ls", "shell_exec"}.issubset(names)


def test_skill_registry_installs_model_facing_instructions() -> None:
    skill_markdown = """---
name: research
description: Research workflow
---
Read primary sources before answering.
"""
    seen_messages: list[str] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        seen_messages.append(str(messages))
        return {"text": "done"}

    async def run() -> None:
        environment = EnvironmentProvider.virtual(
            files={"skills/research/SKILL.md": skill_markdown}
        )
        registry = await SkillRegistry.scan(
            environment,
            SkillSourceScope(root="", directories=["skills"]),
        )
        result = await create_agent(
            model=FunctionModel(respond),
            skills=registry,
            environment=environment,
        ).run("use a skill")
        assert result.output == "done"
        exported = (
            await create_agent(
                model=StarweaverTestModel.text("state"),
                environment=environment,
            )
            .session()
            .export_environment_state()
        )
        assert exported is not None

    asyncio.run(run())
    assert seen_messages
    assert "Available fileops-loaded skills" in seen_messages[0]
    assert "research" in seen_messages[0]


def test_media_uploader_callback_enters_native_filter() -> None:
    png_data_url = (
        "data:image/png;base64,"
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/"
        "l2F0eQAAAABJRU5ErkJggg=="
    )
    uploads: list[str] = []

    @tool
    async def inline_image() -> dict[str, str]:
        return {"data_url": png_data_url, "media_type": "image/png"}

    async def upload(request: starweaver.MediaUploadRequest) -> dict[str, str]:
        uploads.append(request.media_type)
        return {
            "uri": "resource://pytest/image.png",
            "media_type": request.media_type,
            "resource_type": "image",
        }

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        if len(messages) == 1:
            return {"tool_calls": [{"id": "call_image", "name": "inline_image", "arguments": {}}]}
        latest = messages[-1]
        assert isinstance(latest, dict)
        metadata = latest.get("metadata")
        assert isinstance(metadata, dict)
        assert metadata["starweaver_media_uploaded"] == 1
        return {"text": "uploaded"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            tools=[inline_image],
            runtime_config=RuntimeConfig(capabilities=["image_url"]),
            media_uploader=MediaUploader(upload),
        ).run("upload image")
        assert result.output == "uploaded"

    asyncio.run(run())
    assert uploads == ["image/png"]


def test_stream_adapter_projects_canonical_records() -> None:
    events = [
        starweaver.StreamEvent(
            {
                "event": {
                    "kind": "model_stream",
                    "event": {"text_delta": "he"},
                }
            }
        ),
        starweaver.StreamEvent(
            {
                "event": {
                    "kind": "model_stream",
                    "event": {"text_delta": "llo"},
                }
            }
        ),
        starweaver.StreamEvent({"event": {"kind": "run_complete"}}),
    ]
    adapter = StreamAdapter(events)
    assert adapter.text() == "hello"
    assert adapter.text_deltas() == ["he", "llo"]
    assert adapter.terminal() is events[-1]


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


def test_keyword_tool_arguments_reject_missing_and_unexpected_fields() -> None:
    calls: list[dict[str, str]] = []
    model_calls = 0

    @tool(max_retries=2)
    async def greet(name: str, punctuation: str = "!") -> dict[str, str]:
        calls.append({"name": name, "punctuation": punctuation})
        return {"message": f"{name}{punctuation}"}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        nonlocal model_calls
        del info
        model_calls += 1
        if model_calls == 1:
            return {"tool_calls": [{"id": "call_missing", "name": "greet", "arguments": {}}]}
        if model_calls == 2:
            assert "missing required tool argument" in str(messages)
            return {
                "tool_calls": [
                    {
                        "id": "call_extra",
                        "name": "greet",
                        "arguments": {"name": "Ada", "unknown": True},
                    }
                ]
            }
        if model_calls == 3:
            assert "unexpected tool argument" in str(messages)
            return {
                "tool_calls": [{"id": "call_ok", "name": "greet", "arguments": {"name": "Ada"}}]
            }
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(model=FunctionModel(respond), tools=[greet]).run("greet")
        assert result.output == "done"
        assert calls == [{"name": "Ada", "punctuation": "!"}]

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


def test_raw_approval_mapping_requires_explicit_approved_bool() -> None:
    executed = False

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def deploy(ctx: starweaver.ToolContext, args: dict[str, object]) -> dict[str, bool]:
        nonlocal executed
        if ctx.approval is None:
            raise ApprovalRequired("deploy production", metadata={"risk": "high"})
        executed = True
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
        approval_id = str(waiting.pending_approvals[0]["approval_id"])
        with pytest.raises(ValueError, match="approved: bool"):
            await session.resume_after_hitl(approvals={approval_id: {"reason": "ambiguous"}})
        with pytest.raises(ValueError, match="approved: bool"):
            await session.resume_after_hitl(approvals={approval_id: {"approve": False}})

    asyncio.run(run())
    assert not executed


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


def test_typed_deferred_helper_resume() -> None:
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
        assert waiting.hitl.deferred
        result = waiting.hitl.deferred[0].complete({"ok": True}, worker="pytest")
        resumed = await session.hitl.resume(deferred_results=[result])
        assert resumed.output == "collected"

    asyncio.run(run())


def test_session_archive_preserves_waiting_hitl_snapshot_for_restore() -> None:
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
        agent = create_agent(model=model, tools=[deploy])
        session = agent.session()
        waiting = await session.run("deploy")
        assert waiting.hitl.approvals
        curated = session.archive(mode="curated")
        assert "last_run_state" not in curated.to_dict()
        with pytest.raises(ValueError, match="full session archive"):
            starweaver.SessionArchive.from_state(
                session.export_state(),
                mode="curated",
                last_run_state=waiting.raw_run_state,
            )
        malformed = {
            **starweaver.SessionArchive.from_session(session).to_dict(),
            "last_run_state": "not a mapping",
        }
        with pytest.raises(TypeError, match="last_run_state"):
            starweaver.SessionArchive.from_dict(malformed)
        archive = session.archive()
        restored = agent.session_from_archive(archive)
        snapshot = await restored.hitl.snapshot()
        decision = snapshot.approvals[0].approve(decided_by="pytest")
        resumed = await restored.hitl.resume(approvals=[decision])
        assert resumed.output == "deployed"

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
        assert stream.status()["drop_policy"] == "backpressure"
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
        stream.interrupt("stop requested by pytest")
        with pytest.raises(StreamError, match="stop requested by pytest"):
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
        async with create_agent(model=model, tools=[slow]) as agent, agent.session() as session:
            stream = session.run_stream("cancel")
            task = asyncio.create_task(stream.result())
            await asyncio.wait_for(started.wait(), timeout=1)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            await asyncio.wait_for(cancelled.wait(), timeout=1)
            assert stream.status()["cancel_requested"]
            for _ in range(100):
                if stream.is_finished and session.active_run is None:
                    break
                await asyncio.sleep(0.01)
            assert stream.is_finished
            assert session.active_run is None

    asyncio.run(run())


def test_agent_context_exit_interrupts_unjoined_active_run() -> None:
    started = asyncio.Event()
    cancelled = asyncio.Event()
    agent_run: starweaver.AgentRun | None = None

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
        nonlocal agent_run
        with pytest.raises(RuntimeError, match="boom"):
            async with create_agent(model=model, tools=[slow]) as agent:
                agent_run = agent.run_stream("cleanup")
                await asyncio.wait_for(started.wait(), timeout=1)
                raise RuntimeError("boom")
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        assert agent_run is not None
        assert agent_run.is_finished

    asyncio.run(run())


def test_session_context_exit_joins_unjoined_active_run() -> None:
    async def run() -> None:
        async with create_agent(model=StarweaverTestModel.text("done")).session() as session:
            agent_run = session.run_stream("done")
        assert agent_run.is_finished
        assert session.active_run is None
        assert (await agent_run.result()).output == "done"

    asyncio.run(run())


def test_interrupted_session_stream_writes_recoverable_state_back_to_session() -> None:
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
        session = create_agent(model=model, tools=[slow]).session()
        stream = session.run_stream("recover")
        await asyncio.wait_for(started.wait(), timeout=1)
        session.interrupt("persist interrupted state")
        with pytest.raises(StreamError, match="persist interrupted state"):
            await stream.join()
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        state = session.export_full_state()
        assert state["run_id"]
        assert state["message_history"]

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
        with pytest.raises(StateError, match="session is busy"):
            session.export_state()
        with pytest.raises(StateError, match="session is busy"):
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


def test_full_state_and_session_archive_round_trip(tmp_path: Path) -> None:
    async def run() -> None:
        agent = create_agent(
            model=StarweaverTestModel.responses([{"text": "first"}, {"text": "second"}])
        )
        session = agent.session()
        first = await session.run("first")
        assert first.output == "first"
        await session.messages.send("remember this", id="msg-state", topic="note")

        curated = session.export_state()
        full = session.export_full_state()
        assert "message_history" not in curated
        assert "message_bus" not in curated
        assert full["message_history"]
        assert "message_bus" in full
        full["state"] = {"domains": {"domain": {"value": True}}}
        full["trace_snapshot"] = {
            "trace_id": "trace-main",
            "span_id": "span-main",
            "trace_state": "state-main",
        }
        full["metadata"] = {"debug": True}
        full["model_config"] = {"context_window": 100}

        restored_state_session = agent.session_from_state(full)
        full_round_trip = restored_state_session.export_full_state()
        assert full_round_trip["state"]["domains"]["domain"]["value"] is True
        assert full_round_trip["trace_snapshot"]["trace_id"] == "trace-main"
        assert full_round_trip["metadata"]["debug"] is True
        assert full_round_trip["model_config"]["context_window"] == 100

        archive = starweaver.SessionArchive.from_session(restored_state_session)
        assert archive.mode == "full"
        assert archive.state["message_history"] == full_round_trip["message_history"]
        encoded = archive.to_json(sort_keys=True)
        decoded = starweaver.SessionArchive.from_json(encoded)
        assert decoded.to_dict() == archive.to_dict()
        assert decoded.format == "starweaver.session.archive"
        assert decoded.version == 1
        with pytest.raises(ValueError, match="archive format"):
            starweaver.SessionArchive.from_dict({**archive.to_dict(), "format": "wrong"})
        with pytest.raises(ValueError, match="archive version"):
            starweaver.SessionArchive.from_dict({**archive.to_dict(), "version": 999})
        archive_path = tmp_path / "session.json"
        archive.save(archive_path)
        assert starweaver.SessionArchive.load(archive_path).to_dict() == archive.to_dict()

        restored = agent.session_from_archive(decoded)
        second = await restored.run("second")
        assert second.output == "second"

    asyncio.run(run())


def test_session_store_facades_preserve_full_state_and_stream_order(tmp_path: Path) -> None:
    async def run() -> None:
        agent = create_agent(
            model=StarweaverTestModel.responses([{"text": "first"}, {"text": "second"}])
        )
        session = agent.session()
        first = await session.run("first")

        store = starweaver.InMemorySessionStore()
        session_record = await store.save_current_session(session)
        run_record = starweaver.RunRecord.from_result(session_record.session_id, first)
        await store.append_run(run_record)
        await store.append_stream_records(
            session_record.session_id,
            run_record.run_id,
            [
                {"sequence": 2, "event": {"kind": "second"}},
                {"sequence": 1, "event": {"kind": "first"}},
            ],
        )
        replay = await store.replay_stream_records(session_record.session_id, run_record.run_id)
        assert [record.sequence for record in replay] == [1, 2]

        snapshot = await store.resume_snapshot(session_record.session_id, run_record.run_id)
        assert snapshot.state["message_history"]
        assert snapshot.run is not None
        assert snapshot.run.run_id == run_record.run_id

        archive = await store.load_archive(session_record.session_id)
        restored = agent.session_from_archive(archive)
        second = await restored.run("second")
        assert second.output == "second"

        path = tmp_path / "sessions.json"
        json_store = starweaver.JsonSessionStore(path)
        await json_store.save_session(session_record)
        await json_store.append_run(run_record)
        reopened = starweaver.JsonSessionStore(path)
        loaded = await reopened.load_session(session_record.session_id)
        assert loaded.state["message_history"]

    asyncio.run(run())


def test_pythonic_session_context_and_typed_hitl_resume() -> None:
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
        async with create_agent(model=model, tools=[deploy]) as agent, agent.session() as session:
            waiting = await session.run("deploy")
            assert waiting.hitl.approvals
            decision = waiting.hitl.approvals[0].approve(decided_by="pytest")
            resumed = await session.hitl.resume(approvals=[decision])
        assert resumed.output == "deployed"

    asyncio.run(run())


def test_session_message_bus_idle_send_peek_consume() -> None:
    async def run() -> None:
        session = create_agent(model=StarweaverTestModel.text("ok")).session()
        sent = await session.messages.send(
            "hello",
            id="msg-1",
            topic="note",
            metadata={"source": "test"},
        )
        assert isinstance(sent, starweaver.MessageDelivery)
        assert not sent.active
        assert sent.id == "msg-1"
        assert sent.message.topic == "note"
        assert sent.message.metadata["starweaver.topic"] == "note"
        assert session.messages.peek()[0].id == "msg-1"
        consumed = session.messages.consume()
        assert consumed[0].content == "hello"
        assert session.messages.peek() == []

    asyncio.run(run())


def test_bus_message_topic_conflicts_are_rejected() -> None:
    message = starweaver.BusMessage(
        "hello",
        topic="note",
        metadata={"starweaver.topic": "steering"},
    )
    with pytest.raises(ValueError, match="topic conflicts"):
        message.to_dict()


def test_invalid_when_idle_is_rejected_even_when_run_is_active() -> None:
    release = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_wait", "name": "wait", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        async with create_agent(model=model, tools=[wait]) as agent, agent.session() as session:
            async with session.run_stream("deploy") as agent_run:
                async for event in agent_run:
                    if event.kind != "tool_call":
                        continue
                    with pytest.raises(ValueError, match="when_idle"):
                        await session.messages.steer("x", when_idle="drop")
                    release.set()
            assert (await agent_run.result()).output == "done"

    asyncio.run(run())


def test_active_run_steering_reaches_python_runtime_context() -> None:
    release = asyncio.Event()
    captured_messages: list[list[object]] = []
    events: list[starweaver.StreamEvent] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"ok": True}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        if len(captured_messages) == 1:
            return {"tool_calls": [{"id": "call_wait", "name": "wait", "arguments": {}}]}
        assert "Steering update from the user" in str(messages)
        assert str(messages).count("Use the safe rollout path.") == 1
        return {"text": "done"}

    async def run() -> None:
        async with (
            create_agent(model=FunctionModel(respond), tools=[wait]) as agent,
            agent.session() as session,
        ):
            async with session.run_stream("deploy") as agent_run:
                assert isinstance(agent_run, starweaver.AgentRun)
                async for event in agent_run:
                    events.append(event)
                    if event.kind == "tool_call":
                        receipt = await session.steer(
                            "Use the safe rollout path.",
                            id="ui-1",
                        )
                        assert receipt.id == "ui-1"
                        assert receipt.kind == "steering"
                        assert receipt.queued
                        duplicate = await agent_run.steer(
                            "Use the safe rollout path.",
                            id="ui-1",
                        )
                        assert duplicate.id == "ui-1"
                        assert duplicate.kind == "steering"
                        assert duplicate.queued
                        release.set()
                result = await agent_run.result()
            assert session.active_run is None
        assert result.output == "done"

    asyncio.run(run())
    assert len(captured_messages) == 2
    custom_kinds = [event.sideband_kind for event in events if event.kind == "custom"]
    assert custom_kinds.count("steering_submitted") == 1
    assert custom_kinds.count("steering_received") == 1


def test_active_message_send_preserves_fields_and_idempotency_without_steering() -> None:
    release = asyncio.Event()
    captured_messages: list[list[object]] = []
    events: list[starweaver.StreamEvent] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"ok": True}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        if len(captured_messages) == 1:
            return {"tool_calls": [{"id": "call_wait", "name": "wait", "arguments": {}}]}
        assert "generic active note" not in str(messages)
        return {"text": "done"}

    async def run() -> None:
        async with (
            create_agent(model=FunctionModel(respond), tools=[wait]) as agent,
            agent.session() as session,
        ):
            async with session.run_stream("deploy") as agent_run:
                async for event in agent_run:
                    events.append(event)
                    if event.kind != "tool_call":
                        continue
                    first = await agent_run.messages.send(
                        "generic active note",
                        id="active-note",
                        topic="note",
                        target="worker",
                        template="Note: {{ content }}",
                        metadata={"priority": "low"},
                    )
                    second = await session.messages.send(
                        "generic active note duplicate",
                        id="active-note",
                        topic="note",
                        target="worker",
                        template="Note: {{ content }}",
                        metadata={"priority": "ignored"},
                    )
                    assert isinstance(first, starweaver.MessageDelivery)
                    assert isinstance(second, starweaver.MessageDelivery)
                    assert first.receipt is not None
                    assert second.receipt is not None
                    assert first.kind == "message"
                    assert second.kind == "message"
                    assert first.id == second.id == "active-note"
                    assert first.receipt.kind == "message"
                    assert second.receipt.kind == "message"
                    release.set()
                result = await agent_run.result()
            state = session.export_full_state()
        assert result.output == "done"
        assert len(captured_messages) == 2
        submitted = [
            event.sideband_payload for event in events if event.sideband_kind == "message_submitted"
        ]
        assert submitted == [{"id": "active-note", "queued_id": "active-note", "topic": "note"}]
        assert "steering_received" not in {
            event.sideband_kind for event in events if event.kind == "custom"
        }
        messages = state["message_bus"]["messages"]
        assert len([message for message in messages if message["id"] == "active-note"]) == 1
        message = next(message for message in messages if message["id"] == "active-note")
        assert message["source"] == "application"
        assert message["target"] == "worker"
        assert message["template"] == "Note: {{ content }}"
        assert message["metadata"]["starweaver.topic"] == "note"
        assert message["metadata"]["priority"] == "low"

    asyncio.run(run())


def test_user_source_message_requires_steering_topic_to_steer_active_run() -> None:
    release = asyncio.Event()
    captured_messages: list[list[object]] = []
    events: list[starweaver.StreamEvent] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"ok": True}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        if len(captured_messages) == 1:
            return {"tool_calls": [{"id": "call_wait", "name": "wait", "arguments": {}}]}
        rendered = str(messages)
        assert "Steering update from the user" not in rendered
        return {"text": "done"}

    async def run() -> None:
        async with (
            create_agent(model=FunctionModel(respond), tools=[wait]) as agent,
            agent.run_stream("deploy") as agent_run,
        ):
            async for event in agent_run:
                events.append(event)
                if event.kind == "tool_call":
                    sent = await agent_run.messages.send(
                        "explicit user note",
                        id="user-note",
                        topic="note",
                        source="user",
                        target="main",
                    )
                    assert isinstance(sent, starweaver.MessageDelivery)
                    assert sent.receipt is not None
                    assert sent.kind == "message"
                    release.set()
            result = await agent_run.result()
            state = await agent_run.recoverable_state()
        assert result.output == "done"
        assert len(captured_messages) == 2
        assert "message_submitted" in {
            event.sideband_kind for event in events if event.kind == "custom"
        }
        assert "steering_received" not in {
            event.sideband_kind for event in events if event.kind == "custom"
        }
        messages = state["message_bus"]["messages"]
        assert any(message["id"] == "user-note" for message in messages)

    asyncio.run(run())


def test_late_steering_during_output_validation_reaches_guard() -> None:
    entered_validator = asyncio.Event()
    release_validator = asyncio.Event()
    captured_messages: list[list[object]] = []
    attempts = 0

    async def pause_first_output(ctx: starweaver.OutputContext, output: str) -> None:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            entered_validator.set()
            await release_validator.wait()

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        if len(captured_messages) == 1:
            return {"text": "ready"}
        assert "Steering update from the user" in str(messages)
        assert "Use the safe rollout path." in str(messages)
        return {"text": "done"}

    async def run() -> None:
        policy = OutputPolicy.text().with_validator(pause_first_output)
        async with (
            create_agent(model=FunctionModel(respond)) as agent,
            agent.run_stream("finalize", output_policy=policy) as agent_run,
        ):
            join_task = asyncio.create_task(agent_run.join())
            await asyncio.wait_for(entered_validator.wait(), timeout=2)
            receipt = await agent_run.steer("Use the safe rollout path.", id="late-1")
            assert receipt.id == "late-1"
            assert receipt.queued
            release_validator.set()
            stream_result = await join_task
        assert stream_result.result.output == "done"
        assert attempts == 2
        assert len(captured_messages) == 2
        assert any(event.kind == "steering_guard" for event in stream_result.events)
        assert "steering_submitted" in {
            event.sideband_kind for event in stream_result.events if event.kind == "custom"
        }

    asyncio.run(run())


def test_late_steering_during_output_function_validation_reaches_guard() -> None:
    entered_validator = asyncio.Event()
    release_validator = asyncio.Event()
    captured_messages: list[list[object]] = []
    output_calls: list[dict[str, object]] = []
    attempts = 0

    def final_answer(ctx: starweaver.OutputContext, args: dict[str, object]) -> dict[str, object]:
        assert ctx.run_id
        output_calls.append(args)
        return {"answer": args["answer"]}

    async def pause_first_output(
        ctx: starweaver.OutputContext,
        output: dict[str, object],
    ) -> None:
        assert ctx.run_id
        assert "answer" in output
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            entered_validator.set()
            await release_validator.wait()

    def respond(messages: list[object], _info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        if len(captured_messages) == 1:
            return {
                "tool_calls": [
                    {
                        "id": "call_final_1",
                        "name": "final_answer",
                        "arguments": {"answer": "ready"},
                    }
                ]
            }
        assert "Steering update from the user" in str(messages)
        assert "Use the safe rollout path." in str(messages)
        return {
            "tool_calls": [
                {
                    "id": "call_final_2",
                    "name": "final_answer",
                    "arguments": {"answer": "done"},
                }
            ]
        }

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
        policy = OutputPolicy().with_function(output_function).with_validator(pause_first_output)
        async with (
            create_agent(model=FunctionModel(respond), output_policy=policy) as agent,
            agent.run_stream("finalize") as agent_run,
        ):
            join_task = asyncio.create_task(agent_run.join())
            await asyncio.wait_for(entered_validator.wait(), timeout=2)
            receipt = await agent_run.steer("Use the safe rollout path.", id="late-output-fn")
            assert receipt.id == "late-output-fn"
            assert receipt.queued
            release_validator.set()
            stream_result = await join_task
        assert stream_result.result.structured_output == {"answer": "done"}
        assert attempts == 2
        assert len(output_calls) == 2
        assert len(captured_messages) == 2
        assert any(event.kind == "steering_guard" for event in stream_result.events)

    asyncio.run(run())


def test_suspended_stream_rejects_control_and_exposes_hitl_snapshot() -> None:
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
        async with create_agent(model=model, tools=[deploy]) as agent, agent.session() as session:
            resumed: starweaver.RunResult | None = None
            async with session.run_stream("deploy") as agent_run:
                async for event in agent_run:
                    if event.kind != "suspended":
                        continue
                    snapshot = await agent_run.hitl().snapshot()
                    assert snapshot.approvals
                    with pytest.raises(StateError, match="already completed"):
                        await agent_run.steer("too late")
                    decision = snapshot.approvals[0].approve(decided_by="pytest")
                    with pytest.raises(StateError, match="resume_collected"):
                        await agent_run.hitl().resume(approvals=[decision])
                    resumed = await agent_run.hitl().resume_collected(approvals=[decision])
                    break
            assert resumed is not None
            assert resumed.output == "deployed"
            assert (await agent_run.result()).output == "deployed"
            assert (await agent_run.join()).result.output == "deployed"

    asyncio.run(run())


def test_session_bound_stream_hitl_resume_collected_joins_suspended_run() -> None:
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
        async with (
            create_agent(model=model, tools=[deploy]) as agent,
            agent.session() as session,
            session.run_stream("deploy") as agent_run,
        ):
            async for event in agent_run:
                if event.kind != "suspended":
                    continue
                resumed = await agent_run.hitl().resume_collected(
                    approvals={"call_deploy": {"approved": True}}
                )
                assert resumed.output == "deployed"
                assert session.active_run is None
                assert (await agent_run.result()).output == "deployed"
                break

    asyncio.run(run())


def test_suspended_stream_break_without_join_is_finalized_by_session_exit() -> None:
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
        async with create_agent(model=model, tools=[deploy]) as agent:
            async with agent.session() as session:
                agent_run = session.run_stream("deploy")
                async for event in agent_run:
                    if event.kind == "suspended":
                        assert not agent_run.is_finished
                        break
            assert agent_run.is_finished
            snapshot = await session.hitl.snapshot()
            decision = snapshot.approvals[0].approve(decided_by="pytest")
            resumed = await session.hitl.resume(approvals=[decision])
            assert resumed.output == "deployed"

    asyncio.run(run())


def test_direct_stream_hitl_resume_collected_uses_hidden_session() -> None:
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
        async with (
            create_agent(model=model, tools=[deploy]) as agent,
            agent.run_stream("deploy") as agent_run,
        ):
            async for event in agent_run:
                if event.kind != "suspended":
                    continue
                snapshot = await agent_run.hitl().snapshot()
                decision = snapshot.approvals[0].approve(decided_by="pytest")
                resumed = await agent_run.hitl().resume_collected(approvals=[decision])
                assert resumed.output == "deployed"
                assert (await agent_run.result()).output == "deployed"
                break

    asyncio.run(run())


def test_terminal_run_rejects_new_steering() -> None:
    async def run() -> None:
        agent_run = create_agent(model=StarweaverTestModel.text("done")).run_stream("done")
        assert (await agent_run.result()).output == "done"
        with pytest.raises(StateError, match="already completed"):
            await agent_run.steer("too late")

    asyncio.run(run())
