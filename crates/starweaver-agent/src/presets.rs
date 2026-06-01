//! SDK presets and serializable agent specs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use starweaver_model::{get_model_settings, ModelAdapter, ModelPresetError, ModelSettings};
use starweaver_runtime::{AgentRuntimePolicy, OutputPolicy, UsageLimits};
use starweaver_tools::{DynToolset, ToolRegistry};
use thiserror::Error;

use crate::{AgentBuilder, SubagentConfig};

/// Model configuration preset.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelPreset {
    /// Logical model id.
    pub model_id: String,
    /// Built-in model settings preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_preset: Option<String>,
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
    /// Spec content could not be parsed.
    #[error("invalid agent spec: {0}")]
    Invalid(String),
    /// Model settings preset could not be resolved.
    #[error(transparent)]
    ModelPreset(#[from] ModelPresetError),
}

/// Registry used to resolve spec references into runtime objects.
#[derive(Clone, Default)]
pub struct AgentSpecRegistry {
    models: std::collections::BTreeMap<String, Arc<dyn ModelAdapter>>,
    toolsets: Vec<DynToolset>,
    toolsets_by_key: std::collections::BTreeMap<String, DynToolset>,
    subagents: Vec<SubagentConfig>,
    subagents_by_name: std::collections::BTreeMap<String, SubagentConfig>,
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
        self.subagents_by_name
            .insert(subagent.name.clone(), subagent.clone());
        self.subagents.push(subagent);
        self
    }

    fn model(&self, id: &str) -> Option<Arc<dyn ModelAdapter>> {
        self.models.get(id).cloned()
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
        if let Some(settings) = self.resolved_model_settings()? {
            builder = builder.model_settings(settings);
        }
        if let Some(limits) = self.preset.usage_limits.clone() {
            builder = builder.usage_limits(limits);
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
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Convenience preset for plain text output.
#[must_use]
pub fn text_output_preset() -> OutputPolicy {
    OutputPolicy::new()
}
