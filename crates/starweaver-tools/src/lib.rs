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
pub mod tool_proxy;
pub mod toolset;

pub use combinators::{
    ApprovalRequiredToolset, DeferredToolset, DynamicToolset, FilteredToolset, LazyToolset,
    PreparedToolset, RenamedToolset, ToolPredicate,
};
pub use context::{ToolApprovalState, ToolContext};
pub use error::{error_return, ToolError};
pub use execution_hooks::{
    DynToolExecutionHook, ToolExecutionHook, ToolExecutionHooks, ToolExecutionOutcome,
};
pub use instruction::ToolInstruction;
pub use mcp::{
    tool_definition_from_mcp_spec, McpPromptSpec, McpResourceSpec, McpSamplingSpec,
    McpSubscriptionSpec, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport, NativeMcpServer,
};
pub use prefixed::{PrefixedTool, PrefixedToolset};
pub use registry::{ToolAvailabilityReport, ToolRegistry};
pub use tool::{
    extend_tool_metadata_hidden_by_tags, extend_tool_metadata_tags, json_tool,
    set_tool_metadata_kind, tool_metadata_hidden_by_tags, tool_metadata_kind, tool_metadata_tags,
    typed_json_tool, DynTool, EmptyToolArgs, FunctionTool, Tool, ToolKind, ToolResult,
    ToolUserInputPreprocessResult, TypedFunctionTool, TOOL_METADATA_CONTEXT_MANAGEMENT_KEY,
    TOOL_METADATA_HIDDEN_BY_TAGS_KEY, TOOL_METADATA_KIND_KEY, TOOL_METADATA_TAGS_KEY,
};
pub use tool_proxy::{
    dynamic_tool_proxy, dynamic_tool_search, ToolProxyNamePrefixError, ToolProxyToolset,
    ToolSearchInitializationReport, ToolSearchInvalidationResult, ToolSearchLoadResult,
    ToolSearchNamespaceReport, ToolSearchNamespaceStatus, ToolSearchRefreshBinding,
    ToolSearchRefreshDecision, ToolSearchRefreshReason, ToolSearchRefreshResult,
    ToolSearchRefreshSchedule, ToolSearchRefreshScheduleState, ToolSearchScheduledRefreshResult,
    ToolSearchToolset, TOOL_SEARCH_FAILED_EVENT_KIND, TOOL_SEARCH_INVALIDATED_EVENT_KIND,
    TOOL_SEARCH_NO_MATCH_EVENT_KIND, TOOL_SEARCH_REFRESHED_EVENT_KIND,
};
pub use toolset::{
    DynToolset, StaticToolset, Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy,
    ToolsetLifecycleReport, ToolsetLifecycleState, ToolsetPreparation, TOOLSET_CLOSED_EVENT_KIND,
    TOOLSET_FAILED_EVENT_KIND, TOOLSET_INITIALIZED_EVENT_KIND, TOOLSET_REFRESHED_EVENT_KIND,
    TOOLSET_UNAVAILABLE_EVENT_KIND,
};
