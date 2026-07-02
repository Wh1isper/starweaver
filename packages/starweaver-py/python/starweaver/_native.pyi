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
    def interrupt(self) -> None: ...
    def join(self) -> Awaitable[StreamRunResult]: ...
    def result(self) -> Awaitable[RunResult]: ...
    def recoverable_state(self) -> Awaitable[dict[str, object]]: ...
    def status(self) -> dict[str, object]: ...

class AgentSession:
    def run(self, prompt: str) -> Awaitable[RunResult]: ...
    def run_stream_collect(self, prompt: str) -> Awaitable[StreamRunResult]: ...
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
    ) -> AgentStream: ...
    def export_state(self, mode: str | None = None) -> dict[str, object]: ...
    def resume_after_hitl(
        self,
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
    ) -> None: ...
    def run(self, prompt: str) -> Awaitable[RunResult]: ...
    def run_stream_collect(self, prompt: str) -> Awaitable[StreamRunResult]: ...
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
    ) -> AgentStream: ...
    def new_session(self) -> AgentSession: ...
    def session_from_state(self, state: object) -> AgentSession: ...
