"""Python tool registration helpers."""

from __future__ import annotations

import asyncio
import inspect
import re
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, cast, get_type_hints, overload

from . import _native
from .errors import InvalidArguments

ToolContext = _native.ToolContext
ToolResult = _native.ToolResult

JsonObject = dict[str, Any]


@dataclass(frozen=True)
class _InvocationPlan:
    wants_ctx: bool
    args_param: str | None
    pydantic_model: type[Any] | None
    var_keyword: bool
    keyword_params: tuple[str, ...]
    required_keyword_params: tuple[str, ...]


@dataclass
class _InvocationPlanBuilder:
    wants_ctx: bool = False
    args_param: str | None = None
    pydantic_model: type[Any] | None = None
    var_keyword: bool = False
    keyword_params: list[str] | None = None
    required_keyword_params: list[str] | None = None

    def __post_init__(self) -> None:
        if self.keyword_params is None:
            self.keyword_params = []
        if self.required_keyword_params is None:
            self.required_keyword_params = []

    def build(self) -> _InvocationPlan:
        return _InvocationPlan(
            wants_ctx=self.wants_ctx,
            args_param=self.args_param,
            pydantic_model=self.pydantic_model,
            var_keyword=self.var_keyword,
            keyword_params=tuple(self.keyword_params or ()),
            required_keyword_params=tuple(self.required_keyword_params or ()),
        )


class Tool:
    """Python definition for a Starweaver runtime tool."""

    def __init__(
        self,
        func: Callable[..., Any],
        *,
        name: str | None = None,
        description: str | None = None,
        parameters_schema: JsonObject | None = None,
        return_schema: JsonObject | None = None,
        metadata: JsonObject | None = None,
        strict: bool | None = None,
        sequential: bool = False,
        timeout_ms: int | None = None,
        max_retries: int | None = None,
    ) -> None:
        self.func = func
        self.name = name or func.__name__
        self.description = description or inspect.getdoc(func)
        if strict is True and not _has_description(self.description):
            raise ValueError(f"strict tool {self.name!r} requires a description")
        self.return_schema = return_schema
        self.metadata = metadata or {}
        self.strict = strict
        self.sequential = sequential
        self.timeout_ms = timeout_ms
        self.max_retries = max_retries
        self._explicit_schema = parameters_schema
        self._plan = _build_invocation_plan(func)
        self.parameters_schema = _validate_parameters_schema(
            parameters_schema or _infer_schema(func, self._plan)
        )

    async def _callback(self, ctx: ToolContext, args: JsonObject) -> Any:
        result = self._call_user_function(ctx, args)
        if inspect.isawaitable(result):
            result = await result
        return result

    def _call_user_function(self, ctx: ToolContext, args: JsonObject) -> Any:
        plan = self._plan
        positional: list[Any] = []
        keyword: dict[str, Any] = {}
        if plan.wants_ctx:
            positional.append(ctx)
        if plan.pydantic_model is not None:
            positional.append(plan.pydantic_model.model_validate(args))
        elif plan.args_param is not None:
            positional.append(args)
        elif plan.var_keyword:
            keyword.update(args)
        else:
            keyword.update(_validated_keyword_arguments(plan, args))
        return self.func(*positional, **keyword)

    def to_native(self) -> _native.PythonTool:
        loop = asyncio.get_running_loop()
        return _native.PythonTool(
            self.name,
            self.description,
            self.parameters_schema,
            self._callback,
            loop,
            return_schema=self.return_schema,
            metadata=self.metadata,
            strict=self.strict,
            sequential=self.sequential,
            timeout_ms=self.timeout_ms,
            max_retries=self.max_retries,
        )


