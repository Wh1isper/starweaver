//! SDK-level subagent protocol.

mod app;
mod config;
mod inheritance;
mod registry;
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
pub use registry::{SubagentParentTools, SubagentRegistry};
pub use task::{SubagentResult, SubagentTask};
