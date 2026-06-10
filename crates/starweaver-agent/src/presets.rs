//! SDK presets and serializable agent specs.

use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use starweaver_context::{ModelCapability, ModelConfig};
use starweaver_model::{
    get_model_config, get_model_settings, ModelAdapter, ModelConfigPresetData, ModelError,
    ModelPresetError, ModelSettings, ProfileOverrideModel,
};
use starweaver_runtime::{
    AgentRuntimePolicy, CapabilitySpec, OutputPolicy, OutputSchema, UsageLimits,
};
use starweaver_tools::{DynToolset, ToolRegistry};
use thiserror::Error;

use crate::{AgentBuilder, SubagentConfig};

/// Model configuration selected by a serializable agent spec.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelPreset {
    /// Logical model id resolved by [`AgentSpecRegistry`].
    pub model_id: String,
    /// Built-in model settings preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_preset: Option<String>,
    /// Built-in model capability/config preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_preset: Option<String>,
    /// Default model settings overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ModelSettings>,
}

/// Serializable output profile for an agent spec.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputSpec {
    /// Optional structured output schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<OutputSchema>,
    /// Optional output validation retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<usize>,
}

impl OutputSpec {
    fn to_policy(&self) -> Option<OutputPolicy> {
        if self.schema.is_none() && self.retries.is_none() {
            return None;
        }
        let mut policy = OutputPolicy::new();
        if let Some(schema) = self.schema.clone() {
            policy = policy.with_schema(schema);
        }
        if let Some(retries) = self.retries {
            policy = policy.with_retries(retries);
        }
        Some(policy)
    }
}

/// Approval policy preset for tools and host operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalPolicyPreset {
    /// Tool names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_required_tools: Vec<String>,
    /// Tool names using deferred execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_tools: Vec<String>,
    /// Whether network tools require approval.
    #[serde(default, skip_serializing_if = "is_false")]
    pub network_requires_approval: bool,
}

/// Retry and timeout policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetryPolicyPreset {
    /// Maximum model/tool loop steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    /// Output validation retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_retries: Option<usize>,
    /// Agent-level function tool retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_retries: Option<usize>,
    /// Optional timeout in milliseconds for future host adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl RetryPolicyPreset {
    fn merge(&mut self, overlay: &Self) {
        if overlay.max_steps.is_some() {
            self.max_steps = overlay.max_steps;
        }
        if overlay.output_retries.is_some() {
            self.output_retries = overlay.output_retries;
        }
        if overlay.tool_retries.is_some() {
            self.tool_retries = overlay.tool_retries;
        }
        if overlay.timeout_ms.is_some() {
            self.timeout_ms = overlay.timeout_ms;
        }
    }

    const fn apply_runtime(&self, runtime: &mut AgentRuntimePolicy) {
        if let Some(max_steps) = self.max_steps {
            runtime.max_steps = max_steps;
        }
        if let Some(output_retries) = self.output_retries {
            runtime.output_retries = output_retries;
        }
    }
}

/// Streaming policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamingPolicyPreset {
    /// Whether stream events should be collected by default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collect_events: bool,
    /// Stable host stream adapter name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// Whether stream replay metadata should be persisted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub replay: bool,
}

/// Observability policy preset.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ObservabilityPolicyPreset {
    /// Whether tracing is enabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trace_enabled: bool,
    /// Optional exporter name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exporter: Option<String>,
    /// Sensitive keys or paths to redact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_keys: Vec<String>,
    /// Optional sampling ratio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_ratio: Option<f64>,
}

/// Environment policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentPolicyPreset {
    /// Stable environment provider or profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Filesystem roots or logical mount names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Whether process-capable shell support is requested.
    #[serde(default, skip_serializing_if = "is_false")]
    pub process_capable: bool,
    /// Whether sandbox support is requested.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sandbox: bool,
}

/// Durability policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurabilityPolicyPreset {
    /// Stable session store name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
    /// Checkpoint cadence in runtime loop steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_every_steps: Option<usize>,
    /// Whether stream records are persisted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub persist_streams: bool,
    /// Whether resume profiles are enabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub resume_enabled: bool,
}

