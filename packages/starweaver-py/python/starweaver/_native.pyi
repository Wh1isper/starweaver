from collections.abc import Awaitable
from typing import Any

__version__: str

def version() -> str: ...
def sleep_echo(value: object, delay_ms: int = 0) -> Awaitable[Any]: ...

class TestModel:
    def __init__(self, text: str | None = None, responses: object | None = None) -> None: ...
    @staticmethod
    def text(text: str) -> TestModel: ...
    @staticmethod
    def responses(responses: object) -> TestModel: ...
    @staticmethod
    def tool_call_response(calls: object) -> object: ...
    def captured_messages(self) -> list[object]: ...
    def captured_params(self) -> list[object]: ...

class FunctionModel:
    def __init__(
        self,
        callback: object,
        event_loop: object,
        model_name: str | None = None,
    ) -> None: ...
    def captured_messages(self) -> list[object]: ...
    def captured_params(self) -> list[object]: ...

class ModelSettings:
    def __init__(self, value: object | None = None, **kwargs: object) -> None: ...
    @staticmethod
    def preset(name: str) -> ModelSettings: ...
    def to_dict(self) -> dict[str, object]: ...

class RequestParams:
    def __init__(self, value: object | None = None, **kwargs: object) -> None: ...
    def to_dict(self) -> dict[str, object]: ...

