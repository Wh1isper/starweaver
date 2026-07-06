//! Ergonomic SDK facade over the Starweaver bare runtime.

pub mod bundles;
pub mod filters;
pub mod mcp_live;
pub mod mcp_rmcp;
pub(crate) mod media_compression;
pub mod presets;
pub mod runtime;
pub mod session;
pub mod streaming;
pub mod subagent;
pub mod subagent_config;

use std::{collections::BTreeSet, sync::Arc};

use starweaver_model::{ModelAdapter, ModelProfile, ToolDefinition};
use starweaver_runtime::Agent as RuntimeAgent;

pub use bundles::{
    DEFAULT_SHELL_REVIEW_PROMPT, EnvironmentContextCapability, EnvironmentHandle,
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse, ProcessShellHandle,
    RuntimeContextCapability, SKILL_ACTIVATION_EVENT_KIND, SKILL_RELOAD_EVENT_KIND,
    SKILL_SCAN_EVENT_KIND, ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse,
    SearchResultItem, ShellReviewAction, ShellReviewConfig, ShellReviewContextSnapshot,
    ShellReviewDecision, ShellReviewHandle, ShellReviewPreviousDecision, ShellReviewRecord,
    ShellReviewRequest, ShellReviewRiskLevel, SkillDiscoveryCapability, SkillError, SkillPackage,
    SkillRegistry, SkillReloadBinding, SkillReloadChange, SkillReloadChangeKind,
    SkillReloadDecision, SkillReloadReason, SkillReloadReport, SkillReloadSchedule,
    SkillReloadScheduleState, SkillScanDiagnostic, SkillScanDiagnosticKind, SkillScanReport,
    SkillScheduledReloadResult, SkillSourceKind, SkillSourceScope, ToolProxyNamePrefixError,
    ToolProxyToolset, attach_environment, attach_process_shell, attach_shell_review,
    attach_shell_review_handle, context_tools, core_toolsets, dynamic_tool_proxy,
    environment_toolsets, filesystem_tools, host_io_tools, namespaced_toolset,
    parse_skill_markdown, process_shell_toolsets, shell_tools, skill_tools, task_tools,
};
pub use filters::{
    CacheFriendlyCompactCapability, DEFAULT_FILTER_ORDER, MediaUploadRequest, MediaUploader,
    NamedFilterCapability, default_filter_bundle, default_filter_capabilities,
    default_filter_capabilities_with_config,
};
pub use mcp_live::{
    DynLiveMcpClient, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot, LiveMcpToolset,
    live_mcp_toolset,
};
pub use mcp_rmcp::RmcpLiveMcpClient;
pub use presets::{
    AgentSpec, AgentSpecError, AgentSpecHostPolicies, AgentSpecRegistry,
    AgentSpecToolsetWrapperFactory, ApprovalPolicyPreset, DurabilityPolicyPreset,
    EnvironmentPolicyPreset, HostAdapterSpec, HostPolicySpec, McpServerSpec, ModelPreset,
    ObservabilityPolicyPreset, OutputSpec, RetryPolicyPreset, SdkPreset, SkillBundleSpec,
    StreamingPolicyPreset, TemplateStringSpec, ToolsetWrapperSpec, WorkspacePolicySpec,
    text_output_preset,
};
pub use runtime::{AgentDurabilityError, AgentRuntime, AgentRuntimeBuilder, agent_runtime};
pub use session::{
    AgentHitlError, AgentHitlResults, AgentHitlUserInteraction, AgentRunOptions, AgentSession,
    HITL_DECISION_DIAGNOSTIC_EVENT_KIND, ResolvedHitlToolReturns,
};
pub use starweaver_context::{
    AgentContext, AgentContextHandle, BusMessage, MessageBus, ModelCapability, ModelConfig,
    PerThousandRatio, ResumableState, SecurityConfig, ToolAvailabilityPolicy, ToolConfig,
};
pub use starweaver_core::{
    AgentId, CheckpointId, ConversationId, RunId, SessionId, SubagentLifecycleEvent,
    SubagentLifecycleKind, SubagentSpec, TaskId, TraceContext,
};
pub use starweaver_environment::{
    DynEnvironmentProviderFactory, DynProcessShellProvider, DynResourceRestoreFactory,
    ENVIRONMENT_PROVIDER_KIND_KEY, EnvironmentProviderFactory, EnvironmentProviderFactoryRegistry,
    ProcessShellProvider, RESOURCE_REF_KIND_KEY, ResourceRestoreFactory,
    ResourceRestoreFactoryRegistry, ShellProcessSnapshot, ShellProcessStatus,
    TrustedLocalEnvironmentProviderFactory, VirtualEnvironmentProviderFactory,
    environment_provider_kind, resource_ref_kind,
};
pub use starweaver_model::{
    ConcurrencyLimitedModel, ContentPart, DynModelAdapter, DynModelExecutionHook, FallbackModel,
    FunctionModel, FunctionModelInfo, HookedModel, ModelExecutionHook, ModelExecutionMetadata,
    ModelRequestParameters, ModelSettings, ProfileOverrideModel, ProtocolFamily, TestModel,
};
pub use starweaver_model::{
    ModelConfigPreset, ModelConfigPresetData, ModelPresetError, ModelRuntimePreset,
    ModelSettingsPreset, anthropic_http_config, gemini_http_config, get_model_config,
    get_model_settings, list_model_config_presets, list_model_settings_presets,
    model_runtime_preset, openai_chat_http_config, openai_responses_http_config,
};
pub use starweaver_runtime::{
    AdapterTraceRecorder, AgentCapability, AgentCheckpoint, AgentEndStrategy, AgentError,
    AgentExecutionDecision, AgentExecutionNode, AgentExecutor, AgentExecutorError, AgentGraphStep,
    AgentGraphTrace, AgentInput, AgentIterResult, AgentIterationKind, AgentIterationStep,
    AgentIterationTrace, AgentNode, AgentOverride, AgentResult, AgentResumeCursor,
    AgentResumeEvidence, AgentRunState, AgentRuntimePolicy, AgentSidebandEvent,
    AgentSidebandEventCategory, AgentStreamEvent, AgentStreamRecord, AgentStreamResult,
    AgentStreamSink, AgentStreamSource, AgentStreamSourceKind, AgentToolExecutionMode,
    CapabilityBundle, CapabilityError, CapabilityId, CapabilityOrderError, CapabilityOrdering,
    CapabilityResult, CapabilitySpec, DirectModelRequest, DynamicInstruction,
    DynamicInstructionError, DynamicInstructionResult, FunctionDynamicInstruction,
    FunctionOutputFunction, FunctionOutputValidator, GOAL_CAPABILITY_ID, GOAL_COMPLETE_EVENT_KIND,
    GOAL_COMPLETE_MARKER, GOAL_ITERATION_EVENT_KIND, GoalCapability, GoalCompleteReason,
    GoalRunOptions, GraphError, OutputFunction, OutputFunctionContext, OutputFunctionDefinition,
    OutputMedia, OutputPolicy, OutputSchema, OutputValidationError, OutputValidationResult,
    OutputValidator, OutputValue, RUNTIME_CONTEXT_CAPABILITY_ID, RecordedSpan, RetryEventKind,
    RunStatus, SchemaOutputFunction, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus,
    StaticCapabilityBundle, TraceLevel, TraceRecorder, TraceRecorderHandle, model_request,
    model_request_stream, resolve_capability_order, tool_call,
};
pub use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, DeferredToolRequest,
    DeferredToolRequests, DeferredToolResult, DeferredToolResults, ExecutionStatus,
    InMemorySessionStore, InputPart, RunRecord, RunStatus as SessionRunStatus, SessionFilter,
    SessionRecord, SessionResumeSnapshot, SessionStatus, SessionStore, SessionStoreError,
    SessionStoreExecutor, SessionStoreResult, StreamCursorRef, ToolApprovalDecision,
    ToolReturnRecordInput,
};
pub use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, InMemoryReplayEventLog, InMemoryStreamArchive, ReplayCursor,
    ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayScope, ReplaySnapshot, ReplaySubscription,
    StreamArchive, StreamTerminalMarker,
};
pub use starweaver_tools::{
    ApprovalRequiredToolset, DeferredToolset, DynTool, DynToolExecutionHook, DynToolset,
    DynamicToolset, EmptyToolArgs, FilteredToolset, FunctionTool, LazyToolset, McpPromptSpec,
    McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec, McpToolSpec, McpToolset,
    McpToolsetConfig, McpTransport, NativeMcpServer, PrefixedTool, PrefixedToolset,
    PreparedToolset, RenamedToolset, StaticToolset, TOOL_METADATA_CONTEXT_MANAGEMENT_KEY,
    TOOL_METADATA_HIDDEN_BY_TAGS_KEY, TOOL_METADATA_KIND_KEY, TOOL_METADATA_TAGS_KEY,
    TOOL_SEARCH_FAILED_EVENT_KIND, TOOL_SEARCH_INVALIDATED_EVENT_KIND,
    TOOL_SEARCH_NO_MATCH_EVENT_KIND, TOOL_SEARCH_REFRESHED_EVENT_KIND, TOOLSET_CLOSED_EVENT_KIND,
    TOOLSET_FAILED_EVENT_KIND, TOOLSET_INITIALIZED_EVENT_KIND, TOOLSET_REFRESHED_EVENT_KIND,
    TOOLSET_UNAVAILABLE_EVENT_KIND, Tool, ToolApprovalState, ToolContext, ToolError,
    ToolExecutionHook, ToolExecutionHooks, ToolExecutionOutcome, ToolInstruction, ToolKind,
    ToolRegistry, ToolResult, ToolSearchInitializationReport, ToolSearchInvalidationResult,
    ToolSearchLoadResult, ToolSearchNamespaceReport, ToolSearchNamespaceStatus,
    ToolSearchRefreshBinding, ToolSearchRefreshDecision, ToolSearchRefreshReason,
    ToolSearchRefreshResult, ToolSearchRefreshSchedule, ToolSearchRefreshScheduleState,
    ToolSearchScheduledRefreshResult, ToolSearchToolset, ToolUserInputPreprocessResult, Toolset,
    ToolsetLifecycleError, ToolsetLifecyclePolicy, ToolsetLifecycleReport, ToolsetLifecycleState,
    ToolsetPreparation, TypedFunctionTool, dynamic_tool_proxy as tool_proxy_toolset,
    dynamic_tool_search as tool_search_toolset, extend_tool_metadata_hidden_by_tags,
    extend_tool_metadata_tags, json_tool, json_tool as string_tool, set_tool_metadata_kind,
    tool_definition_from_mcp_spec, tool_metadata_hidden_by_tags, tool_metadata_kind,
    tool_metadata_tags, typed_json_tool, typed_json_tool as typed_tool,
};
pub use starweaver_usage::{
    PricingEstimate, Usage, UsageAgentTotal, UsageLimitError, UsageLimits, UsageSnapshot,
    UsageSnapshotEntry, UsageTokenKind, pricing::CostBudget,
};
pub use streaming::{
    AgentControlError, AgentControlHandle, AgentControlKind, AgentControlReceipt,
    AgentLiveStreamResult, AgentStreamCompletion, AgentStreamController, AgentStreamCurrentError,
    AgentStreamDropPolicy, AgentStreamError, AgentStreamHandle, AgentStreamOptions,
    AgentStreamRunStatus, AgentStreamStatus,
};
pub use subagent::{
    AgentApp, BackgroundSubagentCapability, BackgroundSubagentMonitor, BackgroundSubagentTaskInfo,
    BackgroundSubagentTaskResult, BackgroundSubagentTaskStatus, DELEGATE_BACKEND_TOOL_NAME,
    DynSubagentExecutionHook, SPAWN_DELEGATE_TOOL_NAME, SubagentCapabilityInheritancePolicy,
    SubagentConfig, SubagentDelegationMode, SubagentExecutionHook, SubagentExecutionMetadata,
    SubagentExecutionOutcome, SubagentParentTools, SubagentRegistry, SubagentResult, SubagentTask,
    SubagentToolInheritanceError, SubagentToolInheritancePolicy, WAIT_SUBAGENT_TOOL_NAME,
};
pub use subagent_config::{
    SubagentConfigError, SubagentSpecProjection, load_subagent_from_file, load_subagents_from_dir,
    parse_subagent_markdown, project_subagent_spec,
};

