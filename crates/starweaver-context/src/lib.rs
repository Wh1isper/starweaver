//! Agent context, state, event bus, and message bus primitives for Starweaver.

mod agent_context;
mod config;
mod context_handle;
mod context_protocol;
mod dependency;
mod event;
mod message_bus;
mod notes;
mod resumable_state;
mod runtime_context;
mod state;
mod task;

pub use agent_context::AgentContext;
pub use config::{
    ModelCapability, ModelConfig, PerThousandRatio, SecurityConfig, ShellReviewAction,
    ShellReviewConfig, ShellReviewRiskLevel, ToolConfig,
};
pub use context_handle::AgentContextHandle;
pub use context_protocol::{
    AgentInfo, AgentStreamQueueRegistry, ContextLifecycleState, DeferredToolMetadata,
    ModelWrapperMetadata, ToolIdWrapper, ToolSearchState, WrapperMetadata,
};
pub use dependency::DependencyStore;
pub use event::{AgentEvent, EventBus};
pub use message_bus::{BusMessage, MessageBus};
pub use notes::NoteStore;
pub use resumable_state::{ResumableExportOptions, ResumableState};
pub use starweaver_core::AgentId;
pub use state::StateStore;
pub use task::{Task, TaskManager, TaskSnapshot, TaskStatus, TASK_SNAPSHOT_EVENT_KIND};