class BaseTool:
    """Subclass-friendly Starweaver tool definition."""

    name: str | None = None
    description: str | None = None
    parameters_schema: JsonObject | None = None
    return_schema: JsonObject | None = None
    metadata: JsonObject | None = None
    strict: bool | None = None
    sequential: bool = False
    timeout_ms: int | None = None
    max_retries: int | None = None

    def __init__(
        self,
        *,
        name: str | None = None,
        description: str | None = None,
        parameters_schema: JsonObject | None = None,
        return_schema: JsonObject | None = None,
        metadata: JsonObject | None = None,
        strict: bool | None = None,
        sequential: bool | None = None,
        timeout_ms: int | None = None,
        max_retries: int | None = None,
    ) -> None:
        if name is not None:
            self.name = name
        if description is not None:
            self.description = description
        if parameters_schema is not None:
            self.parameters_schema = parameters_schema
        if return_schema is not None:
            self.return_schema = return_schema
        if metadata is not None:
            self.metadata = metadata
        if strict is not None:
            self.strict = strict
        if sequential is not None:
            self.sequential = sequential
        if timeout_ms is not None:
            self.timeout_ms = timeout_ms
        if max_retries is not None:
            self.max_retries = max_retries

    async def call(self, ctx: ToolContext, args: JsonObject) -> Any:
        raise NotImplementedError("BaseTool subclasses must implement call")

    def to_tool(self) -> Tool:
        return Tool(
            self.call,
            name=self.name or _default_tool_name(type(self).__name__),
            description=self.description,
            parameters_schema=self.parameters_schema,
            return_schema=self.return_schema,
            metadata=self.metadata,
            strict=self.strict,
            sequential=self.sequential,
            timeout_ms=self.timeout_ms,
            max_retries=self.max_retries,
        )


@overload
def tool(
    func: Callable[..., Any],
    *,
    name: str | None = None,
    description: str | None = None,
    parameters_schema: JsonObject | None = None,
    return_schema: JsonObject | None = None,
    metadata: JsonObject | None = None,
    strict: bool | None = None,
    sequential: bool = False,
    timeout_ms: int | None = None,
    max_retries: int | None = None,
) -> Tool: ...


@overload
def tool(
    func: None = None,
    *,
    name: str | None = None,
    description: str | None = None,
    parameters_schema: JsonObject | None = None,
    return_schema: JsonObject | None = None,
    metadata: JsonObject | None = None,
    strict: bool | None = None,
    sequential: bool = False,
    timeout_ms: int | None = None,
    max_retries: int | None = None,
) -> Callable[[Callable[..., Any]], Tool]: ...


def tool(
    func: Callable[..., Any] | None = None,
    *,
    name: str | None = None,
    description: str | None = None,
    parameters_schema: JsonObject | None = None,
    return_schema: JsonObject | None = None,
    metadata: JsonObject | None = None,
    strict: bool | None = None,
    sequential: bool = False,
    timeout_ms: int | None = None,
    max_retries: int | None = None,
) -> Tool | Callable[[Callable[..., Any]], Tool]:
    """Decorate a Python callable as a Starweaver runtime tool."""

    def wrap(inner: Callable[..., Any]) -> Tool:
        return Tool(
            inner,
            name=name,
            description=description,
            parameters_schema=parameters_schema,
            return_schema=return_schema,
            metadata=metadata,
            strict=strict,
            sequential=sequential,
            timeout_ms=timeout_ms,
            max_retries=max_retries,
        )

    if func is None:
        return wrap
    return wrap(func)


def _build_invocation_plan(func: Callable[..., Any]) -> _InvocationPlan:
    signature = inspect.signature(func)
    hints = get_type_hints(func)
    builder = _InvocationPlanBuilder()

    for param in signature.parameters.values():
        annotation = hints.get(param.name, param.annotation)
        _add_invocation_parameter(builder, param, annotation)

    return builder.build()


def _add_invocation_parameter(
    builder: _InvocationPlanBuilder,
    param: inspect.Parameter,
    annotation: Any,
) -> None:
    keyword_params, required_keyword_params = _builder_keyword_lists(builder)
    if param.kind is inspect.Parameter.VAR_POSITIONAL:
        raise ValueError("tool functions cannot use *args")
    if param.kind is inspect.Parameter.VAR_KEYWORD:
        _set_var_keyword_argument(builder)
        return
    if _is_context_param(param.name, annotation):
        _set_context_argument(builder, keyword_params)
        return
    if _is_pydantic_model(annotation):
        _set_pydantic_argument(builder, param.name, annotation)
        return
    if param.name == "args":
        _set_args_argument(builder, param.name)
        return
    if builder.args_param is not None or builder.pydantic_model is not None:
        raise ValueError("tool argument styles cannot be mixed")
    keyword_params.append(param.name)
    if param.default is inspect.Parameter.empty:
        required_keyword_params.append(param.name)


