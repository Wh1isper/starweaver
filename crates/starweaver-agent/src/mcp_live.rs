//! Host-backed MCP live adapter seam.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_tools::{
    DynTool, DynToolset, McpPromptSpec, McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec,
    McpToolSpec, McpToolset, McpToolsetConfig, McpTransport, Tool, ToolContext, ToolError,
    ToolInstruction, ToolResult, Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy,
    ToolsetLifecycleReport, ToolsetLifecycleState, ToolsetPreparation,
};
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
    /// Discovered resources.
    pub resources: Vec<McpResourceSpec>,
    /// Discovered prompts.
    pub prompts: Vec<McpPromptSpec>,
    /// Discovered sampling capability.
    pub sampling: Option<McpSamplingSpec>,
    /// Discovered subscriptions.
    pub subscriptions: Vec<McpSubscriptionSpec>,
}

impl LiveMcpServerSnapshot {
    /// Create an empty live MCP server snapshot.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            instructions: None,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            sampling: None,
            subscriptions: Vec::new(),
        }
    }

    /// Attach server instructions.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Add one tool.
    #[must_use]
    pub fn with_tool(mut self, tool: McpToolSpec) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add one resource.
    #[must_use]
    pub fn with_resource(mut self, resource: McpResourceSpec) -> Self {
        self.resources.push(resource);
        self
    }

    /// Add one prompt.
    #[must_use]
    pub fn with_prompt(mut self, prompt: McpPromptSpec) -> Self {
        self.prompts.push(prompt);
        self
    }

    /// Attach sampling capability.
    #[must_use]
    pub fn with_sampling(mut self, sampling: McpSamplingSpec) -> Self {
        self.sampling = Some(sampling);
        self
    }

    /// Add one subscription.
    #[must_use]
    pub fn with_subscription(mut self, subscription: McpSubscriptionSpec) -> Self {
        self.subscriptions.push(subscription);
        self
    }
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

    /// Execute a discovered MCP tool through the host adapter.
    ///
    /// The default returns [`LiveMcpError::ToolCallUnsupported`] so existing discovery-only
    /// clients keep the previous deferred-call behavior until they opt into live execution.
    async fn call_tool(
        &self,
        _context: ToolContext,
        id: &str,
        _transport: &McpTransport,
        tool_name: &str,
        _arguments: Value,
    ) -> Result<ToolResult, LiveMcpError> {
        Err(LiveMcpError::ToolCallUnsupported {
            server_id: id.to_string(),
            tool_name: tool_name.to_string(),
        })
    }

    /// Close any host resources owned for this MCP server.
    async fn close(&self, _id: &str, _transport: &McpTransport) -> Result<(), LiveMcpError> {
        Ok(())
    }
}

/// Shared live MCP client reference.
pub type DynLiveMcpClient = Arc<dyn LiveMcpClient>;

/// Live MCP adapter failure.
#[derive(Debug, Error)]
pub enum LiveMcpError {
    /// Host adapter does not implement live tool calls for this server/tool.
    #[error("live MCP tool call unsupported for {server_id}/{tool_name}")]
    ToolCallUnsupported {
        /// MCP server id.
        server_id: String,
        /// MCP tool name.
        tool_name: String,
    },
    /// Host adapter failed.
    #[error("live MCP adapter failed: {0}")]
    Adapter(String),
}

/// Runtime toolset backed by a host live MCP client.
pub struct LiveMcpToolset {
    client: DynLiveMcpClient,
    transport: McpTransport,
    inner: McpToolset,
}

impl LiveMcpToolset {
    fn new(client: DynLiveMcpClient, transport: McpTransport, inner: McpToolset) -> Self {
        Self {
            client,
            transport,
            inner,
        }
    }

    fn lifecycle_metadata(&self) -> Metadata {
        let config = self.inner.config();
        let mut metadata = Metadata::default();
        metadata.insert("mcp_server_id".to_string(), serde_json::json!(self.name()));
        metadata.insert(
            "mcp_transport".to_string(),
            serde_json::json!(self.transport.kind()),
        );
        metadata.insert("live_mcp".to_string(), serde_json::json!(true));
        metadata.insert(
            "mcp_resource_count".to_string(),
            serde_json::json!(config.resources.len()),
        );
        metadata.insert(
            "mcp_prompt_count".to_string(),
            serde_json::json!(config.prompts.len()),
        );
        metadata.insert(
            "mcp_sampling".to_string(),
            serde_json::json!(
                config
                    .sampling
                    .as_ref()
                    .is_some_and(|sampling| sampling.enabled)
            ),
        );
        metadata.insert(
            "mcp_subscription_count".to_string(),
            serde_json::json!(config.subscriptions.len()),
        );
        metadata
    }
}