/// Skill bundle configuration for fileops-loaded skills.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillBundleSpec {
    /// Whether the skill bundle is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Provider-visible roots to scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Primary skills directory name.
    #[serde(default = "default_skills_dir")]
    pub skills_dir_name: String,
    /// Additional directory names, such as `.agents/skills`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_dir_names: Vec<String>,
    /// Whether hot reload should happen at request boundaries.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hot_reload: bool,
    /// Stable pre-scan hook name resolved by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_scan_hook: Option<String>,
}

impl Default for SkillBundleSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: Vec::new(),
            skills_dir_name: default_skills_dir(),
            extra_dir_names: Vec::new(),
            hot_reload: false,
            pre_scan_hook: None,
        }
    }
}

/// Serializable host adapter reference.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostAdapterSpec {
    /// Stable adapter kind, such as search, scrape, download, or media.
    pub kind: String,
    /// Host adapter name resolved by the SDK host.
    pub name: String,
    /// Adapter metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Serializable MCP server reference.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpServerSpec {
    /// Stable MCP server name resolved by the SDK host.
    pub name: String,
    /// Transport kind, such as `stdio` or `streamable_http`.
    pub transport: String,
    /// Server metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Template string rendered from dependency values by SDK hosts.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TemplateStringSpec {
    /// Stable template name.
    pub name: String,
    /// Template body. Variables use `{{path.to.value}}` placeholders.
    pub template: String,
    /// Target host field, such as `instruction` or `metadata.title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Toolset wrapper requested by an agent spec.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolsetWrapperSpec {
    /// Wrapper kind, such as `filtered`, `renamed`, `approval_required`, `dynamic`, or `deferred_loading`.
    pub kind: String,
    /// Registry key for the inner toolset when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolset: Option<String>,
    /// Wrapper parameters validated by the host.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

/// Serializable host adapter policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostPolicySpec {
    /// Host adapter kind, such as `agui`, `vercel_ai`, or `cli`.
    pub kind: String,
    /// Trust mode used by request/history sanitizers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<String>,
    /// Sanitizer names to apply at host boundaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sanitizers: Vec<String>,
    /// Adapter-specific policy metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Environment/workspace policy requested by an agent spec.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspacePolicySpec {
    /// Workspace provider or profile name resolved by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Allowed root or mount names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Shell execution policy such as `disabled`, `review`, or `trusted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Sandbox policy such as `local`, `docker`, or `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// Policy metadata recorded by the host.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Host-materialized `AgentSpec` policies after registry validation.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentSpecHostPolicies {
    /// Dependency JSON schema supplied by the application author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_schema: Option<Value>,
    /// Template strings validated against dependency fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<TemplateStringSpec>,
    /// Capability specs selected by this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilitySpec>,
    /// Capability registry names selected by this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capability_refs: Vec<String>,
    /// Toolset wrapper specs selected by this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolset_wrappers: Vec<ToolsetWrapperSpec>,
    /// Host adapter policies selected by this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_policies: Vec<HostPolicySpec>,
    /// Workspace policy selected by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspacePolicySpec>,
    /// Durability policy selected by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<DurabilityPolicyPreset>,
    /// Observability policy selected by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<ObservabilityPolicyPreset>,
    /// Streaming policy selected by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<StreamingPolicyPreset>,
    /// Additional metadata fields for hosts and editors.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// SDK policy preset container.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SdkPreset {
    /// Optional model preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelPreset>,
    /// Runtime policy.
    #[serde(default)]
    pub runtime: AgentRuntimePolicy,
    /// Optional usage limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limits: Option<UsageLimits>,
    /// Named approval preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_preset: Option<String>,
    /// Inline approval preset overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalPolicyPreset>,
    /// Named retry preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_preset: Option<String>,
    /// Inline retry preset overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicyPreset>,
    /// Named streaming preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_preset: Option<String>,
    /// Inline streaming preset overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<StreamingPolicyPreset>,
    /// Named observability preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability_preset: Option<String>,
    /// Inline observability preset overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<ObservabilityPolicyPreset>,
    /// Named environment preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_preset: Option<String>,
    /// Inline environment preset overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentPolicyPreset>,
    /// Named durability preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability_preset: Option<String>,
    /// Inline durability preset overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<DurabilityPolicyPreset>,
}

