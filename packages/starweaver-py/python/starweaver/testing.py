"""Deterministic testing helpers for Starweaver Python applications."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
from typing import Any

from . import _native


class TestModel:
    """Deterministic model wrapper for Python SDK tests."""

    def __init__(
        self,
        text: str | None = None,
        *,
        responses: list[str | dict[str, Any]] | None = None,
    ) -> None:
        self._native = _native.TestModel(text=text, responses=responses)

    @classmethod
    def text(cls, text: str) -> TestModel:
        return cls(text=text)

    @classmethod
    def responses(cls, responses: list[str | dict[str, Any]]) -> TestModel:
        return cls(responses=responses)

    @staticmethod
    def tool_call_response(calls: list[dict[str, Any]]) -> dict[str, Any]:
        return {"tool_calls": calls}

    def captured_messages(self) -> list[Any]:
        return self._native.captured_messages()

    def captured_params(self) -> list[Any]:
        return self._native.captured_params()


class FunctionModel:
    """Deterministic model backed by a Python callback."""

    def __init__(
        self,
        callback: Callable[[list[Any], dict[str, Any]], Any],
        *,
        model_name: str | None = None,
    ) -> None:
        self.callback = callback
        self.model_name = model_name
        self._native: _native.FunctionModel | None = None

    def to_native(self) -> _native.FunctionModel:
        native = _native.FunctionModel(
            self.callback,
            asyncio.get_running_loop(),
            self.model_name,
        )
        self._native = native
        return native

    def captured_messages(self) -> list[Any]:
        if self._native is None:
            return []
        return self._native.captured_messages()

    def captured_params(self) -> list[Any]:
        if self._native is None:
            return []
        return self._native.captured_params()


async def sleep_echo(value: Any, delay_ms: int = 0) -> Any:
    """Sleep on the native runtime and return value."""

    return await _native.sleep_echo(value, delay_ms)
