"""Resource reference helpers for environment-backed Starweaver runs."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from typing import Any

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


class ResourceRegistry:
    """Small in-process registry for host-visible resource references."""

    def __init__(self, resources: Iterable[ResourceRef | Mapping[str, Any]] = ()) -> None:
        self._resources: dict[str, ResourceRef] = {}
        for resource in resources:
            self.add(resource)

    def add(self, resource: ResourceRef | Mapping[str, Any]) -> ResourceRef:
        ref = ensure_resource_ref(resource)
        self._resources[ref.id or ref.uri] = ref
        return ref

    def get(self, id: str) -> ResourceRef | None:  # noqa: A002
        return self._resources.get(id)

    def list(self) -> list[ResourceRef]:
        return list(self._resources.values())

    def to_state(self) -> list[dict[str, Any]]:
        return [resource.to_dict() for resource in self._resources.values()]


def ensure_resource_ref(value: ResourceRef | Mapping[str, Any]) -> ResourceRef:
    if isinstance(value, ResourceRef):
        return value
    return ResourceRef.from_dict(value)
