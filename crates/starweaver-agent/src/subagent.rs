//! SDK-level subagent protocol.

mod app;
mod config;
mod inheritance;
mod registry;
mod supervisor;
mod task;

pub use app::AgentApp;
pub use config::{
    DynSubagentExecutionHook, SubagentConfig, SubagentExecutionHook, SubagentExecutionMetadata,
    SubagentExecutionOutcome,
};
pub use inheritance::{
    SubagentCapabilityInheritancePolicy, SubagentToolInheritanceError,
    SubagentToolInheritancePolicy,
};
pub use registry::{
    BackgroundSubagentCapability, CANCEL_SUBAGENT_TOOL_NAME, DELEGATE_BACKEND_TOOL_NAME,
    SPAWN_DELEGATE_TOOL_NAME, STEER_SUBAGENT_TOOL_NAME, SubagentDelegationMode,
    SubagentParentTools, SubagentRegistry, WAIT_SUBAGENT_TOOL_NAME,
};
pub use supervisor::{
    BackgroundSubagentCancellationReceipt, BackgroundSubagentCompletionCallback,
    BackgroundSubagentDeliveryClaim, BackgroundSubagentDeliveryStatus, BackgroundSubagentError,
    BackgroundSubagentExecutionStatus, BackgroundSubagentLimits, BackgroundSubagentMonitor,
    BackgroundSubagentRetentionStatus, BackgroundSubagentSteeringReceipt,
    BackgroundSubagentSupervisor, BackgroundSubagentTaskInfo, BackgroundSubagentTaskResult,
    BackgroundSubagentTaskStatus,
};
pub use task::{SubagentResult, SubagentTask};
