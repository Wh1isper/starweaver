//! Reusable toolsets.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentEvent};
use starweaver_core::Metadata;
use thiserror::Error;

use crate::{DynTool, ToolInstruction};

/// Shared reference to a runtime toolset.
pub type DynToolset = Arc<dyn Toolset>;

/// Event emitted when a toolset is initialized for a context.
pub const TOOLSET_INITIALIZED_EVENT_KIND: &str = "toolset_initialized";
/// Event emitted when a toolset is unavailable for a context.
pub const TOOLSET_UNAVAILABLE_EVENT_KIND: &str = "toolset_unavailable";
/// Event emitted when a toolset initialization fails for a context.
pub const TOOLSET_FAILED_EVENT_KIND: &str = "toolset_failed";
/// Event emitted when a toolset refreshes its visible inventory.
pub const TOOLSET_REFRESHED_EVENT_KIND: &str = "toolset_refreshed";
/// Event emitted when a toolset exits a context.
pub const TOOLSET_CLOSED_EVENT_KIND: &str = "toolset_closed";

/// Lifecycle state reported by a context-aware toolset operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolsetLifecycleState {
    /// Toolset initialized and returned its current inventory.
    Initialized,
    /// Toolset is intentionally unavailable for the current context.
    Unavailable,
    /// Toolset failed while preparing or refreshing.
    Failed,
    /// Toolset refreshed its visible inventory.
    Refreshed,
    /// Toolset completed its context exit/cleanup hook.
    Closed,
}

impl ToolsetLifecycleState {
    /// Stable event payload value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialized => "initialized",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Refreshed => "refreshed",
            Self::Closed => "closed",
        }
    }

    /// Default stream/event kind for this lifecycle state.
    #[must_use]
    pub const fn event_kind(self) -> &'static str {
        match self {
            Self::Initialized => TOOLSET_INITIALIZED_EVENT_KIND,
            Self::Unavailable => TOOLSET_UNAVAILABLE_EVENT_KIND,
            Self::Failed => TOOLSET_FAILED_EVENT_KIND,
            Self::Refreshed => TOOLSET_REFRESHED_EVENT_KIND,
            Self::Closed => TOOLSET_CLOSED_EVENT_KIND,
        }
    }
}

/// Lifecycle timeout and failure policy for a context-aware toolset.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolsetLifecyclePolicy {
    /// Timeout for context-aware initialization/enter hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization_timeout_ms: Option<u64>,
    /// Timeout for context-aware inventory reads and refreshes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_ms: Option<u64>,
    /// Timeout for context-aware exit/cleanup hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_timeout_ms: Option<u64>,
    /// Whether the runtime should call `enter_with_context` before reading inventory.
    #[serde(default)]
    pub enter_before_prepare: bool,
    /// Whether the runtime should call `exit_with_context` before the run exits.
    #[serde(default)]
    pub exit_after_run: bool,
    /// Whether unavailable toolsets should fail the owning run.
    #[serde(default)]
    pub fail_on_unavailable: bool,
}

impl ToolsetLifecyclePolicy {
    /// Set initialization timeout in milliseconds.
    #[must_use]
    pub const fn with_initialization_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.initialization_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set inventory read/refresh timeout in milliseconds.
    #[must_use]
    pub const fn with_read_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.read_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set exit/cleanup timeout in milliseconds.
    #[must_use]
    pub const fn with_exit_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.exit_timeout_ms = Some(timeout_ms);
        self
    }

    /// Configure whether the runtime calls `enter_with_context` before inventory preparation.
    #[must_use]
    pub const fn with_enter_before_prepare(mut self, enter_before_prepare: bool) -> Self {
        self.enter_before_prepare = enter_before_prepare;
        self
    }

    /// Configure whether the runtime calls `exit_with_context` before run exit.
    #[must_use]
    pub const fn with_exit_after_run(mut self, exit_after_run: bool) -> Self {
        self.exit_after_run = exit_after_run;
        self
    }

    /// Configure whether unavailable toolsets fail the run.
    #[must_use]
    pub const fn with_fail_on_unavailable(mut self, fail_on_unavailable: bool) -> Self {
        self.fail_on_unavailable = fail_on_unavailable;
        self
    }
}

