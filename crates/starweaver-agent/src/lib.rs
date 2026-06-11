//! Ergonomic SDK facade over the Starweaver bare runtime.

pub mod bundles;
pub mod filters;
pub mod mcp_live;
pub(crate) mod media_compression;
pub mod presets;
pub mod session;
pub mod subagent;
pub mod subagent_config;

use std::sync::Arc;

use starweaver_model::{ModelAdapter, ToolDefinition};
use starweaver_runtime::Agent as RuntimeAgent;

pub use bundles::{
    attach_environment, attach_process_shell, attach_shell_review, attach_shell_review_handle,
    core_toolsets, environment_toolsets, filesystem_tools, host_operation_tools,
    namespaced_toolset, parse_skill_markdown, process_shell_toolsets, shell_tools, skill_tools,
    task_tools, tool_proxy_toolset, EnvironmentContextCapability, EnvironmentHandle,
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse, ProcessShellHandle, ScrapeRequest,
    ScrapeResponse, SearchRequest, SearchResponse, SearchResultItem, ShellReviewAction,
    ShellReviewConfig, ShellReviewContextSnapshot, ShellReviewDecision, ShellReviewHandle,
    ShellReviewPreviousDecision, ShellReviewRecord, ShellReviewRequest, ShellReviewRiskLevel,
    SkillError, SkillPackage, SkillRegistry, SkillSourceScope, ToolProxyPrefixError,
    ToolProxyToolset, DEFAULT_SHELL_REVIEW_PROMPT,
};
pub use filters::{
    default_filter_bundle, default_filter_capabilities, default_filter_capabilities_with_config,
    CacheFriendlyCompactCapability, MediaUploadRequest, MediaUploader, NamedFilterCapability,
    DEFAULT_FILTER_ORDER,
};
pub use mcp_live::{
    live_mcp_toolset, DynLiveMcpClient, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot,
};
pub use presets::{
    text_output_preset, AgentSpec, AgentSpecError, AgentSpecHostPolicies, AgentSpecRegistry,
    ApprovalPolicyPreset, DurabilityPolicyPreset, EnvironmentPolicyPreset, HostAdapterSpec,
    HostPolicySpec, McpServerSpec, ModelPreset, ObservabilityPolicyPreset, OutputSpec,
    RetryPolicyPreset, SdkPreset, SkillBundleSpec, StreamingPolicyPreset, TemplateStringSpec,
    ToolsetWrapperSpec, WorkspacePolicySpec,
};
pub use session::{AgentRunOptions, AgentSession};
pub use starweaver_context::{
    AgentContext, AgentContextHandle, ModelCapability, ModelConfig, Ratio, ResumableState,
    SecurityConfig, ToolConfig,
};
pub use starweaver_core::{
    AgentId, CheckpointId, ConversationId, RunId, SubagentLifecycleEvent, SubagentLifecycleKind,
    SubagentSpec, TaskId, TraceContext, Usage,
};
pub use starweaver_environment::{
    DynProcessShellProvider, ProcessShellProvider, ShellProcessSnapshot, ShellProcessStatus,
};
pub use starweaver_model::{
    anthropic_http_config, gemini_http_config, get_model_config, get_model_settings,
    list_model_config_presets, list_model_settings_presets, model_runtime_preset,
    openai_chat_http_config, openai_responses_http_config, ModelConfigPreset,
    ModelConfigPresetData, ModelPresetError, ModelRuntimePreset, ModelSettingsPreset,
};
pub use starweaver_model::{
    ConcurrencyLimitedModel, DynModelAdapter, FallbackModel, FunctionModel, FunctionModelInfo,
    ModelRequestParameters, ModelSettings, ProfileOverrideModel, ProtocolFamily, TestModel,
};
pub use starweaver_runtime::{
    model_request, model_request_stream, resolve_capability_order, tool_call, AdapterTraceRecorder,
    AgentCapability, AgentCheckpoint, AgentError, AgentExecutionDecision, AgentExecutionNode,
    AgentExecutor, AgentExecutorError, AgentGraphStep, AgentGraphTrace, AgentIterResult,
    AgentIterationKind, AgentIterationStep, AgentIterationTrace, AgentNode, AgentOverride,
    AgentResult, AgentResumeCursor, AgentResumeEvidence, AgentRunState, AgentRuntimePolicy,
    AgentStreamEvent, AgentStreamRecord, AgentStreamResult, CapabilityBundle, CapabilityId,
    CapabilityOrderError, CapabilityOrdering, CapabilityResult, CapabilitySpec, CostBudget,
    DirectModelRequest, DynamicInstruction, DynamicInstructionError, DynamicInstructionResult,
    FunctionDynamicInstruction, FunctionOutputFunction, FunctionOutputValidator, GraphError,
    OutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputPolicy, OutputSchema,
    OutputValidationError, OutputValidationResult, OutputValidator, OutputValue, RecordedSpan,
    RetryEventKind, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus, StaticCapabilityBundle,
    TraceLevel, TraceRecorder, UsageLimitError, UsageLimits,
};
pub use starweaver_tools::{
    mcp_tool_definition, string_tool, typed_tool, ApprovalRequiredToolset, DeferredLoadingToolset,
    DynTool, DynToolset, DynamicToolset, EmptyToolArgs, FilteredToolset, FunctionTool, McpToolSpec,
    McpToolset, McpToolsetConfig, McpTransport, NativeMcpServer, PrefixedTool, PrefixedToolset,
    PreparedToolset, RenamedToolset, StaticToolset, Tool, ToolApprovalState, ToolContext,
    ToolError, ToolInstruction, ToolRegistry, ToolResult, Toolset, TypedFunctionTool,
};
pub use subagent::{
    AgentApp, SubagentConfig, SubagentParentTools, SubagentRegistry, SubagentResult, SubagentTask,
    SubagentToolInheritanceError, SubagentToolInheritancePolicy,
};
pub use subagent_config::{
    load_subagent_from_file, load_subagents_from_dir, parse_subagent_markdown, SubagentConfigError,
};

