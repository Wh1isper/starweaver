//! Host-backed MCP live adapter seam.

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_tools::{DynToolset, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport};
use thiserror::Error;

/// Snapshot discovered from a live MCP server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveMcpServerSnapshot {
    /// Server id.
    pub id: String,
    /// Server instructions, when provided.
    pub instructions: Option<String>,
    /// Discovered tools.
    pub tools: Vec<McpToolSpec>,
}

/// Host-implemented MCP client adapter.
#[async_trait]
pub trait LiveMcpClient: Send + Sync {
    /// Discover MCP server capabilities and tools.
    async fn discover(
        &self,
        id: &str,
        transport: &McpTransport,
    ) -> Result<LiveMcpServerSnapshot, LiveMcpError>;
}

/// Shared live MCP client reference.
pub type DynLiveMcpClient = Arc<dyn LiveMcpClient>;

/// Live MCP adapter failure.
#[derive(Debug, Error)]
pub enum LiveMcpError {
    /// Host adapter failed.
    #[error("live MCP adapter failed: {0}")]
    Adapter(String),
}

/// Discover a live MCP server and return a Starweaver toolset foundation.
///
/// # Errors
///
/// Returns an error when the host MCP client cannot discover the server.
pub async fn live_mcp_toolset(
    client: DynLiveMcpClient,
    id: impl Into<String>,
    transport: McpTransport,
) -> Result<DynToolset, LiveMcpError> {
    let id = id.into();
    let snapshot = client.discover(&id, &transport).await?;
    let mut config = McpToolsetConfig::new(id, transport).with_include_instructions(true);
    if let Some(instructions) = snapshot.instructions {
        config = config.with_instructions(instructions);
    }
    for tool in snapshot.tools {
        config = config.with_tool(tool);
    }
    Ok(Arc::new(McpToolset::new(config)))
}