/// Error returned while rendering a static instruction template.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum InstructionTemplateError {
    /// Template variables could not be serialized to JSON.
    #[error("instruction template variables could not be serialized: {0}")]
    Serialize(String),
    /// Template syntax is invalid.
    #[error("invalid instruction template: {0}")]
    InvalidTemplate(String),
    /// Referenced variable was not present in the supplied data.
    #[error("missing instruction template variable: {0}")]
    MissingVariable(String),
    /// Referenced variable is not a scalar value that can be rendered safely.
    #[error("instruction template variable is not scalar: {0}")]
    NonScalarVariable(String),
}

/// Render a static instruction template from serializable variables.
///
/// Placeholders use `{{path.to.value}}` syntax and resolve against the serialized JSON object.
///
/// # Errors
///
/// Returns an error when variables cannot be serialized, the template is malformed, a path is
/// missing, or a resolved value is not a string, number, or boolean.
pub fn render_instruction_template<T: serde::Serialize>(
    template: &str,
    variables: &T,
) -> Result<String, InstructionTemplateError> {
    let variables = serde_json::to_value(variables)
        .map_err(|error| InstructionTemplateError::Serialize(error.to_string()))?;
    render_instruction_template_value(template, &variables)
}

fn render_instruction_template_value(
    template: &str,
    variables: &serde_json::Value,
) -> Result<String, InstructionTemplateError> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err(InstructionTemplateError::InvalidTemplate(
                "unclosed '{{' placeholder".to_string(),
            ));
        };
        let variable = after_start[..end].trim();
        validate_instruction_template_variable(variable)?;
        output.push_str(&render_instruction_template_variable(variables, variable)?);
        rest = &after_start[end + 2..];
    }
    if rest.contains("}}") {
        return Err(InstructionTemplateError::InvalidTemplate(
            "unopened '}}' placeholder".to_string(),
        ));
    }
    output.push_str(rest);
    Ok(output)
}