/// Serializable agent spec profile.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentSpec {
    /// Agent name.
    pub name: String,
    /// Human-readable agent description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Dependency JSON schema used by hosts to validate template variables and typed dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_schema: Option<Value>,
    /// Template strings validated against dependency schema property paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<TemplateStringSpec>,
    /// Static instructions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    /// Optional model preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelPreset>,
    /// SDK policy preset container.
    #[serde(default)]
    pub preset: SdkPreset,
    /// Optional output profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputSpec>,
    /// Optional skill bundle config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillBundleSpec>,
    /// Capability specs directly embedded in this agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilitySpec>,
    /// Capability registry names to validate through [`AgentSpecRegistry`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capability_refs: Vec<String>,
    /// Toolset wrapper specs validated by the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolset_wrappers: Vec<ToolsetWrapperSpec>,
    /// Host adapter policies for request adapters and sanitizers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_policies: Vec<HostPolicySpec>,
    /// Workspace policy requested from the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspacePolicySpec>,
    /// Host-materialized metadata fields.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
    /// Host adapter names to validate through [`AgentSpecRegistry`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_adapters: Vec<String>,
    /// MCP server names to validate through [`AgentSpecRegistry`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    /// Attach every toolset registered by the host registry.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all_toolsets: bool,
    /// Toolset ids or names to attach from the registry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolsets: Vec<String>,
    /// Attach every subagent registered by the host registry.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all_subagents: bool,
    /// Subagent names to attach from the registry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<String>,
}

/// Agent spec loading failure.
#[derive(Debug, Error)]
pub enum AgentSpecError {
    /// Spec requested a model id that the caller did not provide.
    #[error("unknown model id: {0}")]
    UnknownModel(String),
    /// Spec requested a toolset id or name that the caller did not provide.
    #[error("unknown toolset: {0}")]
    UnknownToolset(String),
    /// Spec requested a subagent name that the caller did not provide.
    #[error("unknown subagent: {0}")]
    UnknownSubagent(String),
    /// Spec requested a policy preset that the caller did not provide.
    #[error("unknown {kind} preset: {name}")]
    UnknownPolicyPreset {
        /// Preset kind.
        kind: &'static str,
        /// Missing preset name.
        name: String,
    },
    /// Spec requested a host adapter that the caller did not provide.
    #[error("unknown host adapter: {0}")]
    UnknownHostAdapter(String),
    /// Spec requested an MCP server that the caller did not provide.
    #[error("unknown MCP server: {0}")]
    UnknownMcpServer(String),
    /// Spec requested a capability that the caller did not provide.
    #[error("unknown capability: {0}")]
    UnknownCapability(String),
    /// Template references a dependency path absent from the dependency schema.
    #[error("unknown dependency template variable '{variable}' in template '{template}'")]
    UnknownTemplateVariable {
        /// Template name.
        template: String,
        /// Missing dependency variable path.
        variable: String,
    },
    /// Template syntax is invalid.
    #[error("invalid template '{template}': {reason}")]
    InvalidTemplate {
        /// Template name.
        template: String,
        /// Syntax failure reason.
        reason: String,
    },
    /// Spec content could not be parsed.
    #[error("invalid agent spec: {0}")]
    Invalid(String),
    /// OAuth model id used an invalid `oauth@provider:model` form.
    #[error("invalid OAuth model id {model_id:?}: expected oauth@provider:model")]
    InvalidOAuthModel {
        /// Invalid model id.
        model_id: String,
    },
    /// OAuth-backed model construction failed.
    #[error("failed to resolve OAuth model id {model_id:?}: {source}")]
    OAuthModel {
        /// Requested model id.
        model_id: String,
        /// Underlying model construction error.
        #[source]
        source: ModelError,
    },
    /// Model settings preset could not be resolved.
    #[error(transparent)]
    ModelPreset(#[from] ModelPresetError),
}

/// Registry used to resolve spec references into runtime objects.
#[derive(Clone, Default)]
pub struct AgentSpecRegistry {
    models: BTreeMap<String, Arc<dyn ModelAdapter>>,
    toolsets: Vec<DynToolset>,
    toolsets_by_key: BTreeMap<String, DynToolset>,
    subagents: Vec<SubagentConfig>,
    subagents_by_name: BTreeMap<String, SubagentConfig>,
    approval_presets: BTreeMap<String, ApprovalPolicyPreset>,
    retry_presets: BTreeMap<String, RetryPolicyPreset>,
    streaming_presets: BTreeMap<String, StreamingPolicyPreset>,
    observability_presets: BTreeMap<String, ObservabilityPolicyPreset>,
    environment_presets: BTreeMap<String, EnvironmentPolicyPreset>,
    durability_presets: BTreeMap<String, DurabilityPolicyPreset>,
    host_adapters: BTreeMap<String, HostAdapterSpec>,
    mcp_servers: BTreeMap<String, McpServerSpec>,
    capabilities: BTreeMap<String, CapabilitySpec>,
}

impl AgentSpecRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a model id.
    #[must_use]
    pub fn with_model(mut self, id: impl Into<String>, model: Arc<dyn ModelAdapter>) -> Self {
        self.models.insert(id.into(), model);
        self
    }

