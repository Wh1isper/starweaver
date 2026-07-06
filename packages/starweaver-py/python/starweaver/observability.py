"""Typed observability helpers over Starweaver JSON evidence."""

from __future__ import annotations

import copy
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

JsonObject = dict[str, Any]

_USAGE_FIELDS = (
    "requests",
    "input_tokens",
    "cache_write_tokens",
    "cache_read_tokens",
    "output_tokens",
    "total_tokens",
    "tool_calls",
)
_TRACE_METADATA_KEY = "starweaver.trace_metadata"


def _copy_mapping(value: Mapping[str, Any]) -> JsonObject:
    return copy.deepcopy(dict(value))


def _mapping(value: object, *, default: Mapping[str, Any] | None = None) -> JsonObject:
    if value is None:
        return _copy_mapping(default or {})
    if not isinstance(value, Mapping):
        raise TypeError(f"expected mapping, got {type(value).__name__}")
    return _copy_mapping(value)


def _mapping_list(value: object) -> list[JsonObject]:
    if value is None:
        return []
    if isinstance(value, Mapping):
        return [_copy_mapping(item) for item in value.values() if isinstance(item, Mapping)]
    if not isinstance(value, Sequence) or isinstance(value, (str, bytes, bytearray)):
        return []
    return [_copy_mapping(item) for item in value if isinstance(item, Mapping)]


def _optional_str(value: object) -> str | None:
    return None if value is None else str(value)


def _int_field(raw: Mapping[str, Any], key: str) -> int:
    value = raw.get(key, 0)
    if value is None:
        return 0
    if isinstance(value, int | float | str | bytes | bytearray):
        return int(value)
    raise TypeError(f"{key} must be int-compatible")


def _optional_usage(value: object) -> Usage | None:
    return Usage(value) if isinstance(value, Mapping) else None


def _optional_pricing(value: object) -> PricingEstimate | None:
    return PricingEstimate(value) if isinstance(value, Mapping) else None


def _trace_metadata(value: object) -> JsonObject:
    if not isinstance(value, Mapping):
        return {}
    explicit = value.get(_TRACE_METADATA_KEY)
    if isinstance(explicit, Mapping):
        return _copy_mapping(explicit)
    return {
        str(key): copy.deepcopy(item)
        for key, item in value.items()
        if not str(key).startswith("starweaver.") and not str(key).startswith("starweaver_")
    }


@dataclass(frozen=True)
class PricingEstimate:
    """Estimated cost represented in micro USD units."""

    raw: JsonObject

    def __init__(self, raw: Mapping[str, Any] | None = None) -> None:
        object.__setattr__(self, "raw", _mapping(raw))

    @property
    def amount_micros_usd(self) -> int:
        return _int_field(self.raw, "amount_micros_usd")

    @property
    def amount_usd(self) -> float:
        return self.amount_micros_usd / 1_000_000

    def is_zero(self) -> bool:
        return self.amount_micros_usd == 0

    def to_dict(self) -> JsonObject:
        data = _copy_mapping(self.raw)
        data["amount_micros_usd"] = self.amount_micros_usd
        return data


@dataclass(frozen=True)
class Usage:
    """Typed token, request, and tool-call usage counters."""

    raw: JsonObject

    def __init__(self, raw: Mapping[str, Any] | None = None) -> None:
        object.__setattr__(self, "raw", _mapping(raw))

    @property
    def requests(self) -> int:
        return _int_field(self.raw, "requests")

    @property
    def input_tokens(self) -> int:
        return _int_field(self.raw, "input_tokens")

    @property
    def cache_write_tokens(self) -> int:
        return _int_field(self.raw, "cache_write_tokens")

    @property
    def cache_read_tokens(self) -> int:
        return _int_field(self.raw, "cache_read_tokens")

    @property
    def output_tokens(self) -> int:
        return _int_field(self.raw, "output_tokens")

    @property
    def total_tokens(self) -> int:
        return _int_field(self.raw, "total_tokens")

    @property
    def tool_calls(self) -> int:
        return _int_field(self.raw, "tool_calls")

    def is_empty(self) -> bool:
        return all(getattr(self, field) == 0 for field in _USAGE_FIELDS)

    def to_dict(self) -> JsonObject:
        data = _copy_mapping(self.raw)
        for field in _USAGE_FIELDS:
            data[field] = getattr(self, field)
        return data

    def __getitem__(self, key: str) -> Any:
        return self.to_dict()[key]

    def get(self, key: str, default: Any = None) -> Any:
        return self.to_dict().get(key, default)


@dataclass(frozen=True)
class UsageSnapshotEntry:
    """Per-agent or per-source cumulative usage entry."""

    raw: JsonObject

    def __init__(self, raw: Mapping[str, Any]) -> None:
        object.__setattr__(self, "raw", _mapping(raw))

    @property
    def agent_id(self) -> str:
        return str(self.raw.get("agent_id") or "")

    @property
    def agent_name(self) -> str:
        return str(self.raw.get("agent_name") or "")

    @property
    def model_id(self) -> str:
        return str(self.raw.get("model_id") or "")

    @property
    def usage(self) -> Usage:
        return Usage(_mapping(self.raw.get("usage")))

    @property
    def estimate_pricing(self) -> PricingEstimate | None:
        return _optional_pricing(self.raw.get("estimate_pricing"))

    @property
    def usage_id(self) -> str | None:
        return _optional_str(self.raw.get("usage_id"))

    @property
    def source(self) -> str:
        return str(self.raw.get("source") or "model_request")

    def to_dict(self) -> JsonObject:
        return _copy_mapping(self.raw)


