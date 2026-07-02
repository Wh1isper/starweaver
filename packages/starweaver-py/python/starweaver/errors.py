"""Public Starweaver Python exceptions."""

from __future__ import annotations

from typing import Any


class StarweaverError(Exception):
    """Base class for Starweaver Python SDK errors."""


class AgentError(StarweaverError):
    """Agent execution failed."""


class ModelError(StarweaverError):
    """Model execution failed."""


class ToolError(StarweaverError):
    """Tool execution failed."""


class OutputError(StarweaverError):
    """Structured output validation or final-output function failed."""


class InvalidArguments(ToolError):
    """Tool arguments are invalid."""


class ModelRetry(ToolError):
    """Ask the model to retry with corrected input."""


class OutputRetry(OutputError):
    """Ask the model to retry with corrected final output."""


class OutputValidationFailed(OutputError):
    """Fail the run during output validation."""


class ApprovalRequired(ToolError):
    """Tool execution requires human approval."""

    def __init__(
        self,
        reason: str | None = None,
        *,
        metadata: dict[str, Any] | None = None,
    ) -> None:
        self.reason = reason
        self.metadata = metadata or {}
        super().__init__(reason or "approval required")


class CallDeferred(ToolError):
    """Tool execution was deferred to another worker or later run."""

    def __init__(
        self,
        reason: str | None = None,
        *,
        metadata: dict[str, Any] | None = None,
    ) -> None:
        self.reason = reason
        self.metadata = metadata or {}
        super().__init__(reason or "call deferred")


class Cancelled(ToolError):
    """Execution was cancelled."""


class Timeout(ToolError):
    """Execution timed out."""


class StateError(StarweaverError):
    """Session state is invalid."""


class StreamError(StarweaverError):
    """Stream execution failed."""
