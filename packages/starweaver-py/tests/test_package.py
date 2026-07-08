import ast
import asyncio
import importlib.util
import json
import os
import re
import sys
import tomllib
from collections.abc import Mapping
from pathlib import Path
from typing import Any, cast

import pytest
import starweaver
from pydantic import BaseModel
from starweaver import (
    AbstractToolset,
    ApprovalRequired,
    CallDeferred,
    Cancelled,
    CapabilityBundle,
    DockerEnvironmentProvider,
    EnvironmentProvider,
    ExecutionStatus,
    FunctionToolset,
    InputPart,
    InvalidArguments,
    McpPromptSpec,
    McpResourceSpec,
    McpSamplingSpec,
    McpSubscriptionSpec,
    McpToolset,
    McpToolSpec,
    McpTransport,
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
    PythonCapability,
    PythonDynamicToolset,
    PythonEnvironmentProvider,
    RequestParams,
    ResourceRef,
    RunStatus,
    RuntimeConfig,
    SecurityConfig,
    SessionStatus,
    ShellReviewConfig,
    SkillPackage,
    SkillRegistry,
    SkillSourceScope,
    StateError,
    StreamAdapter,
    StreamError,
    Subagent,
    Timeout,
    ToolConfig,
    ToolContext,
    ToolError,
    ToolLibrary,
    ToolProxyToolset,
    ToolResult,
    ToolSearchToolset,
    Toolset,
    ToolsetContext,
    ToolsetFactory,
    ToolsetIdValidation,
    ToolsetLifecyclePolicy,
    ToolsetLifecycleReport,
    ToolsetLifecycleState,
    ToolsetPreparation,
    create_agent,
    create_agent_runtime,
    environment_toolsets,
    filesystem_toolset,
    shell_toolset,
    tool,
    toolset_factory,
    validate_toolset_ids,
    validate_toolsets_for_durability,
)
from starweaver.testing import FunctionModel, sleep_echo
from starweaver.testing import TestModel as StarweaverTestModel


def _public_api_groups(root: Path) -> dict[str, list[str]]:
    checklist = root / "spec" / "sdk" / "python" / "12-api-compatibility-checklist.md"
    content = checklist.read_text()
    start = content.index("<!-- public-api-groups:start -->")
    end = content.index("<!-- public-api-groups:end -->")
    block = content[start:end]
    code = block.split("```python", 1)[1].split("```", 1)[0]
    module = ast.parse(code, filename=str(checklist))
    groups: object | None = None
    for statement in module.body:
        if not isinstance(statement, ast.Assign):
            continue
        if any(
            isinstance(target, ast.Name) and target.id == "PUBLIC_API_GROUPS"
            for target in statement.targets
        ):
            groups = ast.literal_eval(statement.value)
            break
    assert groups is not None
    assert isinstance(groups, dict)
    return cast(dict[str, list[str]], groups)


def test_version_matches_native_extension() -> None:
    assert starweaver.__version__ == starweaver.version()


def test_public_api_compatibility_checklist_matches_starweaver_exports() -> None:
    root = Path(__file__).resolve().parents[3]
    groups = _public_api_groups(root)

    expected: list[str] = []
    for names in groups.values():
        assert isinstance(names, list)
        expected.extend(names)

    duplicates = sorted({name for name in expected if expected.count(name) > 1})
    assert duplicates == []
    assert set(starweaver.__all__) == set(expected)
    assert len(starweaver.__all__) == len(set(starweaver.__all__))
    for name in expected:
        assert hasattr(starweaver, name), name
        if name != "__version__":
            assert not name.startswith("_"), name


def test_python_stability_docs_cover_public_api_checklist() -> None:
    root = Path(__file__).resolve().parents[3]
    groups = _public_api_groups(root)
    stability = root / "docs" / "python" / "stability.md"
    content = stability.read_text()
    start = content.index("<!-- stable-public-api:start -->")
    end = content.index("<!-- stable-public-api:end -->")
    block = content[start:end]
    valid_tokens = set(groups)

    for group, names in groups.items():
        assert f"`{group}`" in block
        valid_tokens.add(group)
        for name in names:
            assert f"`{name}`" in block, name
            valid_tokens.add(name)

    documented_tokens = set(re.findall(r"`([^`]+)`", block))
    assert documented_tokens <= valid_tokens


def test_python_markdown_snippets_compile() -> None:
    root = Path(__file__).resolve().parents[3]
    paths = [
        *sorted((root / "docs" / "python").glob("*.md")),
        *sorted((root / "spec" / "sdk" / "python").glob("*.md")),
    ]

    failures: list[str] = []
    for path in paths:
        content = path.read_text()
        for index, match in enumerate(re.finditer(r"```python\n(.*?)```", content, re.S), 1):
            code = match.group(1)
            try:
                compile(
                    code,
                    f"{path.relative_to(root)}:python-block-{index}",
                    "exec",
                    flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT,
                )
            except SyntaxError as error:
                failures.append(
                    f"{path.relative_to(root)} block {index}: {error.msg} at line {error.lineno}"
                )

    assert failures == []


def test_python_product_boundary_documents_current_package_shape() -> None:
    root = Path(__file__).resolve().parents[3]
    content = (root / "spec" / "sdk" / "python" / "01-product-boundary.md").read_text()
    package = root / "packages" / "starweaver-py"

    expected = [
        "Cargo.toml",
        "pyproject.toml",
        "test_package.py",
        *sorted(path.name for path in (package / "src").glob("*.rs")),
        *sorted(
            path.name
            for path in (package / "python" / "starweaver").iterdir()
            if path.is_file() and (path.suffix in {".py", ".pyi"} or path.name == "py.typed")
        ),
    ]

    for name in expected:
        assert name in content, name


def test_pyo3_unsafe_lint_exception_stays_local_to_python_package() -> None:
    root = Path(__file__).resolve().parents[3]
    root_manifest = root / "Cargo.toml"
    py_manifest = root / "packages" / "starweaver-py" / "Cargo.toml"

    root_cargo = tomllib.loads(root_manifest.read_text())
    assert root_cargo["workspace"]["exclude"] == ["packages/starweaver-py"]
    assert root_cargo["workspace"]["lints"]["rust"]["unsafe_code"] == "forbid"

    py_cargo = tomllib.loads(py_manifest.read_text())
    assert py_cargo["lints"]["rust"]["unsafe_code"] == "allow"

    checked_manifests = [
        root_manifest,
        *sorted((root / "crates").glob("*/Cargo.toml")),
        root / "xtask" / "Cargo.toml",
        py_manifest,
    ]
    allow_manifests: list[Path] = []
    for manifest in checked_manifests:
        cargo = tomllib.loads(manifest.read_text())
        unsafe_code = cargo.get("lints", {}).get("rust", {}).get("unsafe_code")
        if unsafe_code == "allow":
            allow_manifests.append(manifest.relative_to(root))

    assert allow_manifests == [Path("packages/starweaver-py/Cargo.toml")]


def _attribute_path(node: ast.AST) -> tuple[str, ...]:
    if isinstance(node, ast.Name):
        return (node.id,)
    if isinstance(node, ast.Attribute):
        return (*_attribute_path(node.value), node.attr)
    return ()


def _is_process_function(name: str) -> bool:
    return name in {"system", "popen"} or name.startswith(("exec", "spawn"))


def _imports_process_function(names: set[str]) -> bool:
    return any(_is_process_function(name) for name in names)


def _live_provider_env_names() -> set[str]:
    return {
        "_".join(("OPENAI", "API", "KEY")),
        "_".join(("ANTHROPIC", "API", "KEY")),
        "_".join(("GEMINI", "API", "KEY")),
        "_".join(("STARWEAVER", "PY", "PROVIDER", "MODEL")),
    }


def _rust_python_attach_blocks(source: str) -> list[str]:
    marker = "Python::attach("
    blocks: list[str] = []
    offset = 0
    while (start := source.find(marker, offset)) >= 0:
        brace = _rust_attach_brace_start(source, start)
        if brace is None:
            offset = start + len(marker)
            continue
        end = _rust_block_end(source, brace)
        if end is None:
            return blocks
        blocks.append(source[start:end])
        offset = end
    return blocks


def _rust_attach_brace_start(source: str, start: int) -> int | None:
    brace = source.find("{", start)
    if brace < 0:
        return None
    line_end = source.find("\n", start)
    if line_end >= 0 and line_end < brace:
        return None
    inline_end = source.find(");", start)
    if inline_end >= 0 and inline_end < brace:
        return None
    return brace


def _rust_block_end(source: str, brace: int) -> int | None:
    index = brace
    depth = 0
    while index < len(source):
        skip_to = _skip_rust_string_or_comment(source, index)
        if skip_to is not None:
            index = skip_to
            continue
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    return None


def _skip_rust_string(source: str, start: int) -> int:
    index = start + 1
    escaped = False
    while index < len(source):
        char = source[index]
        if escaped:
            escaped = False
        elif char == "\\":
            escaped = True
        elif char == '"':
            return index + 1
        index += 1
    return index


def _skip_rust_string_or_comment(source: str, index: int) -> int | None:
    char = source[index]
    next_char = source[index + 1] if index + 1 < len(source) else ""
    if char == '"':
        return _skip_rust_string(source, index)
    if char == "/" and next_char == "/":
        newline = source.find("\n", index + 2)
        return len(source) if newline < 0 else newline + 1
    if char == "/" and next_char == "*":
        end = source.find("*/", index + 2)
        return len(source) if end < 0 else end + 2
    return None


def test_core_python_path_has_no_binary_or_mcp_shortcuts() -> None:
    root = Path(__file__).resolve().parents[3]
    python_core_modules = [
        root / "packages" / "starweaver-py" / "python" / "starweaver" / "agent.py",
        root / "packages" / "starweaver-py" / "python" / "starweaver" / "tool.py",
        root / "packages" / "starweaver-py" / "python" / "starweaver" / "output.py",
        root / "packages" / "starweaver-py" / "python" / "starweaver" / "runtime.py",
        root / "packages" / "starweaver-py" / "python" / "starweaver" / "store.py",
    ]
    rust_core_modules = [
        root / "packages" / "starweaver-py" / "src" / "agent.rs",
        root / "packages" / "starweaver-py" / "src" / "tool.rs",
        root / "packages" / "starweaver-py" / "src" / "output.rs",
    ]

    for module_path in python_core_modules:
        source = module_path.read_text()
        lowered = source.lower()
        assert "mcp" not in lowered, module_path
        assert "starweaver-cli" not in lowered, module_path
        assert "starweaver_cli" not in lowered, module_path
        tree = ast.parse(source, filename=str(module_path))
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported_roots = {alias.name.split(".", 1)[0] for alias in node.names}
                assert imported_roots.isdisjoint({"subprocess", "shlex", "pty"}), module_path
            elif isinstance(node, ast.ImportFrom):
                module = node.module or ""
                names = {alias.name for alias in node.names}
                assert module not in {"subprocess", "asyncio.subprocess"}, module_path
                assert not (module == "os" and _imports_process_function(names)), module_path
                assert not (
                    module == "asyncio"
                    and names.intersection({"create_subprocess_exec", "create_subprocess_shell"})
                ), module_path
            elif isinstance(node, ast.Call):
                path = _attribute_path(node.func)
                assert path[0:1] != ("subprocess",), module_path
                assert path not in {
                    ("asyncio", "create_subprocess_exec"),
                    ("asyncio", "create_subprocess_shell"),
                    ("os", "system"),
                    ("os", "popen"),
                }, module_path
                assert not (path[0:1] == ("os",) and _is_process_function(path[-1])), module_path

    for module_path in rust_core_modules:
        source = module_path.read_text()
        lowered = source.lower()
        assert "mcp" not in lowered, module_path
        assert "starweaver-cli" not in lowered, module_path
        assert "starweaver_cli" not in lowered, module_path
        for token in ("std::process", "tokio::process", "Command::new"):
            assert token not in source, module_path


def test_python_validation_paths_do_not_require_live_provider_credentials() -> None:
    root = Path(__file__).resolve().parents[3]
    package_tests = root / "packages" / "starweaver-py" / "tests" / "test_package.py"
    wheel_smoke_script = root / "scripts" / "python_wheel_smoke.py"
    deterministic_smoke_examples = [
        root / "examples" / "python" / "claw_like_runtime.py",
        root / "examples" / "python" / "claw_product_runtime.py",
    ]
    provider_smoke = root / "examples" / "python" / "provider_smoke.py"
    live_env_names = _live_provider_env_names()

    package_test_source = package_tests.read_text()
    for env_name in live_env_names:
        assert env_name not in package_test_source, env_name
    assert "STARWEAVER_TEST_API_KEY" in package_test_source

    wheel_smoke_source = wheel_smoke_script.read_text()
    assert "provider_smoke.py" not in wheel_smoke_source
    assert "ProviderModel" not in wheel_smoke_source
    for env_name in live_env_names:
        assert env_name not in wheel_smoke_source, env_name

    for example in deterministic_smoke_examples:
        source = example.read_text()
        assert "ProviderModel" not in source, example
        assert "ProviderAuth" not in source, example
        for env_name in live_env_names:
            assert env_name not in source, example

    provider_source = provider_smoke.read_text()
    assert "ProviderModel" in provider_source
    assert "_".join(("STARWEAVER", "PY", "PROVIDER", "MODEL")) in provider_source


def test_pyright_covers_python_product_examples_and_wheel_smoke() -> None:
    root = Path(__file__).resolve().parents[3]
    pyproject = tomllib.loads((root / "pyproject.toml").read_text())
    include = set(pyproject["tool"]["pyright"]["include"])

    assert "packages/starweaver-py/python/starweaver" in include
    assert "packages/starweaver-py/tests" in include
    assert "examples/python" in include
    assert "scripts/python_wheel_smoke.py" in include


def test_python_callback_bridges_do_not_hold_gil_across_runtime_awaits() -> None:
    root = Path(__file__).resolve().parents[3]
    bridge_files = [
        root / "packages" / "starweaver-py" / "src" / "tool.rs",
        root / "packages" / "starweaver-py" / "src" / "toolset.rs",
        root / "packages" / "starweaver-py" / "src" / "output.rs",
        root / "packages" / "starweaver-py" / "src" / "environment.rs",
        root / "packages" / "starweaver-py" / "src" / "media.rs",
        root / "packages" / "starweaver-py" / "src" / "store.rs",
    ]

    for path in bridge_files:
        source = path.read_text()
        assert "run_coroutine_threadsafe" in source, path
        assert "future.unbind()" in source, path
        assert "tokio::time::interval" in source, path
        assert "tick.tick().await" in source, path
        assert 'future.call_method0(py, "done")' in source, path
        assert 'future.call_method0(py, "cancel")' in source, path
        assert "pyo3_async_runtimes" not in source, path
        for block in _rust_python_attach_blocks(source):
            assert ".await" not in block, path


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
        assert result.usage.total_tokens == result.raw_state["usage"]["total_tokens"]
        assert result.usage.is_empty()
        assert result.usage_snapshot.run_id == result.raw_state["run_id"]
        latest_usage = result.usage_snapshot.latest_usage
        assert latest_usage is not None
        assert latest_usage.total_tokens == 0
        assert result.trace.is_empty()

    asyncio.run(run())


def test_observability_helpers_wrap_usage_snapshots_and_trace_metadata() -> None:
    usage_snapshot = {
        "run_id": "run-1",
        "latest_usage": {
            "requests": 1,
            "input_tokens": 2,
            "cache_write_tokens": 3,
            "cache_read_tokens": 4,
            "output_tokens": 5,
            "total_tokens": 6,
            "tool_calls": 7,
        },
        "total_usage": {
            "requests": 2,
            "input_tokens": 4,
            "output_tokens": 10,
            "total_tokens": 14,
        },
        "estimate_pricing": {"amount_micros_usd": 123},
        "entries": [
            {
                "agent_id": "main",
                "agent_name": "main",
                "model_id": "test:test",
                "usage": {"requests": 2, "total_tokens": 14},
            }
        ],
        "agent_usages": {
            "main": {
                "agent_name": "main",
                "model_id": "test:test",
                "usage": {"requests": 2, "total_tokens": 14},
            }
        },
        "model_usages": {"test:test": {"requests": 2, "total_tokens": 14}},
        "model_estimate_pricing": {"test:test": {"amount_micros_usd": 123}},
    }
    event = starweaver.StreamEvent(
        {
            "event": {
                "kind": "custom",
                "event": {
                    "category": "usage",
                    "kind": "usage_snapshot",
                    "payload": usage_snapshot,
                },
            }
        }
    )

    snapshot = event.usage_snapshot
    assert snapshot is not None
    assert snapshot.run_id == "run-1"
    latest_usage = snapshot.latest_usage
    assert latest_usage is not None
    assert latest_usage.tool_calls == 7
    assert snapshot.total_usage.total_tokens == 14
    estimate_pricing = snapshot.estimate_pricing
    assert estimate_pricing is not None
    assert estimate_pricing.amount_usd == 0.000123
    assert snapshot.entries[0].agent_id == "main"
    assert snapshot.agent_usages["main"].usage.requests == 2
    assert snapshot.model_usages["test:test"].total_tokens == 14
    assert snapshot.model_estimate_pricing["test:test"].amount_micros_usd == 123

    model_event = starweaver.StreamEvent(
        {"event": {"kind": "model_response", "usage": {"requests": 1, "total_tokens": 8}}}
    )
    usage_record = model_event.usage_record
    assert usage_record is not None
    assert usage_record.total_tokens == 8
    adapter = StreamAdapter([event, model_event])
    assert adapter.usage_snapshots()[0]["run_id"] == "run-1"
    assert adapter.usage_snapshots()[1]["total_tokens"] == 8
    assert adapter.typed_usage_snapshots()[0].total_usage.total_tokens == 14

    trace = starweaver.TraceMetadata.from_state(
        {
            "trace_snapshot": {
                "trace_id": "trace-main",
                "span_id": "span-main",
                "parent_span_id": "span-parent",
                "trace_state": "state-main",
                "metadata": {"service": "tests"},
            }
        }
    )
    assert trace.trace_id == "trace-main"
    assert trace.span_id == "span-main"
    assert trace.parent_span_id == "span-parent"
    assert trace.trace_state == "state-main"
    assert trace.metadata == {"service": "tests"}

    metadata_only = starweaver.TraceMetadata.from_state({"metadata": {"audit_id": "run-1"}})
    assert metadata_only.trace_id is None
    assert metadata_only.metadata == {"audit_id": "run-1"}


def test_stream_event_projects_toolset_lifecycle_report() -> None:
    event = starweaver.StreamEvent(
        {
            "event": {
                "kind": "custom",
                "event": {
                    "category": "tool",
                    "kind": "toolset_initialized",
                    "payload": {
                        "name": "workspace",
                        "id": "workspace",
                        "state": "initialized",
                        "tool_count": 2,
                        "instruction_count": 1,
                        "metadata": {"source": "test"},
                    },
                },
            }
        }
    )

    report = event.toolset_lifecycle_report
    assert report is not None
    assert report.name == "workspace"
    assert report.id == "workspace"
    assert report.state is ToolsetLifecycleState.INITIALIZED
    assert report.event_kind == "toolset_initialized"
    assert report.tool_count == 2
    assert report.instruction_count == 1
    assert report.metadata == {"source": "test"}
    assert report.to_dict()["state"] == "initialized"

    adapter = StreamAdapter([event])
    assert adapter.toolset_lifecycle_reports() == [report]
    assert ToolsetLifecycleReport.from_sideband(event.sideband) == report


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


def test_per_run_trace_metadata_reaches_model_without_persisting_session_state() -> None:
    seen_contexts: list[dict[str, object]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, str]:
        context = info["context"]
        assert isinstance(context, dict)
        seen_contexts.append(context)
        return {"text": "traced"}

    async def run() -> None:
        async with create_agent(model=FunctionModel(respond)).new_session() as session:
            result = await session.run(
                "trace",
                trace_metadata={"audit_id": "run-1"},
            )
            assert result.output == "traced"
            assert result.trace_metadata.metadata["audit_id"] == "run-1"
            state = session.export_full_state()
            assert "audit_id" not in state.get("metadata", {})

        assert seen_contexts
        metadata = seen_contexts[0].get("llm_trace_metadata")
        assert isinstance(metadata, Mapping)
        assert metadata["audit_id"] == "run-1"

    asyncio.run(run())