fn validate_instruction_template_variable(variable: &str) -> Result<(), InstructionTemplateError> {
    if variable.is_empty() {
        return Err(InstructionTemplateError::InvalidTemplate(
            "empty placeholder".to_string(),
        ));
    }
    if !variable
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-')
    {
        return Err(InstructionTemplateError::InvalidTemplate(format!(
            "invalid placeholder name '{variable}'"
        )));
    }
    Ok(())
}

fn render_instruction_template_variable(
    variables: &serde_json::Value,
    path: &str,
) -> Result<String, InstructionTemplateError> {
    let mut current = variables;
    for segment in path.split('.') {
        let Some(next) = current.as_object().and_then(|object| object.get(segment)) else {
            return Err(InstructionTemplateError::MissingVariable(path.to_string()));
        };
        current = next;
    }
    match current {
        serde_json::Value::String(value) => Ok(value.clone()),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        serde_json::Value::Bool(value) => Ok(value.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Err(InstructionTemplateError::NonScalarVariable(
                path.to_string(),
            ))
        }
    }
}

/// Builder for a reusable Starweaver agent.
pub struct AgentBuilder {
    agent_id: Option<AgentId>,
    agent_name: Option<String>,
    model: Arc<dyn ModelAdapter>,
    compact_model: Option<Arc<dyn ModelAdapter>>,
    compact_model_settings: Option<ModelSettings>,
    compact_request_params: Option<ModelRequestParameters>,
    instructions: Vec<String>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
    output_schema: Option<OutputSchema>,
    output_policy: Option<OutputPolicy>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    output_functions: Vec<Arc<dyn OutputFunction>>,
    dynamic_instructions: Vec<Arc<dyn DynamicInstruction>>,
    usage_limits: Option<UsageLimits>,
    model_config: Option<ModelConfig>,
    tool_config: Option<ToolConfig>,
    context_window: Option<u64>,
    tools: ToolRegistry,
    toolsets: Vec<DynToolset>,
    approval_required_tools: BTreeSet<String>,
    capabilities: Vec<Arc<dyn AgentCapability>>,
    capability_bundles: Vec<Arc<dyn CapabilityBundle>>,
    subagents: SubagentRegistry,
    subagent_delegation_mode: SubagentDelegationMode,
    executor: Option<starweaver_runtime::DynAgentExecutor>,
    trace_recorder: Option<starweaver_runtime::DynTraceRecorder>,
    media_uploader: Option<Arc<dyn MediaUploader>>,
    policy: AgentRuntimePolicy,
}

