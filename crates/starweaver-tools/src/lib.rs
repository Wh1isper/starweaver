//! Function tools, registries, toolsets, and execution primitives for Starweaver.

pub mod combinators;
pub mod context;
pub mod error;
pub mod execution_hooks;
pub mod instruction;
pub mod mcp;
pub mod prefixed;
pub mod registry;
pub mod tool;
pub mod tool_dependency;
pub mod tool_proxy;
pub mod toolset;

pub use combinators::{
    ApprovalRequiredToolset, CombinedToolset, DeferredToolset, DynamicToolset, FilteredToolset,
    LazyToolset, MetadataToolset, PreparedToolset, RenamedToolset, ToolPredicate,
};
pub use context::{ToolApprovalState, ToolContext};
pub use error::{ToolError, error_return};
pub use execution_hooks::{
    DynToolExecutionHook, ToolExecutionHook, ToolExecutionHooks, ToolExecutionOutcome,
};
pub use instruction::ToolInstruction;
pub use mcp::{
    McpPromptSpec, McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec, McpToolSpec, McpToolset,
    McpToolsetConfig, McpTransport, NativeMcpServer, tool_definition_from_mcp_spec,
};
pub use prefixed::{PrefixedTool, PrefixedToolset};
pub use registry::{ToolAvailabilityReport, ToolRegistry};
pub use tool::{
    DynTool, EmptyToolArgs, FunctionTool, TOOL_METADATA_CONTEXT_MANAGEMENT_KEY,
    TOOL_METADATA_HIDDEN_BY_TAGS_KEY, TOOL_METADATA_KIND_KEY, TOOL_METADATA_SELF_MANAGED_HITL_KEY,
    TOOL_METADATA_TAGS_KEY, Tool, ToolKind, ToolResult, ToolUserInputPreprocessResult,
    TypedFunctionTool, extend_tool_metadata_hidden_by_tags, extend_tool_metadata_tags, json_tool,
    set_tool_metadata_kind, tool_metadata_hidden_by_tags, tool_metadata_kind, tool_metadata_tags,
    typed_json_tool,
};
pub use tool_dependency::{
    TOOL_METADATA_DEPENDENCIES_KEY, ToolDependencyProfile, ToolDependencyRequirements,
    tool_dependency_requirements,
};
pub use tool_proxy::{
    TOOL_SEARCH_FAILED_EVENT_KIND, TOOL_SEARCH_INVALIDATED_EVENT_KIND,
    TOOL_SEARCH_NO_MATCH_EVENT_KIND, TOOL_SEARCH_REFRESHED_EVENT_KIND, ToolProxyNamePrefixError,
    ToolProxyToolset, ToolSearchInitializationReport, ToolSearchInvalidationResult,
    ToolSearchLoadResult, ToolSearchNamespaceReport, ToolSearchNamespaceStatus,
    ToolSearchRefreshBinding, ToolSearchRefreshDecision, ToolSearchRefreshReason,
    ToolSearchRefreshResult, ToolSearchRefreshSchedule, ToolSearchRefreshScheduleState,
    ToolSearchScheduledRefreshResult, ToolSearchToolset, dynamic_tool_proxy, dynamic_tool_search,
};
pub use toolset::{
    DynToolset, StaticToolset, TOOLSET_CLOSED_EVENT_KIND, TOOLSET_FAILED_EVENT_KIND,
    TOOLSET_INITIALIZED_EVENT_KIND, TOOLSET_REFRESHED_EVENT_KIND, TOOLSET_UNAVAILABLE_EVENT_KIND,
    Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy, ToolsetLifecycleReport,
    ToolsetLifecycleState, ToolsetPreparation,
};
