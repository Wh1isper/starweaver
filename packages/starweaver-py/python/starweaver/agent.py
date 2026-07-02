"""Python agent and session facade."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Callable, Iterable
from typing import Any

from . import _native
from .capability import CapabilityBundle, ensure_capability_bundle
from .model import ModelSettings, RequestParams, ensure_model_settings, ensure_request_params
from .output import OutputPolicy, OutputSchema, ensure_output_policy, ensure_output_schema
from .subagent import Subagent, ensure_subagent
from .tool import BaseTool, Tool, ensure_tool


class Agent:
    """Python facade over a native Starweaver agent."""

    def __init__(self, native: _native.Agent) -> None:
        self._native = native

    async def __aenter__(self) -> Agent:
        return self

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> None:
        return None

    async def run(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
    ) -> _native.RunResult:
        return await self.run_stream(
            prompt,
            instructions=instructions,
            tools=tools,
            replace_tools=replace_tools,
            model_settings=model_settings,
            request_params=request_params,
            output_schema=output_schema,
            output_policy=output_policy,
        ).result()

    def run_stream(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
    ) -> AgentStream:
        if output_schema is not None and output_policy is not None:
            raise ValueError("pass output_schema or output_policy, not both")
        native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
        return AgentStream(
            self._native.stream(
                prompt,
                list(instructions or ()),
                native_tools,
                replace_tools,
                ensure_model_settings(model_settings),
                ensure_request_params(request_params),
                ensure_output_schema(output_schema),
                ensure_output_policy(output_policy),
            )
        )

    def new_session(self) -> AgentSession:
        return AgentSession(self._native.new_session())

    def session_from_state(self, state: dict[str, Any]) -> AgentSession:
        return AgentSession(self._native.session_from_state(state))


class AgentSession:
    """Stateful Python facade over a Starweaver agent session."""

    def __init__(self, native: _native.AgentSession) -> None:
        self._native = native

    async def run(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
    ) -> _native.RunResult:
        return await self.run_stream(
            prompt,
            instructions=instructions,
            tools=tools,
            replace_tools=replace_tools,
            model_settings=model_settings,
            request_params=request_params,
            output_schema=output_schema,
            output_policy=output_policy,
        ).result()

    def run_stream(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
    ) -> AgentStream:
        if output_schema is not None and output_policy is not None:
            raise ValueError("pass output_schema or output_policy, not both")
        native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
        return AgentStream(
            self._native.stream(
                prompt,
                list(instructions or ()),
                native_tools,
                replace_tools,
                ensure_model_settings(model_settings),
                ensure_request_params(request_params),
                ensure_output_schema(output_schema),
                ensure_output_policy(output_policy),
            )
        )

    def export_state(self, mode: str = "curated") -> dict[str, Any]:
        return self._native.export_state(mode)

    async def resume_after_hitl(
        self,
        *,
        approvals: dict[str, Any] | None = None,
        deferred_results: dict[str, Any] | None = None,
    ) -> _native.RunResult:
        return await self._native.resume_after_hitl(approvals, deferred_results)


class AgentStream:
    """Live stream handle for one agent run."""

    def __init__(self, native: _native.AgentStream) -> None:
        self._native = native
        self._joined: _native.StreamRunResult | None = None

    async def __aenter__(self) -> AgentStream:
        return self

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> None:
        if exc_type is not None:
            self.interrupt()
        if self._joined is None:
            try:
                await self.join()
            except Exception:
                if exc_type is None:
                    raise

    def __aiter__(self) -> AsyncIterator[_native.StreamEvent]:
        return self

    async def __anext__(self) -> _native.StreamEvent:
        event = await self.recv()
        if event is None:
            await self.join()
            raise StopAsyncIteration
        return event

    async def recv(self) -> _native.StreamEvent | None:
        try:
            return await self._native.recv()
        except asyncio.CancelledError:
            self.interrupt()
            raise

    def interrupt(self) -> None:
        self._native.interrupt()

    def status(self) -> dict[str, Any]:
        return self._native.status()

    async def recoverable_state(self) -> dict[str, Any]:
        return await self._native.recoverable_state()

    async def join(self) -> _native.StreamRunResult:
        if self._joined is not None:
            return self._joined
        try:
            self._joined = await self._native.join()
            return self._joined
        except asyncio.CancelledError:
            self.interrupt()
            raise

    async def result(self) -> _native.RunResult:
        return (await self.join()).result


def create_agent(
    *,
    model: Any,
    tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
    instructions: Iterable[str] | None = None,
    name: str | None = None,
    model_settings: ModelSettings | dict[str, Any] | None = None,
    request_params: RequestParams | dict[str, Any] | None = None,
    output_schema: OutputSchema | dict[str, Any] | None = None,
    output_policy: OutputPolicy | dict[str, Any] | None = None,
    subagents: Iterable[Subagent] | None = None,
    subagent_delegation_mode: str = "blocking",
    capability_bundles: Iterable[CapabilityBundle] | None = None,
) -> Agent:
    """Create a Python Starweaver agent."""

    if output_schema is not None and output_policy is not None:
        raise ValueError("pass output_schema or output_policy, not both")
    to_native = getattr(model, "to_native", None)
    native_model = to_native() if callable(to_native) else getattr(model, "_native", model)
    native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
    native_subagents = [ensure_subagent(subagent) for subagent in subagents or ()]
    native_bundles = [ensure_capability_bundle(bundle) for bundle in capability_bundles or ()]
    return Agent(
        _native.Agent(
            native_model,
            native_tools,
            list(instructions or ()),
            name,
            ensure_model_settings(model_settings),
            ensure_request_params(request_params),
            ensure_output_schema(output_schema),
            ensure_output_policy(output_policy),
            native_subagents,
            subagent_delegation_mode,
            native_bundles,
        )
    )
