"""Python tool registration helpers."""

from __future__ import annotations

import asyncio
import inspect
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any, get_type_hints, overload

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
        self.return_schema = return_schema
        self.metadata = metadata or {}
        self.strict = strict
        self.sequential = sequential
        self.timeout_ms = timeout_ms
        self.max_retries = max_retries
        self._explicit_schema = parameters_schema
        self._plan = _build_invocation_plan(func)
        self.parameters_schema = parameters_schema or _infer_schema(func, self._plan)

    async def _callback(self, ctx: ToolContext, args: JsonObject) -> Any:
        try:
            result = self._call_user_function(ctx, args)
            if inspect.isawaitable(result):
                result = await result
            return result
        except InvalidArguments:
            raise
        except TypeError as exc:
            raise InvalidArguments(str(exc)) from exc

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
            keyword.update({name: args.get(name) for name in plan.keyword_params if name in args})
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
    wants_ctx = False
    args_param: str | None = None
    pydantic_model: type[Any] | None = None
    var_keyword = False
    keyword_params: list[str] = []

    for param in signature.parameters.values():
        annotation = hints.get(param.name, param.annotation)
        if param.kind is inspect.Parameter.VAR_POSITIONAL:
            raise ValueError("tool functions cannot use *args")
        if param.kind is inspect.Parameter.VAR_KEYWORD:
            var_keyword = True
            continue
        if _is_context_param(param.name, annotation):
            wants_ctx = True
            continue
        if _is_pydantic_model(annotation):
            if pydantic_model is not None or args_param is not None or keyword_params:
                raise ValueError("pydantic argument tools must use one model parameter")
            pydantic_model = annotation
            args_param = param.name
            continue
        if param.name == "args":
            args_param = param.name
            continue
        keyword_params.append(param.name)

    return _InvocationPlan(
        wants_ctx=wants_ctx,
        args_param=args_param,
        pydantic_model=pydantic_model,
        var_keyword=var_keyword,
        keyword_params=tuple(keyword_params),
    )


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


def _is_context_param(name: str, annotation: Any) -> bool:
    return name in {"ctx", "context"} or annotation is ToolContext


def _is_pydantic_model(annotation: Any) -> bool:
    return inspect.isclass(annotation) and hasattr(annotation, "model_json_schema")


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
