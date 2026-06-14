//! SDK presets and serializable agent specs.

mod registry;
mod spec;
mod types;

pub use registry::AgentSpecRegistry;
pub use types::{
    AgentSpec, AgentSpecError, AgentSpecHostPolicies, ApprovalPolicyPreset, DurabilityPolicyPreset,
    EnvironmentPolicyPreset, HostAdapterSpec, HostPolicySpec, McpServerSpec, ModelPreset,
    ObservabilityPolicyPreset, OutputSpec, RetryPolicyPreset, SdkPreset, SkillBundleSpec,
    StreamingPolicyPreset, TemplateStringSpec, ToolsetWrapperSpec, WorkspacePolicySpec,
};

use starweaver_runtime::OutputPolicy;

/// Convenience preset for plain text output.
#[must_use]
pub fn text_output_preset() -> OutputPolicy {
    OutputPolicy::new()
}
