"""Runtime configuration helpers."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from typing import Any


def _ratio(value: float | int | Mapping[str, Any] | None) -> dict[str, int] | None:
    if value is None:
        return None
    if isinstance(value, Mapping):
        per_thousand = value.get("per_thousand")
        if not isinstance(per_thousand, int):
            raise TypeError("ratio mapping must include integer per_thousand")
        return {"per_thousand": per_thousand}
    if isinstance(value, float):
        if not 0.0 <= value <= 1.0:
            raise ValueError("ratio float must be between 0.0 and 1.0")
        return {"per_thousand": round(value * 1000)}
    if isinstance(value, int):
        if not 0 <= value <= 1000:
            raise ValueError("ratio integer must be parts per thousand between 0 and 1000")
        return {"per_thousand": value}
    raise TypeError("ratio must be float, integer, mapping, or None")


@dataclass(frozen=True)
class RuntimeConfig:
    """Runtime/context config kept separate from provider settings."""

    context_window: int | None = None
    proactive_context_management_threshold: float | int | Mapping[str, Any] | None = None
    compact_threshold: float | int | Mapping[str, Any] | None = None
    cold_start_trim_seconds: int | None = None
    stream_resume: bool | None = None
    stream_resume_max_attempts: int | None = None
    stream_resume_prompt: str | None = None
    max_images: int | None = None
    max_videos: int | None = None
    support_gif: bool | None = None
    max_image_bytes: int | None = None
    split_large_images: bool | None = None
    image_split_max_height: int | None = None
    image_split_overlap: int | None = None
    capabilities: Iterable[str] = field(default_factory=tuple)

    def to_model_config(self) -> dict[str, Any]:
        payload: dict[str, Any] = {}
        _set_if_not_none(payload, "context_window", self.context_window)
        proactive = _ratio(self.proactive_context_management_threshold)
        compact = _ratio(self.compact_threshold)
        _set_if_not_none(payload, "proactive_context_management_threshold", proactive)
        _set_if_not_none(payload, "compact_threshold", compact)
        for key, value in [
            ("cold_start_trim_seconds", self.cold_start_trim_seconds),
            ("stream_resume_on_error", self.stream_resume),
            ("stream_resume_max_attempts", self.stream_resume_max_attempts),
            ("stream_resume_prompt", self.stream_resume_prompt),
            ("max_images", self.max_images),
            ("max_videos", self.max_videos),
            ("support_gif", self.support_gif),
            ("max_image_bytes", self.max_image_bytes),
            ("split_large_images", self.split_large_images),
            ("image_split_max_height", self.image_split_max_height),
            ("image_split_overlap", self.image_split_overlap),
        ]:
            _set_if_not_none(payload, key, value)
        capabilities = tuple(self.capabilities)
        if capabilities:
            payload["capabilities"] = list(capabilities)
        return payload

    def to_dict(self) -> dict[str, Any]:
        return {"model_config": self.to_model_config()}


def ensure_runtime_config(value: RuntimeConfig | Mapping[str, Any] | None) -> Any | None:
    if value is None:
        return None
    if isinstance(value, RuntimeConfig):
        return value
    return {"model_config": dict(value)}


def _set_if_not_none(payload: dict[str, Any], key: str, value: Any | None) -> None:
    if value is not None:
        payload[key] = value
