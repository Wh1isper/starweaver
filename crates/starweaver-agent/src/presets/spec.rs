//! Agent spec validation and builder projection.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::{json, Value};
use starweaver_context::{ModelCapability, ModelConfig};
use starweaver_model::{
    get_model_config, get_model_settings, ModelConfigPresetData, ModelSettings,
    ProfileOverrideModel,
};
use starweaver_runtime::OutputPolicy;
use starweaver_tools::ToolRegistry;

use crate::AgentBuilder;

use super::{
    registry::AgentSpecRegistry,
    types::{
        AgentSpec, AgentSpecError, AgentSpecHostPolicies, DurabilityPolicyPreset,
        ObservabilityPolicyPreset, RetryPolicyPreset, StreamingPolicyPreset,
    },
};

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