impl AgentBuilder {
    /// Create a builder from a model adapter.
    #[must_use]
    pub fn new(model: Arc<dyn ModelAdapter>) -> Self {
        Self {
            agent_id: None,
            agent_name: None,
            model,
            compact_model: None,
            compact_model_settings: None,
            compact_request_params: None,
            instructions: Vec::new(),
            model_settings: None,
            request_params: ModelRequestParameters::default(),
            output_schema: None,
            output_policy: None,
            output_validators: Vec::new(),
            output_functions: Vec::new(),
            dynamic_instructions: Vec::new(),
            usage_limits: None,
            model_config: None,
            tool_config: None,
            context_window: None,
            tools: ToolRegistry::new(),
            toolsets: Vec::new(),
            approval_required_tools: BTreeSet::new(),
            capabilities: Vec::new(),
            capability_bundles: Vec::new(),
            subagents: SubagentRegistry::new(),
            subagent_delegation_mode: SubagentDelegationMode::default(),
            executor: None,
            trace_recorder: None,
            media_uploader: None,
            policy: AgentRuntimePolicy::default(),
        }
    }

    /// Set the stable agent id used by default sessions and direct runs.
    #[must_use]
    pub fn agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(AgentId::from_string(agent_id));
        self
    }

    /// Set the human-readable agent name used by default sessions and direct runs.
    #[must_use]
    pub fn agent_name(mut self, agent_name: impl Into<String>) -> Self {
        self.agent_name = Some(agent_name.into());
        self
    }

    /// Set both the stable agent id and human-readable name.
    #[must_use]
    pub fn agent_identity(
        mut self,
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Self {
        self.agent_id = Some(AgentId::from_string(agent_id));
        self.agent_name = Some(agent_name.into());
        self
    }

    /// Set the model used by the default compacting filter.
    #[must_use]
    pub fn compact_model(mut self, model: Arc<dyn ModelAdapter>) -> Self {
        self.compact_model = Some(model);
        self
    }

    /// Set model settings used by the default compacting filter.
    #[must_use]
    pub fn compact_model_settings(mut self, settings: ModelSettings) -> Self {
        self.compact_model_settings = Some(settings);
        self
    }

    /// Set request parameters used by the default compacting filter.
    #[must_use]
    pub fn compact_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.compact_request_params = Some(params);
        self
    }

    /// Add a static instruction.
    #[must_use]
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }

    /// Render a static instruction template and add it to the agent.
    ///
    /// # Errors
    ///
    /// Returns an error when the template is malformed, variables cannot be serialized, a variable
    /// path is missing, or a resolved value is not scalar.
    pub fn try_instruction_template<T: serde::Serialize>(
        mut self,
        template: impl AsRef<str>,
        variables: &T,
    ) -> Result<Self, InstructionTemplateError> {
        self.instructions
            .push(render_instruction_template(template.as_ref(), variables)?);
        Ok(self)
    }

    /// Set model settings.
    #[must_use]
    pub fn model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Add a dynamic instruction.
    #[must_use]
    pub fn dynamic_instruction(mut self, instruction: Arc<dyn DynamicInstruction>) -> Self {
        self.dynamic_instructions.push(instruction);
        self
    }

    /// Set provider-neutral request parameters.
    #[must_use]
    pub fn request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = params;
        self
    }

    /// Set structured output schema.
    #[must_use]
    pub fn output_schema(mut self, schema: OutputSchema) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Apply a complete output policy.
    #[must_use]
    pub fn output_policy(mut self, policy: OutputPolicy) -> Self {
        self.output_policy = Some(policy);
        self
    }

    /// Add a structured output validator.
    #[must_use]
    pub fn output_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.output_validators.push(validator);
        self
    }

    /// Add an output function.
    #[must_use]
    pub fn output_function(mut self, function: Arc<dyn OutputFunction>) -> Self {
        self.output_functions.push(function);
        self
    }

    /// Set usage limits.
    #[must_use]
    pub const fn usage_limits(mut self, limits: UsageLimits) -> Self {
        self.usage_limits = Some(limits);
        self
    }

    /// Set the full model config stored on `AgentContext` at run time.
    #[must_use]
    pub fn model_config(mut self, model_config: ModelConfig) -> Self {
        self.model_config = Some(model_config);
        self
    }

    /// Set the tool config stored on `AgentContext` at run time.
    #[must_use]
    pub fn tool_config(mut self, tool_config: ToolConfig) -> Self {
        self.tool_config = Some(tool_config);
        self
    }

    /// Set the model context window exposed to runtime context instructions.
    #[must_use]
    pub const fn context_window(mut self, context_window: u64) -> Self {
        self.context_window = Some(context_window);
        self
    }

    /// Add one function tool.
    #[must_use]
    pub fn tool(mut self, tool: DynTool) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Add one toolset.
    #[must_use]
    pub fn toolset(mut self, toolset: &DynToolset) -> Self {
        self.toolsets.push(toolset.clone());
        self
    }

    /// Add many toolsets in registration order.
    #[must_use]
    pub fn toolsets(mut self, toolsets: impl IntoIterator<Item = DynToolset>) -> Self {
        for toolset in toolsets {
            self.toolsets.push(toolset);
        }
        self
    }

    /// Require HITL approval for matching tools in registered toolsets.
    ///
    /// Entries can match a tool name, toolset name/id, metadata `bundle`, or `*`.
    #[must_use]
    pub fn approval_required_tools(
        mut self,
        tools: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.approval_required_tools
            .extend(tools.into_iter().map(Into::into));
        self
    }

    /// Merge additional runtime tools into this builder.
    #[must_use]
    pub fn append_tool_registry(mut self, tools: &ToolRegistry) -> Self {
        self.tools.insert_registry(tools);
        self
    }

    /// Set the whole tool registry.
    #[must_use]
    pub fn tool_registry(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self.toolsets.clear();
        self
    }

    /// Add a global execution hook around runtime tool calls.
    #[must_use]
    pub fn global_tool_execution_hook(mut self, hook: DynToolExecutionHook) -> Self {
        self.tools.insert_global_execution_hook(hook);
        self
    }

    /// Add a tool-specific execution hook around runtime tool calls.
    #[must_use]
    pub fn tool_execution_hook(
        mut self,
        tool: impl Into<String>,
        hook: DynToolExecutionHook,
    ) -> Self {
        self.tools.insert_tool_execution_hook(tool, hook);
        self
    }

    /// Set the agent-level retry default for runtime tools.
    #[must_use]
    pub const fn tool_retries(mut self, max_retries: usize) -> Self {
        self.tools.set_max_retries(max_retries);
        self
    }

    /// Install scanned skill summaries and relaxed skill markdown discovery.
    #[must_use]
    pub fn skills(mut self, registry: SkillRegistry) -> Self {
        let toolset = registry.toolset();
        self.toolsets.push(toolset);
        self.capabilities
            .push(Arc::new(SkillDiscoveryCapability::new(registry)));
        self
    }

    /// Install scanned skills from a lenient report and publish scan diagnostics at run start.
    #[must_use]
    pub fn skills_report(mut self, report: SkillScanReport) -> Self {
        let toolset = report.registry.toolset();
        self.toolsets.push(toolset);
        self.capabilities
            .push(Arc::new(SkillDiscoveryCapability::from_report(report)));
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn capability(mut self, capability: Arc<dyn AgentCapability>) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Add a capability bundle.
    #[must_use]
    pub fn capability_bundle(mut self, bundle: Arc<dyn CapabilityBundle>) -> Self {
        self.capability_bundles.push(bundle);
        self
    }

    /// Add a subagent configuration to the SDK-level registry.
    #[must_use]
    pub fn subagent(mut self, subagent: SubagentConfig) -> Self {
        self.subagents.insert(subagent);
        self
    }

    /// Set the SDK-level subagent registry.
    #[must_use]
    pub fn subagent_registry(mut self, registry: SubagentRegistry) -> Self {
        self.subagents = registry;
        self
    }

    /// Set how registered subagents are exposed as model-callable tools.
    #[must_use]
    pub const fn subagent_delegation_mode(mut self, mode: SubagentDelegationMode) -> Self {
        self.subagent_delegation_mode = mode;
        self
    }

    /// Set runtime trace recorder.
    #[must_use]
    pub fn trace_recorder(mut self, recorder: starweaver_runtime::DynTraceRecorder) -> Self {
        self.trace_recorder = Some(recorder);
        self
    }

    /// Set durable execution checkpoint handler.
    #[must_use]
    pub fn executor(mut self, executor: starweaver_runtime::DynAgentExecutor) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Set the media uploader used by the default `media_upload` filter.
    #[must_use]
    pub fn media_uploader(mut self, uploader: Arc<dyn MediaUploader>) -> Self {
        self.media_uploader = Some(uploader);
        self
    }

    /// Set runtime policy.
    #[must_use]
    pub const fn policy(mut self, policy: AgentRuntimePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Return the SDK-level subagent registry configured on this builder.
    #[must_use]
    pub const fn subagents(&self) -> &SubagentRegistry {
        &self.subagents
    }

    /// Build an SDK application wrapper with runtime agent and application-level protocols.
    #[must_use]
    pub fn build_app(self) -> AgentApp {
        let subagents = self.resolved_subagents();
        AgentApp::new(self.build()).with_subagents(subagents)
    }

    fn resolved_subagents(&self) -> SubagentRegistry {
        self.subagents
            .clone()
            .with_resolved_capability_inheritance(&self.capabilities, &self.capability_bundles)
    }

    /// Build a reusable runtime agent.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn build(self) -> RuntimeAgent {
        let subagents = self.resolved_subagents();
        let subagent_delegation_mode = self.subagent_delegation_mode;
        let subagents = (!subagents.is_empty()).then(|| Arc::new(subagents));
        let background_subagents = subagent_delegation_mode
            .needs_background_monitor()
            .then(|| Arc::new(BackgroundSubagentMonitor::new()));
        let model_profile_capabilities = model_capabilities_from_profile(self.model.profile());
        let mut configured_model_config = self.model_config;
        if !model_profile_capabilities.is_empty() {
            match &mut configured_model_config {
                Some(model_config) if model_config.capabilities.is_empty() => {
                    model_config.capabilities = model_profile_capabilities;
                }
                None => {
                    configured_model_config = Some(ModelConfig {
                        capabilities: model_profile_capabilities,
                        ..ModelConfig::default()
                    });
                }
                Some(_) => {}
            }
        }
        let media_capabilities = HostMediaCapabilities::from_model_profile(
            Some(self.model.model_name().to_string()),
            self.model.profile(),
        );
        let media_capability_hook = Arc::new(HostMediaCapabilityHook { media_capabilities });
        let mut tools = self.tools;
        let trace_recorder = self.trace_recorder.clone();
        if let Some(subagents) = &subagents {
            if subagent_delegation_mode.exposes_blocking_delegate() {
                tools.insert(subagents.delegate_tool());
            }
            if subagent_delegation_mode.exposes_async_delegate() {
                tools.insert(subagents.hidden_delegate_backend_tool());
                if let Some(monitor) = &background_subagents {
                    tools.insert(subagents.async_delegate_tool(monitor.clone()));
                    tools.insert(subagents.wait_subagent_tool(monitor.clone()));
                }
            }
            if subagent_delegation_mode.exposes_spawn_delegate()
                && let Some(monitor) = &background_subagents
            {
                tools.insert(subagents.spawn_delegate_tool(monitor.clone()));
                tools.insert(subagents.wait_subagent_tool(monitor.clone()));
            }
            tools.insert(subagents.subagent_info_tool());
        }
        let toolsets = approval_wrapped_toolsets(self.toolsets, &self.approval_required_tools);
        let mut tool_preview = tools.clone();
        for toolset in &toolsets {
            tool_preview.insert_toolset(toolset);
        }
        let parent_tools = tool_preview.clone();
        if let Some(subagents) = &subagents {
            match subagent_delegation_mode {
                SubagentDelegationMode::Blocking => {
                    if let Some(instruction) = subagents.delegate_instruction(Some(&parent_tools)) {
                        tools.insert_instruction(instruction);
                    }
                }
                SubagentDelegationMode::Async => {
                    if let Some(instruction) =
                        subagents.async_delegate_instruction(Some(&parent_tools))
                    {
                        tools.insert_instruction(instruction);
                    }
                }
                SubagentDelegationMode::BlockingAndAsync => {
                    if let Some(instruction) = subagents.delegate_instruction(Some(&parent_tools)) {
                        tools.insert_instruction(instruction);
                    }
                    tools.insert_instruction(subagents.spawn_delegate_instruction());
                }
            }
        }
        let model = self.model.clone();
        let compact_model = self.compact_model.unwrap_or_else(|| model.clone());
        let compact_model_settings = self
            .compact_model_settings
            .or_else(|| self.model_settings.clone());
        let compact_request_params_explicit = self.compact_request_params.is_some();
        let mut compact_request_params = self
            .compact_request_params
            .unwrap_or_else(|| self.request_params.clone());
        if !compact_request_params_explicit {
            compact_request_params.tools =
                tool_preview.definitions_for_context(&AgentContext::default());
        }
        let parent_tools_hook = Arc::new(ParentToolsCapabilityHook { parent_tools });
        let environment_context_hook = Arc::new(EnvironmentContextCapability);
        let runtime_context_hook = Arc::new(RuntimeContextCapability);
        let mut agent = RuntimeAgent::new(self.model)
            .with_request_params(self.request_params)
            .with_tools(tools)
            .with_toolsets(toolsets)
            .with_policy(self.policy);
        if let Some(agent_id) = self.agent_id {
            agent = agent.with_agent_id(agent_id);
        }
        if let Some(agent_name) = self.agent_name {
            agent = agent.with_agent_name(agent_name);
        }
        for instruction in self.instructions {
            agent = agent.with_instruction(instruction);
        }
        for instruction in self.dynamic_instructions {
            agent = agent.with_dynamic_instruction(instruction);
        }
        if let Some(settings) = self.model_settings {
            agent = agent.with_model_settings(settings);
        }
        if let Some(schema) = self.output_schema {
            agent = agent.with_output_schema(schema);
        }
        if let Some(policy) = self.output_policy {
            agent = agent.with_output_policy(policy);
        }
        if let Some(limits) = self.usage_limits {
            agent = agent.with_usage_limits(limits);
        }
        if let Some(model_config) = configured_model_config {
            agent = agent.with_model_config(model_config);
        }
        if let Some(tool_config) = self.tool_config {
            agent = agent.with_tool_config(tool_config);
        }
        if let Some(context_window) = self.context_window {
            agent = agent.with_context_window(context_window);
        }
        if let Some(monitor) = background_subagents {
            agent = agent.with_capability(Arc::new(BackgroundSubagentCapability::new(monitor)));
        }
        for capability in crate::filters::default_filter_capabilities_with_media_uploader(
            Some(&compact_model),
            compact_model_settings.as_ref(),
            Some(&compact_request_params),
            trace_recorder.as_ref(),
            self.media_uploader.as_ref(),
        ) {
            agent = agent.with_capability(capability);
        }
        for function in self.output_functions {
            agent = agent.with_output_function(function);
        }
        for validator in self.output_validators {
            agent = agent.with_output_validator(validator);
        }
        agent = agent.with_capability(media_capability_hook);
        agent = agent.with_capability(parent_tools_hook);
        agent = agent.with_capability(environment_context_hook);
        agent = agent.with_capability(runtime_context_hook);
        for capability in self.capabilities {
            agent = agent.with_capability(capability);
        }
        for bundle in self.capability_bundles {
            agent = agent.with_capability_bundle(bundle.as_ref());
        }
        if let Some(executor) = self.executor {
            agent = agent.with_executor(executor);
        }
        if let Some(recorder) = self.trace_recorder {
            agent = agent.with_trace_recorder(recorder);
        }
        agent
    }
}

