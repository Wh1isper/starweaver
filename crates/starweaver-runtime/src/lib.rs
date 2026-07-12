//! Agent-loop graph and runtime executor primitives.

pub mod agent;
pub mod capability;
pub mod dependency_assembly;
pub mod direct;
pub mod executor;
pub mod goal;
pub mod graph;
pub mod instructions;
pub mod iteration;
pub mod output;
pub mod retry_recovery;
pub mod run;
pub mod stream;
pub mod trace;

pub use agent::{
    Agent, AgentEndStrategy, AgentError, AgentInput, AgentOverride, AgentResult,
    AgentRuntimePolicy, AgentToolExecutionMode,
};
pub use capability::{
    AgentCapability, CapabilityBundle, CapabilityError, CapabilityId, CapabilityOrderError,
    CapabilityOrdering, CapabilityResult, CapabilitySpec, RUNTIME_CONTEXT_CAPABILITY_ID,
    RetryEventKind, StaticCapabilityBundle, resolve_capability_order,
};
pub use dependency_assembly::{ToolDependencyAssembly, assemble_tool_dependencies_for_name};
pub use direct::{DirectModelRequest, model_request, model_request_stream, tool_call};
pub use executor::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode, AgentExecutor, AgentExecutorError,
    AgentResumeCursor, AgentResumeEvidence, DirectAgentExecutor, DynAgentExecutor,
};
pub use goal::{
    GOAL_CAPABILITY_ID, GOAL_COMPLETE_EVENT_KIND, GOAL_COMPLETE_MARKER, GOAL_ITERATION_EVENT_KIND,
    GoalCapability, GoalCompleteReason, GoalRunOptions, build_goal_check_prompt,
    build_post_restore_goal_audit_prompt, has_completion_marker,
};
pub use graph::{
    AgentGraphStep, AgentGraphTrace, AgentNode, GraphDecision, GraphError, inspect_graph,
    inspect_next_node, next_node,
};
pub use instructions::{
    DynDynamicInstruction, DynamicInstruction, DynamicInstructionError, DynamicInstructionResult,
    FunctionDynamicInstruction,
};
pub use iteration::{AgentIterResult, AgentIterationKind, AgentIterationStep, AgentIterationTrace};
pub use output::{
    DynOutputFunction, FunctionOutputFunction, FunctionOutputValidator, OutputFunction,
    OutputFunctionContext, OutputFunctionDefinition, OutputMedia, OutputPolicy, OutputSchema,
    OutputValidationError, OutputValidationResult, OutputValidator, OutputValue,
    SchemaOutputFunction, parse_output,
};
pub use retry_recovery::{
    DEFAULT_MODEL_ERROR_RESUME_PROMPT, RetryRecoveryResult, heal_context_overflow_history,
    heal_openai_item_reference_history, recover_retry_message_history,
};
pub use run::{AgentRunResult, AgentRunState, RunStatus};
pub use starweaver_model::{ModelResponseStreamEvent, PartDelta, PartEnd, PartStart};
pub use stream::{
    AgentSidebandEvent, AgentSidebandEventCategory, AgentStreamEvent, AgentStreamRecord,
    AgentStreamResult, AgentStreamSink, AgentStreamSource, AgentStreamSourceKind,
};
pub use trace::{
    AdapterTraceRecorder, DynTraceRecorder, InMemoryTraceRecorder, NoopTraceRecorder,
    OtelGenAiSpan, PolicyTraceRecorder, RecordedSpan, SpanEvent, SpanHandle, SpanKind, SpanSpec,
    SpanStatus, TraceDebugPolicy, TraceLevel, TraceRecorder, TraceRecorderHandle,
    TraceRedactionPolicy, export_otel_gen_ai_spans,
};