def _builder_keyword_lists(
    builder: _InvocationPlanBuilder,
) -> tuple[list[str], list[str]]:
    keyword_params = builder.keyword_params
    if keyword_params is None:
        keyword_params = []
        builder.keyword_params = keyword_params
    required_keyword_params = builder.required_keyword_params
    if required_keyword_params is None:
        required_keyword_params = []
        builder.required_keyword_params = required_keyword_params
    return keyword_params, required_keyword_params


def _set_var_keyword_argument(builder: _InvocationPlanBuilder) -> None:
    if builder.args_param is not None or builder.pydantic_model is not None:
        raise ValueError("**kwargs cannot be mixed with args or pydantic parameters")
    builder.var_keyword = True


def _set_context_argument(
    builder: _InvocationPlanBuilder,
    keyword_params: list[str],
) -> None:
    if builder.args_param is not None or builder.pydantic_model is not None or keyword_params:
        raise ValueError("ToolContext parameter must precede tool argument parameters")
    builder.wants_ctx = True


def _set_pydantic_argument(
    builder: _InvocationPlanBuilder,
    name: str,
    annotation: type[Any],
) -> None:
    if (
        builder.pydantic_model is not None
        or builder.args_param is not None
        or builder.keyword_params
        or builder.var_keyword
    ):
        raise ValueError("pydantic argument tools must use one model parameter")
    builder.pydantic_model = annotation
    builder.args_param = name


def _set_args_argument(builder: _InvocationPlanBuilder, name: str) -> None:
    if (
        builder.args_param is not None
        or builder.pydantic_model is not None
        or builder.keyword_params
        or builder.var_keyword
    ):
        raise ValueError("args parameter cannot be mixed with other tool arguments")
    builder.args_param = name


def _infer_schema(func: Callable[..., Any], plan: _InvocationPlan) -> JsonObject:
    if plan.pydantic_model is not None:
        return plan.pydantic_model.model_json_schema()
    signature = inspect.signature(func)
    hints = get_type_hints(func)
    if plan.args_param is not None and plan.pydantic_model is None:
        return {"type": "object", "additionalProperties": True}
    if plan.var_keyword and not plan.keyword_params:
        raise ValueError("**kwargs tools require an explicit parameters_schema")

    properties: dict[str, Any] = {}
    required: list[str] = []
    for name in plan.keyword_params:
        param = signature.parameters[name]
        annotation = hints.get(name, param.annotation)
        properties[name] = _json_schema_for_type(annotation)
        if param.default is inspect.Parameter.empty:
            required.append(name)
    schema: JsonObject = {"type": "object", "properties": properties}
    if required:
        schema["required"] = required
    return schema


def _validate_parameters_schema(schema: Mapping[str, Any]) -> JsonObject:
    if not isinstance(schema, Mapping):
        raise TypeError("parameters_schema must be a JSON object")
    schema_dict = dict(schema)
    _validate_json_schema_value(schema_dict, "parameters_schema")
    schema_type = schema_dict.get("type")
    if schema_type != "object":
        raise ValueError("parameters_schema must declare type 'object'")
    property_names = _validate_schema_properties(schema_dict.get("properties"))
    _validate_schema_required(schema_dict.get("required"), property_names)
    _validate_schema_additional_properties(schema_dict.get("additionalProperties"))
    return cast(JsonObject, schema_dict)


def _validate_schema_properties(properties: object) -> set[str] | None:
    if properties is not None:
        if not isinstance(properties, Mapping):
            raise TypeError("parameters_schema properties must be an object")
        property_names = set()
        for name, value in properties.items():
            if not isinstance(name, str):
                raise TypeError("parameters_schema property names must be strings")
            if not isinstance(value, Mapping):
                raise TypeError(f"parameters_schema property {name!r} must be an object")
            property_names.add(name)
        return property_names
    return None


