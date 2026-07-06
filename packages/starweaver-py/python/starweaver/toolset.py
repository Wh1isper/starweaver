"""Toolset composition helpers."""

from __future__ import annotations

import asyncio
import copy
import inspect
from collections.abc import Awaitable, Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass
from enum import StrEnum
from os import PathLike
from typing import Any, Literal, cast, overload

from . import _native
from .tool import BaseTool, Tool, ensure_tool

ToolsetContext = _native.ToolsetContext
ToolsetLifecyclePolicy = _native.ToolsetLifecyclePolicy
InstructionCallback = Callable[
    [ToolsetContext],
    str | Iterable[str] | Awaitable[str | Iterable[str] | None] | None,
]
PreparedCallback = Callable[
    [ToolsetContext, list[dict[str, Any]]],
    list[dict[str, Any]] | None | Awaitable[list[dict[str, Any]] | None],
]
ToolDefinitionPredicate = Callable[[dict[str, Any]], bool]


class ToolsetLifecycleState(StrEnum):
    """Stable lifecycle states emitted by Rust toolset preparation."""

    INITIALIZED = "initialized"
    UNAVAILABLE = "unavailable"
    FAILED = "failed"
    REFRESHED = "refreshed"
    CLOSED = "closed"

    @classmethod
    def from_value(cls, value: object) -> ToolsetLifecycleState:
        try:
            return cls(str(value))
        except ValueError as error:
            raise ValueError(f"unknown toolset lifecycle state: {value!r}") from error


_TOOLSET_LIFECYCLE_EVENT_STATES: dict[str, ToolsetLifecycleState] = {
    "toolset_initialized": ToolsetLifecycleState.INITIALIZED,
    "toolset_unavailable": ToolsetLifecycleState.UNAVAILABLE,
    "toolset_failed": ToolsetLifecycleState.FAILED,
    "toolset_refreshed": ToolsetLifecycleState.REFRESHED,
    "toolset_closed": ToolsetLifecycleState.CLOSED,
}
_TOOLSET_LIFECYCLE_STATE_EVENTS = {
    state: event_kind for event_kind, state in _TOOLSET_LIFECYCLE_EVENT_STATES.items()
}


@dataclass(frozen=True)
class ToolsetLifecycleReport:
    """Typed view over a Rust toolset lifecycle sideband event payload."""

    raw: dict[str, Any]

    def __init__(self, raw: Mapping[str, Any]) -> None:
        data = copy.deepcopy(dict(raw))
        state = ToolsetLifecycleState.from_value(data.get("state"))
        data["state"] = state.value
        data["tool_count"] = _int_value(data.get("tool_count"), "tool_count")
        data["instruction_count"] = _int_value(data.get("instruction_count"), "instruction_count")
        if not isinstance(data.get("metadata"), Mapping):
            data["metadata"] = {}
        object.__setattr__(self, "raw", data)

    @classmethod
    def from_sideband(
        cls,
        sideband: Mapping[str, Any] | None,
    ) -> ToolsetLifecycleReport | None:
        if sideband is None:
            return None
        state = _TOOLSET_LIFECYCLE_EVENT_STATES.get(str(sideband.get("kind") or ""))
        if state is None:
            return None
        payload = sideband.get("payload")
        data = copy.deepcopy(dict(payload)) if isinstance(payload, Mapping) else {}
        data.setdefault("state", state.value)
        return cls(data)

    @property
    def name(self) -> str:
        return str(self.raw.get("name") or "")

    @property
    def id(self) -> str | None:
        value = self.raw.get("id")
        return None if value is None else str(value)

    @property
    def state(self) -> ToolsetLifecycleState:
        return ToolsetLifecycleState.from_value(self.raw["state"])

    @property
    def event_kind(self) -> str:
        return _TOOLSET_LIFECYCLE_STATE_EVENTS[self.state]

    @property
    def tool_count(self) -> int:
        return int(self.raw["tool_count"])

    @property
    def instruction_count(self) -> int:
        return int(self.raw["instruction_count"])

    @property
    def message(self) -> str | None:
        value = self.raw.get("message")
        return None if value is None else str(value)

    @property
    def metadata(self) -> dict[str, Any]:
        return copy.deepcopy(dict(self.raw.get("metadata") or {}))

    def to_dict(self) -> dict[str, Any]:
        return copy.deepcopy(self.raw)


@dataclass(frozen=True)
class ToolsetIdentity:
    """Serializable identity extracted from a Python or native toolset."""

    index: int
    name: str
    id: str | None
    source_type: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "index": self.index,
            "name": self.name,
            "id": self.id,
            "source_type": self.source_type,
        }


@dataclass(frozen=True)
class ToolsetIdIssue:
    """Durable toolset identity validation issue."""

    code: str
    message: str
    index: int
    severity: Literal["error", "warning"] = "error"
    name: str | None = None
    id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "code": self.code,
            "message": self.message,
            "index": self.index,
            "severity": self.severity,
            "name": self.name,
            "id": self.id,
        }