def test_tool_context_exposes_run_context_and_run_overrides_without_persisting() -> None:
    observed: list[dict[str, object]] = []

    @tool
    async def inspect_context(ctx: ToolContext) -> dict[str, object]:
        raw_context = ctx.raw_context
        assert isinstance(raw_context, dict)
        metadata = raw_context["metadata"]
        assert isinstance(metadata, dict)
        assert metadata["session_marker"] == "session"
        assert metadata["run_marker"] == "run"
        assert ctx.agent_id
        assert ctx.session_id
        assert ctx.context_handle is not None
        assert ctx.context_handle.metadata["run_marker"] == "run"
        assert ctx.environment is not None
        resources_future = ctx.export_resources()
        assert resources_future is not None
        resources = await resources_future
        assert resources is not None
        tool_config = raw_context["tool_config"]
        assert isinstance(tool_config, dict)
        assert tool_config["view_relaxed_text_patterns"] == ["*.log"]
        security = raw_context["security"]
        assert isinstance(security, dict)
        shell_review = security["shell_review"]
        assert isinstance(shell_review, dict)
        assert shell_review["enabled"] is True
        assert shell_review["risk_threshold"] == "medium"
        observed.append(
            {
                "agent_id": ctx.agent_id,
                "session_id": ctx.session_id,
                "workspace_root": ctx.workspace_root,
            }
        )
        return {"ok": True}

    async def run() -> None:
        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [{"id": "call_inspect", "name": "inspect_context", "arguments": {}}]
                ),
                {"text": "done"},
            ]
        )
        environment = EnvironmentProvider.virtual(
            resources=[
                ResourceRef(
                    id="workspace",
                    uri="file:///workspace",
                    metadata={"kind": "workspace"},
                )
            ]
        )
        session = create_agent(model=model, tools=[inspect_context]).new_session(
            environment=environment
        )
        session.set_metadata("session_marker", "session")
        result = await session.run(
            "inspect",
            context_metadata={"run_marker": "run"},
            tool_config=ToolConfig(view_relaxed_text_patterns=["*.log"]),
            security=SecurityConfig(
                shell_review=ShellReviewConfig(
                    enabled=True,
                    risk_threshold="medium",
                )
            ),
        )
        assert result.output == "done"
        assert result.raw_state["metadata"]["run_marker"] == "run"
        full_state = session.export_full_state()
        assert full_state["metadata"] == {"session_marker": "session"}
        assert full_state.get("tool_config", {}).get("view_relaxed_text_patterns") != ["*.log"]
        assert full_state.get("security", {}) == {}

    asyncio.run(run())
    assert observed


def test_runtime_config_can_carry_tool_and_security_config() -> None:
    runtime_config = RuntimeConfig(
        context_window=42,
        tool_config=ToolConfig(view_relaxed_text_patterns=["*.md"]),
        security=SecurityConfig(shell_review={"enabled": True, "risk_threshold": "low"}),
    )
    payload = runtime_config.to_dict()
    assert payload["model_config"]["context_window"] == 42
    assert payload["tool_config"]["view_relaxed_text_patterns"] == ["*.md"]
    assert payload["security"]["shell_review"]["risk_threshold"] == "low"


def test_docker_environment_provider_is_public_python_provider(tmp_path: Path) -> None:
    provider = DockerEnvironmentProvider("alpine:latest", tmp_path)
    assert provider.id.startswith("docker-")
    assert provider.workspace == tmp_path.resolve()
    state = asyncio.run(provider.export_state())
    assert state["metadata"]["kind"] == "docker"
    assert state["metadata"]["image"] == "alpine:latest"
    asyncio.run(provider.write_text("/workspace/output.txt", "ok"))
    assert (tmp_path / "output.txt").read_text() == "ok"


def test_generic_metadata_does_not_become_provider_routing_settings() -> None:
    seen: list[dict[str, object]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, str]:
        seen.append(info)
        return {"text": "metadata isolated"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            model_settings=ModelSettings({"temperature": 0.2}),
            request_params=RequestParams(
                metadata={
                    "session_id": "generic-session",
                    "thread_id": "generic-thread",
                    "x-client-request-id": "generic-request",
                    "provider.codex.session_id": "metadata-session",
                }
            ),
        ).run(
            "metadata",
            trace_metadata={
                "session_id": "trace-session",
                "thread_id": "trace-thread",
                "x-client-request-id": "trace-request",
            },
        )
        assert result.output == "metadata isolated"

    asyncio.run(run())

    assert len(seen) == 1
    settings = seen[0]["settings"]
    assert isinstance(settings, dict)
    assert settings == {"temperature": 0.2}
    assert "provider_settings" not in settings
    params = seen[0]["params"]
    assert isinstance(params, dict)
    metadata = params["metadata"]
    assert isinstance(metadata, dict)
    assert metadata["session_id"] == "generic-session"
    assert metadata["thread_id"] == "generic-thread"
    assert metadata["x-client-request-id"] == "generic-request"
    context = seen[0]["context"]
    assert isinstance(context, dict)
    trace_metadata = context["llm_trace_metadata"]
    assert isinstance(trace_metadata, dict)
    assert trace_metadata["session_id"] == "trace-session"
    assert trace_metadata["thread_id"] == "trace-thread"


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


def test_output_function_retry_reenters_native_output_loop() -> None:
    calls: list[dict[str, object]] = []

    def final_answer(ctx: starweaver.OutputContext, args: dict[str, object]) -> dict[str, object]:
        assert ctx.run_id
        calls.append({"run_id": ctx.run_id, **args})
        if args["answer"] == "draft":
            raise OutputRetry("final answer needs revision")
        return {"answer": args["answer"]}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_final_draft",
                        "name": "final_answer",
                        "arguments": {"answer": "draft"},
                    }
                ]
            ),
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_final_done",
                        "name": "final_answer",
                        "arguments": {"answer": "done"},
                    }
                ]
            ),
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
        policy = OutputPolicy().with_function(output_function).with_retries(1)
        async with (
            create_agent(model=model, output_policy=policy) as agent,
            agent.run_stream("answer") as agent_run,
        ):
            stream_result = await agent_run.join()

        assert stream_result.result.structured_output == {"answer": "done"}
        assert [call["answer"] for call in calls] == ["draft", "done"]
        assert calls[0]["run_id"] == calls[1]["run_id"]
        assert len(model.captured_messages()) == 2
        assert any(event.kind == "output_retry" for event in stream_result.events)
        assert "final answer needs revision" in str(model.captured_messages()[1])

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


def test_python_capability_run_start_hook_updates_state() -> None:
    def mark_state(state: dict[str, object]) -> dict[str, object]:
        metadata = dict(cast(Mapping[str, object], state.get("metadata") or {}))
        metadata["python_capability"] = "run-start"
        return {**state, "metadata": metadata}

    bundle = CapabilityBundle(
        "hook-bundle",
        hooks=[PythonCapability("hook-start", on_run_start=mark_state)],
    )

    async def run() -> None:
        result = await create_agent(
            model=StarweaverTestModel.text("hooked"),
            capability_bundles=[bundle],
        ).run("hook")
        assert result.output == "hooked"
        assert result.raw_state["metadata"]["python_capability"] == "run-start"

    asyncio.run(run())


def test_python_capability_sync_hook_can_bind_outside_running_loop() -> None:
    def mark_state(state: dict[str, object]) -> dict[str, object]:
        metadata = dict(cast(Mapping[str, object], state.get("metadata") or {}))
        metadata["python_capability_bound"] = "outside-loop"
        return {**state, "metadata": metadata}

    agent = create_agent(
        model=StarweaverTestModel.text("hooked"),
        capability_bundles=[
            CapabilityBundle(
                "hook-bundle",
                hooks=[PythonCapability("hook-start", on_run_start=mark_state)],
            )
        ],
    )

    async def run() -> None:
        result = await agent.run("hook")
        assert result.output == "hooked"
        assert result.raw_state["metadata"]["python_capability_bound"] == "outside-loop"

    asyncio.run(run())


def test_python_capability_async_run_start_hook_updates_state() -> None:
    async def mark_state(state: dict[str, object]) -> dict[str, object]:
        await asyncio.sleep(0)
        metadata = dict(cast(Mapping[str, object], state.get("metadata") or {}))
        metadata["python_capability_async"] = "run-start"
        return {**state, "metadata": metadata}

    async def run() -> None:
        result = await create_agent(
            model=StarweaverTestModel.text("hooked"),
            capability_bundles=[
                CapabilityBundle(
                    "hook-bundle",
                    hooks=[PythonCapability("hook-start", on_run_start=mark_state)],
                )
            ],
        ).run("hook")
        assert result.output == "hooked"
        assert result.raw_state["metadata"]["python_capability_async"] == "run-start"

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
            instructions=["Preserve workspace paths exactly."],
        )
        assert toolset.tool_definitions()[0]["name"] == "lookup"
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset],
        ).run("use workspace")
        assert result.output == "done"
        assert "Preserve workspace paths exactly." in str(seen_messages[0])

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


def test_dynamic_abstract_toolset_prepares_tools_and_instructions() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    class WorkspaceToolset(AbstractToolset):
        name = "workspace"
        id = "workspace"

        def __init__(self) -> None:
            super().__init__()
            self.contexts: list[ToolsetContext] = []

        async def get_tools(self, ctx: ToolsetContext):
            self.contexts.append(ctx)
            return [lookup]

        async def get_instructions(self, ctx: ToolsetContext):
            return f"Use workspace for agent {ctx.agent_id}."

    seen_messages: list[list[object]] = []

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
        toolset = WorkspaceToolset()
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset],
        ).run("use workspace")
        assert result.output == "done"
        assert toolset.contexts
        assert toolset.contexts[0].run_id is not None
        assert "Use workspace for agent" in str(seen_messages[0])

    asyncio.run(run())


def test_dynamic_toolset_context_exposes_environment_resources_and_raw_state() -> None:
    @tool(description="Return a value from the context projection.")
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    class ContextProjectionToolset(AbstractToolset):
        name = "context_projection"
        id = "context_projection"

        def __init__(self) -> None:
            super().__init__()
            self.snapshots: list[dict[str, object]] = []

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            assert ctx.workspace_root == "."
            assert ctx.environment is not None
            assert await ctx.environment.read_text("README.md") == "context readme"
            resources = ctx.resources
            assert isinstance(resources, starweaver.ResourceRegistry)
            artifact = resources.get("artifact")
            assert artifact is not None
            assert artifact.kind == "media"
            raw_context = ctx.raw_context
            assert raw_context["run_id"] == ctx.run_id
            assert raw_context["conversation_id"] == ctx.conversation_id
            self.snapshots.append(
                {
                    "run_id": ctx.run_id,
                    "workspace_root": ctx.workspace_root,
                    "resource_uri": artifact.uri,
                }
            )
            return ToolsetPreparation(
                tools=[lookup],
                instructions=[f"Context projection for {artifact.uri}."],
            )

    async def run() -> None:
        toolset = ContextProjectionToolset()
        environment = EnvironmentProvider.virtual(
            id="context",
            files={"README.md": "context readme"},
            resources=[ResourceRef.typed("resource://artifact", kind="media", id="artifact")],
        )
        result = await create_agent(
            model=StarweaverTestModel.responses([{"text": "ready"}]),
            toolsets=[toolset],
            environment=environment,
        ).run("prepare context")
        assert result.output == "ready"
        assert len(toolset.snapshots) == 1
        assert isinstance(toolset.snapshots[0]["run_id"], str)
        assert toolset.snapshots[0]["workspace_root"] == "."
        assert toolset.snapshots[0]["resource_uri"] == "resource://artifact"

    asyncio.run(run())


def test_resource_base_classes_export_refs_and_explicit_state() -> None:
    class ArtifactResource(starweaver.ResumableResource, starweaver.InstructableResource):
        def get_instructions(self) -> str:
            return "Use artifact resources by URI."

        def get_toolsets(self) -> tuple[object, ...]:
            return ()

    resource = ArtifactResource(
        "resource://artifact",
        id="artifact",
        kind="media",
        metadata={"media_type": "image/png"},
    )

    ref = resource.to_ref()
    assert isinstance(resource, starweaver.BaseResource)
    assert ref.uri == "resource://artifact"
    assert ref.id == "artifact"
    assert ref.kind == "media"
    assert resource.kind == "media"
    assert resource.to_dict() == ref.to_dict()
    assert resource.export_state() == ref.to_dict()
    assert resource.get_instructions() == "Use artifact resources by URI."
    assert tuple(resource.get_toolsets()) == ()

    restored_resource = ArtifactResource.from_state(resource.export_state())
    assert restored_resource.to_ref() == ref

    registry = starweaver.ResourceRegistry([resource])
    assert registry.get("artifact") == ref


def test_resource_registry_state_round_trips_resource_refs() -> None:
    artifact = ResourceRef.typed(
        "resource://artifact",
        kind="media",
        id="artifact",
        metadata={"media_type": "image/png"},
    )
    registry = starweaver.ResourceRegistry([artifact])

    state = registry.state()
    assert isinstance(state, starweaver.ResourceRegistryState)
    assert state.to_list() == registry.to_state()
    assert state.to_dict() == {"resources": registry.to_state()}

    restored = starweaver.ResourceRegistry.from_state(state)
    restored_artifact = restored.get("artifact")
    assert restored_artifact is not None
    assert restored_artifact.uri == "resource://artifact"
    assert restored_artifact.kind == "media"
    assert restored_artifact.metadata["media_type"] == "image/png"

    legacy_restored = starweaver.ResourceRegistry.from_state(registry.to_state())
    assert legacy_restored.get("artifact") == restored_artifact

    dict_state = starweaver.ResourceRegistryState.from_raw(state.to_dict())
    assert dict_state.resources == state.resources
    assert dict_state.to_registry().to_state() == registry.to_state()


def test_resource_registry_factories_bind_environment_and_restore_live_resources() -> None:
    class ArtifactResource(starweaver.ResumableResource, starweaver.InstructableResource):
        def get_instructions(self) -> list[str]:
            return [f"Use artifact {self.uri}."]

        def get_toolsets(self) -> tuple[FunctionToolset, ...]:
            toolset = FunctionToolset("artifact_tools", id="artifact_tools")

            @toolset.tool_plain(name="artifact_uri")
            def artifact_uri() -> str:
                return self.uri

            return (toolset,)

    environment = EnvironmentProvider.virtual(
        id="resources",
        files={"README.md": "resource workspace"},
    )
    seen_environments: list[object] = []

    def build_resources(env: object) -> list[ArtifactResource]:
        seen_environments.append(env)
        return [
            ArtifactResource(
                "resource://artifact",
                id="artifact",
                kind="media",
                metadata={"media_type": "image/png"},
            )
        ]

    registry = starweaver.ResourceRegistry.from_factory(
        build_resources,
        environment=environment,
    )
    assert seen_environments == [environment]
    assert registry.get("artifact") == ResourceRef.typed(
        "resource://artifact",
        id="artifact",
        kind="media",
        metadata={"media_type": "image/png"},
    )
    assert isinstance(registry.live("artifact"), ArtifactResource)
    assert registry.instructions() == ["Use artifact resource://artifact."]
    assert [toolset.id for toolset in registry.toolsets()] == ["artifact_tools"]

    state = registry.state()
    assert isinstance(state.resources[0], ResourceRef)

    restored = starweaver.ResourceRegistry.restore(
        state,
        lambda snapshot, env: [ArtifactResource.from_state(snapshot.resources[0].to_dict())],
        environment=environment,
    )
    assert isinstance(restored.live("artifact"), ArtifactResource)
    assert restored.instructions() == ["Use artifact resource://artifact."]
    assert restored.to_state() == registry.to_state()


def test_python_dynamic_toolset_public_base_uses_native_bridge() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    class WorkspaceToolset(PythonDynamicToolset):
        name = "workspace_dynamic"
        id = "workspace_dynamic"

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            return ToolsetPreparation(
                tools=[lookup],
                instructions=[f"Dynamic workspace for run {ctx.run_id}."],
            )

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
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
        toolset = WorkspaceToolset()
        assert isinstance(toolset, AbstractToolset)
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset],
        ).run("use workspace")
        assert result.output == "done"

    asyncio.run(run())


def test_dynamic_abstract_toolset_prepare_and_lifecycle_hooks() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    class LifecycleToolset(AbstractToolset):
        name = "lifecycle"
        id = "lifecycle"

        def __init__(self) -> None:
            super().__init__()
            self.events: list[str] = []

        async def enter(self, ctx: ToolsetContext) -> None:
            self.events.append(f"enter:{ctx.run_step}")

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            self.events.append(f"prepare:{ctx.run_step}")
            return ToolsetPreparation(
                tools=[lookup],
                instructions=["Lifecycle tools are available."],
            )

        async def exit(self, ctx: ToolsetContext) -> None:
            self.events.append(f"exit:{ctx.run_step}")

    async def run() -> None:
        toolset = LifecycleToolset()
        result = await create_agent(
            model=StarweaverTestModel.responses(
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
                ]
            ),
            toolsets=[toolset],
        ).run("use lifecycle")
        assert result.output == "done"
        assert toolset.events == ["enter:0", "prepare:0", "prepare:1", "exit:2"]

    asyncio.run(run())


def test_dynamic_toolset_lifecycle_policy_controls_enter_and_exit() -> None:
    class LifecyclePolicyToolset(AbstractToolset):
        name = "policy"

        def __init__(self) -> None:
            super().__init__(
                lifecycle_policy=ToolsetLifecyclePolicy(
                    enter_before_prepare=False,
                    exit_after_run=False,
                )
            )
            self.events: list[str] = []

        async def enter(self, ctx: ToolsetContext) -> None:
            self.events.append("enter")

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            self.events.append("prepare")
            return ToolsetPreparation()

        async def exit(self, ctx: ToolsetContext) -> None:
            self.events.append("exit")

    async def run() -> None:
        toolset = LifecyclePolicyToolset()
        result = await create_agent(
            model=StarweaverTestModel.text("ready"),
            toolsets=[toolset],
        ).run("use policy")
        assert result.output == "ready"
        assert toolset.events == ["prepare"]

    asyncio.run(run())


def test_dynamic_toolset_lifecycle_policy_enforces_read_timeout() -> None:
    class SlowToolset(AbstractToolset):
        name = "slow"

        def __init__(self) -> None:
            super().__init__(
                lifecycle_policy=ToolsetLifecyclePolicy(
                    read_timeout_ms=1,
                    enter_before_prepare=False,
                    exit_after_run=False,
                )
            )

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            await asyncio.sleep(0.05)
            return ToolsetPreparation()

    async def run() -> None:
        with pytest.raises(starweaver.AgentError, match="timed out"):
            await create_agent(
                model=StarweaverTestModel.text("ready"),
                toolsets=[SlowToolset()],
            ).run("use slow")

    asyncio.run(run())


def test_dynamic_toolset_with_lifecycle_returns_native_toolset() -> None:
    class PreparedToolset(AbstractToolset):
        name = "prepared"

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            return ToolsetPreparation()

    policy = ToolsetLifecyclePolicy(enter_before_prepare=False, exit_after_run=False)

    async def run() -> None:
        wrapped = PreparedToolset().with_lifecycle(policy)
        result = await create_agent(
            model=StarweaverTestModel.text("ready"),
            toolsets=[wrapped],
        ).run("use prepared")
        assert result.output == "ready"
        assert isinstance(wrapped, Toolset)
        assert policy.to_dict()["enter_before_prepare"] is False

    asyncio.run(run())


def test_runtime_stream_exposes_toolset_lifecycle_reports() -> None:
    class ReportingToolset(AbstractToolset):
        name = "reporting"
        id = "reporting"

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            return ToolsetPreparation(instructions=[f"Reporting run {ctx.run_id}"])

    async def run() -> None:
        stream = create_agent(
            model=StarweaverTestModel.text("ready"),
            toolsets=[ReportingToolset()],
        ).run_stream("use reporting")
        result = await stream.join()
        assert result.result.output == "ready"

        reports = [
            event.toolset_lifecycle_report
            for event in result.events
            if event.toolset_lifecycle_report is not None
        ]
        assert [report.state for report in reports].count(ToolsetLifecycleState.INITIALIZED) >= 1
        assert reports[-1].state is ToolsetLifecycleState.CLOSED
        assert any(report.instruction_count == 1 for report in reports)
        assert StreamAdapter(result.events).toolset_lifecycle_reports() == reports

    asyncio.run(run())


