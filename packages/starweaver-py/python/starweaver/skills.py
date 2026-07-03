"""Starweaver skill registry helpers."""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal, cast

from . import _native
from .environment import EnvironmentProvider, ensure_environment_provider
from .toolset import Toolset

SkillSource = Literal[
    "built_in",
    "user_shared",
    "user_tool",
    "workspace_shared",
    "workspace_tool",
    "custom",
]


@dataclass(frozen=True)
class SkillSourceScope:
    """Provider-visible skill scan scope."""

    root: str = ""
    source: SkillSource = "custom"
    directories: list[str] = field(default_factory=lambda: [".agents/skills", "skills"])

    @classmethod
    def built_in(cls, root: str) -> SkillSourceScope:
        return cls(root=root, source="built_in", directories=["skills"])

    @classmethod
    def user_shared(cls, root: str) -> SkillSourceScope:
        return cls(root=root, source="user_shared", directories=[".agents/skills"])

    @classmethod
    def user_tool(cls, root: str) -> SkillSourceScope:
        return cls(root=root, source="user_tool", directories=["skills"])

    @classmethod
    def workspace_shared(cls, root: str) -> SkillSourceScope:
        return cls(root=root, source="workspace_shared", directories=[".agents/skills"])

    @classmethod
    def workspace_tool(cls, root: str) -> SkillSourceScope:
        return cls(root=root, source="workspace_tool", directories=["skills"])

    def to_dict(self) -> dict[str, Any]:
        return {
            "root": self.root,
            "source": self.source,
            "directories": list(self.directories),
        }


class SkillPackage:
    """One parsed Starweaver skill package."""

    def __init__(
        self,
        name: str,
        description: str,
        path: str,
        *,
        body: str | None = None,
        metadata: Mapping[str, Any] | None = None,
        native: _native.SkillPackage | None = None,
    ) -> None:
        self._native = native or _native.SkillPackage(
            name,
            description,
            path,
            body=body,
            metadata=dict(metadata or {}),
        )

    @classmethod
    def from_native(cls, native: _native.SkillPackage) -> SkillPackage:
        return cls(
            native.name,
            native.description,
            native.path,
            body=native.body,
            metadata=native.metadata,
            native=native,
        )

    @classmethod
    def parse(cls, path: str, content: str) -> SkillPackage:
        return cls.from_native(_native.SkillRegistry.parse(path, content))

    @property
    def name(self) -> str:
        return self._native.name

    @property
    def description(self) -> str:
        return self._native.description

    @property
    def path(self) -> str:
        return self._native.path

    @property
    def body(self) -> str | None:
        return self._native.body

    @property
    def metadata(self) -> dict[str, Any]:
        return dict(self._native.metadata)

    def summary_line(self) -> str:
        return cast(str, self._native.summary_line())

    def to_dict(self) -> dict[str, Any]:
        return cast(dict[str, Any], self._native.to_dict())

    def to_native(self) -> _native.SkillPackage:
        return self._native


class SkillRegistry:
    """Registry of Starweaver skill packages."""

    def __init__(
        self,
        packages: Iterable[SkillPackage | Mapping[str, Any]] = (),
        *,
        native: _native.SkillRegistry | None = None,
    ) -> None:
        if native is not None:
            self._native = native
            return
        self._native = _native.SkillRegistry(
            [ensure_skill_package(package).to_native() for package in packages]
        )

    @classmethod
    async def scan(
        cls,
        environment: EnvironmentProvider | _native.EnvironmentProvider,
        scopes: Sequence[SkillSourceScope | Mapping[str, Any] | str]
        | SkillSourceScope
        | Mapping[str, Any]
        | str
        | None = None,
    ) -> SkillRegistry:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must not be None")
        native = await _native.SkillRegistry.scan(
            native_environment,
            _scope_payload(scopes),
        )
        return cls(native=native)

    @classmethod
    async def scan_with_report(
        cls,
        environment: EnvironmentProvider | _native.EnvironmentProvider,
        scopes: Sequence[SkillSourceScope | Mapping[str, Any] | str]
        | SkillSourceScope
        | Mapping[str, Any]
        | str
        | None = None,
    ) -> dict[str, Any]:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must not be None")
        return cast(
            dict[str, Any],
            await _native.SkillRegistry.scan_with_report(
                native_environment,
                _scope_payload(scopes),
            ),
        )

    @staticmethod
    async def activate(
        environment: EnvironmentProvider | _native.EnvironmentProvider,
        path: str,
    ) -> SkillPackage:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must not be None")
        native = await _native.SkillRegistry.activate(
            native_environment,
            path,
        )
        return SkillPackage.from_native(native)

    def insert(self, package: SkillPackage | Mapping[str, Any]) -> None:
        self._native.insert(ensure_skill_package(package).to_native())

    def get(self, name: str) -> SkillPackage | None:
        native = self._native.get(name)
        return SkillPackage.from_native(native) if native is not None else None

    @property
    def packages(self) -> list[SkillPackage]:
        return [SkillPackage.from_native(package) for package in self._native.packages]

    @property
    def is_empty(self) -> bool:
        return bool(self._native.is_empty)

    def toolset(self) -> Toolset:
        return Toolset.from_native(self._native.toolset())

    def to_dict(self) -> dict[str, Any]:
        return cast(dict[str, Any], self._native.to_dict())

    def to_native(self) -> _native.SkillRegistry:
        return self._native


def ensure_skill_package(value: SkillPackage | Mapping[str, Any]) -> SkillPackage:
    if isinstance(value, SkillPackage):
        return value
    return SkillPackage(
        str(value["name"]),
        str(value["description"]),
        str(value["path"]),
        body=None if value.get("body") is None else str(value["body"]),
        metadata=dict(value.get("metadata") or {}),
    )


def ensure_skill_registry(
    value: SkillRegistry | _native.SkillRegistry | None,
) -> _native.SkillRegistry | None:
    if value is None:
        return None
    if isinstance(value, SkillRegistry):
        return value.to_native()
    if isinstance(value, _native.SkillRegistry):
        return value
    raise TypeError("skills must be a SkillRegistry")


def _scope_payload(
    scopes: Sequence[SkillSourceScope | Mapping[str, Any] | str]
    | SkillSourceScope
    | Mapping[str, Any]
    | str
    | None,
) -> object | None:
    if scopes is None:
        return None
    if isinstance(scopes, SkillSourceScope):
        return scopes.to_dict()
    if isinstance(scopes, Mapping):
        return dict(scopes)
    if isinstance(scopes, str):
        return scopes
    return [
        scope.to_dict()
        if isinstance(scope, SkillSourceScope)
        else dict(scope)
        if isinstance(scope, Mapping)
        else str(scope)
        for scope in scopes
    ]
