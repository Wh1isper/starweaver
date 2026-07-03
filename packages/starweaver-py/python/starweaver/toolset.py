"""Toolset composition helpers."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Sequence
from typing import Any, cast

from . import _native
from .tool import BaseTool, Tool, ensure_tool


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
        self.instructions = tuple(instructions or ())
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
        return cls(native.name, _native_toolset=native)

    def to_native(self) -> _native.Toolset:
        return self._native

    def tool_definitions(self) -> list[dict[str, Any]]:
        return cast(list[dict[str, Any]], self._native.tool_definitions())

    def instruction_records(self) -> list[dict[str, Any]]:
        return cast(list[dict[str, Any]], self._native.instructions())


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
    ) -> None:
        toolsets = _library_toolsets(library)
        native = _native.tool_proxy_toolset(
            [toolset.to_native() for toolset in toolsets],
            prefix,
            max_results,
        )
        super().__init__(
            "tool_proxy",
            _native_toolset=native,
        )
        self.library = ToolLibrary(toolsets)
        self.prefix = prefix
        self.max_results = max_results


def filesystem_toolset() -> Toolset:
    """Return the first-party filesystem toolset for attached environments."""

    return Toolset.from_native(_native.filesystem_toolset())


def shell_toolset() -> Toolset:
    """Return the first-party shell toolset for attached environments."""

    return Toolset.from_native(_native.shell_toolset())


def environment_toolsets() -> list[Toolset]:
    """Return first-party filesystem and shell toolsets."""

    return [Toolset.from_native(toolset) for toolset in _native.environment_toolsets()]


def ensure_toolset(value: Toolset | _native.Toolset) -> Toolset:
    if isinstance(value, Toolset):
        return value
    if isinstance(value, _native.Toolset):
        return Toolset(value.name, _native_toolset=value)
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        native = to_native()
        if isinstance(native, _native.Toolset):
            return Toolset(native.name, _native_toolset=native)
    raise TypeError("expected Toolset")


def ensure_toolsets(
    values: Iterable[Toolset | _native.Toolset] | None,
) -> list[_native.Toolset]:
    return [ensure_toolset(value).to_native() for value in values or ()]


def _library_toolsets(library: ToolLibrary | Iterable[Toolset]) -> Sequence[Toolset]:
    if isinstance(library, ToolLibrary):
        return library.toolsets
    return tuple(ensure_toolset(toolset) for toolset in library)