def test_toolset_id_validation_reports_durable_identity_issues() -> None:
    class DynamicToolset(AbstractToolset):
        name = "workspace"
        id = "workspace"

    class MissingIdToolset(AbstractToolset):
        name = "missing"

    validation = validate_toolset_ids(
        [
            DynamicToolset(),
            Toolset("duplicate-id", id="workspace"),
            MissingIdToolset(),
            Toolset("blank-id", id=" "),
            Toolset("workspace", id="workspace-copy").to_native(),
        ]
    )

    assert isinstance(validation, ToolsetIdValidation)
    assert validation.ok is False
    assert [identity.name for identity in validation.identities] == [
        "workspace",
        "duplicate-id",
        "missing",
        "blank-id",
        "workspace",
    ]
    assert {issue.code for issue in validation.errors} == {
        "duplicate_id",
        "missing_id",
        "empty_id",
    }
    assert [issue.code for issue in validation.warnings] == ["duplicate_name"]
    with pytest.raises(ValueError, match="invalid durable toolset identities"):
        validation.raise_for_errors()

    relaxed = validate_toolset_ids([MissingIdToolset()], require_ids=False)
    assert relaxed.ok
    with pytest.raises(ValueError, match="durable toolsets require explicit non-empty ids"):
        relaxed.require_ids()

    durable = validate_toolsets_for_durability([DynamicToolset()])
    assert durable.require_ids() is durable
    assert durable.require_serializable_dynamic_state() is durable


def test_tool_library_validates_stable_ids() -> None:
    library = ToolLibrary(
        [
            Toolset("workspace", id="workspace"),
            Toolset("browser", id="browser"),
        ]
    )

    validation = library.validate_ids()
    assert validation.ok
    assert validation.to_dict()["identities"][0]["id"] == "workspace"


def test_session_archive_validates_required_toolset_ids_on_restore() -> None:
    class WorkspaceToolset(AbstractToolset):
        name = "workspace"
        id = "workspace"

    class BrowserToolset(AbstractToolset):
        name = "browser"
        id = "browser"

    async def run() -> None:
        agent = create_agent(
            model=StarweaverTestModel.text("ready"),
            toolsets=[WorkspaceToolset()],
        )
        session = agent.session()
        archive = starweaver.SessionArchive.from_session(session)
        assert archive.required_toolset_ids == ("workspace",)
        assert archive.to_dict()["required_toolset_ids"] == ["workspace"]
        assert starweaver.SessionArchive.from_dict(archive.to_dict()).required_toolset_ids == (
            "workspace",
        )
        assert (
            agent.session_from_archive(archive).export_state()["session_id"] == archive.session_id
        )

        store = starweaver.InMemorySessionStore()
        await store.save_archive(archive)
        loaded = await store.load_archive(str(archive.session_id))
        assert loaded.required_toolset_ids == ("workspace",)

        missing_agent = create_agent(
            model=StarweaverTestModel.text("ready"),
            toolsets=[BrowserToolset()],
        )
        with pytest.raises(StateError, match="missing from current agent: workspace"):
            missing_agent.session_from_archive(loaded)

    asyncio.run(run())


def test_session_archive_requires_durable_toolset_ids_before_persistence() -> None:
    class MissingIdToolset(AbstractToolset):
        name = "missing"

    async def run() -> None:
        agent = create_agent(
            model=StarweaverTestModel.text("ready"),
            toolsets=[MissingIdToolset()],
        )
        with pytest.raises(ValueError, match="durable toolsets require explicit non-empty ids"):
            starweaver.SessionArchive.from_session(agent.session())

    asyncio.run(run())


def test_restored_session_uses_current_profile_approval_policy() -> None:
    toolset = FunctionToolset("deployments", id="deployments")
    executed: list[str] = []

    @toolset.tool_plain
    def deploy() -> dict[str, bool]:
        executed.append("deploy")
        return {"ok": True}

    async def run() -> None:
        archived_agent = create_agent(
            model=StarweaverTestModel.text("archived"),
            toolsets=[toolset],
        )
        archive = starweaver.SessionArchive.from_session(archived_agent.session())
        assert archive.required_toolset_ids == ("deployments",)

        restored_agent = create_agent(
            model=StarweaverTestModel.responses(
                [
                    StarweaverTestModel.tool_call_response(
                        [{"id": "call_deploy", "name": "deploy", "arguments": {}}]
                    ),
                    {"text": "deployed"},
                ]
            ),
            toolsets=[toolset],
            approval_required_tools=["deploy"],
        )
        restored = restored_agent.session_from_archive(archive)
        waiting = await restored.run("deploy")
        assert waiting.is_waiting
        assert executed == []
        assert waiting.pending_approvals[0]["name"] == "deploy"

        approval_id = str(waiting.pending_approvals[0]["approval_id"])
        resumed = await restored.resume_after_hitl(approvals={approval_id: {"approved": True}})
        assert resumed.output == "deployed"
        assert executed == ["deploy"]

    asyncio.run(run())


def test_restored_session_rebinds_current_environment_provider() -> None:
    class PolicyToolset(AbstractToolset):
        name = "policy"
        id = "policy"

        def __init__(self) -> None:
            super().__init__()
            self.seen: list[str] = []

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            assert ctx.environment is not None
            self.seen.append(await ctx.environment.read_text("policy.txt"))
            return ToolsetPreparation(instructions=["Use the current environment policy."])

    async def run() -> None:
        old_toolset = PolicyToolset()
        archived_agent = create_agent(
            model=StarweaverTestModel.text("archived"),
            toolsets=[old_toolset],
            environment=EnvironmentProvider.virtual(
                id="old",
                files={"policy.txt": "old"},
            ),
        )
        archive = starweaver.SessionArchive.from_session(archived_agent.session())

        current_toolset = PolicyToolset()
        restored_agent = create_agent(
            model=StarweaverTestModel.text("current"),
            toolsets=[current_toolset],
        )
        restored = restored_agent.session_from_archive(
            archive,
            environment=EnvironmentProvider.virtual(
                id="current",
                files={"policy.txt": "current"},
            ),
        )
        result = await restored.run("check policy")
        assert result.output == "current"
        assert old_toolset.seen == []
        assert current_toolset.seen == ["current"]

    asyncio.run(run())


def test_function_toolset_decorators_defaults_and_dynamic_instructions() -> None:
    toolset = FunctionToolset(
        "functions",
        id="functions",
        instructions=["Return concise function results."],
        max_retries=2,
        timeout_ms=30_000,
        strict=True,
        sequential=True,
        metadata={"toolset": "functions"},
    )

    @toolset.tool(description="Look up a function value.", metadata={"tool": "lookup"})
    async def lookup(ctx: ToolContext, value: str) -> dict[str, object]:
        assert ctx.run_id
        return {"value": value, "run": ctx.run_id}

    @toolset.tool_plain(name="mode", description="Return the current mode.")
    def current_mode() -> str:
        return "review"

    @toolset.instructions
    async def dynamic_instruction(ctx: ToolsetContext) -> str:
        return f"Function toolset run: {ctx.run_id}"

    assert lookup.metadata == {"toolset": "functions", "tool": "lookup"}
    assert lookup.strict is True
    assert lookup.sequential is True
    assert lookup.timeout_ms == 30_000
    assert lookup.max_retries == 2
    assert current_mode.name == "mode"

    seen_messages: list[list[object]] = []
    seen_tools: list[list[dict[str, object]]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        seen_messages.append(messages)
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        seen_tools.append(tools)
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
        if len(messages) == 3:
            return {
                "tool_calls": [
                    {
                        "id": "call_mode",
                        "name": "mode",
                        "arguments": {},
                    }
                ]
            }
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset],
        ).run("use functions")
        assert result.output == "done"
        assert "Return concise function results." in str(seen_messages[0])
        assert "Function toolset run:" in str(seen_messages[0])
        assert {tool["name"] for tool in seen_tools[0]} == {"lookup", "mode"}

    asyncio.run(run())


def test_function_toolset_rejects_duplicate_names_and_plain_context() -> None:
    toolset = FunctionToolset("functions")

    @toolset.tool
    def lookup(value: str) -> str:
        return value

    with pytest.raises(ValueError, match="duplicate tool name"):

        @toolset.tool(name="lookup")
        def lookup_again(value: str) -> str:
            return value

    with pytest.raises(ValueError, match="tool_plain functions cannot accept ToolContext"):

        @toolset.tool_plain
        def invalid_plain(ctx: ToolContext, value: str) -> str:
            return value


def test_duplicate_tool_names_across_toolsets_fail_during_preparation() -> None:
    first = FunctionToolset("first", id="first")
    second = FunctionToolset("second", id="second")

    @first.tool_plain(name="lookup")
    def first_lookup(value: str) -> str:
        return value

    @second.tool_plain(name="lookup")
    def second_lookup(value: str) -> str:
        return value

    async def run() -> None:
        agent = create_agent(
            model=StarweaverTestModel.text("unused"),
            toolsets=[first, second],
        )
        with pytest.raises(starweaver.AgentError, match=r"duplicate tool name .*lookup"):
            await agent.run("prepare duplicate toolsets")

    asyncio.run(run())


def test_strict_tools_require_descriptions() -> None:
    def undocumented(value: str) -> str:
        return value

    with pytest.raises(ValueError, match="strict tool 'undocumented' requires a description"):
        tool(undocumented, strict=True)

    toolset = FunctionToolset("strict", strict=True)
    with pytest.raises(ValueError, match="strict tool 'undocumented' requires a description"):
        toolset.add_function(undocumented)

    @toolset.tool_plain(description="Echo the provided value.")
    def documented(value: str) -> str:
        return value

    assert documented.strict is True


def test_toolset_wrapper_methods_expose_native_combinators() -> None:
    @tool(metadata={"bundle": "workspace"})
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    @tool
    async def write_file(path: str, content: str) -> dict[str, object]:
        return {"path": path, "bytes": len(content)}

    async def run() -> None:
        base = Toolset(
            "workspace",
            id="workspace",
            tools=[lookup, write_file],
            instructions=["Preserve workspace paths exactly."],
            max_retries=2,
            timeout_ms=30_000,
        )

        prefixed = base.prefixed("ws")
        assert prefixed.name == "ws_workspace"
        assert prefixed.id == "ws_workspace"
        assert {definition["name"] for definition in prefixed.tool_definitions()} == {
            "ws_lookup",
            "ws_write_file",
        }
        assert prefixed.instruction_records()[0]["group"] == "ws_workspace"

        included = base.filtered(include=["lookup"])
        assert [definition["name"] for definition in included.tool_definitions()] == ["lookup"]
        excluded = base.filtered(exclude="write_file")
        assert [definition["name"] for definition in excluded.tool_definitions()] == ["lookup"]
        with pytest.raises(ValueError, match="include or exclude"):
            base.filtered(include=["lookup"], exclude=["write_file"])

        renamed = base.renamed({"lookup": "find"})
        renamed_definitions = {
            definition["name"]: definition for definition in renamed.tool_definitions()
        }
        assert set(renamed_definitions) == {"find", "write_file"}
        assert renamed_definitions["find"]["metadata"]["original_tool_name"] == "lookup"

        with_metadata = base.with_metadata(
            {"bundle": "workspace-wrapper", "scope": "product"},
            owner="python",
        )
        assert with_metadata.id == "workspace.metadata"
        metadata_definitions = {
            definition["name"]: definition for definition in with_metadata.tool_definitions()
        }
        assert metadata_definitions["lookup"]["metadata"] == {
            "bundle": "workspace-wrapper",
            "owner": "python",
            "scope": "product",
        }
        assert metadata_definitions["write_file"]["metadata"] == {
            "bundle": "workspace-wrapper",
            "owner": "python",
            "scope": "product",
        }

        approval = base.approval_required("*", reason="review before execution")
        approval_definitions = {
            definition["name"]: definition for definition in approval.tool_definitions()
        }
        assert approval_definitions["lookup"]["metadata"]["approval_required"] is True

        deferred = base.deferred("lookup", reason="external worker")
        deferred_definitions = {
            definition["name"]: definition for definition in deferred.tool_definitions()
        }
        assert deferred_definitions["lookup"]["metadata"]["deferred_call"] is True
        assert "deferred_call" not in deferred_definitions["write_file"].get("metadata", {})

    asyncio.run(run())


def test_abstract_toolset_wrapper_methods_prepare_dynamic_inventory() -> None:
    toolset = FunctionToolset(
        "workspace",
        id="workspace",
        instructions=["Preserve workspace paths exactly."],
    )

    @toolset.tool
    async def lookup(ctx: ToolContext, value: str) -> dict[str, object]:
        assert ctx.run_id
        return {"value": value, "run_id": ctx.run_id}

    @toolset.instructions
    async def dynamic_instruction(ctx: ToolsetContext) -> str:
        return f"Prepared workspace for {ctx.run_id}."

    seen_tools: list[list[dict[str, object]]] = []
    seen_messages: list[list[object]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        seen_messages.append(messages)
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        seen_tools.append(tools)
        if len(messages) == 1:
            return {
                "tool_calls": [
                    {
                        "id": "call_lookup",
                        "name": "ws_lookup",
                        "arguments": {"value": "ok"},
                    }
                ]
            }
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset.with_metadata(bundle="workspace").prefixed("ws")],
        ).run("use workspace")
        assert result.output == "done"
        assert {definition["name"] for definition in seen_tools[0]} == {"ws_lookup"}
        metadata = seen_tools[0][0]["metadata"]
        assert isinstance(metadata, dict)
        assert metadata["bundle"] == "workspace"
        assert "Preserve workspace paths exactly." in str(seen_messages[0])
        assert "Prepared workspace for" in str(seen_messages[0])

    asyncio.run(run())


def test_toolset_factory_prepares_dynamic_toolsets_and_caches_per_run() -> None:
    factory_calls: list[str | None] = []
    tool_calls: list[str] = []

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_lookup", "name": "lookup", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        agent = create_agent(model=model)

        @agent.toolset(id="workspace", per_run_step=False)
        def workspace(ctx: ToolsetContext) -> FunctionToolset:
            factory_calls.append(ctx.run_id)
            toolset = FunctionToolset("workspace", id="workspace")

            @toolset.tool_plain
            def lookup() -> str:
                tool_calls.append("lookup")
                return "ok"

            return toolset

        assert isinstance(workspace, ToolsetFactory)
        result = await agent.run("use workspace")
        assert result.output == "done"
        assert len(factory_calls) == 1
        assert factory_calls[0]
        assert tool_calls == ["lookup"]

    asyncio.run(run())


def test_toolset_factory_default_re_evaluates_each_run_step() -> None:
    factory_steps: list[int] = []
    seen_tools: list[list[str]] = []
    tool_calls: list[str] = []

    def build_step_tools(step: int) -> FunctionToolset:
        toolset = FunctionToolset(f"step_{step}", id=f"step_{step}")

        @toolset.tool_plain(name=f"step_{step}")
        def step_tool() -> str:
            tool_calls.append(f"step_{step}")
            return f"step {step}"

        return toolset

    @toolset_factory(id="step_factory")
    def step_tools(ctx: ToolsetContext) -> FunctionToolset:
        factory_steps.append(ctx.run_step)
        return build_step_tools(ctx.run_step)

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        names = [str(tool["name"]) for tool in tools]
        seen_tools.append(names)
        if len(messages) == 1:
            assert names == ["step_0"]
            return {"tool_calls": [{"id": "call_step_0", "name": "step_0", "arguments": {}}]}
        assert names == ["step_1"]
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[step_tools],
        ).run("use step tools")
        assert result.output == "done"
        assert factory_steps == [0, 1]
        assert seen_tools == [["step_0"], ["step_1"]]
        assert tool_calls == ["step_0"]

    asyncio.run(run())


def test_per_run_toolset_factory_accepts_sequence_and_preparation_toolsets() -> None:
    seen_messages: list[list[object]] = []
    seen_tools: list[list[dict[str, object]]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        seen_messages.append(messages)
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        seen_tools.append(tools)
        return {"text": "ready"}

    @toolset_factory(name="workspace_factory", id="workspace_factory")
    async def workspace_factory(ctx: ToolsetContext) -> ToolsetPreparation:
        read_tools = FunctionToolset("read_tools", id="read_tools")
        write_tools = FunctionToolset("write_tools", id="write_tools")

        @read_tools.tool_plain
        def read_file() -> str:
            return "read"

        @write_tools.tool_plain
        def write_file() -> str:
            return "write"

        return ToolsetPreparation(
            instructions=[f"Factory run: {ctx.run_id}"],
            toolsets=[read_tools, write_tools],
        )

    async def run() -> None:
        validation = validate_toolset_ids([workspace_factory])
        assert validation.ok
        result = await create_agent(model=FunctionModel(respond)).run(
            "use workspace",
            toolsets=[workspace_factory],
        )
        assert result.output == "ready"
        assert {definition["name"] for definition in seen_tools[0]} == {
            "read_file",
            "write_file",
        }
        assert "Factory run:" in str(seen_messages[0])

    asyncio.run(run())


def test_toolset_prepared_callback_filters_and_updates_definitions() -> None:
    toolset = FunctionToolset("workspace", id="workspace")
    callback_runs: list[str | None] = []
    tool_calls: list[str] = []
    seen_tools: list[list[dict[str, object]]] = []

    @toolset.tool_plain(description="Lookup workspace files")
    def lookup() -> str:
        tool_calls.append("lookup")
        return "ok"

    @toolset.tool_plain(description="Write workspace files")
    def write_file() -> str:
        tool_calls.append("write_file")
        return "ok"

    def prepare(
        ctx: ToolsetContext,
        definitions: list[dict[str, object]],
    ) -> list[dict[str, object]]:
        callback_runs.append(ctx.run_id)
        prepared: list[dict[str, object]] = []
        for definition in definitions:
            if definition["name"] != "lookup":
                continue
            updated = dict(definition)
            updated["description"] = "Prepared lookup"
            prepared.append(updated)
        return prepared

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        seen_tools.append(tools)
        if len(messages) == 1:
            return {"tool_calls": [{"id": "call_lookup", "name": "lookup", "arguments": {}}]}
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[toolset.prepared(prepare)],
        ).run("use workspace")
        assert result.output == "done"
        assert callback_runs[0]
        assert tool_calls == ["lookup"]
        assert [definition["name"] for definition in seen_tools[0]] == ["lookup"]
        assert seen_tools[0][0]["description"] == "Prepared lookup"

    asyncio.run(run())


def test_filtered_predicate_uses_prepared_tool_definitions() -> None:
    toolset = FunctionToolset("workspace", id="workspace")
    seen_tools: list[list[dict[str, object]]] = []

    @toolset.tool_plain(metadata={"scope": "read"})
    def read_file() -> str:
        return "read"

    @toolset.tool_plain(metadata={"scope": "write"})
    def write_file() -> str:
        return "write"

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        del messages
        params = info["params"]
        assert isinstance(params, dict)
        tools = params["tools"]
        assert isinstance(tools, list)
        seen_tools.append(tools)
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            toolsets=[
                toolset.filtered(
                    predicate=lambda definition: (
                        dict(definition.get("metadata") or {}).get("scope") == "read"
                    )
                )
            ],
        ).run("use workspace")
        assert result.output == "done"
        assert [definition["name"] for definition in seen_tools[0]] == ["read_file"]

    asyncio.run(run())


def test_dynamic_toolset_refresh_callback_runs_on_hitl_resume_prepare() -> None:
    calls: list[str] = []
    executed: list[str] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def deploy(ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        del ctx, args
        executed.append("deploy")
        return {"ok": True}

    class RefreshingToolset(AbstractToolset):
        name = "refreshing"
        id = "refreshing"

        async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
            calls.append(f"prepare:{ctx.run_id}")
            return ToolsetPreparation(tools=[deploy])

        async def refresh(self, ctx: ToolsetContext) -> ToolsetPreparation:
            calls.append(f"refresh:{ctx.run_id}")
            return ToolsetPreparation(tools=[deploy])

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_deploy", "name": "deploy", "arguments": {}}]
            ),
            {"text": "deployed"},
        ]
    )

    async def run() -> None:
        session = create_agent(
            model=model,
            toolsets=[RefreshingToolset().approval_required("*", reason="review deploy")],
        ).new_session()
        waiting = await session.run("deploy")
        assert waiting.is_waiting
        assert executed == []
        approval_id = str(waiting.pending_approvals[0]["approval_id"])
        resumed = await session.resume_after_hitl(approvals={approval_id: {"approved": True}})
        assert resumed.output == "deployed"
        assert executed == ["deploy"]
        assert [call.split(":", 1)[0] for call in calls[:2]] == ["prepare", "refresh"]
        assert calls[0].split(":", 1)[1] == calls[1].split(":", 1)[1]
        assert calls[2].startswith("prepare:")

    asyncio.run(run())