@dataclass(frozen=True)
class ToolsetIdValidation:
    """Result of validating toolset identities for durable products."""

    identities: tuple[ToolsetIdentity, ...]
    issues: tuple[ToolsetIdIssue, ...]

    @property
    def errors(self) -> list[ToolsetIdIssue]:
        return [issue for issue in self.issues if issue.severity == "error"]

    @property
    def warnings(self) -> list[ToolsetIdIssue]:
        return [issue for issue in self.issues if issue.severity == "warning"]

    @property
    def ok(self) -> bool:
        return not self.errors

    def raise_for_errors(self) -> None:
        if self.ok:
            return
        details = "; ".join(issue.message for issue in self.errors)
        raise ValueError(f"invalid durable toolset identities: {details}")

    def require_ids(self) -> ToolsetIdValidation:
        """Require every validated toolset to have an explicit non-empty id."""

        missing = [
            identity
            for identity in self.identities
            if identity.id is None or not identity.id.strip()
        ]
        if missing:
            details = ", ".join(
                f"{identity.name!r} at index {identity.index}" for identity in missing
            )
            raise ValueError(f"durable toolsets require explicit non-empty ids: {details}")
        self.raise_for_errors()
        return self

    def require_serializable_dynamic_state(self) -> ToolsetIdValidation:
        """Require durable dynamic toolsets to be restorable from stable ids.

        Python callable objects are process-local and are never serialized by
        Starweaver. Durable products should persist the validated ids and
        re-register current Python toolsets at process startup.
        """

        return self.require_ids()

    def to_dict(self) -> dict[str, Any]:
        return {
            "ok": self.ok,
            "identities": [identity.to_dict() for identity in self.identities],
            "issues": [issue.to_dict() for issue in self.issues],
        }


class Toolset:
    """Static group of tools and tool instructions."""

    def __init__(
        self,
        name: str,
        *,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        instructions: Iterable[str] | None = None,
        id: str | None = None,  # noqa: A002
        max_retries: int | None = None,
        timeout_ms: int | None = None,
        _native_toolset: _native.Toolset | None = None,
    ) -> None:
        self.name = name
        self.tools = tuple(ensure_tool(tool) for tool in tools or ())
        self.instructions = tuple(_collapse_instruction_strings(instructions or ()))
        self.id = id
        self.max_retries = max_retries
        self.timeout_ms = timeout_ms
        self._native = _native_toolset or _native.Toolset(
            name,
            [tool.to_native() for tool in self.tools],
            list(self.instructions),
            id,
            max_retries,
            timeout_ms,
        )

    @classmethod
    def from_native(cls, native: _native.Toolset) -> Toolset:
        return cls(native.name, id=native.id, _native_toolset=native)

    def to_native(self) -> _native.Toolset:
        return self._native

    def tool_definitions(self) -> list[dict[str, Any]]:
        return cast(list[dict[str, Any]], self._native.tool_definitions())

    def instruction_records(self) -> list[dict[str, Any]]:
        return cast(list[dict[str, Any]], self._native.instructions())

    def prefixed(self, prefix: str) -> Toolset:
        """Return a toolset whose exposed tool names use a prefix."""

        return Toolset.from_native(_native.prefixed_toolset(self.to_native(), prefix))

    def filtered(
        self,
        *,
        include: Iterable[str] | str | None = None,
        exclude: Iterable[str] | str | None = None,
        predicate: ToolDefinitionPredicate | None = None,
    ) -> Toolset:
        """Return a toolset that includes or excludes static tool names."""

        if predicate is not None:
            include_names = _normalize_name_set(include)
            exclude_names = _normalize_name_set(exclude)
            if include_names is not None and exclude_names is not None:
                raise ValueError("filtered accepts include or exclude, not both")

            def prepare(
                ctx: ToolsetContext,
                definitions: list[dict[str, Any]],
            ) -> list[dict[str, Any]]:
                del ctx
                selected: list[dict[str, Any]] = []
                for definition in definitions:
                    name = str(definition.get("name") or "")
                    if include_names is not None and name not in include_names:
                        continue
                    if exclude_names is not None and name in exclude_names:
                        continue
                    if predicate(definition):
                        selected.append(definition)
                return selected

            return self.prepared(prepare)

        return Toolset.from_native(
            _native.filtered_toolset(
                self.to_native(),
                _normalize_name_list(include),
                _normalize_name_list(exclude),
            )
        )

    def prepared(self, callback: PreparedCallback) -> Toolset:
        """Return a toolset whose model-facing definitions are prepared by callback."""

        return Toolset.from_native(
            _native.prepared_toolset(
                self.to_native(),
                callback,
                asyncio.get_running_loop(),
            )
        )

    def renamed(self, mapping: Mapping[str, str]) -> Toolset:
        """Return a toolset with selected tools exposed under new names."""

        return Toolset.from_native(_native.renamed_toolset(self.to_native(), dict(mapping)))

    def with_metadata(
        self,
        metadata: Mapping[str, Any] | None = None,
        /,
        **extra_metadata: Any,
    ) -> Toolset:
        """Return a toolset whose tools include additional definition metadata."""

        return Toolset.from_native(
            _native.metadata_toolset(
                self.to_native(),
                _merge_metadata(metadata or {}, extra_metadata),
            )
        )

    def approval_required(
        self,
        names: Iterable[str] | str = "*",
        *,
        reason: str | None = None,
    ) -> Toolset:
        """Return a toolset that raises approval control flow for matching tools."""

        return Toolset.from_native(
            _native.approval_required_toolset(
                self.to_native(),
                _normalize_required_names(names),
                reason,
            )
        )

    def deferred(
        self,
        names: Iterable[str] | str = "*",
        *,
        reason: str | None = None,
    ) -> Toolset:
        """Return a toolset that defers matching tools through HITL control flow."""

        return Toolset.from_native(
            _native.deferred_toolset(
                self.to_native(),
                _normalize_required_names(names),
                reason,
            )
        )


class ToolLibrary:
    """Serializable collection of toolsets used by search/proxy facades."""

    def __init__(self, toolsets: Iterable[Toolset]) -> None:
        self.toolsets = tuple(toolsets)

    def to_native_toolsets(self) -> list[_native.Toolset]:
        return [ensure_toolset(toolset).to_native() for toolset in self.toolsets]

    def tool_definitions(self) -> list[dict[str, Any]]:
        definitions: list[dict[str, Any]] = []
        for toolset in self.toolsets:
            definitions.extend(toolset.tool_definitions())
        return definitions

    def validate_ids(self, *, require_ids: bool = True) -> ToolsetIdValidation:
        return validate_toolset_ids(self.toolsets, require_ids=require_ids)


