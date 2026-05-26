//! Agent-loop graph and runtime executor primitives.

pub mod agent;
pub mod capability;
pub mod executor;
pub mod graph;
pub mod history;
pub mod instructions;
pub mod output;
pub mod run;
pub mod stream;
pub mod usage;

pub use agent::{Agent, AgentError, AgentOverride, AgentResult, AgentRuntimePolicy};
pub use capability::{
    AgentCapability, CapabilityBundle, CapabilityError, CapabilityResult, RetryEventKind,
    StaticCapabilityBundle,
};
pub use executor::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode, AgentExecutor, AgentExecutorError,
    DirectAgentExecutor, DynAgentExecutor,
};
pub use graph::{next_node, AgentNode, GraphDecision, GraphError};
pub use history::{
    FunctionHistoryProcessor, HistoryProcessor, HistoryProcessorError, HistoryProcessorResult,
    ReinjectSystemPromptProcessor,
};
pub use instructions::{
    DynDynamicInstruction, DynamicInstruction, DynamicInstructionError, DynamicInstructionResult,
    FunctionDynamicInstruction,
};
pub use output::{
    parse_output, DynOutputFunction, FunctionOutputFunction, FunctionOutputValidator,
    OutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputSchema,
    OutputValidationError, OutputValidationResult, OutputValidator, OutputValue,
};
pub use run::{AgentRunResult, AgentRunState, RunStatus};
pub use stream::{AgentStreamEvent, AgentStreamRecord, AgentStreamResult};
pub use usage::{CostBudget, UsageLimitError, UsageLimits};