class ProviderModel:
    @staticmethod
    def from_model_id(
        model_id: str,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: object | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel: ...
    @staticmethod
    def codex_oauth(
        model_name: str,
        model_settings: object | None = None,
    ) -> ProviderModel: ...
    @staticmethod
    def openai_responses(
        model_name: str,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: object | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel: ...
    @staticmethod
    def openai_chat(
        model_name: str,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: object | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel: ...
    @staticmethod
    def anthropic(
        model_name: str,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: object | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel: ...
    @staticmethod
    def gemini(
        model_name: str,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: object | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel: ...

class OutputSchema:
    def __init__(
        self,
        name: str,
        schema: object,
        description: str | None = None,
        strict: bool = True,
    ) -> None: ...
    def request_schema(self) -> dict[str, object]: ...
    def to_dict(self) -> dict[str, object]: ...

class OutputContext:
    @property
    def run_id(self) -> str: ...
    @property
    def conversation_id(self) -> str: ...
    @property
    def run_step(self) -> int: ...
    @property
    def status(self) -> str: ...
    @property
    def metadata(self) -> dict[str, object]: ...
    def raw_state(self) -> dict[str, object]: ...

class OutputValue:
    @staticmethod
    def text(value: str) -> OutputValue: ...
    @staticmethod
    def json(value: object) -> OutputValue: ...
    def to_python(self) -> object: ...

class OutputValidator:
    def __init__(self, callback: object, event_loop: object) -> None: ...

class OutputFunction:
    def __init__(
        self,
        name: str,
        parameters_schema: object,
        callback: object,
        event_loop: object,
        description: str | None = None,
    ) -> None: ...
    def definition_json(self) -> dict[str, object]: ...

class OutputPolicy:
    def __init__(self) -> None: ...
    @staticmethod
    def text() -> OutputPolicy: ...
    @staticmethod
    def structured(schema: object) -> OutputPolicy: ...
    @staticmethod
    def auto(schema: object) -> OutputPolicy: ...
    @staticmethod
    def native_json_schema(schema: object) -> OutputPolicy: ...
    @staticmethod
    def native_json_object(schema: object) -> OutputPolicy: ...
    @staticmethod
    def tool(schema: object) -> OutputPolicy: ...
    @staticmethod
    def tool_or_text(schema: object) -> OutputPolicy: ...
    @staticmethod
    def prompted(schema: object) -> OutputPolicy: ...
    @staticmethod
    def image() -> OutputPolicy: ...
    def with_retries(self, retries: int) -> OutputPolicy: ...
    def with_mode(self, mode: str) -> OutputPolicy: ...
    def allow_text_output(self, allow: bool) -> OutputPolicy: ...
    def allow_image_output(self, allow: bool) -> OutputPolicy: ...
    def with_validator(self, validator: OutputValidator) -> OutputPolicy: ...
    def with_function(self, function: OutputFunction) -> OutputPolicy: ...

class CapabilityBundle:
    def __init__(
        self,
        name: str,
        instructions: list[str] | None = None,
        tools: list[PythonTool] | None = None,
        model_settings: object | None = None,
        request_params: object | None = None,
        output_validators: list[OutputValidator] | None = None,
        output_functions: list[OutputFunction] | None = None,
    ) -> None: ...

class EnvironmentProvider:
    @staticmethod
    def virtual_provider(
        id: str = "virtual",  # noqa: A002
        files: object | None = None,
        resources: object | None = None,
        shell_outputs: object | None = None,
        tmp_namespace: str | None = None,
    ) -> EnvironmentProvider: ...
    @staticmethod
    def local(
        root: str,
        id: str | None = None,  # noqa: A002
        allowed_paths: list[str] | None = None,
        context_file_tree_roots: list[str] | None = None,
        writable: bool = False,
        allow_shell: bool = False,
        allowed_programs: list[str] | None = None,
        tmp_namespace: str | None = None,
    ) -> EnvironmentProvider: ...
    @property
    def id(self) -> str: ...
    def read_text(self, path: str) -> Awaitable[str]: ...
    def read_bytes(
        self,
        path: str,
        offset: int,
        length: int | None = None,
    ) -> Awaitable[bytes]: ...
    def write_text(self, path: str, content: str) -> Awaitable[None]: ...
    def write_tmp_file(self, filename: str, content: object) -> Awaitable[str]: ...
    def create_dir(self, path: str, parents: bool) -> Awaitable[None]: ...
    def delete_path(self, path: str, recursive: bool) -> Awaitable[None]: ...
    def list(self, path: str) -> Awaitable[list[str]]: ...
    def list_with_options(
        self,
        path: str,
        max_entries: int = 0,
        ignore_patterns: list[str] | None = None,
    ) -> Awaitable[dict[str, object]]: ...
    def stat(self, path: str) -> Awaitable[dict[str, object]]: ...
    def glob(
        self,
        path: str,
        pattern: str,
        include_hidden: bool = False,
        include_ignored: bool = False,
        max_results: int = 500,
    ) -> Awaitable[list[dict[str, object]]]: ...
    def grep(
        self,
        path: str,
        pattern: str,
        include: str | None = None,
        context_lines: int = 0,
        max_results: int = 100,
        max_matches_per_file: int = 20,
        max_files: int = 50,
        include_hidden: bool = False,
        include_ignored: bool = False,
    ) -> Awaitable[list[dict[str, object]]]: ...
    def run_shell(
        self,
        command: str,
        timeout_seconds: int | None = None,
        cwd: str | None = None,
        environment: object | None = None,
    ) -> Awaitable[dict[str, object]]: ...
    def export_state(self) -> Awaitable[dict[str, object]]: ...

class MediaUploader:
    def __init__(self, callback: object, event_loop: object) -> None: ...

class Subagent:
    def __init__(
        self,
        name: str,
        agent: object,
        description: str | None = None,
        required_tools: list[str] | None = None,
        optional_tools: list[str] | None = None,
        denied_tools: list[str] | None = None,
        auto_inherit: bool = True,
        inherit_all_when_empty: bool = False,
        allow_nested_delegation: bool = False,
        inherit_hooks: bool = False,
        inherit_capability_bundles: bool = False,
        denied_capabilities: list[str] | None = None,
    ) -> None: ...

class ToolContext:
    @property
    def run_id(self) -> str: ...
    @property
    def conversation_id(self) -> str: ...
    @property
    def run_step(self) -> int: ...
    @property
    def retry(self) -> int: ...
    @property
    def max_retries(self) -> int: ...
    @property
    def metadata(self) -> dict[str, object]: ...
    @property
    def approval(self) -> dict[str, object] | None: ...
    @property
    def deferred_result(self) -> object | None: ...
    def is_cancelled(self) -> bool: ...

class ToolResult:
    def __init__(
        self,
        content: object,
        metadata: object | None = None,
        app_value: object | None = None,
        model_content: object | None = None,
        user_content: object | None = None,
        private_metadata: object | None = None,
    ) -> None: ...
    @property
    def content(self) -> object: ...
    @property
    def metadata(self) -> dict[str, object]: ...
    @property
    def app_value(self) -> object | None: ...
    @property
    def model_content(self) -> object | None: ...
    @property
    def user_content(self) -> object | None: ...
    @property
    def private_metadata(self) -> dict[str, object]: ...

class PythonTool:
    def __init__(
        self,
        name: str,
        description: str | None,
        parameters_schema: object,
        callback: object,
        event_loop: object,
        return_schema: object | None = None,
        metadata: object | None = None,
        strict: bool | None = None,
        sequential: bool | None = None,
        timeout_ms: int | None = None,
        max_retries: int | None = None,
    ) -> None: ...
    @property
    def name(self) -> str: ...
    def definition_json(self) -> dict[str, object]: ...

class Toolset:
    def __init__(
        self,
        name: str,
        tools: list[PythonTool] | None = None,
        instructions: list[str] | None = None,
        id: str | None = None,  # noqa: A002
        max_retries: int | None = None,
        timeout_ms: int | None = None,
    ) -> None: ...
    @property
    def name(self) -> str: ...
    @property
    def id(self) -> str | None: ...
    def tool_definitions(self) -> list[dict[str, object]]: ...
    def instructions(self) -> list[dict[str, object]]: ...

def tool_search_toolset(
    toolsets: list[Toolset],
    max_results: int | None = None,
) -> Toolset: ...
def tool_proxy_toolset(
    toolsets: list[Toolset],
    prefix: str | None = None,
    max_results: int | None = None,
) -> Toolset: ...
def filesystem_toolset() -> Toolset: ...
def shell_toolset() -> Toolset: ...
def environment_toolsets() -> list[Toolset]: ...

class SkillPackage:
    def __init__(
        self,
        name: str,
        description: str,
        path: str,
        body: str | None = None,
        metadata: object | None = None,
    ) -> None: ...
    @property
    def name(self) -> str: ...
    @property
    def description(self) -> str: ...
    @property
    def path(self) -> str: ...
    @property
    def body(self) -> str | None: ...
    @property
    def metadata(self) -> dict[str, object]: ...
    def summary_line(self) -> str: ...
    def to_dict(self) -> dict[str, object]: ...

class SkillRegistry:
    def __init__(self, packages: list[SkillPackage] | None = None) -> None: ...
    @staticmethod
    def parse(path: str, content: str) -> SkillPackage: ...
    @staticmethod
    def scan(
        environment: EnvironmentProvider,
        scopes: object | None = None,
    ) -> Awaitable[SkillRegistry]: ...
    @staticmethod
    def scan_with_report(
        environment: EnvironmentProvider,
        scopes: object | None = None,
    ) -> Awaitable[dict[str, object]]: ...
    @staticmethod
    def activate(
        environment: EnvironmentProvider,
        path: str,
    ) -> Awaitable[SkillPackage]: ...
    def insert(self, package: SkillPackage) -> None: ...
    def get(self, name: str) -> SkillPackage | None: ...
    @property
    def packages(self) -> list[SkillPackage]: ...
    @property
    def is_empty(self) -> bool: ...
    def toolset(self) -> Toolset: ...
    def to_dict(self) -> dict[str, object]: ...

class RunResult:
    @property
    def output(self) -> str: ...
    @property
    def structured_output(self) -> object | None: ...
    @property
    def messages(self) -> list[object]: ...
    @property
    def raw_state(self) -> dict[str, object]: ...
    @property
    def status(self) -> str: ...
    @property
    def is_waiting(self) -> bool: ...
    @property
    def needs_approval(self) -> bool: ...
    @property
    def pending_approvals(self) -> list[dict[str, object]]: ...
    @property
    def pending_deferred_tools(self) -> list[dict[str, object]]: ...
    @property
    def pending_deferred(self) -> list[dict[str, object]]: ...

class StreamEvent:
    @property
    def kind(self) -> str: ...
    @property
    def raw(self) -> Any: ...

class StreamRunResult:
    @property
    def result(self) -> RunResult: ...
    @property
    def events(self) -> list[StreamEvent]: ...

class AgentStream:
    def recv(self) -> Awaitable[StreamEvent | None]: ...
    def interrupt(self, reason: str | None = None) -> None: ...
    def steer(self, text: str, id: str | None = None) -> Awaitable[dict[str, object]]: ...  # noqa: A002
    def send_message(self, message: object) -> Awaitable[dict[str, object]]: ...
    def join(self) -> Awaitable[StreamRunResult]: ...
    def result(self) -> Awaitable[RunResult]: ...
    def resume_after_hitl(
        self,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> Awaitable[RunResult]: ...
    def resume_after_hitl_for_state(
        self,
        state: object,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> Awaitable[RunResult]: ...
    def recoverable_state(self) -> Awaitable[dict[str, object]]: ...
    def status(self) -> dict[str, object]: ...

class AgentSession:
    def run(self, prompt: str) -> Awaitable[RunResult]: ...
    def stream(
        self,
        prompt: str,
        instructions: list[str] | None = None,
        tools: list[PythonTool] | None = None,
        replace_tools: bool = False,
        model_settings: object | None = None,
        request_params: object | None = None,
        output_schema: object | None = None,
        output_policy: object | None = None,
        toolsets: list[Toolset] | None = None,
        environment: EnvironmentProvider | None = None,
    ) -> AgentStream: ...
    def export_state(self, mode: str | None = None) -> dict[str, object]: ...
    def set_environment(self, environment: EnvironmentProvider) -> None: ...
    def export_environment_state(self) -> Awaitable[dict[str, object] | None]: ...
    def steer(self, text: str, id: str | None = None) -> Awaitable[dict[str, object]]: ...  # noqa: A002
    def interrupt(self, reason: str | None = None) -> None: ...
    def message_send(self, message: object) -> dict[str, object]: ...
    def message_peek(self, agent_id: str | None = None) -> list[dict[str, object]]: ...
    def message_consume(self, agent_id: str | None = None) -> list[dict[str, object]]: ...
    def message_subscribe(self, agent_id: str | None = None) -> None: ...
    def message_unsubscribe(self, agent_id: str | None = None) -> None: ...
    def resume_after_hitl(
        self,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> Awaitable[RunResult]: ...
    def resume_after_hitl_for_state(
        self,
        state: object,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> Awaitable[RunResult]: ...

class Agent:
    def __init__(
        self,
        model: object,
        tools: list[PythonTool] | None = None,
        instructions: list[str] | None = None,
        name: str | None = None,
        model_settings: object | None = None,
        request_params: object | None = None,
        output_schema: object | None = None,
        output_policy: object | None = None,
        subagents: list[Subagent] | None = None,
        subagent_delegation_mode: str | None = None,
        capability_bundles: list[CapabilityBundle] | None = None,
        toolsets: list[Toolset] | None = None,
        runtime_config: object | None = None,
        skills: SkillRegistry | None = None,
        environment: EnvironmentProvider | None = None,
        media_uploader: MediaUploader | None = None,
    ) -> None: ...
    def run(self, prompt: str) -> Awaitable[RunResult]: ...
    def stream(
        self,
        prompt: str,
        instructions: list[str] | None = None,
        tools: list[PythonTool] | None = None,
        replace_tools: bool = False,
        model_settings: object | None = None,
        request_params: object | None = None,
        output_schema: object | None = None,
        output_policy: object | None = None,
        toolsets: list[Toolset] | None = None,
        environment: EnvironmentProvider | None = None,
    ) -> AgentStream: ...
    def new_session(self, environment: EnvironmentProvider | None = None) -> AgentSession: ...
    def session_from_state(
        self,
        state: object,
        environment: EnvironmentProvider | None = None,
    ) -> AgentSession: ...
