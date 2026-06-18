use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_runtime::{AgentRuntimePolicy, CapabilitySpec};
use starweaver_usage::UsageLimits;

use super::{
    is_false, ApprovalPolicyPreset, DurabilityPolicyPreset, EnvironmentPolicyPreset,
    HostPolicySpec, ModelPreset, ObservabilityPolicyPreset, OutputSpec, RetryPolicyPreset,
    SkillBundleSpec, StreamingPolicyPreset, TemplateStringSpec, ToolsetWrapperSpec,
    WorkspacePolicySpec,
};

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
    /// Approval policy selected by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalPolicyPreset>,
    /// Environment policy selected by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentPolicyPreset>,
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
