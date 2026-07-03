"""Media upload adapters for Starweaver model-message filters."""

from __future__ import annotations

import asyncio
import inspect
from collections.abc import Awaitable, Mapping
from dataclasses import dataclass
from typing import Any, Protocol, cast

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


class MediaUploader:
    """Python callback adapter for the native media upload filter."""

    def __init__(self, callback: MediaUploadCallback) -> None:
        self.callback = callback
        self._native: _native.MediaUploader | None = None

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