@dataclass(frozen=True)
class UsageAgentTotal:
    """Cumulative usage grouped by agent or source."""

    raw: JsonObject

    def __init__(self, raw: Mapping[str, Any]) -> None:
        object.__setattr__(self, "raw", _mapping(raw))

    @property
    def agent_name(self) -> str:
        return str(self.raw.get("agent_name") or "")

    @property
    def model_id(self) -> str:
        return str(self.raw.get("model_id") or "")

    @property
    def usage(self) -> Usage:
        return Usage(_mapping(self.raw.get("usage")))

    @property
    def estimate_pricing(self) -> PricingEstimate | None:
        return _optional_pricing(self.raw.get("estimate_pricing"))

    @property
    def usage_id(self) -> str | None:
        return _optional_str(self.raw.get("usage_id"))

    @property
    def source(self) -> str:
        return str(self.raw.get("source") or "model_request")

    def to_dict(self) -> JsonObject:
        return _copy_mapping(self.raw)


@dataclass(frozen=True)
class UsageSnapshot:
    """Cumulative usage snapshot for a run."""

    raw: JsonObject

    def __init__(self, raw: Mapping[str, Any] | None = None) -> None:
        object.__setattr__(self, "raw", _mapping(raw))

    @classmethod
    def from_state(cls, state: Mapping[str, Any]) -> UsageSnapshot:
        if "total_usage" in state or "entries" in state:
            return cls(state)
        latest_response = state.get("latest_response")
        latest_usage = (
            latest_response.get("usage")
            if isinstance(latest_response, Mapping)
            and isinstance(latest_response.get("usage"), Mapping)
            else None
        )
        raw: JsonObject = {
            "run_id": str(state.get("run_id") or ""),
            "total_usage": _mapping(state.get("usage")),
            "entries": _mapping_list(state.get("usage_snapshot_entries")),
        }
        if latest_usage is not None:
            raw["latest_usage"] = _copy_mapping(latest_usage)
        return cls(raw)

    @property
    def run_id(self) -> str:
        return str(self.raw.get("run_id") or "")

    @property
    def latest_usage(self) -> Usage | None:
        return _optional_usage(self.raw.get("latest_usage"))

    @property
    def total_usage(self) -> Usage:
        return Usage(_mapping(self.raw.get("total_usage")))

    @property
    def estimate_pricing(self) -> PricingEstimate | None:
        return _optional_pricing(self.raw.get("estimate_pricing"))

    @property
    def entries(self) -> list[UsageSnapshotEntry]:
        return [UsageSnapshotEntry(item) for item in _mapping_list(self.raw.get("entries"))]

    @property
    def agent_usages(self) -> dict[str, UsageAgentTotal]:
        value = self.raw.get("agent_usages")
        if not isinstance(value, Mapping):
            return {}
        return {
            str(key): UsageAgentTotal(item)
            for key, item in value.items()
            if isinstance(item, Mapping)
        }

    @property
    def model_usages(self) -> dict[str, Usage]:
        value = self.raw.get("model_usages")
        if not isinstance(value, Mapping):
            return {}
        return {str(key): Usage(item) for key, item in value.items() if isinstance(item, Mapping)}

    @property
    def model_estimate_pricing(self) -> dict[str, PricingEstimate]:
        value = self.raw.get("model_estimate_pricing")
        if not isinstance(value, Mapping):
            return {}
        return {
            str(key): PricingEstimate(item)
            for key, item in value.items()
            if isinstance(item, Mapping)
        }

    def to_dict(self) -> JsonObject:
        return _copy_mapping(self.raw)


@dataclass(frozen=True)
class TraceMetadata:
    """Trace identifiers and low-cardinality metadata."""

    raw: JsonObject

    def __init__(self, raw: Mapping[str, Any] | None = None) -> None:
        object.__setattr__(self, "raw", _mapping(raw))

    @classmethod
    def from_state(cls, state: Mapping[str, Any]) -> TraceMetadata:
        snapshot = state.get("trace_snapshot")
        if isinstance(snapshot, Mapping):
            return cls(snapshot)
        trace_keys = ("trace_id", "span_id", "parent_span_id", "trace_state")
        raw = {key: state[key] for key in trace_keys if key in state}
        metadata = _trace_metadata(state.get("metadata"))
        if metadata:
            raw["metadata"] = metadata
        return cls(raw)

    @property
    def trace_id(self) -> str | None:
        return _optional_str(self.raw.get("trace_id"))

    @property
    def span_id(self) -> str | None:
        return _optional_str(self.raw.get("span_id"))

    @property
    def parent_span_id(self) -> str | None:
        return _optional_str(self.raw.get("parent_span_id"))

    @property
    def trace_state(self) -> str | None:
        return _optional_str(self.raw.get("trace_state"))

    @property
    def metadata(self) -> JsonObject:
        value = self.raw.get("metadata")
        return _copy_mapping(value) if isinstance(value, Mapping) else {}

    def is_empty(self) -> bool:
        return (
            self.trace_id is None
            and self.span_id is None
            and self.parent_span_id is None
            and self.trace_state is None
            and not self.metadata
        )

    def to_dict(self) -> JsonObject:
        return _copy_mapping(self.raw)
