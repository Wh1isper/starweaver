"""Media upload adapters for Starweaver model-message filters."""

from __future__ import annotations

import asyncio
import inspect
from collections.abc import Awaitable, Mapping
from dataclasses import dataclass
from typing import Any, Protocol, cast
from urllib.parse import quote

from . import _native


@dataclass(frozen=True)
class MediaUploadRequest:
    """Python view of a media upload request."""

    data: bytes
    media_type: str
    preflight: dict[str, Any]

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> MediaUploadRequest:
        data = raw["data"]
        if not isinstance(data, bytes):
            data = bytes(data)
        return cls(
            data=data,
            media_type=str(raw["media_type"]),
            preflight=dict(raw.get("preflight") or {}),
        )


class MediaUploadCallback(Protocol):
    def __call__(
        self,
        request: MediaUploadRequest,
    ) -> Mapping[str, Any] | Awaitable[Mapping[str, Any]]: ...


class MediaResourceStore(Protocol):
    def put(
        self,
        request: MediaUploadRequest,
    ) -> str | Mapping[str, Any] | Awaitable[str | Mapping[str, Any]]: ...


class MediaUploader:
    """Python callback adapter for the native media upload filter."""

    def __init__(self, callback: MediaUploadCallback) -> None:
        self.callback = callback
        self._native: _native.MediaUploader | None = None

    @classmethod
    def resource_store(
        cls,
        store: MediaResourceStore | MediaUploadCallback,
        *,
        uri_prefix: str = "resource://media",
        resource_type: str = "media",
        metadata: Mapping[str, Any] | None = None,
    ) -> MediaUploader:
        """Adapt a product-owned resource store into a media uploader."""

        async def upload(request: MediaUploadRequest) -> Mapping[str, Any]:
            result = _call_resource_store(store, request)
            if inspect.isawaitable(result):
                result = await result
            return _normalize_resource_store_result(
                result,
                request=request,
                uri_prefix=uri_prefix,
                resource_type=resource_type,
                metadata=metadata,
            )

        return cls(upload)

    async def _callback(self, raw: Mapping[str, Any]) -> Mapping[str, Any]:
        result = self.callback(MediaUploadRequest.from_raw(raw))
        if inspect.isawaitable(result):
            result = await result
        return cast(Mapping[str, Any], result)

    def to_native(self) -> _native.MediaUploader:
        if self._native is None:
            self._native = _native.MediaUploader(
                self._callback,
                asyncio.get_running_loop(),
            )
        return self._native


def ensure_media_uploader(
    value: MediaUploader | _native.MediaUploader | None,
) -> _native.MediaUploader | None:
    if value is None:
        return None
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.MediaUploader, to_native())
    if isinstance(value, _native.MediaUploader):
        return value
    raise TypeError("media_uploader must be a MediaUploader")


def _call_resource_store(
    store: MediaResourceStore | MediaUploadCallback,
    request: MediaUploadRequest,
) -> str | Mapping[str, Any] | Awaitable[str | Mapping[str, Any]]:
    put = getattr(store, "put", None)
    if callable(put):
        return cast(
            str | Mapping[str, Any] | Awaitable[str | Mapping[str, Any]],
            put(request),
        )
    if callable(store):
        return cast(
            str | Mapping[str, Any] | Awaitable[str | Mapping[str, Any]],
            store(request),
        )
    raise TypeError("resource store must be callable or expose put(request)")


def _normalize_resource_store_result(
    result: str | Mapping[str, Any],
    *,
    request: MediaUploadRequest,
    uri_prefix: str,
    resource_type: str,
    metadata: Mapping[str, Any] | None,
) -> Mapping[str, Any]:
    base_metadata = dict(metadata or {})
    if isinstance(result, str):
        return {
            "uri": result,
            "media_type": request.media_type,
            "resource_type": resource_type,
            "metadata": base_metadata,
        }
    if not isinstance(result, Mapping):
        raise TypeError("resource store result must be a URI string or mapping")
    payload = dict(result)
    if "data_url" in payload or "url" in payload:
        payload.setdefault("media_type", request.media_type)
        return payload
    uri = payload.get("uri")
    if uri is None:
        resource_id = payload.get("id")
        if resource_id is None:
            raise ValueError("resource store result must include uri, id, url, or data_url")
        uri = _resource_uri(uri_prefix, str(resource_id))
        payload["uri"] = uri
    payload.setdefault("media_type", request.media_type)
    payload.setdefault("resource_type", payload.pop("type", resource_type))
    result_metadata = payload.get("metadata")
    if result_metadata is None:
        payload["metadata"] = base_metadata
    elif isinstance(result_metadata, Mapping):
        merged = base_metadata
        merged.update(dict(result_metadata))
        payload["metadata"] = merged
    else:
        raise TypeError("resource store metadata must be a mapping")
    return payload


def _resource_uri(uri_prefix: str, resource_id: str) -> str:
    prefix = uri_prefix.rstrip("/")
    return f"{prefix}/{quote(resource_id, safe='')}"
