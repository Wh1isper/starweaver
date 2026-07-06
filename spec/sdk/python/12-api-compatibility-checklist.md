# Python Public API Compatibility Checklist

This checklist is the public-name contract for `starweaver-py` application
readiness. It complements the narrative API specs by giving release reviewers a
single auditable list of top-level names that must remain importable from
`starweaver`.

The Python package test suite reads the literal block below and compares it
against `starweaver.__all__`. Any public name added, removed, renamed, or moved
between stability tiers must update this checklist and the corresponding docs
or migration notes in the same change.

Rules:

- Names listed here are stable top-level imports unless a migration note says
  otherwise.
- Compatibility aliases such as `AgentStream` stay listed while supported.
- Raw evidence helpers stay listed when downstream product code may need them
  for forward compatibility.
- Experimental product-specific concepts must not be added here until they are
  SDK-owned and tested.
- Private modules, PyO3 internals, callback objects, and process-local handles
  must not appear here.

## Public Export Groups

<!-- public-api-groups:start -->

```python
PUBLIC_API_GROUPS = {
    "agent_session_runtime": [
        "create_agent",
        "create_agent_runtime",
        "Agent",
        "AgentRuntime",
        "AgentSession",
        "AgentRun",
        "AgentStream",
        "RunResult",
        "StreamRunResult",
        "RunStatusSnapshot",
        "StreamEvent",
        "SessionArchive",
    ],
    "active_control_message_hitl": [
        "BusMessage",
        "MessageBus",
        "MessageDelivery",
        "ControlReceipt",
        "HitlSnapshot",
        "PendingApproval",
        "PendingDeferred",
        "ApprovalDecision",
        "DeferredResult",
    ],
    "tools_toolsets_mcp": [
        "tool",
        "Tool",
        "BaseTool",
        "ToolContext",
        "ToolResult",
        "Toolset",
        "ToolsetContext",
        "ToolsetPreparation",
        "AbstractToolset",
        "PythonDynamicToolset",
        "FunctionToolset",
        "ToolsetFactory",
        "toolset_factory",
        "ToolLibrary",
        "ToolSearchToolset",
        "ToolProxyToolset",
        "ToolsetIdentity",
        "ToolsetIdIssue",
        "ToolsetIdValidation",
        "validate_toolset_ids",
        "validate_toolsets_for_durability",
        "ToolsetLifecyclePolicy",
        "ToolsetLifecycleReport",
        "ToolsetLifecycleState",
        "filesystem_toolset",
        "shell_toolset",
        "environment_toolsets",
        "McpTransport",
        "McpToolset",
        "McpToolSpec",
        "McpResourceSpec",
        "McpPromptSpec",
        "McpSamplingSpec",
        "McpSubscriptionSpec",
    ],
    "models_output_runtime_composition": [
        "ProviderModel",
        "ProviderAuth",
        "ModelSettings",
        "RequestParams",
        "RuntimeConfig",
        "CapabilityBundle",
        "PythonCapability",
        "OutputSchema",
        "OutputPolicy",
        "OutputContext",
        "OutputFunction",
        "OutputValidator",
        "OutputValue",
        "output_validator",
    ],
    "environment_resources_skills_media": [
        "Environment",
        "EnvironmentProvider",
        "EnvdEnvironment",
        "PythonEnvironmentProvider",
        "LocalEnvironment",
        "VirtualEnvironment",
        "FileOperator",
        "Shell",
        "ShellProcess",
        "WorkspaceBinding",
        "VirtualMount",
        "VirtualPath",
        "BaseResource",
        "ResumableResource",
        "InstructableResource",
        "ResourceRef",
        "ResourceRegistry",
        "ResourceRegistryState",
        "RESOURCE_REF_KIND_KEY",
        "SkillRegistry",
        "SkillPackage",
        "SkillSourceScope",
        "MediaUploader",
        "MediaUploadRequest",
    ],
    "storage_stream_observability": [
        "SessionStore",
        "InMemorySessionStore",
        "JsonSessionStore",
        "SqliteSessionStore",
        "SqliteReplayEventLog",
        "SqliteStreamArchive",
        "InputPart",
        "SessionStatus",
        "RunStatus",
        "ExecutionStatus",
        "SessionRecord",
        "RunRecord",
        "StreamRecord",
        "CheckpointRef",
        "ApprovalRecord",
        "DeferredToolRecord",
        "SessionResumeSnapshot",
        "StreamAdapter",
        "Usage",
        "UsageAgentTotal",
        "UsageSnapshot",
        "UsageSnapshotEntry",
        "PricingEstimate",
        "TraceMetadata",
    ],
    "subagents_testing_errors_version": [
        "Subagent",
        "TestModel",
        "FunctionModel",
        "StarweaverError",
        "AgentError",
        "ToolError",
        "ModelError",
        "StateError",
        "StreamError",
        "OutputError",
        "InvalidArguments",
        "ApprovalRequired",
        "CallDeferred",
        "Cancelled",
        "Timeout",
        "ModelRetry",
        "OutputRetry",
        "OutputValidationFailed",
        "__version__",
        "version",
    ],
}
```

<!-- public-api-groups:end -->

## Review Checklist

Before release:

1. Run `uv run pytest packages/starweaver-py/tests`.
2. Confirm `test_public_api_compatibility_checklist_matches_starweaver_exports`
   passes.
3. Confirm each added public name is documented in the relevant Python docs
   page before it is added to this checklist.
4. Keep migration notes in `docs/python/stability.md` when a supported alias or
   provisional name changes behavior.
5. Do not list product-owned Claw names, service DTOs, UI types, scheduler
   concepts, Docker policy, or bridge-specific records in this SDK checklist.