/// Builder for a reusable Starweaver agent.
pub struct AgentBuilder {
    model: Arc<dyn ModelAdapter>,
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
    capabilities: Vec<Arc<dyn AgentCapability>>,
    capability_bundles: Vec<Arc<dyn CapabilityBundle>>,
    subagents: SubagentRegistry,
    trace_recorder: Option<starweaver_runtime::DynTraceRecorder>,
    policy: AgentRuntimePolicy,
}

impl AgentBuilder {
    /// Create a builder from a model adapter.
    #[must_use]
    pub fn new(model: Arc<dyn ModelAdapter>) -> Self {
        Self {
            model,
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
            capabilities: Vec::new(),
            capability_bundles: Vec::new(),
            subagents: SubagentRegistry::new(),
            trace_recorder: None,
            policy: AgentRuntimePolicy::default(),
        }
    }

    /// Add a static instruction.
    #[must_use]
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
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
        self.tools.insert_toolset(toolset);
        self
    }

    /// Add many toolsets in registration order.
    #[must_use]
    pub fn toolsets(mut self, toolsets: impl IntoIterator<Item = DynToolset>) -> Self {
        for toolset in toolsets {
            self.tools.insert_toolset(&toolset);
        }
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
        self
    }

    /// Set the agent-level retry default for runtime tools.
    #[must_use]
    pub fn tool_retries(mut self, max_retries: usize) -> Self {
        self.tools.set_max_retries(max_retries);
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

    /// Set runtime trace recorder.
    #[must_use]
    pub fn trace_recorder(mut self, recorder: starweaver_runtime::DynTraceRecorder) -> Self {
        self.trace_recorder = Some(recorder);
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
        let subagents = self.subagents.clone();
        AgentApp::new(self.build()).with_subagents(subagents)
    }

    /// Build a reusable runtime agent.
    #[must_use]
    pub fn build(self) -> RuntimeAgent {
        let media_capabilities = HostMediaCapabilities::from_model_profile(
            Some(self.model.model_name().to_string()),
            self.model.profile(),
        );
        let media_capability_hook = Arc::new(HostMediaCapabilityHook { media_capabilities });
        let mut tools = self.tools;
        if !self.subagents.is_empty() {
            let subagents = Arc::new(self.subagents.clone());
            tools.insert(subagents.delegate_tool());
            tools.insert(subagents.subagent_info_tool());
        }
        let parent_tools = tools.clone();
        let mut compact_request_params = self.request_params.clone();
        compact_request_params.tools = tools.definitions();
        let compact_model_settings = self.model_settings.clone();
        let parent_tools_hook = Arc::new(ParentToolsCapabilityHook { parent_tools });
        let environment_context_hook = Arc::new(EnvironmentContextCapability);
        let model = self.model.clone();
        let mut agent = RuntimeAgent::new(self.model)
            .with_request_params(self.request_params)
            .with_tools(tools)
            .with_policy(self.policy);
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
        if let Some(model_config) = self.model_config {
            agent = agent.with_model_config(model_config);
        }
        if let Some(tool_config) = self.tool_config {
            agent = agent.with_tool_config(tool_config);
        }
        if let Some(context_window) = self.context_window {
            agent = agent.with_context_window(context_window);
        }
        for capability in crate::filters::default_filter_capabilities_with_config(
            Some(&model),
            compact_model_settings.as_ref(),
            Some(&compact_request_params),
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
        for capability in self.capabilities {
            agent = agent.with_capability(capability);
        }
        for bundle in self.capability_bundles {
            agent = agent.with_capability_bundle(bundle.as_ref());
        }
        if let Some(recorder) = self.trace_recorder {
            agent = agent.with_trace_recorder(recorder);
        }
        agent
    }
}

/// Create an agent builder from a model.
#[must_use]
pub fn agent(model: Arc<dyn ModelAdapter>) -> AgentBuilder {
    AgentBuilder::new(model)
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