    /// Register a toolset.
    #[must_use]
    pub fn with_toolset(mut self, toolset: DynToolset) -> Self {
        self.register_toolset_keys(&toolset);
        self.toolsets.push(toolset);
        self
    }

    /// Register a toolset under an additional caller-provided alias.
    #[must_use]
    pub fn with_toolset_alias(mut self, alias: impl Into<String>, toolset: DynToolset) -> Self {
        self.toolsets_by_key.insert(alias.into(), toolset.clone());
        self.register_toolset_keys(&toolset);
        self.toolsets.push(toolset);
        self
    }

    /// Register a subagent.
    #[must_use]
    pub fn with_subagent(mut self, subagent: SubagentConfig) -> Self {
        if let Some(existing) = self
            .subagents_by_name
            .insert(subagent.name.clone(), subagent.clone())
        {
            self.subagents
                .retain(|registered| registered.name != existing.name);
        }
        self.subagents.push(subagent);
        self
    }

    /// Register an approval preset.
    #[must_use]
    pub fn with_approval_preset(
        mut self,
        name: impl Into<String>,
        preset: ApprovalPolicyPreset,
    ) -> Self {
        self.approval_presets.insert(name.into(), preset);
        self
    }

    /// Register a retry preset.
    #[must_use]
    pub fn with_retry_preset(mut self, name: impl Into<String>, preset: RetryPolicyPreset) -> Self {
        self.retry_presets.insert(name.into(), preset);
        self
    }

    /// Register a streaming preset.
    #[must_use]
    pub fn with_streaming_preset(
        mut self,
        name: impl Into<String>,
        preset: StreamingPolicyPreset,
    ) -> Self {
        self.streaming_presets.insert(name.into(), preset);
        self
    }

    /// Register an observability preset.
    #[must_use]
    pub fn with_observability_preset(
        mut self,
        name: impl Into<String>,
        preset: ObservabilityPolicyPreset,
    ) -> Self {
        self.observability_presets.insert(name.into(), preset);
        self
    }

    /// Register an environment preset.
    #[must_use]
    pub fn with_environment_preset(
        mut self,
        name: impl Into<String>,
        preset: EnvironmentPolicyPreset,
    ) -> Self {
        self.environment_presets.insert(name.into(), preset);
        self
    }

    /// Register a durability preset.
    #[must_use]
    pub fn with_durability_preset(
        mut self,
        name: impl Into<String>,
        preset: DurabilityPolicyPreset,
    ) -> Self {
        self.durability_presets.insert(name.into(), preset);
        self
    }