#[derive(Clone)]
struct LiveMcpTool {
    client: DynLiveMcpClient,
    config: McpToolsetConfig,
    spec: McpToolSpec,
    name: String,
}

impl LiveMcpTool {
    fn new(client: DynLiveMcpClient, config: McpToolsetConfig, spec: McpToolSpec) -> Self {
        let name = config.tool_prefix.as_ref().map_or_else(
            || spec.name.clone(),
            |prefix| format!("{prefix}_{}", spec.name),
        );
        Self {
            client,
            config,
            spec,
            name,
        }
    }

    fn mcp_metadata(&self) -> Metadata {
        let mut metadata = self.spec.metadata.clone();
        metadata.insert(
            "mcp_server_id".to_string(),
            serde_json::json!(self.config.id),
        );
        metadata.insert(
            "mcp_transport".to_string(),
            serde_json::json!(self.config.transport.kind()),
        );
        metadata.insert(
            "mcp_tool_name".to_string(),
            serde_json::json!(self.spec.name),
        );
        if self.spec.task {
            metadata.insert("mcp_task".to_string(), serde_json::json!(true));
        }
        metadata
    }

    fn deferred_metadata(&self, arguments: &Value) -> Value {
        serde_json::json!({
            "kind": "mcp_tool_call",
            "server_id": self.config.id,
            "transport": self.config.transport,
            "tool_name": self.spec.name,
            "exposed_name": self.name,
            "arguments": arguments,
            "task": self.spec.task,
        })
    }
}

#[async_trait]
impl Tool for LiveMcpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.spec.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.spec.parameters.clone()
    }

    fn metadata(&self) -> Metadata {
        self.mcp_metadata()
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        if self.spec.task {
            return Err(ToolError::CallDeferred {
                tool: self.name.clone(),
                metadata: self.deferred_metadata(&arguments),
            });
        }
        match self
            .client
            .call_tool(
                context,
                &self.config.id,
                &self.config.transport,
                &self.spec.name,
                arguments.clone(),
            )
            .await
        {
            Ok(mut result) => {
                for (key, value) in self.mcp_metadata() {
                    result.metadata.insert(key, value);
                }
                Ok(result)
            }
            Err(LiveMcpError::ToolCallUnsupported { .. }) => Err(ToolError::CallDeferred {
                tool: self.name.clone(),
                metadata: self.deferred_metadata(&arguments),
            }),
            Err(error) => Err(ToolError::Execution {
                tool: self.name.clone(),
                message: error.to_string(),
            }),
        }
    }
}

#[async_trait]
impl Toolset for LiveMcpToolset {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn id(&self) -> Option<&str> {
        self.inner.id()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        let config = self.inner.config().clone();
        config
            .tools
            .iter()
            .cloned()
            .map(|spec| {
                Arc::new(LiveMcpTool::new(self.client.clone(), config.clone(), spec)) as DynTool
            })
            .collect()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        ToolsetLifecyclePolicy::default().with_exit_after_run(true)
    }

    async fn prepare_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let tools = self.get_tools();
        let instructions = self.get_instructions();
        let report = ToolsetLifecycleReport::new(
            self.name(),
            self.id().map(ToOwned::to_owned),
            ToolsetLifecycleState::Initialized,
            tools.len(),
            instructions.len(),
        )
        .with_metadata(self.lifecycle_metadata());
        Ok(ToolsetPreparation {
            tools,
            instructions,
            report,
        })
    }

    async fn exit_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        self.client
            .close(self.name(), &self.transport)
            .await
            .map_err(|error| ToolsetLifecycleError::failed(self.name(), error.to_string()))?;
        Ok(ToolsetLifecycleReport::new(
            self.name(),
            self.id().map(ToOwned::to_owned),
            ToolsetLifecycleState::Closed,
            0,
            0,
        ))
        .map(|report| report.with_metadata(self.lifecycle_metadata()))
    }
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
    let mut config = McpToolsetConfig::new(id, transport.clone()).with_include_instructions(true);
    if let Some(instructions) = snapshot.instructions {
        config = config.with_instructions(instructions);
    }
    for tool in snapshot.tools {
        config = config.with_tool(tool);
    }
    for resource in snapshot.resources {
        config = config.with_resource(resource);
    }
    for prompt in snapshot.prompts {
        config = config.with_prompt(prompt);
    }
    if let Some(sampling) = snapshot.sampling {
        config = config.with_sampling(sampling);
    }
    for subscription in snapshot.subscriptions {
        config = config.with_subscription(subscription);
    }
    let toolset = McpToolset::new(config);
    Ok(Arc::new(LiveMcpToolset::new(client, transport, toolset)))
}