def test_abstract_toolset_approval_and_deferred_wrappers_preserve_hitl() -> None:
    async def run_approval() -> None:
        toolset = FunctionToolset("deployments", id="deployments")
        executed: list[str] = []

        @toolset.tool_plain
        def deploy() -> dict[str, bool]:
            executed.append("deploy")
            return {"ok": True}

        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [{"id": "call_deploy", "name": "deploy", "arguments": {}}]
                ),
                {"text": "deployed"},
            ]
        )
        session = create_agent(
            model=model,
            toolsets=[toolset.approval_required("*", reason="review deploy")],
        ).new_session()
        waiting = await session.run("deploy")
        assert waiting.is_waiting
        assert waiting.pending_approvals[0]["name"] == "deploy"
        assert executed == []
        approval_id = str(waiting.pending_approvals[0]["approval_id"])
        resumed = await session.resume_after_hitl(approvals={approval_id: {"approved": True}})
        assert resumed.output == "deployed"
        assert executed == ["deploy"]

    async def run_deferred() -> None:
        toolset = FunctionToolset("collectors", id="collectors")
        executed: list[str] = []

        @toolset.tool_plain
        def collect() -> dict[str, bool]:
            executed.append("collect")
            return {"ok": True}

        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [{"id": "call_collect", "name": "collect", "arguments": {}}]
                ),
                {"text": "collected"},
            ]
        )
        session = create_agent(
            model=model,
            toolsets=[toolset.deferred("*", reason="worker queue")],
        ).new_session()
        waiting = await session.run("collect")
        assert waiting.is_waiting
        assert waiting.pending_deferred[0]["name"] == "collect"
        assert executed == []
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
        assert executed == []

    asyncio.run(run_approval())
    asyncio.run(run_deferred())


def test_agent_approval_required_tools_wraps_registered_toolsets() -> None:
    async def run() -> None:
        toolset = FunctionToolset("deployments", id="deployments")
        executed: list[str] = []

        @toolset.tool_plain
        def deploy() -> dict[str, bool]:
            executed.append("deploy")
            return {"ok": True}

        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [{"id": "call_deploy", "name": "deploy", "arguments": {}}]
                ),
                {"text": "deployed"},
            ]
        )
        session = create_agent(
            model=model,
            toolsets=[toolset],
            approval_required_tools=["deploy"],
        ).new_session()
        waiting = await session.run("deploy")
        assert waiting.is_waiting
        assert waiting.pending_approvals[0]["name"] == "deploy"
        assert executed == []

        approval_id = str(waiting.pending_approvals[0]["approval_id"])
        resumed = await session.resume_after_hitl(approvals={approval_id: {"approved": True}})
        assert resumed.output == "deployed"
        assert executed == ["deploy"]

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


def test_tool_search_persists_loaded_state_and_sideband_events() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    async def run() -> None:
        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_search_missing",
                            "name": "tool_search",
                            "arguments": {"query": "zzzzzz"},
                        }
                    ]
                ),
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_search_workspace",
                            "name": "tool_search",
                            "arguments": {"query": "workspace"},
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
        session = create_agent(
            model=model,
            toolsets=[ToolSearchToolset([Toolset("workspace", id="workspace", tools=[lookup])])],
        ).new_session()
        stream = session.run_stream("search lookup")
        joined = await stream.join()

        assert joined.result.output == "done"
        full_state = session.export_full_state()
        assert full_state["tool_search_loaded_tools"] == ["lookup"]
        assert full_state["tool_search_loaded_namespaces"] == ["workspace"]

        sidebands = [event.sideband for event in joined.events if event.sideband is not None]
        sideband_kinds = [event["kind"] for event in sidebands]
        assert "tool_search_initialized" in sideband_kinds
        assert "tool_search_no_match" in sideband_kinds
        assert "tool_search_loaded" in sideband_kinds
        loaded = [event for event in sidebands if event["kind"] == "tool_search_loaded"][-1]
        assert loaded["payload"]["loaded_tools"] == ["lookup"]
        assert loaded["payload"]["loaded_namespaces"] == ["workspace"]
        no_match = [event for event in sidebands if event["kind"] == "tool_search_no_match"][-1]
        assert no_match["payload"]["error_kind"] == "no_match"

    asyncio.run(run())


def test_tool_proxy_scoped_prefix_uses_fixed_surface_and_records_loaded_state() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    captured_tool_names: list[list[str]] = []

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        params = info["params"]
        assert isinstance(params, dict)
        tools = params.get("tools")
        assert isinstance(tools, list)
        captured_tool_names.append([str(tool["name"]) for tool in tools])
        if len(messages) == 1:
            return {
                "tool_calls": [
                    {
                        "id": "call_proxy_search",
                        "name": "workspace_search_tool",
                        "arguments": {"query": "lookup"},
                    }
                ]
            }
        if len(messages) == 3:
            return {
                "tool_calls": [
                    {
                        "id": "call_proxy_lookup",
                        "name": "workspace_call_tool",
                        "arguments": {"name": "lookup", "arguments": {"value": "ok"}},
                    }
                ]
            }
        return {"text": "done"}

    async def run() -> None:
        session = create_agent(
            model=FunctionModel(respond),
            toolsets=[
                ToolProxyToolset(
                    [Toolset("workspace", id="workspace", tools=[lookup])],
                    prefix="workspace",
                )
            ],
        ).new_session()
        stream = session.run_stream("proxy lookup")
        joined = await stream.join()
        assert joined.result.output == "done"
        assert all("lookup" not in names for names in captured_tool_names)
        assert set(captured_tool_names[0]) == {"workspace_search_tool", "workspace_call_tool"}

        full_state = session.export_full_state()
        assert full_state["tool_search_loaded_tools"] == ["lookup"]
        assert full_state["tool_search_loaded_namespaces"] == ["workspace"]

        sidebands = [event.sideband for event in joined.events if event.sideband is not None]
        loaded = [event for event in sidebands if event["kind"] == "tool_search_loaded"]
        assert loaded
        assert loaded[-1]["payload"]["loaded_tools"] == ["lookup"]
        assert loaded[-1]["payload"]["loaded_namespaces"] == ["workspace"]

    asyncio.run(run())