class ToolSearchToolset(Toolset):
    """Direct dynamic tool-search toolset."""

    def __init__(
        self,
        library: ToolLibrary | Iterable[Toolset],
        *,
        max_results: int | None = None,
    ) -> None:
        toolsets = _library_toolsets(library)
        native = _native.tool_search_toolset(
            [toolset.to_native() for toolset in toolsets],
            max_results,
        )
        super().__init__(
            "tool_search",
            _native_toolset=native,
        )
        self.library = ToolLibrary(toolsets)
        self.max_results = max_results


class ToolProxyToolset(Toolset):
    """Fixed search/call proxy over hidden toolsets."""

    def __init__(
        self,
        library: ToolLibrary | Iterable[Toolset],
        *,
        prefix: str | None = None,
        max_results: int | None = None,
        namespace_descriptions: Mapping[str, str] | None = None,
    ) -> None:
        toolsets = _library_toolsets(library)
        native = _native.tool_proxy_toolset(
            [toolset.to_native() for toolset in toolsets],
            prefix,
            max_results,
            dict(namespace_descriptions) if namespace_descriptions is not None else None,
        )
        super().__init__(
            "tool_proxy",
            _native_toolset=native,
        )
        self.library = ToolLibrary(toolsets)
        self.prefix = prefix
        self.max_results = max_results
        self.namespace_descriptions = dict(namespace_descriptions or {})


@dataclass(frozen=True)
class McpTransport:
    """Typed MCP transport configuration."""

    kind: Literal["streamable_http", "sse", "stdio"]
    url: str | None = None
    command: str | None = None
    headers: Mapping[str, Any] | None = None
    args: Sequence[str] = ()
    cwd: str | None = None
    env: Mapping[str, Any] | None = None

    @classmethod
    def streamable_http(
        cls,
        url: str,
        *,
        headers: Mapping[str, Any] | None = None,
    ) -> McpTransport:
        return cls("streamable_http", url=url, headers=dict(headers or {}))

    @classmethod
    def sse(
        cls,
        url: str,
        *,
        headers: Mapping[str, Any] | None = None,
    ) -> McpTransport:
        return cls("sse", url=url, headers=dict(headers or {}))

    @classmethod
    def stdio(
        cls,
        command: str | PathLike[str],
        *,
        args: Sequence[str] | None = None,
        cwd: str | PathLike[str] | None = None,
        env: Mapping[str, Any] | None = None,
    ) -> McpTransport:
        return cls(
            "stdio",
            command=str(command),
            args=tuple(args or ()),
            cwd=str(cwd) if cwd is not None else None,
            env=dict(env or {}),
        )

    def with_headers(self, headers: Mapping[str, Any]) -> McpTransport:
        if self.kind == "streamable_http":
            return McpTransport.streamable_http(
                _require_non_empty(self.url, "MCP HTTP URL"),
                headers=headers,
            )
        if self.kind == "sse":
            return McpTransport.sse(
                _require_non_empty(self.url, "MCP SSE URL"),
                headers=headers,
            )
        raise ValueError("MCP stdio transport does not accept HTTP headers")

    def to_dict(self) -> dict[str, Any]:
        if self.kind == "streamable_http":
            return {
                "StreamableHttp": {
                    "url": _require_non_empty(self.url, "MCP HTTP URL"),
                    "headers": dict(self.headers or {}),
                }
            }
        if self.kind == "sse":
            return {
                "Sse": {
                    "url": _require_non_empty(self.url, "MCP SSE URL"),
                    "headers": dict(self.headers or {}),
                }
            }
        if self.kind == "stdio":
            payload: dict[str, Any] = {
                "command": _require_non_empty(self.command, "MCP stdio command"),
                "args": list(self.args),
                "env": dict(self.env or {}),
            }
            if self.cwd is not None:
                payload["cwd"] = self.cwd
            return {"Stdio": payload}
        raise ValueError(f"unsupported MCP transport kind: {self.kind}")


@dataclass(frozen=True)
class McpToolSpec:
    """Declared MCP server tool exposed through a deferred Starweaver tool."""

    name: str
    parameters: Mapping[str, Any] | None = None
    description: str | None = None
    task: bool = False
    metadata: Mapping[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "name": _require_non_empty(self.name, "MCP tool name"),
            "parameters": dict(self.parameters or _empty_object_schema()),
            "task": self.task,
            "metadata": dict(self.metadata or {}),
        }
        if self.description is not None:
            payload["description"] = self.description
        return payload


@dataclass(frozen=True)
class McpResourceSpec:
    """Declared MCP server resource."""

    uri: str
    name: str | None = None
    description: str | None = None
    mime_type: str | None = None
    metadata: Mapping[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "uri": _require_non_empty(self.uri, "MCP resource URI"),
            "metadata": dict(self.metadata or {}),
        }
        if self.name is not None:
            payload["name"] = self.name
        if self.description is not None:
            payload["description"] = self.description
        if self.mime_type is not None:
            payload["mime_type"] = self.mime_type
        return payload


@dataclass(frozen=True)
class McpPromptSpec:
    """Declared MCP server prompt."""

    name: str
    arguments: Mapping[str, Any] | None = None
    description: str | None = None
    metadata: Mapping[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "name": _require_non_empty(self.name, "MCP prompt name"),
            "arguments": dict(self.arguments or _empty_object_schema()),
            "metadata": dict(self.metadata or {}),
        }
        if self.description is not None:
            payload["description"] = self.description
        return payload