    /// Register a host adapter by stable name.
    #[must_use]
    pub fn with_host_adapter(mut self, name: impl Into<String>, adapter: HostAdapterSpec) -> Self {
        self.host_adapters.insert(name.into(), adapter);
        self
    }

    /// Register an MCP server by stable name.
    #[must_use]
    pub fn with_mcp_server(mut self, name: impl Into<String>, server: McpServerSpec) -> Self {
        self.mcp_servers.insert(name.into(), server);
        self
    }

    /// Register a capability spec by stable id or alias.
    #[must_use]
    pub fn with_capability(mut self, name: impl Into<String>, capability: CapabilitySpec) -> Self {
        self.capabilities.insert(name.into(), capability);
        self
    }

    fn model(&self, id: &str) -> Result<Option<Arc<dyn ModelAdapter>>, AgentSpecError> {
        if let Some(model) = self.models.get(id).cloned() {
            return Ok(Some(model));
        }
        infer_oauth_model_from_id(id)
    }

    fn toolset(&self, key: &str) -> Option<DynToolset> {
        self.toolsets_by_key.get(key).cloned()
    }

    fn subagent(&self, name: &str) -> Option<SubagentConfig> {
        self.subagents_by_name.get(name).cloned()
    }

    fn register_toolset_keys(&mut self, toolset: &DynToolset) {
        self.toolsets_by_key
            .insert(toolset.name().to_string(), toolset.clone());
        if let Some(id) = toolset.id() {
            self.toolsets_by_key.insert(id.to_string(), toolset.clone());
        }
    }
}

fn infer_oauth_model_from_id(
    model_id: &str,
) -> Result<Option<Arc<dyn ModelAdapter>>, AgentSpecError> {
    let Some(rest) = model_id.strip_prefix("oauth@") else {
        return Ok(None);
    };
    let Some((provider_name, model_name)) = rest.split_once(':') else {
        return Err(AgentSpecError::InvalidOAuthModel {
            model_id: model_id.to_string(),
        });
    };
    if provider_name.is_empty() || model_name.is_empty() {
        return Err(AgentSpecError::InvalidOAuthModel {
            model_id: model_id.to_string(),
        });
    }
    let model = starweaver_oauth_provider::infer_oauth_model(provider_name, model_name).map_err(
        |source| AgentSpecError::OAuthModel {
            model_id: model_id.to_string(),
            source,
        },
    )?;
    Ok(Some(Arc::new(model)))
}

impl AgentSpec {
    /// Build a spec from YAML.
    ///
    /// # Errors
    ///
    /// Returns an error when YAML parsing fails.
    pub fn from_yaml(text: &str) -> Result<Self, AgentSpecError> {
        serde_yaml::from_str(text).map_err(|error| AgentSpecError::Invalid(error.to_string()))
    }

