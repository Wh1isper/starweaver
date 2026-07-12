//! Agent context, checkpoint state, executor callbacks, and runtime evidence contracts.

mod agent_context;
mod agent_tool_state;
mod checkpoint;
mod config;
mod context_handle;
mod context_protocol;
mod dependency;
mod event;
mod host_capabilities;
mod message_bus;
mod notes;
mod resumable_state;
mod run_state;
mod runtime_context;
mod runtime_state;
mod state;
mod task;
mod tool_runtime;

pub use agent_context::AgentContext;
pub use agent_tool_state::AgentToolState;
pub use checkpoint::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutor, AgentExecutorError, AgentResumeCursor,
    AgentResumeEvidence,
};
pub use config::{
    ModelCapability, ModelConfig, PerThousandRatio, SecurityConfig, ShellReviewAction,
    ShellReviewConfig, ShellReviewRiskLevel, ToolAvailabilityPolicy, ToolConfig,
};
pub use context_handle::{
    AgentContextHandle, CONTEXT_HANDOFF_CAPABILITY, CONTEXT_TASKS_CAPABILITY,
    CONTEXT_TOOL_SEARCH_CAPABILITY, CONTEXT_USAGE_CAPABILITY, ContextHandoffHandle,
    ContextMutationHandles, TaskContextHandle, ToolSearchContextHandle, UsageContextHandle,
};
pub use context_protocol::{
    AgentInfo, AgentStreamQueueRegistry, ContextLifecycleState, DeferredToolMetadata,
    ModelWrapperMetadata, ToolIdWrapper, ToolSearchInvalidation, ToolSearchState, WrapperMetadata,
};
pub use dependency::DependencyStore;
pub use event::{AgentEvent, EventBus};
pub use host_capabilities::{HostCapabilities, ToolCapabilityGrant};
pub use message_bus::{BusMessage, MessageBus};
pub use notes::NoteStore;
pub use resumable_state::{ResumableExportOptions, ResumableState};
pub use run_state::AgentRunState;
pub use runtime_state::RuntimeEphemeralState;
pub use starweaver_core::{AgentId, TASK_SNAPSHOT_EVENT_KIND};
pub use state::StateStore;
pub use task::{Task, TaskManager, TaskSnapshot, TaskStatus};
pub use tool_runtime::{ShellEnvironmentSnapshot, ToolRuntimeSnapshot};