def _validate_schema_required(required: object, property_names: set[str] | None) -> None:
    if required is not None:
        if isinstance(required, str) or not isinstance(required, Sequence):
            raise TypeError("parameters_schema required must be a list of strings")
        required_names: list[str] = []
        for value in required:
            if not isinstance(value, str):
                raise TypeError("parameters_schema required must be a list of strings")
            required_names.append(value)
        if property_names is not None:
            unknown = sorted(set(required_names) - property_names)
            if unknown:
                joined = ", ".join(unknown)
                raise ValueError(f"parameters_schema required names are not properties: {joined}")


def _validate_schema_additional_properties(additional: object) -> None:
    if additional is not None and not isinstance(additional, (bool, Mapping)):
        raise TypeError("parameters_schema additionalProperties must be a boolean or object")


def _validate_json_schema_value(value: object, path: str) -> None:
    if value is None or isinstance(value, (str, int, float, bool)):
        return
    if isinstance(value, Mapping):
        for key, nested in value.items():
            if not isinstance(key, str):
                raise TypeError(f"{path} object keys must be strings")
            _validate_json_schema_value(nested, f"{path}.{key}")
        return
    if isinstance(value, Sequence) and not isinstance(value, (str, bytes, bytearray)):
        for index, nested in enumerate(value):
            _validate_json_schema_value(nested, f"{path}[{index}]")
        return
    raise ValueError(f"{path} must contain only JSON-compatible values")


def _validated_keyword_arguments(plan: _InvocationPlan, args: JsonObject) -> dict[str, Any]:
    accepted = set(plan.keyword_params)
    unexpected = sorted(set(args) - accepted)
    if unexpected:
        raise InvalidArguments(f"unexpected tool argument(s): {', '.join(unexpected)}")
    missing = [name for name in plan.required_keyword_params if name not in args]
    if missing:
        raise InvalidArguments(f"missing required tool argument(s): {', '.join(missing)}")
    return {name: args[name] for name in plan.keyword_params if name in args}


def _is_context_param(name: str, annotation: Any) -> bool:
    if _is_tool_context_annotation(annotation):
        return True
    return name == "ctx" and annotation is inspect.Parameter.empty


def _is_tool_context_annotation(annotation: Any) -> bool:
    if annotation is ToolContext:
        return True
    return getattr(annotation, "__name__", None) == "ToolContext" and getattr(
        annotation, "__module__", ""
    ).startswith("starweaver")


def _is_pydantic_model(annotation: Any) -> bool:
    return inspect.isclass(annotation) and hasattr(annotation, "model_json_schema")


def _has_description(description: str | None) -> bool:
    return bool(description and description.strip())


def _json_schema_for_type(annotation: Any) -> JsonObject:
    if annotation is inspect.Parameter.empty or annotation is Any:
        raise ValueError("tool parameter type hints are required unless parameters_schema is set")
    if annotation is str:
        return {"type": "string"}
    if annotation is int:
        return {"type": "integer"}
    if annotation is float:
        return {"type": "number"}
    if annotation is bool:
        return {"type": "boolean"}
    if annotation in {dict, Mapping}:
        return {"type": "object"}
    if annotation is list:
        return {"type": "array"}
    origin = getattr(annotation, "__origin__", None)
    if origin is list:
        return {"type": "array"}
    if origin in {dict, Mapping}:
        return {"type": "object"}
    raise ValueError(f"unsupported tool parameter type hint: {annotation!r}")


ToolLike = Tool | BaseTool | Callable[..., Any]


def ensure_tool(value: ToolLike) -> Tool:
    if isinstance(value, Tool):
        return value
    if isinstance(value, BaseTool):
        return value.to_tool()
    if callable(value):
        return Tool(value)
    raise TypeError("tools must be Tool, BaseTool, or callable objects")


def _default_tool_name(class_name: str) -> str:
    if class_name.endswith("Tool") and len(class_name) > len("Tool"):
        class_name = class_name[: -len("Tool")]
    snake = re.sub(r"(?<!^)(?=[A-Z])", "_", class_name).lower()
    return snake or "tool"
