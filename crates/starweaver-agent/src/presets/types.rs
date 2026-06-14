//! Serializable agent spec and policy types.

use serde::{Deserialize, Serialize};
use starweaver_model::ModelSettings;
use starweaver_runtime::{OutputPolicy, OutputSchema};

mod agent_spec;
mod error;
mod policy;
mod resource;

pub use agent_spec::{AgentSpec, AgentSpecHostPolicies, SdkPreset};
pub use error::AgentSpecError;
pub use policy::{
    ApprovalPolicyPreset, DurabilityPolicyPreset, EnvironmentPolicyPreset,
    ObservabilityPolicyPreset, RetryPolicyPreset, StreamingPolicyPreset,
};
pub use resource::{
    HostAdapterSpec, HostPolicySpec, McpServerSpec, SkillBundleSpec, TemplateStringSpec,
    ToolsetWrapperSpec, WorkspacePolicySpec,
};

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(super) const fn is_false(value: &bool) -> bool {
    !*value
}

pub(super) const fn default_true() -> bool {
    true
}

pub(super) fn default_skills_dir() -> String {
    "skills".to_string()
}

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
    pub(in crate::presets) fn to_policy(&self) -> Option<OutputPolicy> {
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