/// Toolset lifecycle report emitted into context events.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolsetLifecycleReport {
    /// Toolset display name.
    pub name: String,
    /// Stable toolset identifier, when provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Lifecycle state.
    pub state: ToolsetLifecycleState,
    /// Number of tools visible after this operation.
    pub tool_count: usize,
    /// Number of instruction blocks visible after this operation.
    pub instruction_count: usize,
    /// Optional diagnostic message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Extra lifecycle metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl ToolsetLifecycleReport {
    /// Build a report for a toolset operation.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        id: Option<String>,
        state: ToolsetLifecycleState,
        tool_count: usize,
        instruction_count: usize,
    ) -> Self {
        Self {
            name: name.into(),
            id,
            state,
            tool_count,
            instruction_count,
            message: None,
            metadata: Metadata::default(),
        }
    }

    /// Attach a diagnostic message.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attach lifecycle metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Convert this report into an event payload.
    #[must_use]
    pub fn into_payload(self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "id": self.id,
            "state": self.state.as_str(),
            "tool_count": self.tool_count,
            "instruction_count": self.instruction_count,
            "message": self.message,
            "metadata": self.metadata,
        })
    }

    /// Convert this report into a context event.
    #[must_use]
    pub fn into_event(self) -> AgentEvent {
        AgentEvent::new(self.state.event_kind(), self.into_payload())
    }
}

/// Materialized toolset inventory for a specific context.
#[derive(Clone)]
pub struct ToolsetPreparation {
    /// Tools visible for the current context.
    pub tools: Vec<DynTool>,
    /// Instructions visible for the current context.
    pub instructions: Vec<ToolInstruction>,
    /// Lifecycle report for this preparation.
    pub report: ToolsetLifecycleReport,
}

impl ToolsetPreparation {
    /// Build a successful initialization preparation.
    #[must_use]
    pub fn initialized(
        name: impl Into<String>,
        id: Option<String>,
        tools: Vec<DynTool>,
        instructions: Vec<ToolInstruction>,
    ) -> Self {
        let report = ToolsetLifecycleReport::new(
            name,
            id,
            ToolsetLifecycleState::Initialized,
            tools.len(),
            instructions.len(),
        );
        Self {
            tools,
            instructions,
            report,
        }
    }

    /// Build an unavailable preparation that exposes no inventory.
    #[must_use]
    pub fn unavailable(
        name: impl Into<String>,
        id: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        let report =
            ToolsetLifecycleReport::new(name, id, ToolsetLifecycleState::Unavailable, 0, 0)
                .with_message(message);
        Self {
            tools: Vec::new(),
            instructions: Vec::new(),
            report,
        }
    }

    /// Override the lifecycle report.
    #[must_use]
    pub fn with_report(mut self, report: ToolsetLifecycleReport) -> Self {
        self.report = report;
        self
    }
}

/// Error returned by context-aware toolset lifecycle hooks.
#[derive(Debug, Error)]
pub enum ToolsetLifecycleError {
    /// Toolset is unavailable for the current context.
    #[error("toolset {toolset} unavailable: {message}")]
    Unavailable {
        /// Toolset name.
        toolset: String,
        /// Diagnostic message.
        message: String,
    },
    /// Toolset lifecycle operation failed.
    #[error("toolset {toolset} failed: {message}")]
    Failed {
        /// Toolset name.
        toolset: String,
        /// Diagnostic message.
        message: String,
    },
    /// Toolset lifecycle operation exceeded its timeout.
    #[error("toolset {toolset} timed out after {timeout_ms}ms")]
    Timeout {
        /// Toolset name.
        toolset: String,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
}

impl ToolsetLifecycleError {
    /// Build an unavailable error.
    #[must_use]
    pub fn unavailable(toolset: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Unavailable {
            toolset: toolset.into(),
            message: message.into(),
        }
    }

    /// Build a failed error.
    #[must_use]
    pub fn failed(toolset: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Failed {
            toolset: toolset.into(),
            message: message.into(),
        }
    }