def test_tool_proxy_uses_namespace_descriptions_for_search() -> None:
    @tool
    async def lookup(value: str) -> dict[str, str]:
        return {"value": value}

    async def run() -> None:
        model = StarweaverTestModel.responses(
            [
                StarweaverTestModel.tool_call_response(
                    [
                        {
                            "id": "call_proxy_search",
                            "name": "search_tools",
                            "arguments": {"query": "deployments"},
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
        session = create_agent(
            model=model,
            toolsets=[
                ToolProxyToolset(
                    [Toolset("workspace", id="workspace", tools=[lookup])],
                    namespace_descriptions={
                        "workspace": "Deployment operations exposed by the workspace runtime."
                    },
                )
            ],
        ).new_session()
        stream = session.run_stream("proxy deployment tool")
        joined = await stream.join()
        assert joined.result.output == "done"

        full_state = session.export_full_state()
        assert full_state["tool_search_loaded_tools"] == ["lookup"]
        assert full_state["tool_search_loaded_namespaces"] == ["workspace"]

    asyncio.run(run())


def test_mcp_toolset_config_exposes_deferred_native_tools() -> None:
    async def run() -> None:
        toolset = McpToolset(
            "github",
            transport=McpTransport.streamable_http("https://example.com/mcp"),
            headers={"authorization": "Bearer test"},
            tool_prefix="github",
            include_instructions=True,
            instructions="Prefer repository-grounded evidence for repository tasks.",
            tools=[
                McpToolSpec(
                    "search",
                    parameters={
                        "type": "object",
                        "properties": {"query": {"type": "string"}},
                        "required": ["query"],
                    },
                    description="Search repositories.",
                    task=True,
                    metadata={"scope": "repo"},
                )
            ],
            resources=[
                McpResourceSpec(
                    "resource://github/repository",
                    name="repository",
                    mime_type="application/json",
                )
            ],
            prompts=[
                McpPromptSpec(
                    "triage",
                    arguments={
                        "type": "object",
                        "properties": {"issue": {"type": "string"}},
                    },
                )
            ],
            sampling=McpSamplingSpec(metadata={"owner": "product"}),
            subscriptions=[McpSubscriptionSpec("repo-updates", "resource://github/repository")],
        )

        assert toolset.to_dict()["transport"]["StreamableHttp"]["headers"] == {
            "authorization": "Bearer test"
        }
        definitions = toolset.tool_definitions()
        assert definitions[0]["name"] == "github_search"
        assert definitions[0]["description"] == "Search repositories."
        assert definitions[0]["metadata"]["mcp_server_id"] == "github"
        assert definitions[0]["metadata"]["mcp_transport"] == "streamable_http"
        assert definitions[0]["metadata"]["mcp_tool_name"] == "search"
        assert definitions[0]["metadata"]["mcp_task"] is True
        assert definitions[0]["metadata"]["scope"] == "repo"
        instructions = toolset.instruction_records()
        assert instructions[0]["group"] == "mcp:github"
        assert "repository-grounded evidence" in instructions[0]["content"]

        session = create_agent(
            model=StarweaverTestModel.responses(
                [
                    StarweaverTestModel.tool_call_response(
                        [
                            {
                                "id": "call_mcp_search",
                                "name": "github_search",
                                "arguments": {"query": "starweaver"},
                            }
                        ]
                    ),
                    {"text": "searched"},
                ]
            ),
            toolsets=[toolset],
        ).new_session()
        waiting = await session.run("search GitHub")
        assert waiting.is_waiting
        pending = waiting.pending_deferred[0]
        assert pending["name"] == "github_search"
        request = pending["metadata"]["deferred"]
        assert request["kind"] == "mcp_tool_call"
        assert request["server_id"] == "github"
        assert request["transport"]["StreamableHttp"]["url"] == ("https://example.com/mcp")
        assert request["tool_name"] == "search"
        assert request["arguments"] == {"query": "starweaver"}
        deferred_id = str(pending["deferred_id"])
        resumed = await session.resume_after_hitl(
            deferred_results={
                "results": [
                    {
                        "deferred_id": deferred_id,
                        "status": "completed",
                        "response": {"items": []},
                    }
                ]
            }
        )
        assert resumed.output == "searched"

        with pytest.raises(ValueError, match="does not accept HTTP headers"):
            McpToolset(
                "local",
                transport=McpTransport.stdio("mcp-server"),
                headers={"authorization": "Bearer test"},
            )

    asyncio.run(run())


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


def test_runtime_config_mapping_is_strict_and_normalized() -> None:
    with pytest.raises(TypeError, match="unknown runtime_config field: temperature"):
        create_agent(
            model=StarweaverTestModel.text("unused"),
            runtime_config={"temperature": 0.1},
        )

    with pytest.raises(TypeError, match="appears both"):
        create_agent(
            model=StarweaverTestModel.text("unused"),
            runtime_config={
                "model_config": {"context_window": 1000},
                "context_window": 2000,
            },
        )

    with pytest.raises(TypeError, match="capabilities"):
        RuntimeConfig(capabilities="image_url").to_model_config()

    async def run() -> None:
        async with create_agent(
            model=StarweaverTestModel.text("configured"),
            runtime_config={
                "model_config": {
                    "context_window": 2048,
                    "compact_threshold": {"per_thousand": 875},
                    "stream_resume_on_error": True,
                    "capabilities": ["image_url"],
                }
            },
        ).session() as session:
            result = await session.run("configured")
            assert result.output == "configured"
            state = session.export_full_state()
        assert state["model_config"]["context_window"] == 2048
        assert state["model_config"]["compact_threshold"]["per_thousand"] == 875
        assert state["model_config"]["stream_resume_on_error"] is True
        assert state["model_config"]["capabilities"] == ["image_url"]

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


def test_provider_model_from_model_id_supports_cli_gateway_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    websocket = ProviderModel.from_model_id("openai-responses-ws:gpt-test", api_key="sk-test")
    assert websocket.to_native() is not None

    gateway = ProviderModel.from_model_id(
        "homelab@openai-responses-ws:gpt-test",
        api_key="gateway-test",
        base_url="https://gateway.example/v1",
    )
    assert gateway.to_native() is not None

    monkeypatch.setenv("HOMELAB_API_KEY", "gateway-env-test")
    monkeypatch.setenv("HOMELAB_BASE_URL", "https://gateway-env.example/v1")
    gateway_from_env = ProviderModel.from_model_id("homelab@openai-responses-ws:gpt-test")
    assert gateway_from_env.to_native() is not None

    google_alias = ProviderModel.from_model_id("google-cloud:gemini-test", api_key="google-test")
    assert google_alias.to_native() is not None


def test_provider_model_openai_facade_selects_protocols_and_auth() -> None:
    responses = ProviderModel.openai(
        "gpt-test",
        api_key="sk-test",
        stream_transport="websocket",
    )
    assert responses.to_native() is not None

    chat = ProviderModel.openai(
        "gpt-test",
        protocol="chat",
        auth=ProviderAuth.openai(api_key="sk-test"),
    )
    assert chat.to_native() is not None

    with pytest.raises(ValueError, match="stream_transport"):
        ProviderModel.openai(
            "gpt-test",
            protocol="chat",
            api_key="sk-test",
            stream_transport="websocket",
        )

    with pytest.raises(ValueError, match="protocol"):
        ProviderModel.openai("gpt-test", protocol="legacy", api_key="sk-test")

    with pytest.raises(
        ValueError,
        match=re.escape("ProviderModel.openai requires ProviderAuth.openai"),
    ):
        ProviderModel.openai(
            "gpt-test",
            auth=ProviderAuth.anthropic(api_key="anthropic-test"),
        )


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


def test_provider_auth_oauth_status_helpers_redact_token_material(tmp_path: Path) -> None:
    auth_file = tmp_path / "auth.json"
    auth_file.write_text(
        json.dumps(
            {
                "version": 1,
                "providers": {
                    "codex": {
                        "type": "oauth2",
                        "issuer": "https://auth.openai.com",
                        "client_id": "client-test",
                        "token_endpoint": "https://auth.openai.com/oauth/token",
                        "revoke_endpoint": "https://auth.openai.com/oauth/revoke",
                        "base_url": "https://chatgpt.com/backend-api/codex",
                        "scopes": ["openid", "profile", "email"],
                        "tokens": {
                            "id_token": "id-secret",
                            "access_token": "access-secret",
                            "refresh_token": "refresh-secret",
                        },
                        "account": {
                            "email": "agent@example.com",
                            "chatgpt_user_id": "user-test",
                            "chatgpt_account_id": "acct-test",
                            "chatgpt_plan_type": "team",
                            "chatgpt_account_is_fedramp": False,
                        },
                        "last_refresh_at": "2026-01-01T00:00:00Z",
                    }
                },
            }
        ),
        encoding="utf-8",
    )

    auth = ProviderAuth.codex_oauth(auth_file=auth_file)
    status = auth.status()
    assert status["provider_name"] == "codex"
    assert status["logged_in"] is True
    assert status["has_access_token"] is True
    assert status["has_refresh_token"] is True
    assert status["account"] == {
        "email": "agent@example.com",
        "chatgpt_user_id": "user-test",
        "chatgpt_account_id": "acct-test",
        "chatgpt_plan_type": "team",
        "chatgpt_account_is_fedramp": False,
    }
    assert auth.account_metadata() == status["account"]

    redacted = auth.redacted_record()
    assert redacted is not None
    assert redacted["tokens"] == {
        "id_token": "<redacted>",
        "access_token": "<redacted>",
        "refresh_token": "<redacted>",
    }
    serialized_status = repr(status)
    serialized_redacted = repr(redacted)
    for secret in ("id-secret", "access-secret", "refresh-secret"):
        assert secret not in serialized_status
        assert secret not in serialized_redacted

    assert ProviderModel.codex_oauth("gpt-5.5", auth=auth).to_native() is not None

    missing = ProviderAuth.codex_oauth(auth_file=tmp_path / "missing.json").status()
    assert missing["logged_in"] is False
    assert missing["account"] is None

    api_key_status = ProviderAuth.openai(api_key="sk-secret", api_key_env=None).status()
    assert api_key_status == {
        "provider_name": "openai",
        "auth_type": "api_key",
        "api_key_env": None,
        "has_inline_api_key": True,
    }
    assert "sk-secret" not in repr(api_key_status)


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
        assert isinstance(environment.files, starweaver.FileOperator)
        assert isinstance(environment.shell, starweaver.Shell)
        assert await environment.files.read("README.md") == "workspace readme"
        typed_listing = await environment.files.list_dir_with_types("")
        assert any(entry["name"] == "README.md" and entry["is_file"] for entry in typed_listing)
        walked = [entry async for entry in environment.files.walk_files("", max_results=10)]
        assert any(entry["path"] == "README.md" for entry in walked)
        tmp_ref = await environment.files.truncate_to_tmp("spilled", suffix=".txt")
        assert tmp_ref.kind == "file"
        assert await environment.read_text(str(tmp_ref.metadata["path"])) == "spilled"
        assert (await environment.shell.execute("echo ok"))["stdout"] == "ok\n"
        process = await environment.shell.start("sleep 5", cwd="/workspace")
        assert isinstance(process, starweaver.ShellProcess)
        assert process.process_id.startswith("process_")
        assert process.command == "sleep 5"
        assert process.running
        assert process.metadata["cwd"] == "/workspace"
        assert (await environment.shell.wait_process(process)).process_id == process.process_id
        assert any(
            snapshot.process_id == process.process_id
            for snapshot in await environment.shell.list_processes()
        )
        with_input = await environment.shell.write_stdin(
            process.process_id,
            "continue",
            close_stdin=True,
        )
        assert with_input.metadata["last_input"] == "continue"
        assert with_input.metadata["close_stdin"] is True
        signaled = await environment.shell.send_signal(process, "TERM")
        assert signaled.metadata["last_signal"] == 15
        killed = await environment.shell.kill_process(process)
        assert killed.status == "killed"
        assert killed.terminal
        assert killed.to_dict()["process_id"] == process.process_id
        grep = await environment.grep("workspace", include="**/*.md")
        assert grep[0]["path"] == "README.md"
        state = await environment.export_state()
        assert state["provider_id"] == "skills"
        assert state["resources"][0]["metadata"]["resource_kind"] == "media"
        assert any(
            snapshot["process_id"] == process.process_id and snapshot["status"] == "killed"
            for snapshot in state["processes"]
        )

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


def test_named_environment_facades_wrap_native_providers(tmp_path: Path) -> None:
    async def run() -> None:
        virtual = starweaver.VirtualEnvironment(
            id="named-virtual",
            files={"README.md": "virtual"},
            shell_outputs={"echo virtual": "virtual\n"},
        )
        assert isinstance(virtual, starweaver.Environment)
        assert isinstance(virtual, EnvironmentProvider)
        assert virtual.id == "named-virtual"
        assert await virtual.read_text("README.md") == "virtual"
        assert (await virtual.shell.execute("echo virtual"))["stdout"] == "virtual\n"

        workspace = tmp_path / "workspace"
        workspace.mkdir()
        (workspace / "README.md").write_text("local")
        local = starweaver.LocalEnvironment(
            workspace,
            id="named-local",
            allowed_paths=[workspace],
            context_file_tree_roots=[workspace],
            writable=True,
        )
        assert isinstance(local, starweaver.Environment)
        assert local.id == "named-local"
        assert await local.read_text("README.md") == "local"
        await local.write_text("generated.txt", "ok")
        assert (workspace / "generated.txt").read_text() == "ok"

        envd = starweaver.EnvdEnvironment.from_local(
            virtual,
            environment_id="named-envd",
            id="named-envd-provider",
        )
        assert isinstance(envd, starweaver.EnvdEnvironment)
        assert isinstance(envd, starweaver.Environment)
        assert envd.id == "named-envd-provider"
        assert await envd.read_text("README.md") == "virtual"
        await envd.write_text("envd.txt", "via envd")
        assert await virtual.read_text("envd.txt") == "via envd"
        state = await envd.export_state()
        assert state["metadata"]["envd_kind"] == "local"
        assert state["metadata"]["envd_environment_id"] == "named-envd"

    asyncio.run(run())


class _MemoryEnvironmentProvider(PythonEnvironmentProvider):
    def __init__(self) -> None:
        super().__init__(id="python-memory")
        self.files = {"README.md": "hello"}
        self.dirs = {""}

    def _normalize(self, path: str) -> str:
        return path.strip("/")

    async def read_text(self, path: str) -> str:
        path = self._normalize(path)
        if path not in self.files:
            raise FileNotFoundError(path)
        return self.files[path]

    async def write_text(self, path: str, content: str) -> None:
        path = self._normalize(path)
        parent = path.rsplit("/", 1)[0] if "/" in path else ""
        self.dirs.add(parent)
        self.files[path] = content

    async def create_dir(self, path: str, parents: bool = True) -> None:
        path = self._normalize(path)
        if parents:
            current = ""
            for part in path.split("/"):
                current = part if not current else f"{current}/{part}"
                self.dirs.add(current)
        else:
            self.dirs.add(path)

    async def delete_path(self, path: str, recursive: bool = False) -> None:
        path = self._normalize(path)
        if path in self.files:
            del self.files[path]
            return
        if path in self.dirs:
            if not recursive and any(name.startswith(f"{path}/") for name in self.files):
                raise PermissionError("directory is not empty")
            self.dirs.discard(path)
            for name in list(self.files):
                if name.startswith(f"{path}/"):
                    del self.files[name]
            return
        raise FileNotFoundError(path)

    async def copy_path(self, src: str, dst: str, overwrite: bool = False) -> None:
        src = self._normalize(src)
        dst = self._normalize(dst)
        if not overwrite and dst in self.files:
            raise FileExistsError(dst)
        await self.write_text(dst, await self.read_text(src))

    async def stat(self, path: str) -> dict[str, object]:
        path = self._normalize(path)
        if path in self.files:
            return {
                "size": len(self.files[path].encode()),
                "is_file": True,
                "is_dir": False,
            }
        if path in self.dirs:
            return {"size": 0, "is_file": False, "is_dir": True}
        raise FileNotFoundError(path)

    async def list(self, path: str = "") -> list[str]:
        path = self._normalize(path)
        prefix = "" if not path else f"{path}/"
        entries = [name for name in self.files if not path or name.startswith(prefix)]
        entries.extend(name for name in self.dirs if name and (not path or name.startswith(prefix)))
        return sorted(entries)

    async def run_shell(self, command: Mapping[str, Any]) -> dict[str, object]:
        return {
            "status": 0,
            "stdout": f"python:{command['command']}",
            "stderr": "",
            "metadata": {"provider": self.id, "cwd": command.get("cwd")},
        }

    async def render_context(self) -> str:
        return '<environment id="python-memory">memory context</environment>'

    async def export_state(self) -> dict[str, object]:
        return {"provider_id": self.id, "files": dict(self.files)}


def test_python_environment_provider_adapts_to_native_trait() -> None:
    async def run() -> None:
        backing = _MemoryEnvironmentProvider()
        environment = EnvironmentProvider.from_python(backing)

        assert environment.id == "python-memory"
        assert await environment.read_text("README.md") == "hello"
        await environment.create_dir("notes", parents=True)
        await environment.write_text("notes/item.txt", "item")
        assert await environment.read_bytes("notes/item.txt", offset=1, length=2) == b"te"
        assert (await environment.stat("notes"))["is_dir"] is True
        assert "notes/item.txt" in await environment.list("")

        await environment.copy_path("notes/item.txt", "notes/copy.txt", overwrite=True)
        await environment.move_path("notes/copy.txt", "notes/moved.txt", overwrite=True)
        assert await environment.read_text("notes/moved.txt") == "item"
        with pytest.raises(StateError, match="not found"):
            await environment.read_text("notes/copy.txt")

        tmp_path = await environment.write_tmp_file("spill.txt", b"spilled")
        assert tmp_path == ".tmp/spill.txt"
        assert await environment.read_text(tmp_path) == "spilled"

        shell = await environment.run_shell("echo bridged", cwd="/workspace")
        assert shell["stdout"] == "python:echo bridged"
        assert shell["metadata"]["cwd"] == "/workspace"
        assert (
            await environment.render_context()
            == '<environment id="python-memory">memory context</environment>'
        )
        state = await environment.export_state()
        assert state["provider_id"] == "python-memory"
        assert state["files"]["notes/moved.txt"] == "item"

    asyncio.run(run())


def test_local_environment_separates_allowed_paths_from_context_roots(tmp_path: Path) -> None:
    async def run() -> None:
        workspace = tmp_path / "workspace"
        cache = tmp_path / "cache"
        workspace.mkdir()
        cache.mkdir()
        (workspace / "main.py").write_text("print('workspace')")
        (cache / "cache-marker.txt").write_text("cache")

        environment = EnvironmentProvider.local(
            workspace,
            allowed_paths=[cache],
            context_file_tree_roots=[workspace],
        )
        assert await environment.read_text(str(cache / "cache-marker.txt")) == "cache"
        context = await environment.render_context()
        assert context is not None
        assert "main.py" in context
        assert "cache-marker.txt" not in context
        state = await environment.export_state()
        allowed_paths = [
            Path(path).resolve() for path in cast(list[str], state["metadata"]["allowed_paths"])
        ]
        assert cache.resolve() in allowed_paths

    asyncio.run(run())


def test_workspace_binding_composes_virtual_mounts() -> None:
    async def run() -> None:
        workspace = EnvironmentProvider.virtual(
            id="workspace",
            files={"README.md": "workspace", "jobs/note.txt": "jobs"},
        )
        data = EnvironmentProvider.virtual(
            id="data",
            files={"README.md": "data", "table.csv": "x,y\n1,2\n"},
        )
        binding = starweaver.WorkspaceBinding(
            [
                starweaver.VirtualMount(
                    "workspace",
                    workspace,
                    default=True,
                    default_for_shell=True,
                ),
                starweaver.VirtualMount("data", data, mode="read_only"),
            ],
            id="workspace-binding",
        )

        assert str(binding.mounts[1].root.join("table.csv")) == "/environment/data/table.csv"
        assert binding.to_dict()["mounts"][0]["provider_id"] == "workspace"
        environment = binding.environment()
        assert environment.id == "workspace-binding"
        assert await environment.read_text("README.md") == "workspace"
        assert await environment.read_text("/environment/data/README.md") == "data"
        assert await environment.list("/environment") == [
            "/environment/workspace",
            "/environment/data",
        ]
        with pytest.raises(StateError, match="read-only"):
            await environment.write_text("/environment/data/new.txt", "denied")
        await environment.write_text("new.txt", "ok")
        assert await workspace.read_text("new.txt") == "ok"

        process = await environment.shell.start("sleep 1", cwd="/environment/workspace/jobs")
        assert process.process_id == "workspace:process_1"
        assert process.metadata["cwd"] == "/environment/workspace/jobs"
        state = await environment.export_state()
        assert state["provider_id"] == "workspace-binding"
        assert state["metadata"]["provider_kind"] == "composite"
        assert state["metadata"]["mounts"][1]["mode"] == "read_only"

    asyncio.run(run())


def test_envd_local_provider_wraps_native_environment() -> None:
    async def run() -> None:
        backing = EnvironmentProvider.virtual(
            id="backing",
            files={"README.md": "backing"},
            shell_outputs={"echo via-envd": "via-envd\n"},
        )
        environment = EnvironmentProvider.envd_local(
            backing,
            environment_id="pytest-envd",
            id="py-envd",
        )

        assert environment.id == "py-envd"
        assert await environment.read_text("README.md") == "backing"
        await environment.write_text("generated.txt", "from envd")
        assert await backing.read_text("generated.txt") == "from envd"
        assert await environment.files.read("generated.txt") == "from envd"
        listing = await environment.list("")
        assert "README.md" in listing
        assert "generated.txt" in listing

        shell_output = await environment.shell.execute("echo via-envd")
        assert shell_output["stdout"] == "via-envd\n"
        process = await environment.shell.start("sleep 1", cwd="/workspace")
        assert process.running
        killed = await environment.shell.kill_process(process)
        assert killed.status == "killed"

        state = await environment.export_state()
        assert state["provider_id"] == "backing"
        assert state["metadata"]["envd_environment_id"] == "pytest-envd"
        assert state["metadata"]["envd_kind"] == "local"
        assert state["metadata"]["envd_store"] == "ephemeral"
        assert state["metadata"]["envd_state_version"] >= 2
        assert state["metadata"]["envd_operation_ids"]

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


def test_media_uploader_resource_store_adapter_returns_resource_refs() -> None:
    png_data_url = (
        "data:image/png;base64,"
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/"
        "l2F0eQAAAABJRU5ErkJggg=="
    )
    latest_request: dict[str, object] = {}

    class Store:
        def __init__(self) -> None:
            self.saved: list[dict[str, object]] = []

        async def put(self, request: starweaver.MediaUploadRequest) -> dict[str, object]:
            self.saved.append(
                {
                    "size": len(request.data),
                    "media_type": request.media_type,
                    "preflight": request.preflight,
                }
            )
            return {
                "id": "image 1",
                "metadata": {"scope": "pytest"},
            }

    store = Store()
    uploader = MediaUploader.resource_store(
        store,
        uri_prefix="resource://pytest-media",
        resource_type="image",
        metadata={"owner": "pytest"},
    )

    @tool
    async def inline_image() -> dict[str, str]:
        return {"data_url": png_data_url, "media_type": "image/png"}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        _ = info
        if len(messages) == 1:
            return {"tool_calls": [{"id": "call_image", "name": "inline_image", "arguments": {}}]}
        latest = messages[-1]
        assert isinstance(latest, dict)
        latest_request.update(latest)
        return {"text": "stored"}

    async def run() -> None:
        normalized = await cast(
            Any,
            uploader.callback(
                starweaver.MediaUploadRequest(
                    data=b"raw",
                    media_type="image/png",
                    preflight={"detected_kind": "png"},
                )
            ),
        )
        assert normalized["uri"] == "resource://pytest-media/image%201"
        assert normalized["resource_type"] == "image"
        assert normalized["metadata"] == {"owner": "pytest", "scope": "pytest"}

        result = await create_agent(
            model=FunctionModel(respond),
            tools=[inline_image],
            runtime_config=RuntimeConfig(capabilities=["image_url"]),
            media_uploader=uploader,
        ).run("upload image")
        assert result.output == "stored"

    asyncio.run(run())

    assert store.saved[-1]["media_type"] == "image/png"
    preflight = cast(dict[str, object], store.saved[-1]["preflight"])
    assert preflight["detected_kind"] == "png"
    metadata = cast(dict[str, object], latest_request["metadata"])
    assert metadata["starweaver_media_uploaded"] == 1
    rendered_parts = str(latest_request["parts"])
    assert png_data_url not in rendered_parts


def test_media_uploader_failure_records_metadata_without_content_leak() -> None:
    png_data_url = (
        "data:image/png;base64,"
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/"
        "l2F0eQAAAABJRU5ErkJggg=="
    )
    uploads: list[dict[str, object]] = []
    latest_request: dict[str, object] = {}

    @tool
    async def inline_image() -> dict[str, str]:
        return {"data_url": png_data_url, "media_type": "image/png"}

    async def upload(request: starweaver.MediaUploadRequest) -> dict[str, str]:
        uploads.append(
            {
                "media_type": request.media_type,
                "preflight": request.preflight,
            }
        )
        raise RuntimeError("private-url=https://secret.example/upload failed")

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        _ = info
        if len(messages) == 1:
            return {"tool_calls": [{"id": "call_image", "name": "inline_image", "arguments": {}}]}
        latest = messages[-1]
        assert isinstance(latest, dict)
        latest_request.update(latest)
        return {"text": "continued after upload failure"}

    async def run() -> None:
        result = await create_agent(
            model=FunctionModel(respond),
            tools=[inline_image],
            runtime_config=RuntimeConfig(capabilities=["image_url"]),
            media_uploader=MediaUploader(upload),
        ).run("upload image")
        assert result.output == "continued after upload failure"

    asyncio.run(run())

    assert uploads[0]["media_type"] == "image/png"
    preflight = cast(dict[str, object], uploads[0]["preflight"])
    assert preflight["detected_kind"] == "png"
    assert preflight["corrected_media_type"] == "image/png"
    metadata = cast(dict[str, object], latest_request["metadata"])
    assert metadata["starweaver_media_upload_failures"] == [
        "RuntimeError: private-url=https://secret.example/upload failed"
    ]
    assert "secret.example" not in str(latest_request["parts"])


def test_stream_adapter_projects_canonical_records() -> None:
    events = [
        starweaver.StreamEvent(
            {
                "sequence": 1,
                "event": {
                    "kind": "model_stream",
                    "event": {"text_delta": "he"},
                },
            }
        ),
        starweaver.StreamEvent(
            {
                "sequence": 0,
                "event": {
                    "kind": "run_start",
                    "run_id": "run_stream",
                    "conversation_id": "conversation_stream",
                },
            }
        ),
        starweaver.StreamEvent(
            {
                "sequence": 2,
                "event": {
                    "kind": "model_stream",
                    "event": {"text_delta": "llo"},
                },
            }
        ),
        starweaver.StreamEvent(
            {
                "sequence": 3,
                "event": {
                    "kind": "tool_call",
                    "step": 1,
                    "call": {"id": "call_lookup", "name": "lookup", "arguments": {}},
                },
            }
        ),
        starweaver.StreamEvent(
            {
                "sequence": 4,
                "event": {
                    "kind": "custom",
                    "event": {
                        "kind": "toolset_initialized",
                        "payload": {"name": "workspace"},
                    },
                },
            }
        ),
        starweaver.StreamEvent(
            {
                "sequence": 5,
                "event": {"kind": "run_complete", "run_id": "run_stream"},
            }
        ),
    ]
    adapter = StreamAdapter(events)
    assert adapter.text() == "hello"
    assert adapter.text_deltas() == ["he", "llo"]
    assert adapter.terminal() is events[-1]
    assert [record["sequence"] for record in adapter.ordered_records()] == [0, 1, 2, 3, 4, 5]
    assert [record["sequence"] for record in adapter.replay_window(after_sequence=2)] == [3, 4, 5]
    assert adapter.cursor_range(scope="run:run_stream") == {
        "first": {"scope": "run:run_stream", "sequence": 0},
        "last": {"scope": "run:run_stream", "sequence": 5},
    }

    display = adapter.display_messages(session_id="session_stream")
    assert [message["type"] for message in display] == [
        "RUN_STARTED",
        "TEXT_MESSAGE_CONTENT",
        "TEXT_MESSAGE_CONTENT",
        "TOOL_CALL_START",
        "TOOLSET_INITIALIZED",
        "RUN_FINISHED",
    ]
    assert display[1]["payload"]["text_delta"] == "he"
    assert display[3]["payload"]["call"]["name"] == "lookup"
    assert display[4]["payload"]["payload"]["name"] == "workspace"

    agui = adapter.agui_events(session_id="session_stream")
    assert agui[0]["type"] == "RUN_STARTED"
    assert agui[1]["payload"]["text_delta"] == "he"
    assert '"type":"RUN_STARTED"' in adapter.agui_jsonl(session_id="session_stream")

    frames = adapter.sse_frames(scope="run:run_stream")
    assert frames[0]["id"] == "0"
    assert frames[0]["event"] == "raw"
    assert frames[0]["cursor"] == {"scope": "run:run_stream", "sequence": 0}
    assert "event: raw" in adapter.sse_text(scope="run:run_stream")

    buffer = adapter.replay_buffer(session_id="session_stream")
    assert buffer["cursor_range"]["last"]["sequence"] == 5
    assert buffer["display_messages"] == display
    assert buffer["raw_records"][0]["sequence"] == 0


def test_stream_adapter_replay_buffer_preserves_subagent_and_unknown_records() -> None:
    subagent_record = {
        "sequence": 8,
        "timestamp": "2026-01-01T00:00:08+00:00",
        "event": {
            "kind": "custom",
            "run_id": "run_stream",
            "event": {
                "kind": "subagent_started",
                "payload": {
                    "agent_name": "researcher",
                    "child_run_id": "run_child",
                    "task": "inspect traces",
                },
            },
        },
    }
    unknown_record = {
        "sequence": 9,
        "timestamp": "2026-01-01T00:00:09+00:00",
        "event": {
            "kind": "provider_experimental",
            "run_id": "run_stream",
            "event": {
                "provider": "example",
                "opaque": {"nested": True},
            },
        },
    }

    adapter = StreamAdapter([unknown_record, subagent_record])
    buffer = adapter.replay_buffer(session_id="session_stream", run_id="run_stream")

    assert buffer["raw_records"] == [subagent_record, unknown_record]
    assert buffer["terminal"] is None
    assert buffer["cursor_range"] == {
        "first": {"scope": "run:run_stream", "sequence": 8},
        "last": {"scope": "run:run_stream", "sequence": 9},
    }

    display = buffer["display_messages"]
    assert [message["type"] for message in display] == ["SUBAGENT_STARTED", "HOST_EVENT"]
    assert display[0]["payload"]["payload"]["child_run_id"] == "run_child"
    assert display[1]["payload"]["kind"] == "provider_experimental"
    assert display[1]["payload"]["event"]["opaque"] == {"nested": True}


def test_async_and_sync_python_tools_execute_in_runtime_loop() -> None:
    current_pid = os.getpid()
    executions: list[tuple[str, int]] = []

    @tool
    async def alpha(value: int) -> dict[str, int]:
        await asyncio.sleep(0)
        pid = os.getpid()
        executions.append(("alpha", pid))
        return {"value": value + 1, "pid": pid}

    @tool
    def beta(value: int) -> ToolResult:
        pid = os.getpid()
        executions.append(("beta", pid))
        return ToolResult({"value": value + 2, "pid": pid}, metadata={"source": "beta"})

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
        assert sorted(executions) == [("alpha", current_pid), ("beta", current_pid)]
        tool_defs = model.captured_params()[0]["tools"]
        assert [definition["name"] for definition in tool_defs] == ["alpha", "beta"]
        second_request = model.captured_messages()[1]
        assert "'name': 'alpha'" in str(second_request)
        assert "'name': 'beta'" in str(second_request)
        alpha_return = _captured_tool_return(second_request, "alpha")
        beta_return = _captured_tool_return(second_request, "beta")
        assert alpha_return["content"] == {"value": 2, "pid": current_pid}
        assert beta_return["content"] == {"value": 4, "pid": current_pid}
        assert beta_return["metadata"]["source"] == "beta"

    asyncio.run(run())


def _captured_tool_return(messages: list[Any], tool_name: str) -> dict[str, Any]:
    for message in messages:
        if not isinstance(message, Mapping) or message.get("kind") != "request":
            continue
        parts = message.get("parts", [])
        if not isinstance(parts, list):
            continue
        for part in parts:
            if (
                isinstance(part, Mapping)
                and part.get("kind") == "tool_return"
                and part.get("name") == tool_name
            ):
                return dict(part)
    raise AssertionError(f"missing captured tool return for {tool_name}")


def _stream_tool_return(event: starweaver.StreamEvent) -> dict[str, Any]:
    tool_return_event = event.tool_return
    assert tool_return_event is not None
    raw_tool_return = tool_return_event["tool_return"]
    assert isinstance(raw_tool_return, Mapping)
    return dict(raw_tool_return)


def test_python_tool_result_layers_preserve_private_metadata_without_content_leak() -> None:
    @tool(parameters_schema={"type": "object", "properties": {}})
    async def layered(args: dict[str, object]) -> ToolResult:
        return ToolResult(
            {"raw": "application raw"},
            metadata={"audit": "public"},
            app_value={"domain": {"id": 42}},
            model_content={"summary": "model safe"},
            user_content={"markdown": "user safe"},
            private_metadata={"debug_note": "host-only evidence"},
        )

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_layered", "name": "layered", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        stream_returns: list[dict[str, Any]] = []
        async with (
            create_agent(model=model, tools=[layered]) as agent,
            agent.run_stream("call layered") as agent_run,
        ):
            async for event in agent_run:
                if event.kind == "tool_return":
                    stream_returns.append(_stream_tool_return(event))
            result = await agent_run.result()

        assert result.output == "done"
        captured_return = _captured_tool_return(model.captured_messages()[1], "layered")
        assert len(stream_returns) == 1

        for tool_return in [stream_returns[0], captured_return]:
            assert tool_return["is_error"] is False
            assert tool_return["content"] == {"summary": "model safe"}
            assert tool_return["metadata"]["audit"] == "public"
            assert tool_return["app_value"] == {"domain": {"id": 42}}
            assert tool_return["user_content"] == {"markdown": "user safe"}
            assert tool_return["private_metadata"]["debug_note"] == "host-only evidence"
            assert "host-only evidence" not in str(tool_return["content"])
            assert "host-only evidence" not in str(tool_return["metadata"])
            assert "application raw" not in str(tool_return["content"])

    asyncio.run(run())


def test_python_tool_exception_traceback_is_private_metadata_only() -> None:
    @tool(parameters_schema={"type": "object", "properties": {}})
    async def fail(args: dict[str, object]) -> dict[str, object]:
        raise RuntimeError("ordinary failure")

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_fail", "name": "fail", "arguments": {}}]
            ),
            {"text": "handled"},
        ]
    )

    async def run() -> None:
        stream_returns: list[dict[str, Any]] = []
        async with (
            create_agent(model=model, tools=[fail]) as agent,
            agent.run_stream("fail") as agent_run,
        ):
            async for event in agent_run:
                if event.kind == "tool_return":
                    stream_returns.append(_stream_tool_return(event))
            result = await agent_run.result()

        assert result.output == "handled"
        captured_return = _captured_tool_return(model.captured_messages()[1], "fail")
        assert len(stream_returns) == 1

        for tool_return in [stream_returns[0], captured_return]:
            assert tool_return["is_error"] is True
            assert tool_return["metadata"]["error_kind"] == "execution"
            private_metadata = tool_return["private_metadata"]
            assert private_metadata["python_exception_type"] == "RuntimeError"
            assert private_metadata["python_exception"] == "RuntimeError: ordinary failure"
            assert "Traceback (most recent call last)" in private_metadata["python_traceback"]
            assert "python_traceback" not in tool_return["metadata"]
            assert "Traceback (most recent call last)" not in str(tool_return["content"])
            assert "Traceback (most recent call last)" not in str(tool_return["metadata"])

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

    class ApprovalRequired(Exception):
        pass

    class CallDeferred(Exception):
        pass

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def fake_retry(args: dict[str, object]) -> dict[str, bool]:
        kind = args["kind"]
        if kind == "retry":
            raise ModelRetry("not starweaver retry control flow")
        if kind == "approval":
            raise ApprovalRequired("not starweaver approval control flow")
        if kind == "deferred":
            raise CallDeferred("not starweaver deferred control flow")
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_fake_retry",
                        "name": "fake_retry",
                        "arguments": {"kind": "retry"},
                    },
                    {
                        "id": "call_fake_approval",
                        "name": "fake_retry",
                        "arguments": {"kind": "approval"},
                    },
                    {
                        "id": "call_fake_deferred",
                        "name": "fake_retry",
                        "arguments": {"kind": "deferred"},
                    },
                ]
            ),
            {"text": "handled"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[fake_retry]).run("fake retry")
        assert result.output == "handled"
        assert not result.needs_approval
        assert not result.pending_deferred
        second_request = str(model.captured_messages()[1])
        assert "not starweaver retry control flow" in second_request
        assert "not starweaver approval control flow" in second_request
        assert "not starweaver deferred control flow" in second_request
        assert "requested model retry" not in second_request
        assert "requires approval" not in second_request
        assert "call deferred" not in second_request

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


