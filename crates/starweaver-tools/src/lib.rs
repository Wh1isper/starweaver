//! Function tools, registries, toolsets, and execution primitives for Starweaver.

pub mod combinators;
pub mod context;
pub mod error;
pub mod instruction;
pub mod mcp;
pub mod prefixed;
pub mod registry;
pub mod tool;
pub mod tool_proxy;
pub mod toolset;

pub use combinators::{
    ApprovalRequiredToolset, DeferredLoadingToolset, DynamicToolset, FilteredToolset,
    PreparedToolset, RenamedToolset, ToolPredicate,
};
pub use context::{ToolApprovalState, ToolContext};
pub use error::{error_return, ToolError};
pub use instruction::ToolInstruction;
pub use mcp::{
    tool_definition_from_mcp_spec, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport,
    NativeMcpServer,
};
pub use prefixed::{PrefixedTool, PrefixedToolset};
pub use registry::ToolRegistry;
pub use tool::{
    json_tool, typed_json_tool, DynTool, EmptyToolArgs, FunctionTool, Tool, ToolResult,
    TypedFunctionTool,
};
pub use tool_proxy::{dynamic_tool_proxy, ToolProxyNamePrefixError, ToolProxyToolset};
pub use toolset::{DynToolset, StaticToolset, Toolset};