    /// Build a timeout error.
    #[must_use]
    pub fn timeout(toolset: impl Into<String>, timeout_ms: u64) -> Self {
        Self::Timeout {
            toolset: toolset.into(),
            timeout_ms,
        }
    }

    /// Convert this error into a lifecycle report.
    #[must_use]
    pub fn to_report(&self, id: Option<String>) -> ToolsetLifecycleReport {
        match self {
            Self::Unavailable { toolset, message } => ToolsetLifecycleReport::new(
                toolset.clone(),
                id,
                ToolsetLifecycleState::Unavailable,
                0,
                0,
            )
            .with_message(message.clone()),
            Self::Failed { toolset, message } => ToolsetLifecycleReport::new(
                toolset.clone(),
                id,
                ToolsetLifecycleState::Failed,
                0,
                0,
            )
            .with_message(message.clone()),
            Self::Timeout {
                toolset,
                timeout_ms,
            } => ToolsetLifecycleReport::new(
                toolset.clone(),
                id,
                ToolsetLifecycleState::Failed,
                0,
                0,
            )
            .with_message(format!("timed out after {timeout_ms}ms")),
        }
    }
}

/// Reusable group of tools and instructions.
#[async_trait]
pub trait Toolset: Send + Sync {
    /// Toolset name.
    fn name(&self) -> &str;

    /// Optional stable toolset identifier for durable runtimes and namespace-level loading.
    fn id(&self) -> Option<&str> {
        None
    }

    /// Tools currently available from this toolset.
    fn get_tools(&self) -> Vec<DynTool>;

    /// Retry default inherited by tools that do not set their own limit.
    fn max_retries(&self) -> Option<usize> {
        None
    }

    /// Execution timeout inherited by tools that do not set their own timeout.
    fn timeout_ms(&self) -> Option<u64> {
        None
    }

    /// Instruction blocks contributed by this toolset.
    fn get_instructions(&self) -> Vec<ToolInstruction> {
        Vec::new()
    }

    /// Lifecycle policy for context-aware preparation and cleanup.
    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        ToolsetLifecyclePolicy::default()
    }

    /// Prepare tools and instructions for a concrete agent context.
    async fn prepare_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        Ok(ToolsetPreparation::initialized(
            self.name(),
            self.id().map(ToOwned::to_owned),
            self.get_tools(),
            self.get_instructions(),
        ))
    }

    /// Enter a context for resource-backed toolsets.
    async fn enter_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        Ok(ToolsetLifecycleReport::new(
            self.name(),
            self.id().map(ToOwned::to_owned),
            ToolsetLifecycleState::Initialized,
            self.get_tools().len(),
            self.get_instructions().len(),
        ))
    }

    /// Exit a context for resource-backed toolsets.
    async fn exit_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        Ok(ToolsetLifecycleReport::new(
            self.name(),
            self.id().map(ToOwned::to_owned),
            ToolsetLifecycleState::Closed,
            0,
            0,
        ))
    }
}

/// Static reusable toolset.
#[derive(Clone, Default)]
pub struct StaticToolset {
    name: String,
    id: Option<String>,
    tools: Vec<DynTool>,
    instructions: Vec<ToolInstruction>,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
}

impl StaticToolset {
    /// Create an empty static toolset.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: None,
            tools: Vec::new(),
            instructions: Vec::new(),
            max_retries: None,
            timeout_ms: None,
        }
    }

    /// Set a stable toolset identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add a tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add many tools.
    #[must_use]
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = DynTool>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Add an instruction.
    #[must_use]
    pub fn with_instruction(mut self, instruction: ToolInstruction) -> Self {
        self.instructions.push(instruction);
        self
    }

    /// Add many instructions.
    #[must_use]
    pub fn with_instructions(
        mut self,
        instructions: impl IntoIterator<Item = ToolInstruction>,
    ) -> Self {
        self.instructions.extend(instructions);
        self
    }

    /// Set a toolset-level retry default.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set a toolset-level execution timeout default.
    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }
}

impl Toolset for StaticToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.tools.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.instructions.clone()
    }
}
