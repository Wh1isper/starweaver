"""Structured output schema and policy helpers."""

from __future__ import annotations

import asyncio
import inspect
from collections.abc import Callable, Mapping
from typing import Any, cast, overload

from . import _native

JsonObject = dict[str, Any]
OutputContext = _native.OutputContext
OutputValue = _native.OutputValue


class OutputSchema:
    """Structured output schema passed to model preparation and validation."""

    def __init__(
        self,
        name: str,
        schema: Mapping[str, Any],
        *,
        description: str | None = None,
        strict: bool = True,
    ) -> None:
        self._native = _native.OutputSchema(
            name,
            dict(schema),
            description=description,
            strict=strict,
        )

    @classmethod
    def from_pydantic(
        cls,
        model: type[Any],
        *,
        name: str | None = None,
        description: str | None = None,
        strict: bool = True,
    ) -> OutputSchema:
        schema = model.model_json_schema()
        return cls(
            name or getattr(model, "__name__", "output"),
            schema,
            description=description,
            strict=strict,
        )

    def request_schema(self) -> JsonObject:
        return self._native.request_schema()

    def to_native(self) -> _native.OutputSchema:
        return self._native

    def to_dict(self) -> JsonObject:
        return self._native.to_dict()


class OutputPolicy:
    """Complete output behavior for one agent or run."""

    def __init__(
        self,
        native: _native.OutputPolicy | None = None,
        *,
        validators: tuple[OutputValidator, ...] = (),
        functions: tuple[OutputFunction, ...] = (),
    ) -> None:
        self._native = native or _native.OutputPolicy()
        self._validators = validators
        self._functions = functions

    @classmethod
    def text(cls) -> OutputPolicy:
        return cls(_native.OutputPolicy.text())

    @classmethod
    def structured(
        cls,
        schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema,
    ) -> OutputPolicy:
        return cls(_native.OutputPolicy.structured(ensure_output_schema(schema)))

    @classmethod
    def auto(cls, schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema) -> OutputPolicy:
        return cls(_native.OutputPolicy.auto(ensure_output_schema(schema)))

    @classmethod
    def native_json_schema(
        cls,
        schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema,
    ) -> OutputPolicy:
        return cls(_native.OutputPolicy.native_json_schema(ensure_output_schema(schema)))

    @classmethod
    def native_json_object(
        cls,
        schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema,
    ) -> OutputPolicy:
        return cls(_native.OutputPolicy.native_json_object(ensure_output_schema(schema)))

    @classmethod
    def tool(cls, schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema) -> OutputPolicy:
        return cls(_native.OutputPolicy.tool(ensure_output_schema(schema)))

    @classmethod
    def tool_or_text(
        cls,
        schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema,
    ) -> OutputPolicy:
        return cls(_native.OutputPolicy.tool_or_text(ensure_output_schema(schema)))

    @classmethod
    def prompted(
        cls,
        schema: OutputSchema | Mapping[str, Any] | _native.OutputSchema,
    ) -> OutputPolicy:
        return cls(_native.OutputPolicy.prompted(ensure_output_schema(schema)))

    @classmethod
    def image(cls) -> OutputPolicy:
        return cls(_native.OutputPolicy.image())

    def with_retries(self, retries: int) -> OutputPolicy:
        return OutputPolicy(
            self._native.with_retries(retries),
            validators=self._validators,
            functions=self._functions,
        )

    def with_mode(self, mode: str) -> OutputPolicy:
        return OutputPolicy(
            self._native.with_mode(mode),
            validators=self._validators,
            functions=self._functions,
        )

    def allow_text_output(self, allow: bool = True) -> OutputPolicy:
        return OutputPolicy(
            self._native.allow_text_output(allow),
            validators=self._validators,
            functions=self._functions,
        )

    def allow_image_output(self, allow: bool = True) -> OutputPolicy:
        return OutputPolicy(
            self._native.allow_image_output(allow),
            validators=self._validators,
            functions=self._functions,
        )

    def with_validator(
        self,
        validator: OutputValidator | Callable[..., Any],
    ) -> OutputPolicy:
        return OutputPolicy(
            self._native,
            validators=(*self._validators, ensure_output_validator(validator)),
            functions=self._functions,
        )

    def with_function(
        self,
        function: OutputFunction,
    ) -> OutputPolicy:
        return OutputPolicy(
            self._native,
            validators=self._validators,
            functions=(*self._functions, function),
        )

    def to_native(self) -> _native.OutputPolicy:
        native = self._native
        for validator in self._validators:
            native = native.with_validator(validator.to_native())
        for function in self._functions:
            native = native.with_function(function.to_native())
        return native


class OutputValidator:
    """Python output validator attached through `OutputPolicy` or bundles."""

    def __init__(self, func: Callable[..., Any]) -> None:
        self.func = func

    async def _callback(self, ctx: OutputContext, output: Any) -> None | bool:
        result = self.func(ctx, output) if _accepts_context(self.func) else self.func(output)
        if inspect.isawaitable(result):
            result = await result
        return cast(None | bool, result)

    def to_native(self) -> _native.OutputValidator:
        return _native.OutputValidator(self._callback, asyncio.get_running_loop())