@dataclass(frozen=True)
class McpSamplingSpec:
    """Declared MCP sampling capability."""

    enabled: bool = True
    metadata: Mapping[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        return {"enabled": self.enabled, "metadata": dict(self.metadata or {})}


@dataclass(frozen=True)
class McpSubscriptionSpec:
    """Declared MCP server subscription."""

    name: str
    target: str
    metadata: Mapping[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": _require_non_empty(self.name, "MCP subscription name"),
            "target": _require_non_empty(self.target, "MCP subscription target"),
            "metadata": dict(self.metadata or {}),
        }


class McpToolset(Toolset):
    """Typed Python constructor for Starweaver's Rust MCP toolset config."""

    def __init__(
        self,
        id: str,  # noqa: A002
        *,
        transport: McpTransport,
        headers: Mapping[str, Any] | None = None,
        tools: Iterable[McpToolSpec | Mapping[str, Any]] | None = None,
        include_instructions: bool = False,
        cache_tools: bool = True,
        tool_prefix: str | None = None,
        read_timeout_ms: int | None = None,
        init_timeout_ms: int | None = None,
        instructions: str | None = None,
        resources: Iterable[McpResourceSpec | Mapping[str, Any]] | None = None,
        prompts: Iterable[McpPromptSpec | Mapping[str, Any]] | None = None,
        sampling: McpSamplingSpec | Mapping[str, Any] | None = None,
        subscriptions: Iterable[McpSubscriptionSpec | Mapping[str, Any]] | None = None,
    ) -> None:
        if headers is not None:
            transport = transport.with_headers(headers)
        config: dict[str, Any] = {
            "id": _require_non_empty(id, "MCP toolset id"),
            "transport": transport.to_dict(),
            "include_instructions": include_instructions,
            "cache_tools": cache_tools,
            "tools": [_mcp_spec_dict(tool) for tool in tools or ()],
            "resources": [_mcp_spec_dict(resource) for resource in resources or ()],
            "prompts": [_mcp_spec_dict(prompt) for prompt in prompts or ()],
            "subscriptions": [_mcp_spec_dict(subscription) for subscription in subscriptions or ()],
        }
        if tool_prefix is not None:
            config["tool_prefix"] = tool_prefix
        if read_timeout_ms is not None:
            config["read_timeout_ms"] = read_timeout_ms
        if init_timeout_ms is not None:
            config["init_timeout_ms"] = init_timeout_ms
        if instructions is not None:
            config["instructions"] = instructions
        if sampling is not None:
            config["sampling"] = _mcp_spec_dict(sampling)
        native = _native.mcp_toolset(config)
        super().__init__(id, _native_toolset=native)
        self.transport = transport
        self.config = config

    def to_dict(self) -> dict[str, Any]:
        return dict(self.config)


def filesystem_toolset() -> Toolset:
    """Return the first-party filesystem toolset for attached environments."""

    return Toolset.from_native(_native.filesystem_toolset())


def shell_toolset() -> Toolset:
    """Return the first-party shell toolset for attached environments."""

    return Toolset.from_native(_native.shell_toolset())


def environment_toolsets() -> list[Toolset]:
    """Return first-party filesystem and shell toolsets."""

    return [Toolset.from_native(toolset) for toolset in _native.environment_toolsets()]


@dataclass(frozen=True)
class ToolsetPreparation:
    """Prepared Python toolset inventory for one agent context."""

    tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None
    instructions: str | Iterable[str] | None = None
    toolsets: Iterable[Toolset | AbstractToolset | _native.Toolset] | None = None


class AbstractToolset:
    """Context-aware Python toolset adapted into Starweaver's native runtime."""

    name: str
    id: str | None = None
    max_retries: int | None = None
    timeout_ms: int | None = None
    lifecycle_policy: ToolsetLifecyclePolicy | None = None

    def __init__(
        self,
        name: str | None = None,
        *,
        id: str | None = None,  # noqa: A002
        max_retries: int | None = None,
        timeout_ms: int | None = None,
        lifecycle_policy: ToolsetLifecyclePolicy | None = None,
    ) -> None:
        if name is not None:
            self.name = name
        if id is not None:
            self.id = id
        if max_retries is not None:
            self.max_retries = max_retries
        if timeout_ms is not None:
            self.timeout_ms = timeout_ms
        if lifecycle_policy is not None:
            self.lifecycle_policy = lifecycle_policy

    async def enter(self, ctx: ToolsetContext) -> None:
        """Enter an agent context before preparing this toolset."""

    async def exit(self, ctx: ToolsetContext) -> None:
        """Exit an agent context after the run finishes."""

    def get_tools(
        self,
        ctx: ToolsetContext,
    ) -> (
        Iterable[Tool | BaseTool | Callable[..., Any]]
        | Awaitable[Iterable[Tool | BaseTool | Callable[..., Any]]]
    ):
        """Return tools visible for this agent context."""

        return ()

    def get_instructions(
        self,
        ctx: ToolsetContext,
    ) -> str | Iterable[str] | Awaitable[str | Iterable[str] | None] | None:
        """Return instruction blocks visible for this agent context."""

        return None

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        """Prepare tools and instructions for this agent context."""

        tools = self.get_tools(ctx)
        if inspect.isawaitable(tools):
            tools = await tools
        instructions = self.get_instructions(ctx)
        if inspect.isawaitable(instructions):
            instructions = await instructions
        return ToolsetPreparation(
            tools=cast(Iterable[Tool | BaseTool | Callable[..., Any]], tools),
            instructions=cast(str | Iterable[str] | None, instructions),
        )

    async def refresh(self, ctx: ToolsetContext) -> ToolsetPreparation:
        """Refresh tools and instructions for a context already prepared in this run."""

        return await self.prepare(ctx)

    async def _enter_native(self, ctx: ToolsetContext) -> None:
        result = self.enter(ctx)
        if inspect.isawaitable(result):
            await result

    async def _exit_native(self, ctx: ToolsetContext) -> None:
        result = self.exit(ctx)
        if inspect.isawaitable(result):
            await result

    async def _prepare_native(self, ctx: ToolsetContext) -> _native.Toolset:
        preparation = self.prepare(ctx)
        if inspect.isawaitable(preparation):
            preparation = await preparation
        return self._preparation_to_toolset(preparation).to_native()

    async def _refresh_native(self, ctx: ToolsetContext) -> _native.Toolset:
        preparation = self.refresh(ctx)
        if inspect.isawaitable(preparation):
            preparation = await preparation
        return self._preparation_to_toolset(preparation).to_native()

    def _preparation_to_toolset(self, preparation: Any) -> Toolset:
        if preparation is None:
            preparation = ToolsetPreparation()
        if isinstance(preparation, ToolsetPreparation):
            tools = tuple(preparation.tools or ())
            instructions = _normalize_instructions(preparation.instructions)
            nested_toolsets = tuple(
                ensure_toolset(toolset) for toolset in preparation.toolsets or ()
            )
            if nested_toolsets:
                members: list[Toolset] = []
                if tools or instructions:
                    members.append(
                        Toolset(
                            self._toolset_name(),
                            tools=tools,
                            instructions=instructions,
                            id=self.id,
                            max_retries=self.max_retries,
                            timeout_ms=self.timeout_ms,
                        )
                    )
                members.extend(nested_toolsets)
                return _combine_toolsets(
                    self._toolset_name(),
                    members,
                    id=self.id,
                    max_retries=self.max_retries,
                    timeout_ms=self.timeout_ms,
                )
            return Toolset(
                self._toolset_name(),
                tools=tools,
                instructions=instructions,
                id=self.id,
                max_retries=self.max_retries,
                timeout_ms=self.timeout_ms,
            )
        return ensure_toolset(preparation)

    def to_native(self) -> _native.Toolset:
        return _native.dynamic_toolset(
            self._toolset_name(),
            self._prepare_native,
            self._refresh_native,
            self._enter_native,
            self._exit_native,
            asyncio.get_running_loop(),
            self.id,
            self.max_retries,
            self.timeout_ms,
            self.lifecycle_policy,
        )

    def _toolset_name(self) -> str:
        name = getattr(self, "name", None)
        if not isinstance(name, str) or not name.strip():
            raise ValueError("toolset name must not be empty")
        return name

    def prefixed(self, prefix: str) -> Toolset:
        return ensure_toolset(self).prefixed(prefix)

    def filtered(
        self,
        *,
        include: Iterable[str] | str | None = None,
        exclude: Iterable[str] | str | None = None,
        predicate: ToolDefinitionPredicate | None = None,
    ) -> Toolset:
        return ensure_toolset(self).filtered(
            include=include,
            exclude=exclude,
            predicate=predicate,
        )

    def renamed(self, mapping: Mapping[str, str]) -> Toolset:
        return ensure_toolset(self).renamed(mapping)

    def prepared(self, callback: PreparedCallback) -> Toolset:
        return ensure_toolset(self).prepared(callback)

    def with_metadata(
        self,
        metadata: Mapping[str, Any] | None = None,
        /,
        **extra_metadata: Any,
    ) -> Toolset:
        return ensure_toolset(self).with_metadata(metadata, **extra_metadata)

    def approval_required(
        self,
        names: Iterable[str] | str = "*",
        *,
        reason: str | None = None,
    ) -> Toolset:
        return ensure_toolset(self).approval_required(names, reason=reason)

    def deferred(
        self,
        names: Iterable[str] | str = "*",
        *,
        reason: str | None = None,
    ) -> Toolset:
        return ensure_toolset(self).deferred(names, reason=reason)

    def with_lifecycle(self, policy: ToolsetLifecyclePolicy) -> Toolset:
        cloned = copy.copy(self)
        cloned.lifecycle_policy = policy
        return ensure_toolset(cloned)


class PythonDynamicToolset(AbstractToolset):
    """Public compatibility base for context-aware dynamic Python toolsets.

    Subclassing this is equivalent to subclassing ``AbstractToolset``. The
    native ``PythonDynamicToolset`` bridge is still created by ``to_native()``.
    """


class ToolsetFactory(AbstractToolset):
    """Context-aware factory adapted into a native dynamic toolset."""

    def __init__(
        self,
        factory: Callable[[ToolsetContext], Any],
        *,
        name: str | None = None,
        id: str | None = None,  # noqa: A002
        per_run_step: bool = True,
        max_retries: int | None = None,
        timeout_ms: int | None = None,
        lifecycle_policy: ToolsetLifecyclePolicy | None = None,
    ) -> None:
        self.factory = factory
        self.per_run_step = per_run_step
        self._cache: dict[str, Toolset] = {}
        super().__init__(
            name or _default_factory_name(factory),
            id=id or _optional_str(getattr(factory, "id", None)),
            max_retries=max_retries,
            timeout_ms=timeout_ms,
            lifecycle_policy=lifecycle_policy,
        )

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        cache_key = None if self.per_run_step else _factory_cache_key(ctx)
        if cache_key is not None:
            cached = self._cache.get(cache_key)
            if cached is not None:
                return ToolsetPreparation(toolsets=[cached])
        result = self.factory(ctx)
        if inspect.isawaitable(result):
            result = await result
        prepared = self._factory_result_to_toolset(result)
        if cache_key is not None:
            self._cache[cache_key] = prepared
        return ToolsetPreparation(toolsets=[prepared])

    def _factory_result_to_toolset(self, result: object) -> Toolset:
        if isinstance(result, ToolsetPreparation):
            return self._preparation_to_toolset(result)
        values = _coerce_factory_toolsets(result)
        if len(values) == 1:
            return ensure_toolset(values[0])
        return _combine_toolsets(
            self._toolset_name(),
            values,
            id=self.id,
            max_retries=self.max_retries,
            timeout_ms=self.timeout_ms,
        )


@overload
def toolset_factory(
    factory: Callable[[ToolsetContext], Any],
    /,
    *,
    name: str | None = None,
    id: str | None = None,
    per_run_step: bool = True,
    max_retries: int | None = None,
    timeout_ms: int | None = None,
    lifecycle_policy: ToolsetLifecyclePolicy | None = None,
) -> ToolsetFactory: ...


@overload
def toolset_factory(
    factory: None = None,
    /,
    *,
    name: str | None = None,
    id: str | None = None,
    per_run_step: bool = True,
    max_retries: int | None = None,
    timeout_ms: int | None = None,
    lifecycle_policy: ToolsetLifecyclePolicy | None = None,
) -> Callable[[Callable[[ToolsetContext], Any]], ToolsetFactory]: ...


def toolset_factory(
    factory: Callable[[ToolsetContext], Any] | None = None,
    /,
    *,
    name: str | None = None,
    id: str | None = None,  # noqa: A002
    per_run_step: bool = True,
    max_retries: int | None = None,
    timeout_ms: int | None = None,
    lifecycle_policy: ToolsetLifecyclePolicy | None = None,
) -> ToolsetFactory | Callable[[Callable[[ToolsetContext], Any]], ToolsetFactory]:
    """Wrap a context-aware callable as a dynamic Starweaver toolset."""

    def wrap(inner: Callable[[ToolsetContext], Any]) -> ToolsetFactory:
        return ToolsetFactory(
            inner,
            name=name,
            id=id,
            per_run_step=per_run_step,
            max_retries=max_retries,
            timeout_ms=timeout_ms,
            lifecycle_policy=lifecycle_policy,
        )

    if factory is None:
        return wrap
    return wrap(factory)


class FunctionToolset(AbstractToolset):
    """Python-native toolset for grouping local functions and instructions."""

    def __init__(
        self,
        name: str,
        *,
        id: str | None = None,  # noqa: A002
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        instructions: str | Iterable[str | InstructionCallback] | None = None,
        max_retries: int | None = None,
        timeout_ms: int | None = None,
        lifecycle_policy: ToolsetLifecyclePolicy | None = None,
        strict: bool | None = None,
        sequential: bool = False,
        metadata: Mapping[str, object] | None = None,
    ) -> None:
        super().__init__(
            name,
            id=id,
            max_retries=max_retries,
            timeout_ms=timeout_ms,
            lifecycle_policy=lifecycle_policy,
        )
        self.strict = strict
        self.sequential = sequential
        self.metadata = dict(metadata or {})
        self._tools: list[Tool] = []
        self._tool_names: set[str] = set()
        self._static_instructions: list[str] = []
        self._instruction_callbacks: list[InstructionCallback] = []
        for tool_value in tools or ():
            if isinstance(tool_value, Tool | BaseTool):
                self.add_tool(tool_value)
            else:
                self.add_function(tool_value)
        self._add_instruction_source(instructions)

    @overload
    def tool(self, func: Callable[..., Any], /, **options: Any) -> Tool: ...

    @overload
    def tool(
        self, func: None = None, /, **options: Any
    ) -> Callable[[Callable[..., Any]], Tool]: ...

    def tool(
        self,
        func: Callable[..., Any] | None = None,
        /,
        **options: Any,
    ) -> Tool | Callable[[Callable[..., Any]], Tool]:
        """Decorate a context-aware callable and add it to this toolset."""

        def wrap(inner: Callable[..., Any]) -> Tool:
            return self.add_function(inner, **options)

        if func is None:
            return wrap
        return wrap(func)

    @overload
    def tool_plain(self, func: Callable[..., Any], /, **options: Any) -> Tool: ...

    @overload
    def tool_plain(
        self, func: None = None, /, **options: Any
    ) -> Callable[[Callable[..., Any]], Tool]: ...

    def tool_plain(
        self,
        func: Callable[..., Any] | None = None,
        /,
        **options: Any,
    ) -> Tool | Callable[[Callable[..., Any]], Tool]:
        """Decorate a context-free callable and add it to this toolset."""

        def wrap(inner: Callable[..., Any]) -> Tool:
            if _function_accepts_tool_context(inner):
                raise ValueError("tool_plain functions cannot accept ToolContext")
            return self.add_function(inner, **options)

        if func is None:
            return wrap
        return wrap(func)

    def add_tool(self, tool: Tool | BaseTool) -> Tool:
        """Add an existing `Tool` or `BaseTool` to this toolset."""

        prepared = _apply_toolset_defaults(ensure_tool(tool), self)
        self._insert_tool(prepared)
        return prepared

    def add_function(self, func: Callable[..., Any], /, **options: Any) -> Tool:
        """Add a callable to this toolset using `Tool` constructor options."""

        metadata = _merge_metadata(self.metadata, options.pop("metadata", None))
        prepared = Tool(
            func,
            name=options.pop("name", None),
            description=options.pop("description", None),
            parameters_schema=options.pop("parameters_schema", None),
            return_schema=options.pop("return_schema", None),
            metadata=metadata,
            strict=options.pop("strict", self.strict),
            sequential=options.pop("sequential", self.sequential),
            timeout_ms=options.pop("timeout_ms", self.timeout_ms),
            max_retries=options.pop("max_retries", self.max_retries),
        )
        if options:
            unexpected = ", ".join(sorted(options))
            raise TypeError(f"unexpected tool option(s): {unexpected}")
        self._insert_tool(prepared)
        return prepared

    def instructions(
        self,
        func: InstructionCallback | None = None,
        /,
    ) -> InstructionCallback | Callable[[InstructionCallback], InstructionCallback]:
        """Decorate a dynamic instruction callback."""

        def wrap(inner: InstructionCallback) -> InstructionCallback:
            self._instruction_callbacks.append(inner)
            return inner

        if func is None:
            return wrap
        return wrap(func)

    def get_tools(self, ctx: ToolsetContext) -> Iterable[Tool]:
        return tuple(self._tools)

    async def get_instructions(self, ctx: ToolsetContext) -> list[str]:
        instructions = list(self._static_instructions)
        for callback in self._instruction_callbacks:
            result = callback(ctx)
            if inspect.isawaitable(result):
                result = await result
            instructions.extend(_normalize_instructions(result))
        return instructions

    def _insert_tool(self, tool: Tool) -> None:
        if tool.name in self._tool_names:
            raise ValueError(
                f"duplicate tool name in toolset {self._toolset_name()!r}: {tool.name!r}"
            )
        self._tools.append(tool)
        self._tool_names.add(tool.name)

    def _add_instruction_source(
        self,
        instructions: str | Iterable[str | InstructionCallback] | None,
    ) -> None:
        if instructions is None:
            return
        if isinstance(instructions, str):
            self._static_instructions.append(instructions)
            return
        for instruction in instructions:
            if isinstance(instruction, str):
                self._static_instructions.append(instruction)
            elif callable(instruction):
                self._instruction_callbacks.append(instruction)
            else:
                raise TypeError("instructions must be strings or callables")


def ensure_toolset(
    value: Toolset | AbstractToolset | _native.Toolset | Callable[[ToolsetContext], Any],
) -> Toolset:
    if isinstance(value, Toolset):
        return value
    if isinstance(value, _native.Toolset):
        return Toolset.from_native(value)
    if callable(value) and not isinstance(value, AbstractToolset):
        value = toolset_factory(value)
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        native = to_native()
        if isinstance(native, _native.Toolset):
            return Toolset.from_native(native)
    raise TypeError("expected Toolset, AbstractToolset, native Toolset, or toolset factory")


def ensure_toolsets(
    values: Iterable[Toolset | AbstractToolset | _native.Toolset | Callable[[ToolsetContext], Any]]
    | None,
) -> list[_native.Toolset]:
    return [ensure_toolset(value).to_native() for value in values or ()]


def validate_toolset_ids(
    values: Iterable[Toolset | AbstractToolset | _native.Toolset | Callable[[ToolsetContext], Any]],
    *,
    require_ids: bool = True,
) -> ToolsetIdValidation:
    """Validate stable toolset identities before using them in durable products."""

    identities = tuple(_read_toolset_identity(value, index) for index, value in enumerate(values))
    issues: list[ToolsetIdIssue] = []
    seen_ids: dict[str, ToolsetIdentity] = {}
    seen_names: dict[str, ToolsetIdentity] = {}
    for identity in identities:
        if not identity.name.strip():
            issues.append(
                ToolsetIdIssue(
                    code="empty_name",
                    message=f"toolset at index {identity.index} has an empty name",
                    index=identity.index,
                    name=identity.name,
                    id=identity.id,
                )
            )
        previous_name = seen_names.get(identity.name)
        if previous_name is not None:
            issues.append(
                ToolsetIdIssue(
                    code="duplicate_name",
                    message=(
                        f"toolset name {identity.name!r} appears at indexes "
                        f"{previous_name.index} and {identity.index}"
                    ),
                    index=identity.index,
                    severity="warning",
                    name=identity.name,
                    id=identity.id,
                )
            )
        else:
            seen_names[identity.name] = identity
        if identity.id is None:
            if require_ids:
                issues.append(
                    ToolsetIdIssue(
                        code="missing_id",
                        message=(
                            f"toolset {identity.name!r} at index {identity.index} "
                            "is missing a durable id"
                        ),
                        index=identity.index,
                        name=identity.name,
                        id=identity.id,
                    )
                )
            continue
        if not identity.id.strip():
            issues.append(
                ToolsetIdIssue(
                    code="empty_id",
                    message=f"toolset {identity.name!r} at index {identity.index} has an empty id",
                    index=identity.index,
                    name=identity.name,
                    id=identity.id,
                )
            )
            continue
        previous_id = seen_ids.get(identity.id)
        if previous_id is not None:
            issues.append(
                ToolsetIdIssue(
                    code="duplicate_id",
                    message=(
                        f"toolset id {identity.id!r} appears at indexes "
                        f"{previous_id.index} and {identity.index}"
                    ),
                    index=identity.index,
                    name=identity.name,
                    id=identity.id,
                )
            )
        else:
            seen_ids[identity.id] = identity
    return ToolsetIdValidation(identities=identities, issues=tuple(issues))


def validate_toolsets_for_durability(
    values: Iterable[Toolset | AbstractToolset | _native.Toolset | Callable[[ToolsetContext], Any]],
    *,
    require_ids: bool = True,
) -> ToolsetIdValidation:
    """Validate toolset identities before storing durable product profiles."""

    return validate_toolset_ids(values, require_ids=require_ids)


def _library_toolsets(library: ToolLibrary | Iterable[Toolset]) -> Sequence[Toolset]:
    if isinstance(library, ToolLibrary):
        return library.toolsets
    return tuple(ensure_toolset(toolset) for toolset in library)


def _combine_toolsets(
    name: str,
    toolsets: Iterable[Toolset | AbstractToolset | _native.Toolset],
    *,
    id: str | None = None,  # noqa: A002
    max_retries: int | None = None,
    timeout_ms: int | None = None,
) -> Toolset:
    members = [ensure_toolset(toolset) for toolset in toolsets]
    native = _native.combined_toolset(
        name,
        [toolset.to_native() for toolset in members],
        id,
        max_retries,
        timeout_ms,
    )
    return Toolset.from_native(native)


def _coerce_factory_toolsets(
    result: object,
) -> tuple[Toolset | AbstractToolset | _native.Toolset, ...]:
    if isinstance(result, Toolset | AbstractToolset | _native.Toolset):
        return (result,)
    if isinstance(result, str | bytes | bytearray | Mapping):
        raise TypeError("toolset factories must return a Toolset or an iterable of toolsets")
    if isinstance(result, Iterable):
        values = tuple(result)
        for value in values:
            if not isinstance(value, Toolset | AbstractToolset | _native.Toolset):
                raise TypeError(
                    "toolset factories must return only Toolset, AbstractToolset, "
                    "or native Toolset values"
                )
        return values
    raise TypeError("toolset factories must return a Toolset or an iterable of toolsets")


def _default_factory_name(factory: Callable[[ToolsetContext], Any]) -> str:
    explicit = _optional_str(getattr(factory, "name", None))
    if explicit:
        return explicit
    name = _optional_str(getattr(factory, "__name__", None))
    if name and name != "<lambda>":
        return name
    return "toolset_factory"


def _factory_cache_key(ctx: ToolsetContext) -> str:
    run_id = _optional_str(getattr(ctx, "run_id", None))
    if run_id:
        return run_id
    return str(getattr(ctx, "conversation_id", "default"))


def _optional_str(value: object) -> str | None:
    return None if value is None else str(value)


def _read_toolset_identity(value: object, index: int) -> ToolsetIdentity:
    if isinstance(value, Toolset):
        return ToolsetIdentity(
            index=index,
            name=value.name,
            id=value.id,
            source_type=type(value).__name__,
        )
    if isinstance(value, _native.Toolset):
        return ToolsetIdentity(
            index=index,
            name=value.name,
            id=value.id,
            source_type=type(value).__name__,
        )
    if isinstance(value, AbstractToolset):
        return ToolsetIdentity(
            index=index,
            name=value._toolset_name(),
            id=value.id,
            source_type=type(value).__name__,
        )
    if callable(value):
        return ToolsetIdentity(
            index=index,
            name=_default_factory_name(value),
            id=_optional_str(getattr(value, "id", None)),
            source_type="toolset_factory",
        )
    raise TypeError("expected Toolset, AbstractToolset, native Toolset, or toolset factory")


def _normalize_instructions(instructions: str | Iterable[str] | None) -> list[str]:
    if instructions is None:
        return []
    if isinstance(instructions, str):
        return [instructions]
    result = list(instructions)
    for instruction in result:
        if not isinstance(instruction, str):
            raise TypeError("toolset instructions must be strings")
    return result


def _normalize_name_list(names: Iterable[str] | str | None) -> list[str] | None:
    if names is None:
        return None
    if isinstance(names, str):
        return [names]
    result = list(names)
    for name in result:
        if not isinstance(name, str):
            raise TypeError("tool names must be strings")
    return result


def _normalize_name_set(names: Iterable[str] | str | None) -> set[str] | None:
    result = _normalize_name_list(names)
    return None if result is None else set(result)


def _normalize_required_names(names: Iterable[str] | str) -> list[str]:
    result = _normalize_name_list(names)
    if not result:
        raise ValueError("at least one tool name or '*' is required")
    return result


def _require_non_empty(value: str | None, label: str) -> str:
    if value is None or not value.strip():
        raise ValueError(f"{label} must not be empty")
    return value


def _int_value(value: object, label: str) -> int:
    if value is None:
        return 0
    if isinstance(value, int | float | str | bytes | bytearray):
        return int(value)
    raise TypeError(f"{label} must be int-compatible")


def _empty_object_schema() -> dict[str, Any]:
    return {"type": "object", "properties": {}}


def _mcp_spec_dict(value: object) -> dict[str, Any]:
    to_dict = getattr(value, "to_dict", None)
    if callable(to_dict):
        result = to_dict()
        if not isinstance(result, Mapping):
            raise TypeError("MCP spec to_dict() must return a mapping")
        return dict(result)
    if isinstance(value, Mapping):
        return dict(value)
    raise TypeError("MCP specs must be dataclass instances or mappings")


def _collapse_instruction_strings(instructions: Iterable[str]) -> list[str]:
    result = [instruction for instruction in instructions if instruction]
    if len(result) <= 1:
        return result
    return ["\n\n".join(result)]


def _merge_metadata(
    base: Mapping[str, object],
    override: Mapping[str, object] | None,
) -> dict[str, Any]:
    merged = dict(base)
    if override:
        merged.update(dict(override))
    return merged


def _apply_toolset_defaults(tool: Tool, toolset: FunctionToolset) -> Tool:
    return Tool(
        tool.func,
        name=tool.name,
        description=tool.description,
        parameters_schema=tool._explicit_schema,
        return_schema=tool.return_schema,
        metadata=_merge_metadata(toolset.metadata, tool.metadata),
        strict=tool.strict if tool.strict is not None else toolset.strict,
        sequential=tool.sequential,
        timeout_ms=tool.timeout_ms if tool.timeout_ms is not None else toolset.timeout_ms,
        max_retries=tool.max_retries if tool.max_retries is not None else toolset.max_retries,
    )


def _function_accepts_tool_context(func: Callable[..., Any]) -> bool:
    signature = inspect.signature(func)
    try:
        hints = inspect.get_annotations(func, eval_str=True)
    except Exception:
        hints = {}
    for param in signature.parameters.values():
        annotation = hints.get(param.name, param.annotation)
        if param.name == "ctx":
            return True
        if annotation is ToolsetContext:
            return True
        if getattr(annotation, "__name__", None) == "ToolContext" and getattr(
            annotation, "__module__", ""
        ).startswith("starweaver"):
            return True
    return False
