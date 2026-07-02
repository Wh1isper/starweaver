"""SDK-level subagent composition helpers."""

from __future__ import annotations

from collections.abc import Iterable
from typing import TYPE_CHECKING, cast

from . import _native

if TYPE_CHECKING:
    from .agent import Agent


class Subagent:
    """Registered child agent exposed through Starweaver delegation tools."""

    def __init__(
        self,
        name: str,
        agent: Agent,
        *,
        description: str | None = None,
        required_tools: Iterable[str] | None = None,
        optional_tools: Iterable[str] | None = None,
        denied_tools: Iterable[str] | None = None,
        auto_inherit: bool = True,
        inherit_all_when_empty: bool = False,
        allow_nested_delegation: bool = False,
        inherit_hooks: bool = False,
        inherit_capability_bundles: bool = False,
        denied_capabilities: Iterable[str] | None = None,
    ) -> None:
        self._native = _native.Subagent(
            name,
            getattr(agent, "_native", agent),
            description=description,
            required_tools=list(required_tools or ()),
            optional_tools=list(optional_tools or ()),
            denied_tools=list(denied_tools or ()),
            auto_inherit=auto_inherit,
            inherit_all_when_empty=inherit_all_when_empty,
            allow_nested_delegation=allow_nested_delegation,
            inherit_hooks=inherit_hooks,
            inherit_capability_bundles=inherit_capability_bundles,
            denied_capabilities=list(denied_capabilities or ()),
        )

    def to_native(self) -> _native.Subagent:
        return self._native


def ensure_subagent(value: Subagent | _native.Subagent) -> _native.Subagent:
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.Subagent, to_native())
    if isinstance(value, _native.Subagent):
        return value
    return cast(_native.Subagent, value)
