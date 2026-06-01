//! Agent-loop graph and runtime executor primitives.

pub mod agent;
pub mod capability;
pub mod direct;
pub mod executor;
pub mod graph;
pub mod history;
pub mod instructions;
pub mod iteration;
pub mod output;
pub mod run;
pub mod stream;
pub mod trace;
pub mod usage;

pub use agent::{Agent, AgentError, AgentOverride, AgentResult, AgentRuntimePolicy};
pub use capability::{
    AgentCapability, CapabilityBundle, CapabilityError, CapabilityResult, RetryEventKind,
    StaticCapabilityBundle,
};
pub use direct::{model_request, model_request_stream, tool_call, DirectModelRequest};
pub use executor::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode, AgentExecutor, AgentExecutorError,
    AgentResumeCursor, AgentResumeEvidence, DirectAgentExecutor, DynAgentExecutor,
};
pub use graph::{
    inspect_graph, inspect_next_node, next_node, AgentGraphStep, AgentGraphTrace, AgentNode,
    GraphDecision, GraphError,
};
pub use history::{
    FunctionHistoryProcessor, HistoryProcessor, HistoryProcessorError, HistoryProcessorResult,
    ReinjectSystemPromptProcessor,
};
pub use instructions::{
    DynDynamicInstruction, DynamicInstruction, DynamicInstructionError, DynamicInstructionResult,
    FunctionDynamicInstruction,
};
pub use iteration::{AgentIterResult, AgentIterationKind, AgentIterationStep, AgentIterationTrace};
pub use output::{
    parse_output, DynOutputFunction, FunctionOutputFunction, FunctionOutputValidator,
    OutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputPolicy, OutputSchema,
    OutputValidationError, OutputValidationResult, OutputValidator, OutputValue,
};
pub use run::{AgentRunResult, AgentRunState, RunStatus};
pub use starweaver_model::{ModelResponseStreamEvent, PartDelta, PartEnd, PartStart};
pub use stream::{AgentStreamEvent, AgentStreamRecord, AgentStreamResult};
pub use trace::{
    AdapterTraceRecorder, DynTraceRecorder, InMemoryTraceRecorder, NoopTraceRecorder, RecordedSpan,
    SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus, TraceLevel, TraceRecorder,
};
pub use usage::{CostBudget, UsageLimitError, UsageLimits};