def test_pydantic_validation_error_returns_invalid_arguments_tool_return() -> None:
    class AddArgs(BaseModel):
        left: int
        right: int

    @tool
    async def add(args: AddArgs) -> dict[str, int]:
        return {"total": args.left + args.right}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {
                        "id": "call_add",
                        "name": "add",
                        "arguments": {"left": "not-an-int", "right": 3},
                    }
                ]
            ),
            {"text": "handled invalid args"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[add]).run("add")
        assert result.output == "handled invalid args"
        tool_return = _captured_tool_return(model.captured_messages()[1], "add")
        assert tool_return["is_error"] is True
        assert tool_return["metadata"]["error_kind"] == "invalid_arguments"
        assert tool_return["content"]["kind"] == "invalid_arguments"
        assert tool_return["content"]["retry_requires_corrected_input"] is True
        assert tool_return["private_metadata"]["python_exception_type"] == "ValidationError"

    asyncio.run(run())


def test_public_tool_exceptions_map_to_canonical_tool_returns() -> None:
    @tool(parameters_schema={"type": "object", "properties": {}})
    async def invalid(args: dict[str, object]) -> dict[str, bool]:
        raise InvalidArguments("bad public arguments")

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def cancelled(args: dict[str, object]) -> dict[str, bool]:
        raise Cancelled("public cancellation")

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def asyncio_cancelled(args: dict[str, object]) -> dict[str, bool]:
        raise asyncio.CancelledError("asyncio cancellation")

    @tool(parameters_schema={"type": "object", "properties": {}}, timeout_ms=250)
    async def timed_out(args: dict[str, object]) -> dict[str, bool]:
        raise Timeout("public timeout")

    @tool(parameters_schema={"type": "object", "properties": {}}, timeout_ms=250)
    async def timeout_error(args: dict[str, object]) -> dict[str, bool]:
        raise TimeoutError("stdlib timeout")

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [
                    {"id": "call_invalid", "name": "invalid", "arguments": {}},
                    {"id": "call_cancelled", "name": "cancelled", "arguments": {}},
                    {
                        "id": "call_asyncio_cancelled",
                        "name": "asyncio_cancelled",
                        "arguments": {},
                    },
                    {"id": "call_timeout", "name": "timed_out", "arguments": {}},
                    {"id": "call_timeout_error", "name": "timeout_error", "arguments": {}},
                ]
            ),
            {"text": "handled public tool errors"},
        ]
    )

    async def run() -> None:
        result = await create_agent(
            model=model,
            tools=[invalid, cancelled, asyncio_cancelled, timed_out, timeout_error],
        ).run("public tool errors")
        assert result.output == "handled public tool errors"
        returns = {
            name: _captured_tool_return(model.captured_messages()[1], name)
            for name in ("invalid", "cancelled", "asyncio_cancelled", "timed_out", "timeout_error")
        }
        assert returns["invalid"]["metadata"]["error_kind"] == "invalid_arguments"
        assert returns["invalid"]["content"]["kind"] == "invalid_arguments"
        assert returns["invalid"]["content"]["retry_requires_corrected_input"] is True
        assert returns["cancelled"]["metadata"]["error_kind"] == "cancelled"
        assert returns["cancelled"]["content"]["kind"] == "cancelled"
        assert returns["asyncio_cancelled"]["metadata"]["error_kind"] == "cancelled"
        assert returns["asyncio_cancelled"]["content"]["kind"] == "cancelled"
        assert returns["timed_out"]["metadata"]["error_kind"] == "timeout"
        assert returns["timed_out"]["metadata"]["timeout_ms"] == 250
        assert returns["timed_out"]["content"]["kind"] == "timeout"
        assert returns["timeout_error"]["metadata"]["error_kind"] == "timeout"
        assert returns["timeout_error"]["metadata"]["timeout_ms"] == 250
        assert returns["timeout_error"]["content"]["kind"] == "timeout"

    asyncio.run(run())


def test_explicit_json_schema_registration_reaches_model_request() -> None:
    schema = {
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Search query"},
            "limit": {"type": "integer"},
        },
        "required": ["query"],
        "additionalProperties": False,
    }
    seen_schema: list[dict[str, object]] = []

    @tool(parameters_schema=schema, description="Lookup records.")
    async def lookup(**kwargs: object) -> dict[str, object]:
        return {"received": kwargs}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        params = info["params"]
        assert isinstance(params, dict)
        tool_defs = params["tools"]
        assert isinstance(tool_defs, list)
        tool_def = tool_defs[0]
        assert isinstance(tool_def, dict)
        assert tool_def["name"] == "lookup"
        assert tool_def["parameters"] == schema
        seen_schema.append(cast(dict[str, object], tool_def["parameters"]))
        if len(messages) == 1:
            return {
                "tool_calls": [
                    {
                        "id": "call_lookup",
                        "name": "lookup",
                        "arguments": {"query": "starweaver", "limit": 3},
                    }
                ]
            }
        return {"text": "done"}

    async def run() -> None:
        result = await create_agent(model=FunctionModel(respond), tools=[lookup]).run("lookup")
        assert result.output == "done"
        assert lookup.parameters_schema == schema
        assert seen_schema
        assert all(item == schema for item in seen_schema)

    asyncio.run(run())


def test_invalid_explicit_tool_json_schemas_are_rejected() -> None:
    async def accepts_kwargs(**kwargs: object) -> dict[str, object]:
        return kwargs

    invalid_schemas: list[object] = [
        {"type": "array"},
        {"type": "object", "properties": []},
        {"type": "object", "properties": {"value": "string"}},
        {"type": "object", "required": "value"},
        {"type": "object", "properties": {}, "required": ["value"]},
        {"type": "object", "additionalProperties": "yes"},
        {"type": "object", "properties": {1: {"type": "string"}}},
        {"type": "object", "properties": {"value": object()}},
    ]
    for schema in invalid_schemas:
        with pytest.raises((TypeError, ValueError), match="parameters_schema"):
            tool(accepts_kwargs, parameters_schema=cast(dict[str, Any], schema))


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


def test_python_tool_timeout_returns_canonical_error_and_cancels_coroutine() -> None:
    started = asyncio.Event()
    cancelled = asyncio.Event()
    release = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}}, timeout_ms=20)
    async def slow(args: dict[str, object]) -> dict[str, bool]:
        started.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            cancelled.set()
            raise
        return {"released": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_slow", "name": "slow", "arguments": {}}]
            ),
            {"text": "handled timeout"},
        ]
    )

    async def run() -> None:
        result = await create_agent(model=model, tools=[slow]).run("timeout")
        assert result.output == "handled timeout"
        await asyncio.wait_for(started.wait(), timeout=1)
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        tool_return = _captured_tool_return(model.captured_messages()[1], "slow")
        assert tool_return["is_error"] is True
        assert tool_return["metadata"]["error_kind"] == "timeout"
        assert tool_return["metadata"]["timeout_ms"] == 20
        assert tool_return["content"]["kind"] == "timeout"

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


def test_typed_hitl_helpers_preserve_ids_metadata_and_raw_escape_hatches() -> None:
    raw_approval = {
        "approval_id": "approval_canonical",
        "tool_call_id": "call_deploy",
        "name": "deploy",
        "arguments": {"service": "api"},
        "metadata": {"risk": "high"},
        "extra": {"host": "ui"},
    }
    raw_deferred = {
        "deferred_id": "deferred_canonical",
        "tool_call_id": "call_collect",
        "name": "collect",
        "metadata": {"queue": "default"},
        "extra": {"host": "worker"},
    }

    approval = starweaver.PendingApproval.from_raw(raw_approval)
    deferred = starweaver.PendingDeferred.from_raw(raw_deferred)
    snapshot = starweaver.HitlSnapshot(
        approvals=[approval],
        deferred=[deferred],
        raw_approvals=[raw_approval],
        raw_deferred=[raw_deferred],
    )

    assert approval.id == "approval_canonical"
    assert approval.tool_call_id == "call_deploy"
    assert approval.tool_name == "deploy"
    assert approval.arguments == {"service": "api"}
    assert approval.metadata == {"risk": "high"}
    assert approval.raw == raw_approval
    assert deferred.id == "deferred_canonical"
    assert deferred.tool_call_id == "call_collect"
    assert deferred.tool_name == "collect"
    assert deferred.metadata == {"queue": "default"}
    assert deferred.raw == raw_deferred
    assert snapshot.pending_approvals == [raw_approval]
    assert snapshot.pending_deferred == [raw_deferred]

    decision = approval.approve(
        decided_by="pytest",
        reason="reviewed",
        override_arguments={"service": "api-canary"},
        metadata={"channel": "tests"},
        ticket="T-1",
    )
    assert decision.to_dict() == {
        "approval_id": "approval_canonical",
        "approved": True,
        "decided_by": "pytest",
        "reason": "reviewed",
        "override_arguments": {"service": "api-canary"},
        "metadata": {"channel": "tests", "ticket": "T-1"},
    }
    assert approval.deny("unsafe", decided_by="pytest").to_dict() == {
        "approval_id": "approval_canonical",
        "approved": False,
        "decided_by": "pytest",
        "reason": "unsafe",
        "metadata": {},
    }
    assert deferred.complete({"ok": True}, metadata={"worker": "a"}, queue="default").to_dict() == {
        "deferred_id": "deferred_canonical",
        "status": "completed",
        "response": {"ok": True},
        "metadata": {"worker": "a", "queue": "default"},
    }
    assert deferred.fail("boom").to_dict() == {
        "deferred_id": "deferred_canonical",
        "status": "failed",
        "response": {"error": "boom"},
        "metadata": {},
    }
    assert deferred.cancel("stopped").to_dict() == {
        "deferred_id": "deferred_canonical",
        "status": "cancelled",
        "response": {"reason": "stopped"},
        "metadata": {},
    }


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
        stream = create_agent(model=StarweaverTestModel.text("streamed")).run_stream("stream")
        events = [event async for event in stream]
        result = await stream.result()
        adapter = StreamAdapter(events)
        raw_records = [event.raw for event in events]
        sequences = [record["sequence"] for record in raw_records]

        assert raw_records
        assert events[0].kind == "run_start"
        assert events[0].raw["event"]["kind"] == "run_start"
        assert events[-1].kind == "run_complete"
        assert all(event.raw["event"]["kind"] == event.kind for event in events)
        assert sequences == sorted(sequences)
        assert adapter.records() == raw_records
        assert adapter.ordered_records() == raw_records
        assert adapter.replay_window(after_sequence=sequences[0]) == raw_records[1:]
        assert adapter.terminal() is events[-1]
        assert adapter.replay_buffer(session_id="session_streamed")["raw_records"] == raw_records
        assert [frame["data"] for frame in adapter.sse_frames()] == raw_records
        assert result.output == "streamed"
        assert result.raw_state["run_id"] == events[0].run_id
        assert result.raw_state["status"] == "completed"

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


def test_stream_close_receiver_allows_explicit_join() -> None:
    async def run() -> None:
        stream = create_agent(model=StarweaverTestModel.text("closed")).run_stream("close")
        stream.close_receiver()
        result = await stream.result()
        assert result.output == "closed"

    asyncio.run(run())


def test_stream_detach_does_not_interrupt_running_python_tool() -> None:
    started = asyncio.Event()
    completed = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def background(args: dict[str, object]) -> dict[str, bool]:
        started.set()
        await asyncio.sleep(0.01)
        completed.set()
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_background", "name": "background", "arguments": {}}]
            ),
            {"text": "done"},
        ]
    )

    async def run() -> None:
        agent_run = create_agent(model=model, tools=[background]).run_stream("detach")
        await asyncio.wait_for(started.wait(), timeout=1)
        agent_run.detach()
        with pytest.raises(StateError, match="detached"):
            await agent_run.result()
        del agent_run
        await asyncio.wait_for(completed.wait(), timeout=1)

    asyncio.run(run())


def test_session_stream_detach_finishes_in_background_and_releases_session() -> None:
    started = asyncio.Event()
    completed = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def background(args: dict[str, object]) -> dict[str, bool]:
        started.set()
        await asyncio.sleep(0.01)
        completed.set()
        return {"ok": True}

    model = StarweaverTestModel.responses(
        [
            StarweaverTestModel.tool_call_response(
                [{"id": "call_background", "name": "background", "arguments": {}}]
            ),
            {"text": "detached"},
            {"text": "second"},
        ]
    )

    async def run() -> None:
        async with create_agent(model=model, tools=[background]).session() as session:
            agent_run = session.run_stream("detach")
            await asyncio.wait_for(started.wait(), timeout=1)
            agent_run.detach()
            assert session.active_run is agent_run
            await asyncio.wait_for(completed.wait(), timeout=1)
            for _ in range(100):
                if session.active_run is None:
                    break
                await asyncio.sleep(0.01)
            assert agent_run.is_finished
            assert session.active_run is None
            with pytest.raises(StateError, match="detached"):
                await agent_run.result()
            result = await session.run("continue")
            assert result.output == "second"

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


