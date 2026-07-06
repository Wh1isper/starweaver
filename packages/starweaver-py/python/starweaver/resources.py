"""Resource reference helpers for environment-backed Starweaver runs."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from typing import Any, Self, cast

RESOURCE_REF_KIND_KEY = "resource_kind"


@dataclass(frozen=True)
class ResourceRef:
    """Stable provider or host-owned resource reference."""

    uri: str
    id: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> ResourceRef:
        uri = str(raw["uri"])
        return cls(
            uri=uri,
            id=str(raw.get("id") or uri),
            metadata=dict(raw.get("metadata") or {}),
        )

    @classmethod
    def typed(
        cls,
        uri: str,
        *,
        kind: str,
        id: str | None = None,  # noqa: A002
        metadata: Mapping[str, Any] | None = None,
    ) -> ResourceRef:
        payload = dict(metadata or {})
        payload[RESOURCE_REF_KIND_KEY] = kind
        return cls(uri=uri, id=id or uri, metadata=payload)

    @property
    def kind(self) -> str | None:
        value = self.metadata.get(RESOURCE_REF_KIND_KEY)
        return None if value is None else str(value)

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id or self.uri,
            "uri": self.uri,
            "metadata": dict(self.metadata),
        }


class BaseResource:
    """Host-owned resource with a stable Starweaver reference."""

    uri: str
    id: str | None
    metadata: dict[str, Any]

    def __init__(
        self,
        uri: str,
        *,
        id: str | None = None,  # noqa: A002
        kind: str | None = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> None:
        payload = dict(metadata or {})
        if kind is not None:
            payload[RESOURCE_REF_KIND_KEY] = kind
        self.uri = uri
        self.id = id or uri
        self.metadata = payload

    @property
    def kind(self) -> str | None:
        value = self.metadata.get(RESOURCE_REF_KIND_KEY)
        return None if value is None else str(value)

    def to_ref(self) -> ResourceRef:
        return ResourceRef(uri=self.uri, id=self.id or self.uri, metadata=dict(self.metadata))

    def to_dict(self) -> dict[str, Any]:
        return self.to_ref().to_dict()


class ResumableResource(BaseResource):
    """Resource whose serializable reference state can be restored by a factory."""

    def export_state(self) -> dict[str, Any]:
        return self.to_ref().to_dict()

    @classmethod
    def from_state(cls, state: Mapping[str, Any]) -> Self:
        ref = ResourceRef.from_dict(state)
        return cls(ref.uri, id=ref.id, metadata=ref.metadata)


class InstructableResource(BaseResource):
    """Resource that may contribute model instructions or toolsets."""

    def get_instructions(self) -> str | Iterable[str] | None:
        return None

    def get_toolsets(self) -> Iterable[Any]:
        return ()


@dataclass(frozen=True)
class ResourceRegistryState:
    """Serializable snapshot of host-visible resource references."""

    resources: tuple[ResourceRef, ...] = ()

    @classmethod
    def from_raw(
        cls,
        raw: ResourceRegistryState
        | Mapping[str, Any]
        | Iterable[ResourceRef | BaseResource | Mapping[str, Any]],
    ) -> ResourceRegistryState:
        if isinstance(raw, ResourceRegistryState):
            return raw
        if isinstance(raw, Mapping):
            raw_mapping = cast(Mapping[str, Any], raw)
            resources = raw_mapping.get("resources", ())
            if resources is None:
                resources = ()
            return cls(tuple(ensure_resource_ref(resource) for resource in resources))
        return cls(tuple(ensure_resource_ref(resource) for resource in raw))

    def to_list(self) -> list[dict[str, Any]]:
        return [resource.to_dict() for resource in self.resources]

    def to_dict(self) -> dict[str, Any]:
        return {"resources": self.to_list()}

    def to_registry(self) -> ResourceRegistry:
        return ResourceRegistry(self.resources)


class ResourceRegistry:
    """Small in-process registry for host-visible resource references."""

    def __init__(
        self,
        resources: Iterable[ResourceRef | BaseResource | Mapping[str, Any]] = (),
    ) -> None:
        self._resources: dict[str, ResourceRef] = {}
        self._live_resources: dict[str, BaseResource] = {}
        for resource in resources:
            self.add(resource)

    @classmethod
    def from_state(
        cls,
        state: ResourceRegistryState
        | Mapping[str, Any]
        | Iterable[ResourceRef | BaseResource | Mapping[str, Any]],
    ) -> ResourceRegistry:
        return ResourceRegistryState.from_raw(state).to_registry()

    @classmethod
    def from_factory(
        cls,
        factory: Callable[
            ..., ResourceRegistry | Iterable[ResourceRef | BaseResource | Mapping[str, Any]]
        ],
        *,
        environment: Any | None = None,
    ) -> ResourceRegistry:
        """Build live resources through an environment-bound product factory."""

        result = factory(environment) if environment is not None else factory()
        return _resource_registry_from_factory_result(result)

    @classmethod
    def restore(
        cls,
        state: ResourceRegistryState
        | Mapping[str, Any]
        | Iterable[ResourceRef | BaseResource | Mapping[str, Any]],
        factory: Callable[
            ...,
            ResourceRegistry | Iterable[ResourceRef | BaseResource | Mapping[str, Any]],
        ]
        | None = None,
        *,
        environment: Any | None = None,
    ) -> ResourceRegistry:
        """Restore references, optionally rebinding live resources through a factory."""

        snapshot = ResourceRegistryState.from_raw(state)
        if factory is None:
            return snapshot.to_registry()
        result = factory(snapshot, environment) if environment is not None else factory(snapshot)
        return _resource_registry_from_factory_result(result)

    def add(self, resource: ResourceRef | BaseResource | Mapping[str, Any]) -> ResourceRef:
        ref = ensure_resource_ref(resource)
        self._resources[ref.id or ref.uri] = ref
        if isinstance(resource, BaseResource):
            self._live_resources[ref.id or ref.uri] = resource
        return ref

    def get(self, id: str) -> ResourceRef | None:  # noqa: A002
        return self._resources.get(id)

    def live(self, id: str) -> BaseResource | None:  # noqa: A002
        """Return the process-local live resource handle when one is registered."""

        return self._live_resources.get(id)

    def list(self) -> list[ResourceRef]:
        return list(self._resources.values())

    def instructions(self) -> list[str]:
        """Return instruction blocks contributed by live instructable resources."""

        instructions: list[str] = []
        for resource in self._live_resources.values():
            if isinstance(resource, InstructableResource):
                instructions.extend(_normalize_instructions(resource.get_instructions()))
        return instructions

    def toolsets(self) -> list[Any]:
        """Return toolsets contributed by live instructable resources."""

        toolsets: list[Any] = []
        for resource in self._live_resources.values():
            if isinstance(resource, InstructableResource):
                toolsets.extend(resource.get_toolsets())
        return toolsets

    def state(self) -> ResourceRegistryState:
        return ResourceRegistryState(tuple(self._resources.values()))

    def to_state(self) -> list[dict[str, Any]]:
        return [resource.to_dict() for resource in self._resources.values()]


def ensure_resource_ref(value: ResourceRef | BaseResource | Mapping[str, Any]) -> ResourceRef:
    if isinstance(value, ResourceRef):
        return value
    if isinstance(value, BaseResource):
        return value.to_ref()
    return ResourceRef.from_dict(value)


def _resource_registry_from_factory_result(
    result: ResourceRegistry | Iterable[ResourceRef | BaseResource | Mapping[str, Any]],
) -> ResourceRegistry:
    if isinstance(result, ResourceRegistry):
        return result
    return ResourceRegistry(result)


def _normalize_instructions(value: str | Iterable[str] | None) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    return [str(item) for item in value]
