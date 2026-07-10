"""Runtime configuration helpers."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field, fields
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
class ShellReviewConfig:
    """Shell command safety review configuration."""

    enabled: bool = True
    model: str | None = None
    on_needs_approval: str = "defer"
    risk_threshold: str = "high"
    system_prompt: str | None = None

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> ShellReviewConfig:
        return cls(**dict(value))

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "enabled": self.enabled,
            "on_needs_approval": self.on_needs_approval,
            "risk_threshold": self.risk_threshold,
        }
        _set_if_not_none(payload, "model", self.model)
        _set_if_not_none(payload, "system_prompt", self.system_prompt)
        return payload


@dataclass(frozen=True)
class SecurityConfig:
    """Security-related runtime configuration."""

    shell_review: ShellReviewConfig | Mapping[str, Any] | None = None

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> SecurityConfig:
        return cls(**dict(value))

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {}
        if self.shell_review is not None:
            payload["shell_review"] = ensure_shell_review_config(self.shell_review)
        return payload


@dataclass(frozen=True, init=False)
class ToolConfig:
    """Tool-level runtime configuration.

    The payload is intentionally open-ended and validated by the native Rust
    config schema, so Python does not drift when Rust adds a tool config field.
    """

    values: Mapping[str, Any] = field(default_factory=dict)

    def __init__(self, **values: Any) -> None:
        object.__setattr__(self, "values", dict(values))

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> ToolConfig:
        return cls(**dict(value))

    def to_dict(self) -> dict[str, Any]:
        return dict(self.values)


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
    max_image_dimension: int | None = None
    split_large_images: bool | None = None
    image_split_max_height: int | None = None
    image_split_overlap: int | None = None
    capabilities: Iterable[str] = field(default_factory=tuple)
    tool_config: ToolConfig | Mapping[str, Any] | None = None
    security: SecurityConfig | Mapping[str, Any] | None = None

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> RuntimeConfig:
        payload = _unwrap_runtime_config_mapping(value)
        kwargs: dict[str, Any] = {}
        seen_stream_resume = False
        for key, item in payload.items():
            if key == "stream_resume_on_error":
                if seen_stream_resume and kwargs.get("stream_resume") != item:
                    raise ValueError(
                        "runtime_config cannot set both stream_resume and "
                        "stream_resume_on_error to different values"
                    )
                kwargs["stream_resume"] = item
                seen_stream_resume = True
                continue
            if key == "stream_resume":
                if seen_stream_resume and kwargs.get("stream_resume") != item:
                    raise ValueError(
                        "runtime_config cannot set both stream_resume and "
                        "stream_resume_on_error to different values"
                    )
                kwargs["stream_resume"] = item
                seen_stream_resume = True
                continue
            if key not in _RUNTIME_CONFIG_FIELD_NAMES:
                raise TypeError(f"unknown runtime_config field: {key}")
            kwargs[key] = item
        return cls(**kwargs)

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
            ("max_image_dimension", self.max_image_dimension),
            ("split_large_images", self.split_large_images),
            ("image_split_max_height", self.image_split_max_height),
            ("image_split_overlap", self.image_split_overlap),
        ]:
            _set_if_not_none(payload, key, value)
        capabilities = _capabilities(self.capabilities)
        if capabilities:
            payload["capabilities"] = capabilities
        return payload

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"model_config": self.to_model_config()}
        tool_config = ensure_tool_config(self.tool_config)
        security = ensure_security_config(self.security)
        if tool_config is not None:
            payload["tool_config"] = tool_config
        if security is not None:
            payload["security"] = security
        return payload


def ensure_runtime_config(value: RuntimeConfig | Mapping[str, Any] | None) -> Any | None:
    if value is None:
        return None
    if isinstance(value, RuntimeConfig):
        return value
    if isinstance(value, Mapping):
        return RuntimeConfig.from_mapping(value)
    raise TypeError("runtime_config must be RuntimeConfig, mapping, or None")


def ensure_shell_review_config(
    value: ShellReviewConfig | Mapping[str, Any],
) -> dict[str, Any]:
    if isinstance(value, ShellReviewConfig):
        return value.to_dict()
    if isinstance(value, Mapping):
        return ShellReviewConfig.from_mapping(value).to_dict()
    raise TypeError("shell_review must be ShellReviewConfig or mapping")


def ensure_security_config(
    value: SecurityConfig | Mapping[str, Any] | None,
) -> dict[str, Any] | None:
    if value is None:
        return None
    if isinstance(value, SecurityConfig):
        return value.to_dict()
    if isinstance(value, Mapping):
        return SecurityConfig.from_mapping(value).to_dict()
    raise TypeError("security must be SecurityConfig, mapping, or None")


def ensure_tool_config(value: ToolConfig | Mapping[str, Any] | None) -> dict[str, Any] | None:
    if value is None:
        return None
    if isinstance(value, ToolConfig):
        return value.to_dict()
    if isinstance(value, Mapping):
        return ToolConfig.from_mapping(value).to_dict()
    raise TypeError("tool_config must be ToolConfig, mapping, or None")


def _set_if_not_none(payload: dict[str, Any], key: str, value: Any | None) -> None:
    if value is not None:
        payload[key] = value


def _unwrap_runtime_config_mapping(value: Mapping[str, Any]) -> dict[str, Any]:
    payload = dict(value)
    model_config = payload.pop("model_config", None)
    if model_config is None:
        return payload
    if not isinstance(model_config, Mapping):
        raise TypeError("runtime_config['model_config'] must be a mapping")
    merged = dict(model_config)
    for key, item in payload.items():
        if key in merged:
            raise TypeError(
                f"runtime_config field appears both inside and outside model_config: {key}"
            )
        merged[key] = item
    return merged


def _capabilities(value: Iterable[str]) -> list[str]:
    if isinstance(value, str):
        raise TypeError("RuntimeConfig.capabilities must be an iterable of strings, not str")
    capabilities = list(value)
    for capability in capabilities:
        if not isinstance(capability, str):
            raise TypeError("RuntimeConfig.capabilities must contain only strings")
    return capabilities


_RUNTIME_CONFIG_FIELD_NAMES = frozenset(field.name for field in fields(RuntimeConfig))
