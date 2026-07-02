"""Python bindings for Starweaver."""

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _distribution_version

from ._native import version
from .agent import Agent, AgentSession, AgentStream, create_agent
from .capability import CapabilityBundle
from .errors import (
    AgentError,
    ApprovalRequired,
    CallDeferred,
    Cancelled,
    InvalidArguments,
    ModelError,
    ModelRetry,
    OutputError,
    OutputRetry,
    OutputValidationFailed,
    StarweaverError,
    StateError,
    StreamError,
    Timeout,
    ToolError,
)
from .model import ModelSettings, ProviderModel, RequestParams
from .output import (
    OutputContext,
    OutputFunction,
    OutputPolicy,
    OutputSchema,
    OutputValidator,
    OutputValue,
    output_validator,
)
from .subagent import Subagent
from .testing import FunctionModel, TestModel
from .tool import BaseTool, Tool, ToolContext, ToolResult, tool

try:
    __version__ = _distribution_version("starweaver")
except PackageNotFoundError:
    __version__ = version()

__all__ = [
    "Agent",
    "AgentError",
    "AgentSession",
    "AgentStream",
    "ApprovalRequired",
    "BaseTool",
    "CallDeferred",
    "Cancelled",
    "CapabilityBundle",
    "FunctionModel",
    "InvalidArguments",
    "ModelError",
    "ModelRetry",
    "ModelSettings",
    "OutputContext",
    "OutputError",
    "OutputFunction",
    "OutputPolicy",
    "OutputRetry",
    "OutputSchema",
    "OutputValidationFailed",
    "OutputValidator",
    "OutputValue",
    "ProviderModel",
    "RequestParams",
    "StarweaverError",
    "StateError",
    "StreamError",
    "Subagent",
    "TestModel",
    "Timeout",
    "Tool",
    "ToolContext",
    "ToolError",
    "ToolResult",
    "__version__",
    "create_agent",
    "output_validator",
    "tool",
    "version",
]