def test_tool_context_is_cancelled_is_observable_during_interrupt() -> None:
    started = asyncio.Event()
    cancelled = asyncio.Event()
    observed_cancelled: list[bool] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def slow(ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        started.set()
        try:
            await asyncio.sleep(60)
        except asyncio.CancelledError:
            observed_cancelled.append(ctx.is_cancelled())
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
        stream.interrupt("observe cancellation")
        with pytest.raises(StreamError, match="observe cancellation"):
            await stream.join()
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        assert observed_cancelled == [True]

    asyncio.run(run())


def test_tool_context_cancelled_awaitable_is_observable_during_interrupt() -> None:
    started = asyncio.Event()
    cancelled = asyncio.Event()
    observed_cancelled: list[bool] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def slow(ctx: ToolContext, args: dict[str, object]) -> dict[str, bool]:
        started.set()
        try:
            observed_cancelled.append(await ctx.cancelled())
        except asyncio.CancelledError:
            observed_cancelled.append(await ctx.cancelled())
            cancelled.set()
            raise
        cancelled.set()
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
        stream.interrupt("await cancellation")
        with pytest.raises(StreamError, match="await cancellation"):
            await stream.join()
        await asyncio.wait_for(cancelled.wait(), timeout=1)
        assert observed_cancelled == [True]

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


def test_cancelling_stream_join_interrupts_running_python_tool() -> None:
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
            stream = session.run_stream("cancel join")
            task = asyncio.create_task(stream.join())
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


def test_cancelling_stream_recv_interrupts_running_python_tool() -> None:
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
            stream = session.run_stream("cancel recv")
            await asyncio.wait_for(started.wait(), timeout=1)
            cancelled_recv = False
            for _ in range(100):
                task = asyncio.create_task(stream.recv())
                done, _ = await asyncio.wait({task}, timeout=0.01)
                if task not in done:
                    task.cancel()
                    with pytest.raises(asyncio.CancelledError):
                        await task
                    cancelled_recv = True
                    break
                assert task.result() is not None
            assert cancelled_recv
            with pytest.raises(StreamError):
                await stream.join()
            await asyncio.wait_for(cancelled.wait(), timeout=1)
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
        assert full["session_id"]
        assert curated["session_id"] == full["session_id"]
        assert "message_history" not in curated
        assert "message_bus" not in curated
        assert full["message_history"]
        assert "message_bus" in full
        full_message = next(
            message for message in full["message_bus"]["messages"] if message["id"] == "msg-state"
        )
        assert full_message["content"] == "remember this"
        assert full_message["metadata"]["starweaver.topic"] == "note"
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
        assert full_round_trip["session_id"] == full["session_id"]
        assert any(
            message["id"] == "msg-state" for message in full_round_trip["message_bus"]["messages"]
        )
        assert full_round_trip["state"]["domains"]["domain"]["value"] is True
        assert full_round_trip["trace_snapshot"]["trace_id"] == "trace-main"
        assert full_round_trip["metadata"]["debug"] is True
        assert full_round_trip["model_config"]["context_window"] == 100

        archive = starweaver.SessionArchive.from_session(restored_state_session)
        assert archive.mode == "full"
        assert archive.session_id == full["session_id"]
        assert archive.state["message_history"] == full_round_trip["message_history"]
        encoded = archive.to_json(sort_keys=True)
        decoded = starweaver.SessionArchive.from_json(encoded)
        assert decoded.to_dict() == archive.to_dict()
        assert decoded.session_id == full["session_id"]
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
        assert restored.export_full_state()["session_id"] == full["session_id"]
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


def test_input_parts_and_status_enums_use_canonical_store_contract() -> None:
    text = InputPart.text("hello", metadata={"source": "test"})
    url = InputPart.url("https://example.com")
    file_resource = ResourceRef.typed(
        "file://workspace/spec.md",
        kind="document",
        metadata={
            "media_type": "text/markdown",
            "name": "spec.md",
            "etag": "doc-v1",
        },
    )
    file_part = InputPart.file(file_resource, metadata={"source": "upload"})
    binary_resource = ResourceRef.typed(
        "resource://image",
        kind="image",
        metadata={"media_type": "image/png", "bytes": 12},
    )
    binary = InputPart.binary(binary_resource)
    mode = InputPart.mode("content_part", config={"kind": "image"})
    command = InputPart.command("plan", ["--fast"], payload={"scope": "runtime"})
    raw = InputPart.from_raw({"kind": "custom", "value": {"ok": True}})

    assert text.kind == "text"
    assert text.metadata == {"source": "test"}
    assert text.to_dict() == {
        "kind": "text",
        "text": "hello",
        "metadata": {"source": "test"},
    }
    assert url.to_dict()["kind"] == "url"
    assert file_part.to_dict()["file"] == {
        "uri": "file://workspace/spec.md",
        "media_type": "text/markdown",
        "name": "spec.md",
    }
    assert file_part.to_dict()["metadata"] == {
        "resource_kind": "document",
        "media_type": "text/markdown",
        "name": "spec.md",
        "etag": "doc-v1",
        "source": "upload",
    }
    assert binary.to_dict()["binary"] == {
        "uri": "resource://image",
        "media_type": "image/png",
        "bytes": 12,
    }
    assert binary.to_dict()["metadata"]["resource_kind"] == "image"
    assert mode.to_dict()["config"] == {"kind": "image"}
    assert command.to_dict()["args"] == ["--fast"]
    assert command.to_dict()["payload"] == {"scope": "runtime"}
    assert raw.to_dict()["kind"] == "custom"

    assert SessionStatus.ACTIVE.value == "active"
    assert SessionStatus.from_value("archived") is SessionStatus.ARCHIVED
    assert RunStatus.WAITING.value == "waiting"
    assert RunStatus.from_value("completed") is RunStatus.COMPLETED
    assert ExecutionStatus.PENDING.value == "pending"
    assert ExecutionStatus.from_value("cancelled") is ExecutionStatus.CANCELLED
    with pytest.raises(ValueError, match="unknown run status"):
        RunStatus.from_value("unknown")


def test_run_record_and_store_status_helpers_accept_typed_inputs() -> None:
    async def run() -> None:
        agent = create_agent(model=StarweaverTestModel.text("done"))
        session = agent.session()
        result = await session.run("done")

        store = starweaver.InMemorySessionStore()
        session_record = starweaver.SessionRecord.from_state(
            session.export_full_state(),
            metadata={"test": "status"},
        )
        await store.save_session(session_record)
        await store.update_session_status(session_record.session_id, SessionStatus.ARCHIVED)
        assert (await store.load_session(session_record.session_id)).to_dict()[
            "status"
        ] == "archived"

        run_record = starweaver.RunRecord.from_result(
            session_record.session_id,
            result,
            input_parts=[
                InputPart.text("done"),
                {"kind": "command", "command": "plan", "args": ["--fast"]},
            ],
        )
        await store.append_run(run_record)
        loaded = await store.load_run(session_record.session_id, run_record.run_id)
        assert loaded.to_dict()["input"][0] == {"kind": "text", "text": "done"}
        assert loaded.to_dict()["input"][1]["command"] == "plan"

        await store.update_run_status(
            session_record.session_id,
            run_record.run_id,
            RunStatus.WAITING,
            output_preview="waiting",
        )
        updated = await store.load_run(session_record.session_id, run_record.run_id)
        assert updated.to_dict()["status"] == "waiting"
        assert updated.to_dict()["output_preview"] == "waiting"
        with pytest.raises(ValueError, match="unknown session status"):
            await store.update_session_status(session_record.session_id, "deleted")
        with pytest.raises(StateError, match="input part must include kind"):
            starweaver.RunRecord.from_result(
                session_record.session_id,
                result,
                input_parts=[{"text": "missing kind"}],
            )

    asyncio.run(run())


def test_python_session_store_to_native_adapts_python_backend() -> None:
    async def run() -> None:
        source_store = starweaver.InMemorySessionStore()
        source_runtime = create_agent_runtime(
            model=StarweaverTestModel.text("stored"),
            session_store=source_store,
            durable_session_id="source-session",
        )
        source_result = await source_runtime.run("persist through callback bridge")
        assert source_result.output == "stored"

        source_raw = source_store.to_dict()
        source_sessions = cast(dict[str, dict[str, Any]], source_raw["sessions"])
        source_runs = cast(dict[str, dict[str, Any]], source_raw["runs"])
        source_streams = cast(dict[str, list[dict[str, Any]]], source_raw["streams"])
        source_checkpoint_records = cast(
            dict[str, list[dict[str, Any]]],
            source_raw["checkpoints"],
        )
        source_session = source_sessions["source-session"]
        source_run = next(iter(source_runs.values()))
        source_session_id = str(source_session["session_id"])
        source_run_id = str(source_run["run_id"])
        source_key = f"{source_session_id}:{source_run_id}"
        source_stream = source_streams[source_key]
        source_checkpoints = source_checkpoint_records[source_key]
        assert source_stream
        assert source_checkpoints

        store = starweaver.InMemorySessionStore()
        native = store.to_native()
        await native.save_session(source_session)
        loaded_session = await native.load_session(source_session_id)
        assert loaded_session["session_id"] == source_session_id

        filtered_sessions = await native.list_sessions({"status": "active"})
        assert [record["session_id"] for record in filtered_sessions] == [source_session_id]

        await native.update_session_status(source_session_id, "archived")
        loaded_session = await native.load_session(source_session_id)
        assert loaded_session["status"] == "archived"

        await native.save_context_state(
            source_session_id,
            {
                "agent_id": "main",
                "session_id": source_session_id,
                "conversation_id": source_session["state"]["conversation_id"],
                "message_history": source_session["state"]["message_history"],
                "metadata": {"bridge": "context"},
            },
        )
        loaded_session = await native.load_session(source_session_id)
        loaded_state = cast(dict[str, Any], loaded_session["state"])
        assert loaded_state["metadata"]["bridge"] == "context"

        await native.save_environment_state(
            source_session_id,
            {
                "provider": "virtual",
                "reference": "env://bridge",
                "revision": "rev-1",
                "metadata": {"workspace": "bridge"},
            },
        )
        loaded_session = await native.load_session(source_session_id)
        environment_state = cast(dict[str, Any], loaded_session["environment_state"])
        assert environment_state["reference"] == "env://bridge"

        await native.append_run(source_run)
        loaded_run = await native.load_run(source_session_id, source_run_id)
        assert loaded_run["output_preview"] == "stored"
        listed_runs = await native.list_runs(source_session_id)
        assert [record["run_id"] for record in listed_runs] == [source_run_id]

        await native.update_run_status(
            source_session_id,
            source_run_id,
            "waiting",
            output_preview="waiting for bridge",
        )
        loaded_run = await native.load_run(source_session_id, source_run_id)
        assert loaded_run["status"] == "waiting"
        assert loaded_run["output_preview"] == "waiting for bridge"

        await native.append_stream_records(
            source_session_id,
            source_run_id,
            list(reversed(source_stream[:3])),
        )
        replay = await native.replay_stream_records(source_session_id, source_run_id)

        def stream_sequence(record: Mapping[str, object]) -> int:
            sequence = record["sequence"]
            assert isinstance(sequence, int)
            return sequence

        replay_sequences = [stream_sequence(record) for record in replay]
        assert replay_sequences == sorted(replay_sequences)
        replay_tail = await native.replay_stream_records(
            source_session_id,
            source_run_id,
            replay_sequences[0],
        )
        assert all(stream_sequence(record) > replay_sequences[0] for record in replay_tail)

        timestamp = str(source_session["created_at"])
        cursor = {
            "family": "raw_runtime",
            "scope": f"run:{source_run_id}",
            "sequence": replay_sequences[-1],
            "created_at": timestamp,
            "metadata": {"bridge": True},
        }
        await native.save_stream_cursor(source_session_id, source_run_id, cursor)
        loaded_run = await native.load_run(source_session_id, source_run_id)
        stream_cursors = cast(list[dict[str, Any]], loaded_run["stream_cursors"])
        assert stream_cursors[-1]["scope"] == f"run:{source_run_id}"

        await native.append_checkpoint(source_session_id, source_checkpoints[0])
        checkpoints = await native.load_checkpoints(source_session_id, source_run_id)
        assert checkpoints[0]["checkpoint_id"] == source_checkpoints[0]["checkpoint_id"]

        approval = {
            "approval_id": "approval_bridge",
            "session_id": source_session_id,
            "run_id": source_run_id,
            "action_id": "call_deploy",
            "action_name": "deploy",
            "request": {"risk": "high"},
            "status": "pending",
            "created_at": timestamp,
            "updated_at": timestamp,
            "metadata": {"bridge": True},
        }
        await native.append_approval(approval)
        approvals = await native.load_approvals(source_session_id, source_run_id)
        assert approvals[0]["approval_id"] == "approval_bridge"

        deferred = {
            "deferred_id": "deferred_bridge",
            "session_id": source_session_id,
            "run_id": source_run_id,
            "tool_call_id": "call_collect",
            "tool_name": "collect",
            "request": {"queue": "default"},
            "status": "waiting",
            "response": None,
            "created_at": timestamp,
            "updated_at": timestamp,
            "metadata": {"bridge": True},
        }
        await native.append_deferred_tool(deferred)
        deferred_tools = await native.load_deferred_tools(source_session_id, source_run_id)
        assert deferred_tools[0]["deferred_id"] == "deferred_bridge"

        snapshot = await native.resume_snapshot(source_session_id, source_run_id)
        snapshot_run = cast(dict[str, Any], snapshot["run"])
        snapshot_checkpoint = cast(dict[str, Any], snapshot["latest_checkpoint"])
        snapshot_approvals = cast(list[dict[str, Any]], snapshot["approvals"])
        snapshot_deferred = cast(list[dict[str, Any]], snapshot["deferred_tools"])
        assert snapshot_run["run_id"] == source_run_id
        assert snapshot_checkpoint["checkpoint_id"] == source_checkpoints[0]["checkpoint_id"]
        assert snapshot["stream_records"]
        assert snapshot_approvals[0]["approval_id"] == "approval_bridge"
        assert snapshot_deferred[0]["deferred_id"] == "deferred_bridge"

        run_trace = await native.compact_run_trace(source_session_id, source_run_id)
        assert run_trace["latest_checkpoint"] == source_checkpoints[0]["checkpoint_id"]
        assert run_trace["approvals"] == 1
        assert run_trace["deferred_tools"] == 1

        trace = await native.compact_session_trace(source_session_id)
        assert trace["runs"] == 1
        assert trace["latest_run_id"] == source_run_id

    asyncio.run(run())


def test_agent_runtime_binds_python_session_store_to_durable_runs() -> None:
    async def run() -> None:
        store = starweaver.InMemorySessionStore()
        runtime = create_agent_runtime(
            model=StarweaverTestModel.responses(
                [{"text": "stored"}, {"text": "stored"}, {"text": "stored"}]
            ),
            session_store=store,
            durable_session_id="runtime-session",
        )

        assert runtime.durable_session_id == "runtime-session"
        result = await runtime.run("persist this")
        assert result.output == "stored"

        loaded = await store.load_session("runtime-session")
        assert loaded.session_id == "runtime-session"
        assert loaded.state["message_history"]

        runs = await store.list_runs("runtime-session")
        assert len(runs) == 1
        assert runs[0].to_dict()["output_preview"] == "stored"
        stream_records = await store.replay_stream_records("runtime-session", runs[0].run_id)
        assert stream_records

        streamed = await runtime.run_stream("persist stream")
        assert streamed.result.output == "stored"
        all_runs = await store.list_runs("runtime-session")
        assert len(all_runs) == 2

        snapshot = await runtime.resume_snapshot("runtime-session", all_runs[-1].run_id)
        assert snapshot["state"]["message_history"]

        live = runtime.stream("persist live stream")
        assert (await live.result()).output == "stored"
        live_runs = await store.list_runs("runtime-session")
        assert len(live_runs) == 3

    asyncio.run(run())


def test_sqlite_session_store_wraps_native_storage(tmp_path: Path) -> None:
    async def run() -> None:
        database_path = tmp_path / "sessions.sqlite3"
        initial_status = starweaver.SqliteSessionStore.migration_status(database_path)
        assert initial_status["current"] is False
        applied = starweaver.SqliteSessionStore.migrate(database_path)
        assert applied
        migrated_status = starweaver.SqliteSessionStore.migration_status(database_path)
        assert migrated_status["current"] is True
        file_url = database_path.resolve().as_uri()
        sqlite_url = f"sqlite:///{database_path.resolve()}"
        assert starweaver.SqliteSessionStore.migration_status(file_url)["current"] is True

        store = starweaver.SqliteSessionStore(database_path)
        url_store = starweaver.SqliteSessionStore.open(file_url)
        assert url_store.path == database_path.resolve()
        agent = create_agent(
            model=StarweaverTestModel.responses([{"text": "first"}, {"text": "second"}])
        )
        session = agent.session()
        async with session.run_stream("first") as agent_run:
            stream_result = await agent_run.join()

        session_record = await store.save_current_session(session)
        run_record = starweaver.RunRecord.from_result(
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

        created_at = session_record.to_dict()["created_at"]
        approval = {
            "approval_id": "approval_py",
            "session_id": session_record.session_id,
            "run_id": run_record.run_id,
            "action_id": "call_deploy",
            "action_name": "deploy",
            "request": {"risk": "high"},
            "status": "pending",
            "created_at": created_at,
            "updated_at": created_at,
        }
        deferred = {
            "deferred_id": "deferred_py",
            "session_id": session_record.session_id,
            "run_id": run_record.run_id,
            "tool_call_id": "call_collect",
            "tool_name": "collect",
            "request": {"queue": "default"},
            "status": "waiting",
            "response": None,
            "created_at": created_at,
            "updated_at": created_at,
        }
        await store.append_approval(approval)
        await store.append_deferred_tool(deferred)

        loaded = await store.load_session(session_record.session_id)
        assert loaded.state["message_history"]
        assert (await store.list_runs(session_record.session_id))[0].run_id == run_record.run_id

        replay = await store.replay_stream_records(session_record.session_id, run_record.run_id)
        assert replay
        assert [record.sequence for record in replay] == sorted(
            record.sequence for record in replay
        )
        replay_tail = await store.replay_stream_records(
            session_record.session_id,
            run_record.run_id,
            after_sequence=replay[0].sequence,
        )
        assert all(record.sequence > replay[0].sequence for record in replay_tail)

        approvals = await store.load_approvals(session_record.session_id, run_record.run_id)
        assert approvals[0].to_dict()["approval_id"] == "approval_py"
        deferred_tools = await store.load_deferred_tools(
            session_record.session_id,
            run_record.run_id,
        )
        assert deferred_tools[0].to_dict()["deferred_id"] == "deferred_py"

        snapshot = await store.resume_snapshot(session_record.session_id, run_record.run_id)
        assert snapshot.run is not None
        assert snapshot.run.run_id == run_record.run_id
        assert snapshot.stream_records
        assert snapshot.approvals[0].to_dict()["approval_id"] == "approval_py"
        assert snapshot.deferred_tools[0].to_dict()["deferred_id"] == "deferred_py"

        scope = f"run:{run_record.run_id}"
        run_cursor = {"scope": scope, "sequence": replay[0].sequence}
        archive_store = starweaver.SqliteStreamArchive(database_path)
        await archive_store.append_raw_records(
            session_record.session_id,
            run_record.run_id,
            [event.raw for event in stream_result.events],
        )
        archived_raw = await archive_store.replay_raw_after(
            session_record.session_id,
            run_record.run_id,
        )
        assert [record.sequence for record in archived_raw] == [
            record.sequence for record in replay
        ]
        archived_tail = await archive_store.replay_raw_after(
            session_record.session_id,
            run_record.run_id,
            run_cursor,
        )
        assert all(record.sequence > replay[0].sequence for record in archived_tail)

        display_message = {
            "sequence": 0,
            "session_id": session_record.session_id,
            "run_id": run_record.run_id,
            "timestamp": created_at,
            "type": "RUN_STARTED",
            "payload": {"prompt": "first"},
            "preview": "run started",
        }
        await archive_store.append_display_messages(scope, [display_message])
        archived_display = await archive_store.replay_display_after(scope)
        assert archived_display[0]["type"] == "RUN_STARTED"
        assert (
            await archive_store.replay_display_after(
                scope,
                {"scope": scope, "sequence": 0},
            )
            == []
        )

        archive_snapshot = {
            "scope": scope,
            "revision": 1,
            "cursor": {"scope": scope, "sequence": 0},
            "display_messages": [display_message],
            "metadata": {"source": "pytest"},
        }
        await archive_store.append_snapshot(scope, archive_snapshot)
        latest_archive_snapshot = await archive_store.latest_snapshot(scope)
        assert latest_archive_snapshot is not None
        assert latest_archive_snapshot["revision"] == 1
        cursor_range = await archive_store.cursor_range(scope)
        assert cursor_range is not None
        assert cursor_range["first"]["sequence"] == 0
        assert cursor_range["last"]["sequence"] == 0

        replay_log = starweaver.SqliteReplayEventLog(database_path)
        heartbeat = {
            "scope": scope,
            "sequence": 1,
            "timestamp": created_at,
            "event": {"kind": "heartbeat"},
        }
        await replay_log.append(scope, heartbeat)
        replay_events = await replay_log.replay_after(scope)
        assert replay_events[-1]["event"]["kind"] == "heartbeat"
        assert (
            await replay_log.replay_after(
                scope,
                {"scope": scope, "sequence": 1},
            )
            == []
        )
        await replay_log.save_snapshot(scope, archive_snapshot)
        assert (await replay_log.compact_snapshot(scope))["revision"] == 1
        sqlite_url_archive = starweaver.SqliteStreamArchive.open(sqlite_url)
        assert (await sqlite_url_archive.cursor_range(scope)) is not None
        sqlite_url_log = starweaver.SqliteReplayEventLog.open(sqlite_url)
        assert any(
            event["event"]["kind"] == "heartbeat"
            for event in await sqlite_url_log.replay_after(scope)
        )

        reopened = starweaver.SqliteSessionStore(database_path)
        archive = await reopened.load_archive(session_record.session_id)
        restored = agent.session_from_archive(archive)
        second = await restored.run("second")
        assert second.output == "second"

        memory_store = starweaver.SqliteSessionStore.in_memory()
        await memory_store.save_session(session_record)
        assert (await memory_store.load_session(session_record.session_id)).session_id
        memory_archive = starweaver.SqliteStreamArchive.in_memory()
        await memory_archive.append_display_messages(scope, [display_message])
        assert (await memory_archive.cursor_range(scope)) is not None
        memory_log = starweaver.SqliteReplayEventLog.in_memory()
        await memory_log.append(scope, heartbeat)
        assert (await memory_log.replay_after(scope))[0]["sequence"] == 1

    asyncio.run(run())


def test_pythonic_session_context_and_typed_hitl_resume() -> None:
    seen_approvals: list[dict[str, Any]] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def deploy(ctx: starweaver.ToolContext, args: dict[str, object]) -> dict[str, bool]:
        if ctx.approval is None:
            raise ApprovalRequired("deploy production", metadata={"risk": "high"})
        seen_approvals.append(dict(ctx.approval))
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
            approval = waiting.hitl.approvals[0]
            assert approval.raw == waiting.hitl.raw_approvals[0]
            decision = approval.approve(
                decided_by="pytest",
                metadata={"channel": "typed-helper"},
            )
            resumed = await session.hitl.resume(approvals=[decision])
        assert resumed.output == "deployed"
        assert seen_approvals
        assert decision.to_dict()["approval_id"] == approval.id
        assert seen_approvals[0]["status"] == "approved"
        assert seen_approvals[0]["metadata"]["channel"] == "typed-helper"
        assert seen_approvals[0]["metadata"]["decided_by"] == "pytest"

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


def test_session_message_bus_subscribers_targets_and_unsubscribe() -> None:
    async def run() -> None:
        session = create_agent(model=StarweaverTestModel.text("ok")).session()
        await session.messages.send("before subscribe", id="warmup")

        session.messages.subscribe("primary")
        session.messages.subscribe("debugger")
        assert session.messages.consume("primary") == []
        assert session.messages.consume("debugger") == []

        broadcast = await session.messages.send(
            "broadcast",
            id="broadcast-1",
            source="system",
            topic="notice",
        )
        duplicate = await session.messages.send(
            "changed broadcast",
            id="broadcast-1",
            source="system",
            topic="notice",
        )
        assert broadcast.id == duplicate.id
        assert duplicate.message.content == "broadcast"
        await session.messages.send(
            "debug only",
            id="target-debugger",
            source="main",
            target="debugger",
        )
        await session.messages.send(
            "main only",
            id="target-primary",
            source="debugger",
            target="primary",
        )

        primary_messages = session.messages.consume("primary")
        assert [message.id for message in primary_messages] == [
            "broadcast-1",
            "target-primary",
        ]
        assert primary_messages[0].topic == "notice"
        assert primary_messages[0].metadata["starweaver.topic"] == "notice"
        assert session.messages.consume("primary") == []

        debugger_messages = session.messages.consume("debugger")
        assert [message.id for message in debugger_messages] == [
            "broadcast-1",
            "target-debugger",
        ]
        assert session.messages.consume("debugger") == []

        session.messages.unsubscribe("debugger")
        await session.messages.send(
            "missed while unsubscribed",
            id="missed-debugger",
            source="system",
            target="debugger",
        )
        session.messages.subscribe("debugger")
        assert session.messages.consume("debugger") == []

        await session.messages.send(
            "after resubscribe",
            id="after-resubscribe",
            source="system",
            target="debugger",
        )
        assert [message.id for message in session.messages.consume("debugger")] == [
            "after-resubscribe"
        ]

    asyncio.run(run())


def test_bus_message_topic_conflicts_are_rejected() -> None:
    message = starweaver.BusMessage(
        "hello",
        topic="note",
        metadata={"starweaver.topic": "steering"},
    )
    with pytest.raises(ValueError, match="topic conflicts"):
        message.to_dict()


def test_idle_session_steer_can_queue_message_state() -> None:
    async def run() -> None:
        async with create_agent(model=StarweaverTestModel.text("done")) as agent:
            session = agent.session()

            with pytest.raises(StateError, match="no active run"):
                await session.steer("not queued")

            receipt = await session.steer(
                "Queue this for the next run.",
                when_idle="queue",
                id="idle-steer",
            )

            assert receipt.id == "idle-steer"
            assert receipt.kind == "steering"
            assert receipt.queued
            assert receipt.run_id is None
            assert receipt.session_id == session.export_state()["session_id"]
            messages = session.messages.consume()
            assert len(messages) == 1
            assert messages[0].id == "idle-steer"
            assert messages[0].content == "Queue this for the next run."
            assert messages[0].topic == "steering"
            assert messages[0].source == "user"

    asyncio.run(run())


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
                        await session.steer("x", when_idle="drop")
                    with pytest.raises(ValueError, match="when_idle"):
                        await session.messages.steer("x", when_idle="drop")
                    release.set()
            assert (await agent_run.result()).output == "done"

    asyncio.run(run())


def test_agent_steer_targets_exactly_one_direct_active_run() -> None:
    release = asyncio.Event()
    captured_messages: list[list[object]] = []

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"ok": True}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        captured_messages.append(messages)
        if len(messages) == 1:
            return {"tool_calls": [{"id": "call_wait", "name": "wait", "arguments": {}}]}
        assert "Steering update from the user" in str(messages)
        assert "Use the agent-level steering path." in str(messages)
        return {"text": "done"}

    async def run() -> None:
        async with create_agent(model=FunctionModel(respond), tools=[wait]) as agent:
            with pytest.raises(StateError, match="exactly one direct active run"):
                await agent.steer("no active run")
            agent_run = agent.run_stream("deploy")
            async for event in agent_run:
                if event.kind != "tool_call":
                    continue
                receipt = await agent.steer(
                    "Use the agent-level steering path.",
                    id="agent-steer",
                )
                assert receipt.id == "agent-steer"
                assert receipt.kind == "steering"
                assert receipt.queued
                assert receipt.run_id
                assert receipt.session_id
                release.set()
            result = await agent_run.result()
            assert result.output == "done"

    asyncio.run(run())
    assert len(captured_messages) == 2


def test_agent_steer_rejects_ambiguous_direct_active_runs() -> None:
    release = asyncio.Event()

    @tool(parameters_schema={"type": "object", "properties": {}})
    async def wait(args: dict[str, object]) -> dict[str, bool]:
        await release.wait()
        return {"ok": True}

    def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
        if len(messages) == 1:
            return {"tool_calls": [{"id": "call_wait", "name": "wait", "arguments": {}}]}
        return {"text": "done"}

    async def wait_for_tool_call(agent_run: starweaver.AgentRun) -> None:
        while True:
            event = await agent_run.recv()
            assert event is not None
            if event.kind == "tool_call":
                return

    async def run() -> None:
        async with create_agent(model=FunctionModel(respond), tools=[wait]) as agent:
            first = agent.run_stream("first")
            second = agent.run_stream("second")
            await asyncio.gather(wait_for_tool_call(first), wait_for_tool_call(second))

            with pytest.raises(StateError, match="exactly one direct active run"):
                await agent.steer("ambiguous")

            release.set()
            assert (await first.result()).output == "done"
            assert (await second.result()).output == "done"

    asyncio.run(run())


def test_active_run_steering_reaches_python_runtime_context() -> None:
    release = asyncio.Event()
    captured_messages: list[list[object]] = []
    events: list[starweaver.StreamEvent] = []
    receipts: list[starweaver.ControlReceipt] = []

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
                        assert receipt.run_id
                        assert receipt.session_id
                        duplicate = await agent_run.steer(
                            "Use the safe rollout path.",
                            id="ui-1",
                        )
                        assert duplicate.id == "ui-1"
                        assert duplicate.kind == "steering"
                        assert duplicate.queued
                        assert duplicate.run_id == receipt.run_id
                        assert duplicate.session_id == receipt.session_id
                        receipts.extend([receipt, duplicate])
                        release.set()
                result = await agent_run.result()
            assert session.active_run is None
        assert result.output == "done"

    asyncio.run(run())
    assert len(captured_messages) == 2
    assert len(receipts) == 2
    assert receipts[0].run_id == receipts[1].run_id
    assert receipts[0].session_id == receipts[1].session_id
    custom_kinds = [event.sideband_kind for event in events if event.kind == "custom"]
    assert custom_kinds.count("steering_submitted") == 1
    assert custom_kinds.count("steering_received") == 1


def test_active_message_send_preserves_fields_and_idempotency_without_steering() -> None:
    release = asyncio.Event()
    captured_messages: list[list[object]] = []
    events: list[starweaver.StreamEvent] = []
    receipts: list[starweaver.ControlReceipt] = []

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
                    assert first.receipt.run_id
                    assert first.receipt.session_id
                    assert second.receipt.run_id == first.receipt.run_id
                    assert second.receipt.session_id == first.receipt.session_id
                    receipts.extend([first.receipt, second.receipt])
                    release.set()
                result = await agent_run.result()
            state = session.export_full_state()
        assert result.output == "done"
        assert len(captured_messages) == 2
        assert len(receipts) == 2
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
                    continuation = await agent_run.hitl().resume(approvals=[decision])
                    assert session.active_run is continuation
                    resumed = await continuation.result()
                    break
            assert resumed is not None
            assert resumed.output == "deployed"
            assert (await agent_run.result()).is_waiting
            assert (await agent_run.join()).result.is_waiting

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


def test_direct_stream_hitl_resume_returns_live_continuation() -> None:
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
                continuation = await agent_run.hitl().resume(approvals=[decision])
                assert (await continuation.result()).output == "deployed"
                assert (await agent_run.result()).is_waiting
                break

    asyncio.run(run())


def test_terminal_run_rejects_new_steering() -> None:
    async def run() -> None:
        agent_run = create_agent(model=StarweaverTestModel.text("done")).run_stream("done")
        assert (await agent_run.result()).output == "done"
        with pytest.raises(StateError, match="already completed"):
            await agent_run.steer("too late")

    asyncio.run(run())


def test_claw_like_runtime_example_uses_in_process_sdk_path(tmp_path: Path) -> None:
    example_path = (
        Path(__file__).resolve().parents[3] / "examples" / "python" / "claw_like_runtime.py"
    )
    spec = importlib.util.spec_from_file_location("claw_like_runtime", example_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["claw_like_runtime"] = module
    try:
        spec.loader.exec_module(module)
    finally:
        sys.modules.pop("claw_like_runtime", None)
    run_claw_like_smoke = module.run_claw_like_smoke

    async def run() -> None:
        result = await run_claw_like_smoke(tmp_path / "claw-like.sqlite3")
        assert result["output"] == "deployment complete"
        assert result["steering_seen"] is True
        assert result["raw_stream_records"] > 0
        assert result["archived_records"] == result["raw_stream_records"]
        assert result["replay_events"] == 1
        assert result["restored_messages"] > 0

    asyncio.run(run())


def test_claw_product_runtime_example_covers_service_state_machine(tmp_path: Path) -> None:
    example_path = (
        Path(__file__).resolve().parents[3] / "examples" / "python" / "claw_product_runtime.py"
    )
    spec = importlib.util.spec_from_file_location("claw_product_runtime", example_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["claw_product_runtime"] = module
    try:
        spec.loader.exec_module(module)
    finally:
        sys.modules.pop("claw_product_runtime", None)

    async def run() -> None:
        service_database_path = tmp_path / "claw-product-service.sqlite3"
        service = module.create_product_service_app(
            service_database_path,
            module.ProductServiceConfig(
                api=module.ProductApiConfig(
                    expected_authorization="Bearer product-api",
                    cors_origins=("https://product.example",),
                ),
                start_bridge_supervisor=True,
            ),
        )
        assert service.ready()["ready"] is False
        with pytest.raises(RuntimeError, match="runtime is not started"):
            await service.runtime.create_session()
        migration = service.migrate()
        assert migration["native_store_current"] is True
        async with service.lifespan():
            ready = service.ready()
            assert ready["ready"] is True
            assert ready["startup_recovery_completed"] is True
            assert ready["service"]["factory"] == "fastapi_compatible_product_facade"
            assert "POST /api/v1/sessions" in ready["service"]["routes"]
            doctor = service.doctor()
            assert doctor["startup"]["steps"] == [
                "settings_loaded",
                "data_directories_ready",
                "product_database_migrated",
                "native_store_opened",
                "runtime_state_built",
                "workspace_provider_built",
                "profile_resolver_built",
                "runtime_registered",
                "startup_recovery_completed",
                "supervisors_available",
                "service_ready",
            ]
            assert doctor["startup"]["recovered_orphan_runs"] == 0
            assert doctor["stores"]["native_store_current"] is True
            assert doctor["notification_hub"]["available"] is True
            assert doctor["workspace_provider"]["backend"] == "virtual"
            assert doctor["supervisors"]["dispatcher"]["running"] is True
            assert doctor["supervisors"]["bridge"]["available"] is True
            assert doctor["api"]["auth"]["scheme"] == "bearer"
            assert doctor["api"]["cors"]["origins"] == ["https://product.example"]
            assert doctor["static"]["fallback"] == "web/index.html"
            assert service.api.ready(authorization="Bearer product-api")["ready"] is True
        assert service.ready()["ready"] is False

        database_path = tmp_path / "claw-product.sqlite3"
        result = await module.run_product_runtime_smoke(database_path)
        assert result["queued_behavior"] == "created"
        assert result["steered_behavior"] == "steered"
        assert result["suspended_status"] == "hitl"
        assert result["completed_status"] == "completed"
        assert result["output"] == "deployment complete"
        assert result["bridge_hitl_message_status"] == "completed"
        assert result["bridge_hitl_approval_id_preserved"] is True
        assert result["bridge_hitl_run_status"] == "completed"
        assert result["async_run_status"] == "completed"
        assert result["async_run_output"] == "async task lifecycle complete"
        assert result["async_task_status"] == "cancelled"
        assert result["async_task_transcript"] == 3
        assert result["async_task_wake_parent"] is True
        assert result["background_async_spawn_output"] == "background async task spawned"
        assert result["background_async_task_status"] == "completed"
        assert result["background_async_task_output"] == "background async task complete"
        assert result["background_async_task_wake_parent"] is True
        assert result["background_async_worker_output"] == "background async task complete"
        assert result["trace_run_status"] == "completed"
        assert result["trace_run_output"] == "session trace inspected"
        assert result["trace_summary_status"] == "completed"
        assert result["trace_summary_raw_records"] > 0
        assert result["trace_summary_terminal"] == "suspended"
        assert result["schedule_fire_status"] == "completed"
        assert result["schedule_run_output"] == "scheduled run complete"
        assert result["heartbeat_fire_status"] == "completed"
        assert result["heartbeat_run_output"] == "heartbeat run complete"
        assert result["workflow_run_status"] == "completed"
        assert result["workflow_node_count"] == 2
        assert result["workflow_node_outputs"] == [
            "workflow node plan complete",
            "workflow node execute complete",
        ]
        assert result["schedule_workflow_inspection_status"] == "completed"
        assert result["schedule_workflow_inspection_output"] == "schedule workflow inspected"
        assert result["memory_entry_summary"] == "memory entry extracted"
        assert result["memory_run_output"] == "memory entry extracted"
        assert result["agency_fire_status"] == "completed"
        assert result["agency_run_output"] == "agency fire complete"
        assert result["memory_agency_inspection_status"] == "completed"
        assert result["memory_agency_inspection_output"] == "memory agency inspected"
        assert result["api_auth_rejected"] is True
        assert result["api_ready"] is True
        assert result["api_session_status"] == "created"
        assert result["api_submit_status"] == "queued"
        assert result["api_run_output"] == "scheduled run complete"
        assert result["api_notification_sse_events"] > result["api_notification_sse_after_first"]
        assert result["api_run_sse_events"] > 0
        assert result["api_run_sse_first_event"] == "run.display"
        assert result["api_sandbox_status"] == "stopped"
        assert result["api_sandbox_cleanup_cleaned"] > 0
        assert result["api_sandbox_cleanup_contains_run"] is True
        assert result["api_sandbox_status_after_cleanup"] == "cleaned"
        assert result["dispatcher_schedule_fires"] == 1
        assert result["dispatcher_heartbeat_fired"] is True
        assert result["dispatcher_loop_started"] is True
        assert result["dispatcher_loop_stopped"] is True
        assert result["workspace_backend"] == "virtual"
        assert result["workspace_default_cwd"] == "/environment/workspace"
        assert len(result["workspace_fingerprint"]) == 64
        assert result["sandbox_status"] == "stopped"
        assert result["raw_records"] > 0
        assert result["display_messages"] == result["raw_records"]
        assert result["replay_events"] > 0
        assert result["notifications"] > 0
        assert result["ready"] is True

        runtime = module.ClawProductRuntime(database_path)
        await runtime.start()
        try:
            merge_session = await runtime.create_session()
            queued = await runtime.submit(merge_session, "first")
            merged = await runtime.submit(merge_session, "second")
            assert merged.run_id == queued.run_id
            assert merged.behavior == "merged"

            orphan_session = await runtime.create_session()
            orphan = await runtime.submit(orphan_session, "recover me")
            runtime.force_orphan_running(orphan.run_id)
        finally:
            await runtime.shutdown()

        recovered = module.ClawProductRuntime(database_path)
        await recovered.start()
        try:
            recovered_run = recovered.database.run(orphan.run_id)
            assert recovered_run["status"] == "queued"
            assert recovered_run["behavior"] == "recovered"
            assert any(
                notification["topic"] == "runtime.recovered"
                for notification in recovered.notifications.replay()
            )
            doctor = recovered.doctor()
            assert doctor["ready"] is True
            assert doctor["native_store_current"] is True
            assert doctor["counts"]["product_sessions"] >= 3
            assert doctor["counts"]["product_runs"] >= 1
            assert doctor["counts"]["product_async_tasks"] == 2
            assert doctor["counts"]["product_schedule_fires"] == 2
            assert doctor["counts"]["product_heartbeat_fires"] == 2
            assert doctor["counts"]["product_workflow_runs"] == 1
            assert doctor["counts"]["product_workflow_node_runs"] == 2
            assert doctor["counts"]["product_memory_entries"] == 1
            assert doctor["counts"]["product_agency_sessions"] == 1
            assert doctor["counts"]["product_agency_fires"] == 1
            assert doctor["counts"]["product_bridge_conversations"] == 1
            assert doctor["counts"]["product_bridge_events"] == 2
            assert doctor["counts"]["product_bridge_hitl_messages"] == 1
            assert (
                recovered.schedule_fire_details(result["schedule_fire_id"])["status"] == "completed"
            )
            assert (
                recovered.heartbeat_fire_details(result["heartbeat_fire_id"])["status"]
                == "completed"
            )
            workflow_run = recovered.workflow_run_details(result["workflow_run_id"])
            assert workflow_run["status"] == "completed"
            assert [node["status"] for node in workflow_run["nodes"]] == [
                "completed",
                "completed",
            ]
            assert (
                recovered.memory_entry_details(result["memory_entry_id"])["summary"]
                == "memory entry extracted"
            )
            assert recovered.agency_fire_details(result["agency_fire_id"])["status"] == "completed"
            async_task = recovered.async_task_details("async_task_demo")
            assert async_task["status"] == "cancelled"
            assert async_task["result"]["wake_parent"] is True
            assert [entry["kind"] for entry in async_task["transcript"]] == [
                "spawned",
                "steering",
                "cancelled",
            ]
            background_task = recovered.async_task_details("background_task_demo")
            assert background_task["status"] == "completed"
            assert background_task["worker_run_id"]
            assert background_task["result"]["output"] == "background async task complete"
            assert background_task["wake_parent"]["parent_session_id"] == "product-session"
            profile_toolsets = recovered.resolver.resolve().toolset_factory()
            toolset_ids = {toolset.id for toolset in profile_toolsets}
            assert toolset_ids == {
                "claw.product.service",
                "claw.product.async_tasks",
                "claw.product.session_trace",
                "claw.product.schedule_workflow",
                "claw.product.memory_agency",
            }
            trace_summary = await recovered.trace_summary(result["run_id"])
            assert trace_summary["raw_records"] == result["trace_summary_raw_records"]
            assert trace_summary["display_messages"] == result["display_messages"]
            assert trace_summary["replay_events"] == result["replay_events"]
            details = recovered.run_details(result["run_id"])
            workspace_snapshot = details["workspace_snapshot"]
            assert workspace_snapshot["format"] == "claw.product.workspace"
            assert workspace_snapshot["backend"] == "virtual"
            assert workspace_snapshot["default_cwd"] == "/environment/workspace"
            assert (
                workspace_snapshot["environment_state"]["metadata"]["provider_kind"] == "composite"
            )
            assert (
                workspace_snapshot["environment_state"]["metadata"]["mounts"][1]["mode"]
                == "read_only"
            )
            assert details["sandbox_status"]["status"] == "cleaned"
            assert details["sandbox_status"]["previous_status"] == "stopped"
            assert details["sandbox_status"]["cleanup_reason"] == "ttl_expired"
            assert (
                details["sandbox_status"]["workspace_fingerprint"]
                == workspace_snapshot["fingerprint"]
            )
            api = module.ProductApi(
                recovered,
                module.ProductApiConfig(expected_authorization="Bearer product-api"),
            )
            with pytest.raises(module.ProductApiAuthError):
                api.doctor(authorization="Bearer wrong")
            assert api.doctor(authorization="Bearer product-api")["ready"] is True
            assert (
                api.sandbox_status(
                    result["run_id"],
                    authorization="Bearer product-api",
                )["status"]
                == "cleaned"
            )
            assert api.notification_sse(authorization="Bearer product-api")
            run_events = await api.run_sse(
                result["run_id"],
                authorization="Bearer product-api",
            )
            assert run_events[0]["event"] == "run.display"

            with pytest.raises(ValueError, match="default_cwd"):
                await recovered.create_session(
                    workspace={
                        "backend": "virtual",
                        "binding_id": "invalid-workspace",
                        "default_cwd": "/host/path",
                        "mounts": [
                            {
                                "id": "workspace",
                                "files": {"README.md": "invalid"},
                                "default": True,
                            }
                        ],
                    }
                )

            profile = recovered.resolver.resolve()
            workspace_runtime = await recovered.workspace_factory.runtime_for(
                workspace_snapshot,
            )
            collected = recovered.runtime_builder.build_runtime(
                profile,
                durable_session_id="collected-product-session",
                environment=workspace_runtime.environment,
            )
            assert collected.durable_session_id == "collected-product-session"
        finally:
            await recovered.shutdown()

    asyncio.run(run())
