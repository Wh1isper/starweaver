//! Agent spec validation and builder projection.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::{json, Value};
use starweaver_context::{ModelCapability, ModelConfig};
use starweaver_model::{
    get_model_config, get_model_settings, ModelConfigPresetData, ModelSettings,
    ProfileOverrideModel,
};
use starweaver_runtime::OutputPolicy;
use starweaver_tools::{
    dynamic_tool_search, ApprovalRequiredToolset, DeferredToolset, DynToolset, FilteredToolset,
    RenamedToolset, ToolProxyToolset,
};

use crate::{AgentBuilder, AgentRuntimeBuilder, SkillRegistry};

use super::{
    registry::AgentSpecRegistry,
    types::{
        AgentSpec, AgentSpecError, AgentSpecHostPolicies, ApprovalPolicyPreset,
        DurabilityPolicyPreset, EnvironmentPolicyPreset, ObservabilityPolicyPreset,
        RetryPolicyPreset, SkillBundleSpec, StreamingPolicyPreset, ToolsetWrapperSpec,
    },
};

impl AgentSpec {
    /// Build a spec from YAML.
    ///
    /// # Errors
    ///
    /// Returns an error when YAML parsing fails.
    pub fn from_yaml(text: &str) -> Result<Self, AgentSpecError> {
        yaml_serde::from_str(text).map_err(|error| AgentSpecError::Invalid(error.to_string()))
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
            approval: self.resolved_approval(registry)?,
            environment: self.resolved_environment(registry)?,
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
    #[allow(clippy::too_many_lines)]
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
        let approval = self.resolved_approval(registry)?;
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
        if let Some(approval) = approval.as_ref() {
            builder = builder.approval_required_tools(approval.approval_required_tools.clone());
        }
        if let Some(output) = self.resolved_output(&retry) {
            builder = builder.output_policy(output);
        }
        if let Some(skill_registry) = self.materialized_skill_registry(registry)? {
            builder = builder.skills(skill_registry);
        }
        for name in &self.capability_refs {
            if let Some(bundle) = registry.capability_bundles.get(name) {
                builder = builder.capability_bundle(bundle.clone());
            }
        }
        let mut selected_toolsets = Vec::new();
        for key in &self.toolsets {
            selected_toolsets.push(
                registry
                    .toolset(key)
                    .ok_or_else(|| AgentSpecError::UnknownToolset(key.clone()))?,
            );
        }
        let mut materialized_toolsets = Vec::new();
        let wrapped_toolsets = self.materialized_toolset_wrappers(registry)?;
        let wrapped_keys = self
            .toolset_wrappers
            .iter()
            .filter_map(|wrapper| wrapper.toolset.as_deref())
            .collect::<Vec<_>>();
        if self.all_toolsets {
            for toolset in &registry.toolsets {
                if !wrapped_keys
                    .iter()
                    .any(|key| toolset_matches_key(toolset, key))
                {
                    materialized_toolsets.push(toolset.clone());
                }
            }
        } else {
            for toolset in selected_toolsets {
                if !wrapped_keys
                    .iter()
                    .any(|key| toolset_matches_key(&toolset, key))
                {
                    materialized_toolsets.push(toolset);
                }
            }
        }
        for toolset in wrapped_toolsets {
            materialized_toolsets.push(toolset);
        }
        if !materialized_toolsets.is_empty() {
            builder = builder.toolsets(materialized_toolsets);
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

    /// Build an owned runtime builder from this spec.
    ///
    /// # Errors
    ///
    /// Returns an error when referenced objects cannot be resolved.
    pub fn runtime_builder(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<AgentRuntimeBuilder, AgentSpecError> {
        let mut runtime = AgentRuntimeBuilder::from_builder(self.builder(registry)?);
        if let Some(environment) = self.materialized_environment_provider(registry)? {
            runtime = runtime.environment(environment);
        }
        Ok(runtime)
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

    fn resolved_approval(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<ApprovalPolicyPreset>, AgentSpecError> {
        let mut approval = self
            .preset
            .approval_preset
            .as_deref()
            .map(|name| {
                registry.approval_presets.get(name).cloned().ok_or_else(|| {
                    AgentSpecError::UnknownPolicyPreset {
                        kind: "approval",
                        name: name.to_string(),
                    }
                })
            })
            .transpose()?
            .unwrap_or_default();
        if let Some(overlay) = &self.preset.approval {
            merge_approval_policy(&mut approval, overlay);
        }
        Ok((!approval_policy_empty(&approval)).then_some(approval))
    }

    fn resolved_environment(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<EnvironmentPolicyPreset>, AgentSpecError> {
        Self::resolved_policy(
            self.preset.environment_preset.as_deref(),
            self.preset.environment.clone(),
            "environment",
            &registry.environment_presets,
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

    fn materialized_toolset_wrappers(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Vec<DynToolset>, AgentSpecError> {
        let mut toolsets = Vec::new();
        for wrapper in &self.toolset_wrappers {
            match wrapper.kind.as_str() {
                "approval_required" => {
                    let inner = wrapper_inner_toolset(wrapper, registry)?;
                    let approval_tools = approval_tools_from_wrapper(wrapper)?;
                    toolsets.push(
                        Arc::new(ApprovalRequiredToolset::new(inner, approval_tools)) as DynToolset,
                    );
                }
                "deferred" | "deferred_call" | "deferred_tools" => {
                    let inner = wrapper_inner_toolset(wrapper, registry)?;
                    let deferred_tools = deferred_tools_from_wrapper(wrapper)?;
                    toolsets
                        .push(Arc::new(DeferredToolset::new(inner, deferred_tools)) as DynToolset);
                }
                "dynamic" | "tool_proxy" | "dynamic_tool_proxy" => {
                    let inner = wrapper_inner_toolset(wrapper, registry)?;
                    let mut proxy = ToolProxyToolset::new(vec![inner]);
                    if let Some(prefix) = optional_string_param(wrapper, "prefix")? {
                        proxy = proxy.try_with_name_prefix(prefix).map_err(|error| {
                            AgentSpecError::InvalidToolsetWrapper {
                                kind: wrapper.kind.clone(),
                                reason: error.to_string(),
                            }
                        })?;
                    }
                    if let Some(max_results) = optional_usize_param(wrapper, "max_results")? {
                        proxy = proxy.with_max_results(max_results);
                    }
                    toolsets.push(Arc::new(proxy) as DynToolset);
                }
                "dynamic_search" | "tool_search" | "dynamic_tool_search" => {
                    let inner = wrapper_inner_toolset(wrapper, registry)?;
                    toolsets.push(dynamic_tool_search(vec![inner]));
                }
                "filtered" => {
                    let inner = wrapper_inner_toolset(wrapper, registry)?;
                    let include_tools = optional_string_list_param(wrapper, "include_tools")?
                        .or(optional_string_list_param(wrapper, "tools")?);
                    let exclude_tools = optional_string_list_param(wrapper, "exclude_tools")?;
                    match (include_tools, exclude_tools) {
                        (Some(tools), None) => {
                            toolsets.push(Arc::new(FilteredToolset::include_names(inner, tools))
                                as DynToolset);
                        }
                        (None, Some(tools)) => {
                            toolsets.push(Arc::new(FilteredToolset::exclude_names(inner, tools))
                                as DynToolset);
                        }
                        (Some(_), Some(_)) => {
                            return Err(AgentSpecError::InvalidToolsetWrapper {
                                kind: wrapper.kind.clone(),
                                reason: "include_tools and exclude_tools cannot both be set"
                                    .to_string(),
                            });
                        }
                        (None, None) => {
                            return Err(AgentSpecError::InvalidToolsetWrapper {
                                kind: wrapper.kind.clone(),
                                reason: "missing include_tools or exclude_tools".to_string(),
                            });
                        }
                    }
                }
                "renamed" => {
                    let inner = wrapper_inner_toolset(wrapper, registry)?;
                    let mappings = string_map_param(wrapper, "mappings")?;
                    toolsets.push(Arc::new(RenamedToolset::new(inner, mappings)) as DynToolset);
                }
                kind => {
                    if let Some(factory) = registry.toolset_wrapper_factories.get(kind) {
                        toolsets.push(factory(wrapper, registry)?);
                    } else {
                        return Err(AgentSpecError::UnsupportedToolsetWrapper(kind.to_string()));
                    }
                }
            }
        }
        Ok(toolsets)
    }

    fn materialized_skill_registry(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<SkillRegistry>, AgentSpecError> {
        let Some(skills) = self.skills.as_ref().filter(|skills| skills.enabled) else {
            return Ok(None);
        };
        let skill_registry = materialized_skill_registry(skills, registry)?;
        Ok((!skill_registry.is_empty()).then_some(skill_registry))
    }

    pub(crate) fn materialized_environment_provider(
        &self,
        registry: &AgentSpecRegistry,
    ) -> Result<Option<starweaver_environment::DynEnvironmentProvider>, AgentSpecError> {
        let environment = self.resolved_environment(registry)?;
        let provider = environment
            .as_ref()
            .and_then(|environment| environment.provider.as_deref())
            .or_else(|| {
                self.workspace
                    .as_ref()
                    .and_then(|workspace| workspace.provider.as_deref())
            });
        let Some(provider) = provider else {
            return Ok(None);
        };
        registry
            .environment_providers
            .get(provider)
            .cloned()
            .map(Some)
            .ok_or_else(|| AgentSpecError::UnknownEnvironmentProvider(provider.to_string()))
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

const fn approval_policy_empty(policy: &ApprovalPolicyPreset) -> bool {
    policy.approval_required_tools.is_empty()
        && policy.deferred_tools.is_empty()
        && !policy.network_requires_approval
}

fn materialized_skill_registry(
    skills: &SkillBundleSpec,
    registry: &AgentSpecRegistry,
) -> Result<SkillRegistry, AgentSpecError> {
    let mut merged = SkillRegistry::new();
    if skills.roots.is_empty() {
        for skill_registry in registry.skill_registries.values() {
            merge_skill_registry(&mut merged, skill_registry);
        }
        return Ok(merged);
    }
    for root in &skills.roots {
        let Some(skill_registry) = registry.skill_registries.get(root) else {
            return Err(AgentSpecError::UnknownSkillRoot(root.clone()));
        };
        merge_skill_registry(&mut merged, skill_registry);
    }
    Ok(merged)
}

fn merge_skill_registry(target: &mut SkillRegistry, source: &SkillRegistry) {
    for package in source.packages() {
        target.insert(package);
    }
}

fn merge_approval_policy(target: &mut ApprovalPolicyPreset, overlay: &ApprovalPolicyPreset) {
    extend_unique(
        &mut target.approval_required_tools,
        &overlay.approval_required_tools,
    );
    extend_unique(&mut target.deferred_tools, &overlay.deferred_tools);
    target.network_requires_approval |= overlay.network_requires_approval;
}

fn extend_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        if !target.iter().any(|existing| existing == value) {
            target.push(value.clone());
        }
    }
}

fn toolset_matches_key(toolset: &DynToolset, key: &str) -> bool {
    toolset.name() == key || toolset.id().is_some_and(|id| id == key)
}

fn wrapper_inner_toolset(
    wrapper: &ToolsetWrapperSpec,
    registry: &AgentSpecRegistry,
) -> Result<DynToolset, AgentSpecError> {
    let key = wrapper
        .toolset
        .as_deref()
        .ok_or_else(|| AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: "missing toolset".to_string(),
        })?;
    registry
        .toolset(key)
        .ok_or_else(|| AgentSpecError::UnknownToolset(key.to_string()))
}

fn approval_tools_from_wrapper(
    wrapper: &ToolsetWrapperSpec,
) -> Result<Vec<String>, AgentSpecError> {
    let Some(value) = wrapper
        .params
        .get("tools")
        .or_else(|| wrapper.params.get("tool"))
        .or_else(|| wrapper.params.get("approval_required_tools"))
    else {
        return Ok(vec!["*".to_string()]);
    };
    match value {
        Value::String(tool) if !tool.trim().is_empty() => Ok(vec![tool.trim().to_string()]),
        Value::Array(values) => {
            let mut tools = Vec::new();
            for value in values {
                let Some(tool) = value
                    .as_str()
                    .map(str::trim)
                    .filter(|tool| !tool.is_empty())
                else {
                    return Err(AgentSpecError::InvalidToolsetWrapper {
                        kind: wrapper.kind.clone(),
                        reason: "tools must contain non-empty strings".to_string(),
                    });
                };
                tools.push(tool.to_string());
            }
            if tools.is_empty() {
                return Err(AgentSpecError::InvalidToolsetWrapper {
                    kind: wrapper.kind.clone(),
                    reason: "tools must not be empty".to_string(),
                });
            }
            Ok(tools)
        }
        _ => Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: "tools must be a string or string array".to_string(),
        }),
    }
}

fn deferred_tools_from_wrapper(
    wrapper: &ToolsetWrapperSpec,
) -> Result<Vec<String>, AgentSpecError> {
    let Some(value) = wrapper
        .params
        .get("tools")
        .or_else(|| wrapper.params.get("tool"))
        .or_else(|| wrapper.params.get("deferred_tools"))
    else {
        return Ok(vec!["*".to_string()]);
    };
    match value {
        Value::String(tool) if !tool.trim().is_empty() => Ok(vec![tool.trim().to_string()]),
        Value::Array(values) => {
            let mut tools = Vec::new();
            for value in values {
                let Some(tool) = value
                    .as_str()
                    .map(str::trim)
                    .filter(|tool| !tool.is_empty())
                else {
                    return Err(AgentSpecError::InvalidToolsetWrapper {
                        kind: wrapper.kind.clone(),
                        reason: "deferred_tools must contain non-empty strings".to_string(),
                    });
                };
                tools.push(tool.to_string());
            }
            if tools.is_empty() {
                return Err(AgentSpecError::InvalidToolsetWrapper {
                    kind: wrapper.kind.clone(),
                    reason: "deferred_tools must not be empty".to_string(),
                });
            }
            Ok(tools)
        }
        _ => Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: "deferred_tools must be a string or string array".to_string(),
        }),
    }
}

fn optional_string_param(
    wrapper: &ToolsetWrapperSpec,
    key: &str,
) -> Result<Option<String>, AgentSpecError> {
    let Some(value) = wrapper.params.get(key) else {
        return Ok(None);
    };
    let Some(value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: format!("{key} must be a non-empty string"),
        });
    };
    Ok(Some(value.to_string()))
}

fn optional_usize_param(
    wrapper: &ToolsetWrapperSpec,
    key: &str,
) -> Result<Option<usize>, AgentSpecError> {
    let Some(value) = wrapper.params.get(key) else {
        return Ok(None);
    };
    let Some(value) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
        return Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: format!("{key} must be a non-negative integer"),
        });
    };
    Ok(Some(value))
}

