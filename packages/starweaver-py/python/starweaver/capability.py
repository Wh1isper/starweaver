"""Composable capability bundle helpers."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from typing import Any, cast

from . import _native
from .model import ModelSettings, RequestParams, ensure_model_settings, ensure_request_params
from .output import (
    OutputFunction,
    OutputValidator,
    ensure_output_function,
    ensure_output_validator,
)
from .tool import BaseTool, Tool, ensure_tool


class CapabilityBundle:
    """Static bundle of instructions, tools, and model/request overlays."""

    def __init__(
        self,
        name: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        model_settings: ModelSettings | Mapping[str, Any] | None = None,
        request_params: RequestParams | Mapping[str, Any] | None = None,
        output_validators: Iterable[OutputValidator | Callable[..., Any]] | None = None,
        output_functions: Iterable[OutputFunction] | None = None,
    ) -> None:
        self.name = name
        self.instructions = tuple(instructions or ())
        self.tools = tuple(ensure_tool(tool) for tool in tools or ())
        self.model_settings = model_settings
        self.request_params = request_params
        self.output_validators = tuple(
            ensure_output_validator(validator) for validator in output_validators or ()
        )
        self.output_functions = tuple(
            ensure_output_function(function) for function in output_functions or ()
        )

    def to_native(self) -> _native.CapabilityBundle:
        return _native.CapabilityBundle(
            self.name,
            list(self.instructions),
            [tool.to_native() for tool in self.tools],
            ensure_model_settings(self.model_settings),
            ensure_request_params(self.request_params),
            [validator.to_native() for validator in self.output_validators],
            [function.to_native() for function in self.output_functions],
        )


def ensure_capability_bundle(
    value: CapabilityBundle | _native.CapabilityBundle,
) -> _native.CapabilityBundle:
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.CapabilityBundle, to_native())
    if isinstance(value, _native.CapabilityBundle):
        return value
    return cast(_native.CapabilityBundle, value)