fn approval_wrapped_toolsets(
    toolsets: Vec<DynToolset>,
    approval_required_tools: &BTreeSet<String>,
) -> Vec<DynToolset> {
    if approval_required_tools.is_empty() {
        return toolsets;
    }
    let approval = approval_required_tools.iter().cloned().collect::<Vec<_>>();
    toolsets
        .into_iter()
        .map(|toolset| {
            Arc::new(ApprovalRequiredToolset::new(toolset, approval.clone())) as DynToolset
        })
        .collect()
}

/// Create an agent builder from a model.
#[must_use]
pub fn agent(model: Arc<dyn ModelAdapter>) -> AgentBuilder {
    AgentBuilder::new(model)
}

fn model_capabilities_from_profile(profile: &ModelProfile) -> BTreeSet<ModelCapability> {
    let mut capabilities = BTreeSet::new();
    if profile.supports_image_input {
        capabilities.insert(ModelCapability::Vision);
    }
    if profile.supports_video_input {
        capabilities.insert(ModelCapability::VideoUnderstanding);
    }
    if profile.supports_audio_input {
        capabilities.insert(ModelCapability::AudioUnderstanding);
    }
    if profile.supports_document_input {
        capabilities.insert(ModelCapability::DocumentUnderstanding);
    }
    capabilities
}

