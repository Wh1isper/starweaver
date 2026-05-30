//! Function tools, registries, toolsets, and execution primitives for Starweaver.

pub mod context;
pub mod error;
pub mod instruction;
pub mod mcp;
pub mod prefixed;
pub mod registry;
pub mod tool;
pub mod tool_proxy;
pub mod toolset;

pub use context::ToolContext;
pub use error::{error_return, ToolError};
pub use instruction::ToolInstruction;
pub use mcp::{
    mcp_tool_definition, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport, NativeMcpServer,
};
pub use prefixed::{PrefixedTool, PrefixedToolset};
pub use registry::ToolRegistry;
pub use tool::{
    string_tool, typed_tool, DynTool, EmptyToolArgs, FunctionTool, Tool, ToolResult,
    TypedFunctionTool,
};
pub use tool_proxy::{tool_proxy_toolset, ToolProxyToolset};
pub use toolset::{DynToolset, StaticToolset, Toolset};