    /// Return an editor-oriented JSON schema for `AgentSpec` v2.
    #[must_use]
    pub fn json_schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "Starweaver AgentSpec v2",
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "dependency_schema": {"type": "object"},
                "templates": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["name", "template"],
                        "properties": {
                            "name": {"type": "string"},
                            "template": {"type": "string"},
                            "target": {"type": "string"}
                        }
                    }
                },
                "instructions": {"type": "array", "items": {"type": "string"}},
                "model": {"type": "object"},
                "preset": {"type": "object"},
                "output": {"type": "object"},
                "skills": {"type": "object"},
                "capabilities": {"type": "array", "items": {"type": "object"}},
                "capability_refs": {"type": "array", "items": {"type": "string"}},
                "toolset_wrappers": {"type": "array", "items": {"type": "object"}},
                "host_policies": {"type": "array", "items": {"type": "object"}},
                "workspace": {"type": "object"},
                "metadata": {"type": "object"},
                "host_adapters": {"type": "array", "items": {"type": "string"}},
                "mcp_servers": {"type": "array", "items": {"type": "string"}},
                "all_toolsets": {"type": "boolean"},
                "toolsets": {"type": "array", "items": {"type": "string"}},
                "all_subagents": {"type": "boolean"},
                "subagents": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    /// Validate host-materialized `AgentSpec` v2 fields and return their resolved projection.
    ///
    /// # Errors
    ///
    /// Returns an error when registry references or template variables cannot be resolved.
    pub fn host_policies(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<AgentSpecHostPolicies, AgentSpecError> {
        self.validate_policy_refs(registry)?;
        self.validate_host_refs(registry)?;
        self.validate_capability_refs(registry)?;
        self.validate_templates()?;
        let mut capabilities = self.capabilities.clone();
        for name in &self.capability_refs {
            let capability = registry
                .capabilities
                .get(name)
                .cloned()
                .ok_or_else(|| AgentSpecError::UnknownCapability(name.clone()))?;
            capabilities.push(capability);
        }
        Ok(AgentSpecHostPolicies {
            dependency_schema: self.dependency_schema.clone(),
            templates: self.templates.clone(),
            capabilities,
            capability_refs: self.capability_refs.clone(),
            toolset_wrappers: self.toolset_wrappers.clone(),
            host_policies: self.host_policies.clone(),
            workspace: self.workspace.clone(),
            durability: self.resolved_durability(registry)?,
            observability: self.resolved_observability(registry)?,
            streaming: self.resolved_streaming(registry)?,
            metadata: self.metadata.clone(),
        })
    }

    /// Build an agent builder from this spec.
    ///
    /// # Errors
    ///
    /// Returns an error when referenced objects cannot be resolved.
    pub fn builder(&self, registry: &AgentSpecRegistry) -> Result<AgentBuilder, AgentSpecError> {
        let model_id = self
            .model
            .as_ref()
            .or(self.preset.model.as_ref())
            .map(|model| model.model_id.as_str())
            .ok_or_else(|| AgentSpecError::UnknownModel("<missing>".to_string()))?;
        let mut model = registry
            .model(model_id)?
            .ok_or_else(|| AgentSpecError::UnknownModel(model_id.to_string()))?;
        let retry = self.resolved_retry(registry)?;
        let model_config = self.resolved_model_config()?;
        if let Some(config) = model_config.as_ref() {
            model = Arc::new(ProfileOverrideModel::new(model, config.profile.clone()));
        }
        let mut runtime = self.preset.runtime.clone();
        retry.apply_runtime(&mut runtime);
        self.host_policies(registry)?;
        let mut builder = AgentBuilder::new(model).policy(runtime);
        for instruction in &self.instructions {
            builder = builder.instruction(instruction.clone());
        }
        if let Some(settings) = self.resolved_model_settings()? {
            builder = builder.model_settings(settings);
        }
        if let Some(model_config) = model_config.as_ref() {
            builder = builder.model_config(context_model_config_from_preset(model_config));
        }
        if let Some(limits) = self.preset.usage_limits.clone() {
            builder = builder.usage_limits(limits);
        }
        if let Some(tool_retries) = retry.tool_retries {
            builder = builder.tool_retries(tool_retries);
        }
        if let Some(output) = self.resolved_output(&retry) {
            builder = builder.output_policy(output);
        }
        let mut selected_toolsets = Vec::new();
        for key in &self.toolsets {
            selected_toolsets.push(
                registry
                    .toolset(key)
                    .ok_or_else(|| AgentSpecError::UnknownToolset(key.clone()))?,
            );
        }
        let mut tools = ToolRegistry::new();
        if self.all_toolsets {
            for toolset in &registry.toolsets {
                tools.insert_toolset(toolset);
            }
        } else {
            for toolset in selected_toolsets {
                tools.insert_toolset(&toolset);
            }
        }
        if !tools.is_empty() {
            builder = builder.tool_registry(tools);
        }
        let mut selected_subagents = Vec::new();
        for name in &self.subagents {
            selected_subagents.push(
                registry
                    .subagent(name)
                    .ok_or_else(|| AgentSpecError::UnknownSubagent(name.clone()))?,
            );
        }
        if self.all_subagents {
            for subagent in &registry.subagents {
                builder = builder.subagent(subagent.clone());
            }
        } else {
            for subagent in selected_subagents {
                builder = builder.subagent(subagent);
            }
        }
        Ok(builder)
    }

    fn resolved_model_settings(&self) -> Result<Option<ModelSettings>, AgentSpecError> {
        let Some(model) = self.model.as_ref().or(self.preset.model.as_ref()) else {
            return Ok(None);
        };
        let preset_settings = model
            .settings_preset
            .as_deref()
            .map(get_model_settings)
            .transpose()?;
        Ok(match (preset_settings, model.settings.clone()) {
            (Some(base), Some(overlay)) => Some(base.merge(&overlay)),
            (Some(base), None) => Some(base),
            (None, Some(settings)) => Some(settings),
            (None, None) => None,
        })
    }

    fn resolved_model_config(
        &self,
    ) -> Result<Option<starweaver_model::ModelConfigPresetData>, AgentSpecError> {
        let Some(model) = self.model.as_ref().or(self.preset.model.as_ref()) else {
            return Ok(None);
        };
        model
            .config_preset
            .as_deref()
            .map(get_model_config)
            .transpose()
            .map_err(AgentSpecError::from)
    }

    fn resolved_policy<T: Clone>(
        named: Option<&str>,
        inline: Option<T>,
        kind: &'static str,
        presets: &BTreeMap<String, T>,
    ) -> Result<Option<T>, AgentSpecError> {
        let base = named
            .map(|name| {
                presets
                    .get(name)
                    .cloned()
                    .ok_or_else(|| AgentSpecError::UnknownPolicyPreset {
                        kind,
                        name: name.to_string(),
                    })
            })
            .transpose()?;
        Ok(inline.or(base))
    }

    fn resolved_streaming(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<StreamingPolicyPreset>, AgentSpecError> {
        Self::resolved_policy(
            self.preset.streaming_preset.as_deref(),
            self.preset.streaming.clone(),
            "streaming",
            &registry.streaming_presets,
        )
    }

    fn resolved_observability(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<ObservabilityPolicyPreset>, AgentSpecError> {
        Self::resolved_policy(
            self.preset.observability_preset.as_deref(),
            self.preset.observability.clone(),
            "observability",
            &registry.observability_presets,
        )
    }

    fn resolved_durability(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<DurabilityPolicyPreset>, AgentSpecError> {
        Self::resolved_policy(
            self.preset.durability_preset.as_deref(),
            self.preset.durability.clone(),
            "durability",
            &registry.durability_presets,
        )
    }

    fn resolved_retry(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<RetryPolicyPreset, AgentSpecError> {
        let mut retry = self
            .preset
            .retry_preset
            .as_deref()
            .map(|name| {
                registry.retry_presets.get(name).cloned().ok_or_else(|| {
                    AgentSpecError::UnknownPolicyPreset {
                        kind: "retry",
                        name: name.to_string(),
                    }
                })
            })
            .transpose()?
            .unwrap_or_default();
        if let Some(overlay) = &self.preset.retry {
            retry.merge(overlay);
        }
        Ok(retry)
    }

    fn resolved_output(&self, retry: &RetryPolicyPreset) -> Option<OutputPolicy> {
        let mut spec = self.output.clone().unwrap_or_default();
        if spec.retries.is_none() {
            spec.retries = retry.output_retries;
        }
        spec.to_policy()
    }

    fn validate_policy_refs(&self, registry: &AgentSpecRegistry) -> Result<(), AgentSpecError> {
        validate_named(
            self.preset.approval_preset.as_deref(),
            "approval",
            &registry.approval_presets,
        )?;
        validate_named(
            self.preset.streaming_preset.as_deref(),
            "streaming",
            &registry.streaming_presets,
        )?;
        validate_named(
            self.preset.observability_preset.as_deref(),
            "observability",
            &registry.observability_presets,
        )?;
        validate_named(
            self.preset.environment_preset.as_deref(),
            "environment",
            &registry.environment_presets,
        )?;
        validate_named(
            self.preset.durability_preset.as_deref(),
            "durability",
            &registry.durability_presets,
        )?;
        Ok(())
    }

    fn validate_host_refs(&self, registry: &AgentSpecRegistry) -> Result<(), AgentSpecError> {
        for name in &self.host_adapters {
            if !registry.host_adapters.contains_key(name) {
                return Err(AgentSpecError::UnknownHostAdapter(name.clone()));
            }
        }
        for name in &self.mcp_servers {
            if !registry.mcp_servers.contains_key(name) {
                return Err(AgentSpecError::UnknownMcpServer(name.clone()));
            }
        }
        Ok(())
    }

    fn validate_capability_refs(&self, registry: &AgentSpecRegistry) -> Result<(), AgentSpecError> {
        for name in &self.capability_refs {
            if !registry.capabilities.contains_key(name) {
                return Err(AgentSpecError::UnknownCapability(name.clone()));
            }
        }
        Ok(())
    }

    fn validate_templates(&self) -> Result<(), AgentSpecError> {
        for template in &self.templates {
            for variable in template_variables(&template.template).map_err(|reason| {
                AgentSpecError::InvalidTemplate {
                    template: template.name.clone(),
                    reason,
                }
            })? {
                if !dependency_schema_has_path(self.dependency_schema.as_ref(), &variable) {
                    return Err(AgentSpecError::UnknownTemplateVariable {
                        template: template.name.clone(),
                        variable,
                    });
                }
            }
        }
        Ok(())
    }
}

fn template_variables(template: &str) -> Result<Vec<String>, String> {
    let mut variables = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err("unclosed '{{' placeholder".to_string());
        };
        let variable = after_start[..end].trim();
        if variable.is_empty() {
            return Err("empty placeholder".to_string());
        }
        if !variable
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-')
        {
            return Err(format!("invalid placeholder name '{variable}'"));
        }
        variables.push(variable.to_string());
        rest = &after_start[end + 2..];
    }
    if rest.contains("}}") {
        return Err("unopened '}}' placeholder".to_string());
    }
    Ok(variables)
}

fn dependency_schema_has_path(schema: Option<&Value>, path: &str) -> bool {
    let Some(schema) = schema else {
        return false;
    };
    let mut current = schema;
    for segment in path.split('.') {
        let Some(properties) = current.get("properties").and_then(Value::as_object) else {
            return false;
        };
        let Some(next) = properties.get(segment) else {
            return false;
        };
        current = next;
    }
    true
}

fn validate_named<T>(
    name: Option<&str>,
    kind: &'static str,
    map: &BTreeMap<String, T>,
) -> Result<(), AgentSpecError> {
    if let Some(name) = name {
        if !map.contains_key(name) {
            return Err(AgentSpecError::UnknownPolicyPreset {
                kind,
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

const fn default_true() -> bool {
    true
}

fn default_skills_dir() -> String {
    "skills".to_string()
}

fn context_model_config_from_preset(preset: &ModelConfigPresetData) -> ModelConfig {
    let mut capabilities = std::collections::BTreeSet::new();
    if preset.profile.supports_image_input {
        capabilities.insert(ModelCapability::Vision);
    }
    if preset.profile.supports_video_input {
        capabilities.insert(ModelCapability::VideoUnderstanding);
    }
    if preset.profile.supports_audio_input {
        capabilities.insert(ModelCapability::AudioUnderstanding);
    }
    if preset.profile.supports_document_input {
        capabilities.insert(ModelCapability::DocumentUnderstanding);
    }
    ModelConfig {
        context_window: Some(u64::from(preset.context_window)),
        max_images: usize::try_from(preset.max_images).unwrap_or(usize::MAX),
        max_videos: usize::try_from(preset.max_videos).unwrap_or(usize::MAX),
        support_gif: preset.supports_gif,
        split_large_images: preset.split_large_images,
        image_split_max_height: usize::try_from(preset.image_split_max_height)
            .unwrap_or(usize::MAX),
        image_split_overlap: usize::try_from(preset.image_split_overlap).unwrap_or(usize::MAX),
        capabilities,
        ..ModelConfig::default()
    }
}

/// Convenience preset for plain text output.
#[must_use]
pub fn text_output_preset() -> OutputPolicy {
    OutputPolicy::new()
}