#[derive(Clone)]
struct HostMediaCapabilityHook {
    media_capabilities: HostMediaCapabilities,
}

#[derive(Clone)]
struct ParentToolsCapabilityHook {
    parent_tools: ToolRegistry,
}

#[async_trait::async_trait]
impl AgentCapability for ParentToolsCapabilityHook {
    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        tool_context: &mut ToolContext,
        _call: &starweaver_model::ToolCallPart,
    ) -> CapabilityResult<()> {
        tool_context
            .dependencies
            .insert(SubagentParentTools(self.parent_tools.clone()));
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentCapability for HostMediaCapabilityHook {
    async fn prepare_tools_with_context(
        &self,
        state: &AgentRunState,
        _context: &AgentContext,
        tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        self.prepare_tools(state, tools).await
    }

    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        Ok(tools
            .into_iter()
            .filter(|tool| self.is_media_tool_available(tool.name.as_str()))
            .collect())
    }

    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        tool_context: &mut ToolContext,
        _call: &starweaver_model::ToolCallPart,
    ) -> CapabilityResult<()> {
        tool_context
            .dependencies
            .insert(self.media_capabilities.clone());
        Ok(())
    }
}

impl HostMediaCapabilityHook {
    fn is_media_tool_available(&self, tool_name: &str) -> bool {
        match tool_name {
            "read_image" => !self.media_capabilities.supports_image_url,
            "read_video" => !self.media_capabilities.supports_video_url,
            "read_audio" => !self.media_capabilities.supports_audio_url,
            "load_media_url" => {
                self.media_capabilities.supports_image_url
                    || self.media_capabilities.supports_video_url
                    || self.media_capabilities.supports_audio_url
                    || self.media_capabilities.supports_document_url
            }
            _ => true,
        }
    }
}