class OutputFunction:
    """Final-output function exposed to the model as a tool-like output call."""

    def __init__(
        self,
        name: str,
        parameters_schema: Mapping[str, Any],
        func: Callable[..., Any],
        *,
        description: str | None = None,
    ) -> None:
        self.name = name
        self.parameters_schema = dict(parameters_schema)
        self.func = func
        self.description = description or inspect.getdoc(func)

    @classmethod
    def from_pydantic(
        cls,
        model: type[Any],
        func: Callable[..., Any],
        *,
        name: str | None = None,
        description: str | None = None,
    ) -> OutputFunction:
        return cls(
            name or getattr(model, "__name__", "output"),
            model.model_json_schema(),
            func,
            description=description,
        )

    async def _callback(self, ctx: OutputContext, args: JsonObject) -> Any:
        result = self.func(ctx, args) if _accepts_context(self.func) else self.func(args)
        if inspect.isawaitable(result):
            result = await result
        return result

    def to_native(self) -> _native.OutputFunction:
        return _native.OutputFunction(
            self.name,
            self.parameters_schema,
            self._callback,
            asyncio.get_running_loop(),
            description=self.description,
        )

    def definition_json(self) -> JsonObject:
        return self.to_native().definition_json()


@overload
def output_validator(func: Callable[..., Any]) -> OutputValidator: ...


@overload
def output_validator(func: None = None) -> Callable[[Callable[..., Any]], OutputValidator]: ...


def output_validator(
    func: Callable[..., Any] | None = None,
) -> OutputValidator | Callable[[Callable[..., Any]], OutputValidator]:
    """Decorate a callable as an output validator."""

    def wrap(inner: Callable[..., Any]) -> OutputValidator:
        return OutputValidator(inner)

    if func is None:
        return wrap
    return wrap(func)


def ensure_output_validator(
    value: OutputValidator | Callable[..., Any] | _native.OutputValidator,
) -> OutputValidator:
    if isinstance(value, OutputValidator):
        return value
    if isinstance(value, _native.OutputValidator):
        return _NativeOutputValidator(value)
    return OutputValidator(cast(Callable[..., Any], value))


def ensure_output_function(value: OutputFunction | _native.OutputFunction) -> OutputFunction:
    if isinstance(value, OutputFunction):
        return value
    if isinstance(value, _native.OutputFunction):
        return _NativeOutputFunction(value)
    return cast(OutputFunction, value)


class _NativeOutputValidator(OutputValidator):
    def __init__(self, native: _native.OutputValidator) -> None:
        self._native_output_validator = native

    def to_native(self) -> _native.OutputValidator:
        return self._native_output_validator


class _NativeOutputFunction(OutputFunction):
    def __init__(self, native: _native.OutputFunction) -> None:
        self._native_output_function = native

    def to_native(self) -> _native.OutputFunction:
        return self._native_output_function


def ensure_output_schema(
    value: OutputSchema | Mapping[str, Any] | _native.OutputSchema | None,
) -> _native.OutputSchema | None:
    if value is None:
        return None
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.OutputSchema, to_native())
    if isinstance(value, _native.OutputSchema):
        return value
    mapping = cast(Mapping[str, Any], value)
    if "name" in mapping and "schema" in mapping:
        return _native.OutputSchema(
            str(mapping["name"]),
            mapping["schema"],
            description=cast(str | None, mapping.get("description")),
            strict=bool(mapping.get("strict", True)),
        )
    return OutputSchema("output", mapping).to_native()


def ensure_output_policy(
    value: OutputPolicy | Mapping[str, Any] | _native.OutputPolicy | None,
) -> _native.OutputPolicy | None:
    if value is None:
        return None
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.OutputPolicy, to_native())
    if isinstance(value, _native.OutputPolicy):
        return value
    mapping = cast(Mapping[str, Any], value)
    schema = ensure_output_schema(mapping["schema"]) if "schema" in mapping else None
    policy = OutputPolicy.structured(schema) if schema is not None else OutputPolicy()
    if "mode" in mapping:
        policy = policy.with_mode(str(mapping["mode"]))
    if "retries" in mapping:
        policy = policy.with_retries(int(mapping["retries"]))
    if "allow_text_output" in mapping:
        policy = policy.allow_text_output(bool(mapping["allow_text_output"]))
    if "allow_image_output" in mapping:
        policy = policy.allow_image_output(bool(mapping["allow_image_output"]))
    return policy.to_native()


def _accepts_context(func: Callable[..., Any]) -> bool:
    try:
        parameters = list(inspect.signature(func).parameters.values())
    except (TypeError, ValueError):
        return True
    required = [
        parameter
        for parameter in parameters
        if parameter.default is inspect.Parameter.empty
        and parameter.kind
        in (inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD)
    ]
    if not required:
        return False
    first = required[0]
    return first.name in {"ctx", "context", "output_context"}