fn optional_string_list_param(
    wrapper: &ToolsetWrapperSpec,
    key: &str,
) -> Result<Option<Vec<String>>, AgentSpecError> {
    let Some(value) = wrapper.params.get(key) else {
        return Ok(None);
    };
    string_list_value(wrapper, key, value).map(Some)
}

fn string_list_value(
    wrapper: &ToolsetWrapperSpec,
    key: &str,
    value: &Value,
) -> Result<Vec<String>, AgentSpecError> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Ok(vec![value.trim().to_string()]),
        Value::Array(values) => {
            let mut strings = Vec::new();
            for value in values {
                let Some(value) = value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    return Err(AgentSpecError::InvalidToolsetWrapper {
                        kind: wrapper.kind.clone(),
                        reason: format!("{key} must contain non-empty strings"),
                    });
                };
                strings.push(value.to_string());
            }
            if strings.is_empty() {
                return Err(AgentSpecError::InvalidToolsetWrapper {
                    kind: wrapper.kind.clone(),
                    reason: format!("{key} must not be empty"),
                });
            }
            Ok(strings)
        }
        _ => Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: format!("{key} must be a string or string array"),
        }),
    }
}

fn string_map_param(
    wrapper: &ToolsetWrapperSpec,
    key: &str,
) -> Result<BTreeMap<String, String>, AgentSpecError> {
    let Some(value) = wrapper.params.get(key) else {
        return Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: format!("missing {key}"),
        });
    };
    let Some(object) = value.as_object() else {
        return Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: format!("{key} must be an object"),
        });
    };
    let mut mappings = BTreeMap::new();
    for (from, to) in object {
        let Some(to) = to.as_str().map(str::trim).filter(|value| !value.is_empty()) else {
            return Err(AgentSpecError::InvalidToolsetWrapper {
                kind: wrapper.kind.clone(),
                reason: format!("{key} values must be non-empty strings"),
            });
        };
        mappings.insert(from.clone(), to.to_string());
    }
    if mappings.is_empty() {
        return Err(AgentSpecError::InvalidToolsetWrapper {
            kind: wrapper.kind.clone(),
            reason: format!("{key} must not be empty"),
        });
    }
    Ok(mappings)
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
