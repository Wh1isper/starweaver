"""Python bindings for Starweaver."""

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _distribution_version

from ._native import version
from .agent import (
    Agent,
    AgentRun,
    AgentSession,
    AgentStream,
    ApprovalDecision,
    BusMessage,
    ControlReceipt,
    DeferredResult,
    HitlSnapshot,
    MessageBus,
    MessageDelivery,
    PendingApproval,
    PendingDeferred,
    RunResult,
    RunStatusSnapshot,
    SessionArchive,
    StreamEvent,
    StreamRunResult,
    create_agent,
)
from .capability import CapabilityBundle
from .environment import EnvironmentProvider
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
from .media import MediaUploader, MediaUploadRequest
from .model import ModelSettings, ProviderAuth, ProviderModel, RequestParams
from .output import (
    OutputContext,
    OutputFunction,
    OutputPolicy,
    OutputSchema,
    OutputValidator,
    OutputValue,
    output_validator,
)
from .resources import RESOURCE_REF_KIND_KEY, ResourceRef, ResourceRegistry
from .runtime import RuntimeConfig
from .skills import SkillPackage, SkillRegistry, SkillSourceScope
from .store import (
    ApprovalRecord,
    CheckpointRef,
    DeferredToolRecord,
    InMemorySessionStore,
    JsonSessionStore,
    RunRecord,
    SessionRecord,
    SessionResumeSnapshot,
    SessionStore,
    StreamRecord,
)
from .stream_adapter import StreamAdapter
from .subagent import Subagent
from .testing import FunctionModel, TestModel
from .tool import BaseTool, Tool, ToolContext, ToolResult, tool
from .toolset import (
    ToolLibrary,
    ToolProxyToolset,
    ToolSearchToolset,
    Toolset,
    environment_toolsets,
    filesystem_toolset,
    shell_toolset,
)

try:
    __version__ = _distribution_version("starweaver")
except PackageNotFoundError:
    __version__ = version()

__all__ = [
    "RESOURCE_REF_KIND_KEY",
    "Agent",
    "AgentError",
    "AgentRun",
    "AgentSession",
    "AgentStream",
    "ApprovalDecision",
    "ApprovalRecord",
    "ApprovalRequired",
    "BaseTool",
    "BusMessage",
    "CallDeferred",
    "Cancelled",
    "CapabilityBundle",
    "CheckpointRef",
    "ControlReceipt",
    "DeferredResult",
    "DeferredToolRecord",
    "EnvironmentProvider",
    "FunctionModel",
    "HitlSnapshot",
    "InMemorySessionStore",
    "InvalidArguments",
    "JsonSessionStore",
    "MediaUploadRequest",
    "MediaUploader",
    "MessageBus",
    "MessageDelivery",
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
    "PendingApproval",
    "PendingDeferred",
    "ProviderAuth",
    "ProviderModel",
    "RequestParams",
    "ResourceRef",
    "ResourceRegistry",
    "RunRecord",
    "RunResult",
    "RunStatusSnapshot",
    "RuntimeConfig",
    "SessionArchive",
    "SessionRecord",
    "SessionResumeSnapshot",
    "SessionStore",
    "SkillPackage",
    "SkillRegistry",
    "SkillSourceScope",
    "StarweaverError",
    "StateError",
    "StreamAdapter",
    "StreamError",
    "StreamEvent",
    "StreamRecord",
    "StreamRunResult",
    "Subagent",
    "TestModel",
    "Timeout",
    "Tool",
    "ToolContext",
    "ToolError",
    "ToolLibrary",
    "ToolProxyToolset",
    "ToolResult",
    "ToolSearchToolset",
    "Toolset",
    "__version__",
    "create_agent",
    "environment_toolsets",
    "filesystem_toolset",
    "output_validator",
    "shell_toolset",
    "tool",
    "version",
]
