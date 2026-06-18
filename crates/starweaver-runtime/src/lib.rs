//! Agent-loop graph and runtime executor primitives.

pub mod agent;
pub mod capability;
pub mod direct;
pub mod executor;
pub mod graph;
pub mod instructions;
pub mod iteration;
pub mod output;
pub mod retry_recovery;
pub mod run;
pub mod stream;
pub mod trace;

pub use agent::{
    Agent, AgentEndStrategy, AgentError, AgentInput, AgentOverride, AgentResult, AgentRuntimePolicy,
};
pub use capability::{
    resolve_capability_order, AgentCapability, CapabilityBundle, CapabilityError, CapabilityId,
    CapabilityOrderError, CapabilityOrdering, CapabilityResult, CapabilitySpec, RetryEventKind,
    StaticCapabilityBundle, RUNTIME_CONTEXT_CAPABILITY_ID,
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
pub use instructions::{
    DynDynamicInstruction, DynamicInstruction, DynamicInstructionError, DynamicInstructionResult,
    FunctionDynamicInstruction,
};
pub use iteration::{AgentIterResult, AgentIterationKind, AgentIterationStep, AgentIterationTrace};
pub use output::{
    parse_output, DynOutputFunction, FunctionOutputFunction, FunctionOutputValidator,
    OutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputMedia, OutputPolicy,
    OutputSchema, OutputValidationError, OutputValidationResult, OutputValidator, OutputValue,
    SchemaOutputFunction,
};
pub use retry_recovery::{
    heal_context_overflow_history, heal_openai_item_reference_history,
    recover_retry_message_history, RetryRecoveryResult, DEFAULT_MODEL_ERROR_RESUME_PROMPT,
};
pub use run::{AgentRunResult, AgentRunState, RunStatus};
pub use starweaver_model::{ModelResponseStreamEvent, PartDelta, PartEnd, PartStart};
pub use stream::{
    AgentSidebandEvent, AgentSidebandEventCategory, AgentStreamEvent, AgentStreamRecord,
    AgentStreamResult, AgentStreamSink, AgentStreamSource, AgentStreamSourceKind,
};
pub use trace::{
    export_otel_gen_ai_spans, AdapterTraceRecorder, DynTraceRecorder, InMemoryTraceRecorder,
    NoopTraceRecorder, OtelGenAiSpan, PolicyTraceRecorder, RecordedSpan, SpanEvent, SpanHandle,
    SpanKind, SpanSpec, SpanStatus, TraceDebugPolicy, TraceLevel, TraceRecorder,
    TraceRedactionPolicy,
};
