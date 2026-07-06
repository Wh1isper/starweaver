"""Model settings and provider model helpers."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from os import PathLike, fspath
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


@dataclass(frozen=True)
class ProviderAuth:
    """Typed provider auth selector for model constructors."""

    provider: str
    api_key: str | None = None
    api_key_env: str | None = None
    auth_file: str | None = None

    @classmethod
    def openai(
        cls,
        *,
        api_key: str | None = None,
        api_key_env: str | None = "OPENAI_API_KEY",
    ) -> ProviderAuth:
        return cls("openai", api_key=api_key, api_key_env=api_key_env)

    @classmethod
    def anthropic(
        cls,
        *,
        api_key: str | None = None,
        api_key_env: str | None = "ANTHROPIC_API_KEY",
    ) -> ProviderAuth:
        return cls("anthropic", api_key=api_key, api_key_env=api_key_env)

    @classmethod
    def gemini(
        cls,
        *,
        api_key: str | None = None,
        api_key_env: str | None = "GEMINI_API_KEY",
    ) -> ProviderAuth:
        return cls("gemini", api_key=api_key, api_key_env=api_key_env)

    @classmethod
    def codex_oauth(cls, *, auth_file: str | PathLike[str] | None = None) -> ProviderAuth:
        return cls("codex", auth_file=_optional_path(auth_file))

    def resolve_api_key(
        self, api_key: str | None, api_key_env: str | None
    ) -> tuple[str | None, str | None]:
        return api_key if api_key is not None else self.api_key, (
            api_key_env if api_key_env is not None else self.api_key_env
        )

    def status(self) -> JsonObject:
        """Return a safe auth status snapshot without token material."""

        if self.provider == "codex":
            return _native.oauth_provider_status("codex", auth_file=self.auth_file)
        return {
            "provider_name": self.provider,
            "auth_type": "api_key",
            "api_key_env": self.api_key_env,
            "has_inline_api_key": self.api_key is not None and bool(self.api_key.strip()),
        }

    def account_metadata(self) -> JsonObject | None:
        """Return OAuth account metadata when the provider has a stored record."""

        status = self.status()
        account = status.get("account")
        return dict(account) if isinstance(account, Mapping) else None

    def redacted_record(self) -> JsonObject | None:
        """Return a stored OAuth provider record with token fields redacted."""

        if self.provider != "codex":
            return None
        record = _native.oauth_provider_redacted_record("codex", auth_file=self.auth_file)
        return dict(record) if isinstance(record, Mapping) else None


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
        auth: ProviderAuth | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        if auth is not None:
            api_key, api_key_env = auth.resolve_api_key(api_key, api_key_env)
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
        auth: ProviderAuth | None = None,
        auth_file: str | PathLike[str] | None = None,
        session_id: str | None = None,
        thread_id: str | None = None,
        stream_transport: str | None = None,
    ) -> ProviderModel:
        if auth is not None and auth.provider != "codex":
            raise ValueError("codex_oauth requires ProviderAuth.codex_oauth()")
        resolved_auth_file = _optional_path(auth_file) or (auth.auth_file if auth else None)
        return cls(
            _native.ProviderModel.codex_oauth(
                model_name,
                model_settings=ensure_model_settings(
                    _with_provider_settings(
                        model_settings,
                        codex=_clean_none({"session_id": session_id, "thread_id": thread_id}),
                        openai_responses=_clean_none(
                            {"stream_transport": _normalize_stream_transport(stream_transport)}
                        ),
                    )
                ),
                auth_file=resolved_auth_file,
            )
        )

    @classmethod
    def openai(
        cls,
        model_name: str,
        *,
        protocol: str = "responses",
        api_key: str | None = None,
        api_key_env: str | None = None,
        auth: ProviderAuth | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
        stream_transport: str | None = None,
    ) -> ProviderModel:
        if auth is not None:
            if auth.provider != "openai":
                raise ValueError("ProviderModel.openai requires ProviderAuth.openai()")
            api_key, api_key_env = auth.resolve_api_key(api_key, api_key_env)
        if protocol in {"responses", "openai_responses"}:
            return cls.openai_responses(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=_with_provider_settings(
                    model_settings,
                    openai_responses=_clean_none(
                        {"stream_transport": _normalize_stream_transport(stream_transport)}
                    ),
                ),
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        if protocol in {"chat", "openai_chat"}:
            if stream_transport is not None:
                raise ValueError("stream_transport only applies to OpenAI Responses")
            return cls.openai_chat(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=model_settings,
                base_url=base_url,
                endpoint_path=endpoint_path,
            )
        raise ValueError("protocol must be 'responses' or 'chat'")

    @classmethod
    def openai_responses(
        cls,
        model_name: str,
        *,
        api_key: str | None = None,
        api_key_env: str | None = None,
        auth: ProviderAuth | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
        stream_transport: str | None = None,
    ) -> ProviderModel:
        if auth is not None:
            if auth.provider != "openai":
                raise ValueError("openai_responses requires ProviderAuth.openai()")
            api_key, api_key_env = auth.resolve_api_key(api_key, api_key_env)
        return cls(
            _native.ProviderModel.openai_responses(
                model_name,
                api_key=api_key,
                api_key_env=api_key_env,
                model_config_preset=model_config_preset,
                model_settings=ensure_model_settings(
                    _with_provider_settings(
                        model_settings,
                        openai_responses=_clean_none(
                            {"stream_transport": _normalize_stream_transport(stream_transport)}
                        ),
                    )
                ),
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
        auth: ProviderAuth | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        if auth is not None:
            if auth.provider != "openai":
                raise ValueError("openai_chat requires ProviderAuth.openai()")
            api_key, api_key_env = auth.resolve_api_key(api_key, api_key_env)
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
        auth: ProviderAuth | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        if auth is not None:
            if auth.provider != "anthropic":
                raise ValueError("anthropic requires ProviderAuth.anthropic()")
            api_key, api_key_env = auth.resolve_api_key(api_key, api_key_env)
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
        auth: ProviderAuth | None = None,
        model_config_preset: str | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        base_url: str | None = None,
        endpoint_path: str | None = None,
    ) -> ProviderModel:
        if auth is not None:
            if auth.provider != "gemini":
                raise ValueError("gemini requires ProviderAuth.gemini()")
            api_key, api_key_env = auth.resolve_api_key(api_key, api_key_env)
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


def _with_provider_settings(
    value: ModelSettings | Mapping[str, Any] | None,
    *,
    codex: Mapping[str, Any] | None = None,
    openai_responses: Mapping[str, Any] | None = None,
) -> ModelSettings | Mapping[str, Any] | None:
    overlay: JsonObject = {}
    provider_settings: JsonObject = {}
    if codex:
        provider_settings["codex"] = dict(codex)
    if openai_responses:
        provider_settings["openai_responses"] = dict(openai_responses)
    if provider_settings:
        overlay["provider_settings"] = provider_settings
    if not overlay:
        return value
    payload = _model_settings_payload(value)
    _deep_merge(payload, overlay)
    return ModelSettings(payload)


def _model_settings_payload(value: ModelSettings | Mapping[str, Any] | None) -> JsonObject:
    if value is None:
        return {}
    if isinstance(value, ModelSettings):
        return value.to_dict()
    return dict(value)


def _deep_merge(target: JsonObject, overlay: Mapping[str, Any]) -> None:
    for key, value in overlay.items():
        if isinstance(value, Mapping) and isinstance(target.get(key), Mapping):
            nested = dict(cast(Mapping[str, Any], target[key]))
            _deep_merge(nested, value)
            target[key] = nested
        else:
            target[key] = value


def _clean_none(value: Mapping[str, Any]) -> JsonObject:
    return {key: item for key, item in value.items() if item is not None}


def _normalize_stream_transport(value: str | None) -> str | None:
    if value is None:
        return None
    normalized = value.strip().lower().replace("-", "_")
    if normalized in {"websocket", "ws"}:
        return "web_socket"
    if normalized in {"web_socket", "http", "auto"}:
        return normalized
    raise ValueError("stream_transport must be 'http', 'websocket', or 'auto'")


def _optional_path(value: str | PathLike[str] | None) -> str | None:
    return None if value is None else fspath(value)


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
