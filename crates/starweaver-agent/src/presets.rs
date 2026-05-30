//! SDK presets and serializable agent specs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use starweaver_model::{ModelAdapter, ModelSettings};
use starweaver_runtime::{AgentRuntimePolicy, OutputPolicy, UsageLimits};
use starweaver_tools::{DynToolset, ToolRegistry};
use thiserror::Error;

use crate::{AgentBuilder, SubagentConfig};

/// Model configuration preset.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelPreset {
    /// Logical model id.
    pub model_id: String,
    /// Default model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ModelSettings>,
}

/// SDK policy preset.
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
}

/// Serializable agent spec skeleton.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentSpec {
    /// Agent name.
    pub name: String,
    /// Static instructions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    /// Optional model preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelPreset>,
    /// SDK preset.
    #[serde(default)]
    pub preset: SdkPreset,
}

/// Agent spec loading failure.
#[derive(Debug, Error)]
pub enum AgentSpecError {
    /// Spec requested a model id that the caller did not provide.
    #[error("unknown model id: {0}")]
    UnknownModel(String),
    /// Spec content could not be parsed.
    #[error("invalid agent spec: {0}")]
    Invalid(String),
}

/// Registry used to resolve spec references into runtime objects.
#[derive(Clone, Default)]
pub struct AgentSpecRegistry {
    models: std::collections::BTreeMap<String, Arc<dyn ModelAdapter>>,
    toolsets: Vec<DynToolset>,
    subagents: Vec<SubagentConfig>,
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
        self.toolsets.push(toolset);
        self
    }

    /// Register a subagent.
    #[must_use]
    pub fn with_subagent(mut self, subagent: SubagentConfig) -> Self {
        self.subagents.push(subagent);
        self
    }

    fn model(&self, id: &str) -> Option<Arc<dyn ModelAdapter>> {
        self.models.get(id).cloned()
    }
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
        let model = registry
            .model(model_id)
            .ok_or_else(|| AgentSpecError::UnknownModel(model_id.to_string()))?;
        let mut builder = AgentBuilder::new(model).policy(self.preset.runtime.clone());
        for instruction in &self.instructions {
            builder = builder.instruction(instruction.clone());
        }
        if let Some(settings) = self
            .model
            .as_ref()
            .and_then(|model| model.settings.clone())
            .or_else(|| {
                self.preset
                    .model
                    .as_ref()
                    .and_then(|model| model.settings.clone())
            })
        {
            builder = builder.model_settings(settings);
        }
        if let Some(limits) = self.preset.usage_limits.clone() {
            builder = builder.usage_limits(limits);
        }
        let mut tools = ToolRegistry::new();
        for toolset in &registry.toolsets {
            tools.insert_toolset(toolset);
        }
        if !tools.is_empty() {
            builder = builder.tool_registry(tools);
        }
        for subagent in &registry.subagents {
            builder = builder.subagent(subagent.clone());
        }
        Ok(builder)
    }
}

/// Convenience preset for plain text output.
#[must_use]
pub fn text_output_preset() -> OutputPolicy {
    OutputPolicy::new()
}
