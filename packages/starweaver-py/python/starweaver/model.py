"""Model settings and provider model helpers."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any, cast

from . import _native

JsonObject = dict[str, Any]


class ModelSettings:
    """Provider-neutral model settings plus typed provider escape hatches."""

    def __init__(self, value: Mapping[str, Any] | None = None, **kwargs: Any) -> None:
        payload: JsonObject = dict(value or {})
        payload.update(kwargs)
        self._native = _native.ModelSettings(payload)

    @classmethod
    def preset(cls, name: str) -> ModelSettings:
        instance = cls.__new__(cls)
        instance._native = _native.ModelSettings.preset(name)
        return instance

    def to_native(self) -> _native.ModelSettings:
        return self._native

    def to_dict(self) -> JsonObject:
        return self._native.to_dict()


class RequestParams:
    """Provider-neutral request parameters forwarded into model preparation."""

    def __init__(self, value: Mapping[str, Any] | None = None, **kwargs: Any) -> None:
        payload: JsonObject = dict(value or {})
        payload.update(kwargs)
        self._native = _native.RequestParams(payload)

    def to_native(self) -> _native.RequestParams:
        return self._native

    def to_dict(self) -> JsonObject:
        return self._native.to_dict()


class ProviderModel:
    """Production provider-backed model adapter."""

    def __init__(self, native: _native.ProviderModel) -> None:
        self._native = native

    @classmethod
    def from_model_id(
        cls,
        model_id: str,
        *,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        return cls(
            _native.ProviderModel.from_model_id(
                model_id,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=ensure_model_settings(model_settings),
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        )

    @classmethod
    def codex_oauth(
        cls,
        model_name: str,
        *,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
    ) -> ProviderModel:
        return cls(
            _native.ProviderModel.codex_oauth(
                model_name,
                model_settings=ensure_model_settings(model_settings),
            )
        )

    @classmethod
    def openai_responses(
        cls,
        model_name: str,
        *,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        return cls(
            _native.ProviderModel.openai_responses(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=ensure_model_settings(model_settings),
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        )

    @classmethod
    def openai_chat(
        cls,
        model_name: str,
        *,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        return cls(
            _native.ProviderModel.openai_chat(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=ensure_model_settings(model_settings),
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        )

    @classmethod
    def anthropic(
        cls,
        model_name: str,
        *,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        return cls(
            _native.ProviderModel.anthropic(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=ensure_model_settings(model_settings),
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        )

    @classmethod
    def gemini(
        cls,
        model_name: str,
        *,
        api_key: str | None = None,
        api_key_env: str | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        return cls(
            _native.ProviderModel.gemini(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=ensure_model_settings(model_settings),
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        )

    def to_native(self) -> _native.ProviderModel:
        return self._native


def ensure_model_settings(
    value: ModelSettings | Mapping[str, Any] | _native.ModelSettings | None,
) -> _native.ModelSettings | None:
    if value is None:
        return None
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.ModelSettings, to_native())
    if isinstance(value, _native.ModelSettings):
        return value
    return ModelSettings(cast(Mapping[str, Any], value)).to_native()


def ensure_request_params(
    value: RequestParams | Mapping[str, Any] | _native.RequestParams | None,
) -> _native.RequestParams | None:
    if value is None:
        return None
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.RequestParams, to_native())
    if isinstance(value, _native.RequestParams):
        return value
    return RequestParams(cast(Mapping[str, Any], value)).to_native()
