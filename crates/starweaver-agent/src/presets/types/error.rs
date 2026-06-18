use starweaver_model::{ModelError, ModelPresetError};
use thiserror::Error;

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
    /// Spec requested a skill registry root that the caller did not provide.
    #[error("unknown skill registry root: {0}")]
    UnknownSkillRoot(String),
    /// Spec requested an environment provider that the caller did not provide.
    #[error("unknown environment provider: {0}")]
    UnknownEnvironmentProvider(String),
    /// Spec requested a toolset wrapper that Starweaver cannot materialize.
    #[error("unsupported toolset wrapper kind: {0}")]
    UnsupportedToolsetWrapper(String),
    /// Spec toolset wrapper parameters are invalid.
    #[error("invalid toolset wrapper {kind}: {reason}")]
    InvalidToolsetWrapper {
        /// Wrapper kind.
        kind: String,
        /// Validation failure reason.
        reason: String,
    },
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
