//! CLI `AgentSpec` profile resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
};

use serde::Serialize;
use serde_json::json;
use starweaver_agent::{
    AgentBuilder, AgentSpec, AgentSpecRegistry, ApprovalRequiredToolset,
    BackgroundSubagentSupervisor, DynToolset, HostMediaUnderstandingClient, ShellReviewAction,
    ShellReviewConfig, ShellReviewHandle, ShellReviewRiskLevel, SkillPackage, SkillRegistry,
    StaticToolset, SubagentConfig, SubagentDelegationMode, SubagentToolInheritancePolicy,
    core_toolsets, load_subagents_from_dir, parse_skill_markdown,
};
use starweaver_model::{
    HttpModelConfig, ModelAdapter, ModelProfile, ModelSettings, OpenAiResponsesSettings,
    ProtocolFamily, ProtocolModelClient, ProviderSettings, ReqwestHttpClient,
    ResponseStreamTransport, anthropic_http_config, build_codex_model_with_profile,
    codex_model_profile, gemini_http_config, get_model_config, get_model_settings,
    google_cloud_http_config, google_cloud_project_http_config, openai_chat_http_config,
    openai_responses_http_config,
};

use crate::{
    CliError, CliResult,
    config::{CliConfig, ProviderConfig, tool_need_approval},
    error::io_error,
    oauth::{CODEX_BASE_URL, OAuthStore, create_codex_token_source},
};
use starweaver_context::{AgentContext, ModelCapability, ModelConfig};

mod builtin;
mod catalog;
mod local_runtime;
mod mcp;
mod media;

use builtin::{builtin_spec, config_model_spec};
pub use catalog::{
    doctor_mcp_servers, list_config_model_profiles, list_default_tools, list_mcp_servers,
    list_profiles, list_skills, list_subagents, show_mcp_server, show_profile, show_skill,
    show_subagent,
};
#[cfg(test)]
use local_runtime::capture_subagent_inheritance_model;
use local_runtime::{control_flow_toolset, local_echo_model, scripted_tool_model};
use mcp::{configured_mcp_proxy_toolset, configured_mcp_server_specs, mcp_transport_error};
use media::{CliMediaUnderstandingCapability, configured_media_client};

/// Resolved CLI profile ready to build an agent session.
pub struct ResolvedProfile {
    /// Profile name.
    pub name: String,
    /// Profile source.
    pub source: ProfileSource,
    /// Parsed spec.
    pub spec: AgentSpec,
    /// Registry with CLI-provided models and toolsets.
    pub registry: AgentSpecRegistry,
    /// Optional CLI media-understanding fallback client.
    pub media_client: Option<Arc<dyn HostMediaUnderstandingClient>>,
    /// Optional shell command review handle.
    pub shell_review: Option<ShellReviewHandle>,
    /// Skill registry loaded from CLI skill directories.
    pub skills: SkillRegistry,
}

/// Source of a resolved profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileSource {
    /// Built-in profile bundled with the CLI.
    BuiltIn,
    /// Profile synthesized from CLI config.
    Config,
    /// Profile loaded from YAML.
    File(PathBuf),
}

impl ProfileSource {
    /// Render a source comment for `profile show`.
    #[must_use]
    pub fn render_comment(&self) -> String {
        match self {
            Self::BuiltIn => "# source: built-in\n".to_string(),
            Self::Config => "# source: config\n".to_string(),
            Self::File(path) => format!("# source: {}\n", path.display()),
        }
    }

    /// Return source kind for durable CLI session records.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Config => "config",
            Self::File(_) => "file",
        }
    }

    /// Return source path for durable CLI session records.
    #[must_use]
    pub fn path(&self) -> Option<String> {
        match self {
            Self::File(path) => Some(path.display().to_string()),
            Self::BuiltIn | Self::Config => None,
        }
    }
}

impl ResolvedProfile {
    /// Build the runtime agent from the profile.
    #[allow(dead_code)]
    pub fn build_agent(&self) -> CliResult<starweaver_runtime::Agent> {
        self.build_agent_with_delegation(SubagentDelegationMode::Blocking, None)
    }

    pub(crate) fn build_agent_with_delegation(
        &self,
        mode: SubagentDelegationMode,
        supervisor: Option<Arc<BackgroundSubagentSupervisor>>,
    ) -> CliResult<starweaver_runtime::Agent> {
        let mut builder = self
            .spec
            .builder(&self.registry)
            .map_err(|error| CliError::Config(error.to_string()))?
            .subagent_delegation_mode(mode);
        if let Some(supervisor) = supervisor {
            builder = builder.background_subagent_supervisor(supervisor);
        }
        if let Some(client) = self.media_client.as_ref() {
            builder = builder.capability(Arc::new(CliMediaUnderstandingCapability::new(
                client.clone(),
            )));
        }
        Ok(builder.build())
    }

    /// Apply profile-owned runtime context defaults to a session context.
    pub fn configure_context(&self, context: &mut AgentContext) {
        self.skills.register_relaxed_view_patterns(context);
    }
}

/// Profile listing item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProfileSummary {
    /// Profile name.
    pub name: String,
    /// Optional human label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Profile source kind.
    pub source: String,
    /// Default model id.
    pub model_id: String,
    /// Model settings preset name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<String>,
    /// Model config preset name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_cfg: Option<String>,
    /// Context window in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Profile path for file-backed profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Configured skill catalog item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SkillSummary {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Skill path.
    pub path: String,
}

/// Configured subagent catalog item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SubagentSummary {
    /// Subagent name.
    pub name: String,
    /// Subagent description.
    pub description: String,
    /// Model id or inherit marker.
    pub model: String,
    /// Source file path.
    pub path: String,
    /// Required parent tools.
    pub tools: Vec<String>,
    /// Optional parent tools.
    pub optional_tools: Vec<String>,
}

/// Configured MCP server catalog item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpSummary {
    /// Server name.
    pub name: String,
    /// Transport kind.
    pub transport: String,
    /// Server config metadata.
    pub config: serde_json::Value,
}

/// MCP doctor finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpDoctorFinding {
    /// Server name.
    pub name: String,
    /// Validation status.
    pub status: String,
    /// Transport kind.
    pub transport: String,
    /// Validation error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// First-party tool catalog item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolSummary {
    /// Tool name exposed to the model.
    pub name: String,
    /// Toolset name.
    pub toolset: String,
    /// Tool description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tool metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    /// Whether config marks the tool as approval-gated.
    pub approval_configured: bool,
}

/// Resolve an `AgentSpec` by name or YAML path.
pub fn resolve_profile(config: &CliConfig, requested: Option<&str>) -> CliResult<ResolvedProfile> {
    let requested = requested.unwrap_or(&config.default_profile);
    let (spec, source) = load_profile_spec(config, requested)?;
    let name = spec.name.clone();
    let registry = default_registry(config, &spec)?;
    let skills = configured_skill_registry(config);
    let media_client = configured_media_client(config)?;
    let shell_review = configured_shell_review(config)?;
    Ok(ResolvedProfile {
        name,
        source,
        spec,
        registry,
        media_client,
        shell_review,
        skills,
    })
}

fn load_profile_spec(config: &CliConfig, requested: &str) -> CliResult<(AgentSpec, ProfileSource)> {
    if requested == "default_model"
        && let Some(profile) = config.default_model.as_ref()
    {
        return Ok((
            config_model_spec("default_model", profile),
            ProfileSource::Config,
        ));
    }
    if let Some(profile) = config.model_profiles.get(requested) {
        return Ok((config_model_spec(requested, profile), ProfileSource::Config));
    }
    if let Some(spec) = builtin_spec(requested) {
        return Ok((spec, ProfileSource::BuiltIn));
    }
    if let Some(path) = find_profile_path(config, requested) {
        let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
        let spec =
            AgentSpec::from_yaml(&content).map_err(|error| CliError::Config(error.to_string()))?;
        return Ok((spec, ProfileSource::File(path)));
    }
    Err(CliError::NotFound(format!("profile {requested}")))
}

fn find_profile_path(config: &CliConfig, requested: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(requested);
    if direct.exists() {
        return Some(direct);
    }
    let candidates = [
        config.project_dir.join("profiles").join(requested),
        config
            .project_dir
            .join("profiles")
            .join(format!("{requested}.yaml")),
        config
            .project_dir
            .join("profiles")
            .join(format!("{requested}.yml")),
        config.global_dir.join("profiles").join(requested),
        config
            .global_dir
            .join("profiles")
            .join(format!("{requested}.yaml")),
        config
            .global_dir
            .join("profiles")
            .join(format!("{requested}.yml")),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn default_registry(config: &CliConfig, spec: &AgentSpec) -> CliResult<AgentSpecRegistry> {
    let mut registry = AgentSpecRegistry::new()
        .with_model("local_echo", Arc::new(local_echo_model()))
        .with_model(
            "approval_model",
            Arc::new(scripted_tool_model("approval_probe")),
        )
        .with_model(
            "deferred_model",
            Arc::new(scripted_tool_model("deferred_probe")),
        );
    let mut environment_filesystem = None;
    let mut environment_shell = None;
    for toolset in default_toolsets(config) {
        let name = toolset.name().to_string();
        registry = registry.with_toolset(toolset.clone());
        match name.as_str() {
            "filesystem" => {
                environment_filesystem = Some(toolset.clone());
                registry = registry.with_toolset_alias("filesystem", toolset);
            }
            "shell" => {
                environment_shell = Some(toolset.clone());
                registry = registry.with_toolset_alias("shell", toolset);
            }
            "task" => {
                registry = registry.with_toolset_alias("task", toolset);
            }
            "skills" => {
                registry = registry.with_toolset_alias("skills", toolset);
            }
            other if other.starts_with("mcp_") => {
                registry = registry.with_toolset_alias(other, toolset);
            }
            _ => {}
        }
    }
    if let (Some(filesystem), Some(shell)) = (environment_filesystem, environment_shell) {
        registry =
            registry.with_toolset_alias("environment", environment_toolset(&filesystem, &shell));
    }
    for server in configured_mcp_server_specs(config) {
        registry = registry.with_mcp_server(server.name.clone(), server);
    }
    let inherited_model_id = spec_model_id(spec).unwrap_or("local_echo");
    let inherited_model = spec.model.as_ref().or(spec.preset.model.as_ref());
    let inherited_model_settings = resolve_inherited_model_settings(inherited_model)?;
    let inherited_model_config_preset =
        inherited_model.and_then(|model| model.config_preset.as_deref());
    let inherited_model_config = resolve_inherited_model_config(inherited_model_config_preset)?;
    if let Some(model) = provider_model(config, inherited_model_id, inherited_model_config_preset)?
    {
        registry = registry.with_model(inherited_model_id, model);
    }
    registry = register_configured_subagents(
        config,
        registry,
        inherited_model_id,
        inherited_model_settings.as_ref(),
        inherited_model_config.as_ref(),
        inherited_model_config_preset,
    )?;
    Ok(registry)
}

fn default_toolsets(config: &CliConfig) -> Vec<DynToolset> {
    let approval = tool_need_approval(config);
    let shell_review_approval = shell_review_adjusted_approval(config, &approval);
    let mut toolsets = vec![Arc::new(control_flow_toolset()) as DynToolset];
    for toolset in core_toolsets() {
        let selected_approval = if toolset.name() == "shell" {
            &shell_review_approval
        } else {
            &approval
        };
        toolsets.push(policy_toolset(toolset, selected_approval));
    }
    if let Some(skill_toolset) = configured_skill_toolset(config) {
        toolsets.push(policy_toolset(skill_toolset, &approval));
    }
    if let Some(mcp_proxy) = configured_mcp_proxy_toolset(config) {
        toolsets.push(policy_toolset(mcp_proxy, &approval));
    }
    toolsets
}

fn configured_shell_review(config: &CliConfig) -> CliResult<Option<ShellReviewHandle>> {
    if !config.shell_review.enabled {
        return Ok(None);
    }
    let model_id = config.shell_review.model.as_deref().ok_or_else(|| {
        CliError::Config(
            "security.shell_review.model is required when shell review is enabled".to_string(),
        )
    })?;
    let model = match model_id {
        "local_echo" => Arc::new(local_echo_model()) as Arc<dyn ModelAdapter>,
        other => provider_model(config, other, None)?
            .ok_or_else(|| CliError::Config(format!("unknown shell review model id {other}")))?,
    };
    let mut review_config = ShellReviewConfig::enabled(model)
        .with_action(shell_review_action(&config.shell_review.on_needs_approval)?)
        .with_risk_threshold(shell_review_risk(&config.shell_review.risk_threshold)?);
    if let Some(settings) = config.shell_review.model_settings.as_deref() {
        review_config = review_config.with_model_settings(
            get_model_settings(settings).map_err(|error| CliError::Config(error.to_string()))?,
        );
    }
    if let Some(prompt) = config.shell_review.system_prompt.as_ref() {
        review_config = review_config.with_system_prompt(prompt.clone());
    }
    Ok(Some(ShellReviewHandle::new(review_config)))
}

fn shell_review_action(value: &str) -> CliResult<ShellReviewAction> {
    match value {
        "defer" => Ok(ShellReviewAction::Defer),
        "deny" => Ok(ShellReviewAction::Deny),
        other => Err(CliError::Config(format!(
            "invalid security.shell_review.on_needs_approval: {other}"
        ))),
    }
}

fn shell_review_risk(value: &str) -> CliResult<ShellReviewRiskLevel> {
    match value {
        "low" => Ok(ShellReviewRiskLevel::Low),
        "medium" => Ok(ShellReviewRiskLevel::Medium),
        "high" => Ok(ShellReviewRiskLevel::High),
        "extra_high" => Ok(ShellReviewRiskLevel::ExtraHigh),
        other => Err(CliError::Config(format!(
            "invalid security.shell_review.risk_threshold: {other}"
        ))),
    }
}

fn environment_toolset(filesystem: &DynToolset, shell: &DynToolset) -> DynToolset {
    Arc::new(
        StaticToolset::new("environment")
            .with_id("environment")
            .with_tools(filesystem.get_tools())
            .with_tools(shell.get_tools())
            .with_instructions(filesystem.get_instructions())
            .with_instructions(shell.get_instructions()),
    )
}

fn configured_skill_registry(config: &CliConfig) -> SkillRegistry {
    let mut registry = SkillRegistry::new();
    for dir in &config.skill_dirs {
        for package in load_skill_packages_from_dir(dir) {
            registry.insert(package);
        }
    }
    registry
}

fn configured_skill_toolset(config: &CliConfig) -> Option<DynToolset> {
    let registry = configured_skill_registry(config);
    (!registry.is_empty()).then(|| registry.toolset())
}

fn shell_review_adjusted_approval(config: &CliConfig, approval: &[String]) -> Vec<String> {
    if !config.shell_review.enabled {
        return approval.to_vec();
    }
    approval
        .iter()
        .filter(|entry| entry.as_str() != "shell" && entry.as_str() != "shell_exec")
        .cloned()
        .collect()
}

fn policy_toolset(inner: DynToolset, approval: &[String]) -> DynToolset {
    let name = inner.name().to_string();
    let id = inner.id().map(str::to_string);
    let mut wrapper = ApprovalRequiredToolset::new(inner, approval.iter().cloned()).with_name(name);
    if let Some(id) = id {
        wrapper = wrapper.with_id(id);
    }
    Arc::new(wrapper)
}

fn load_skill_packages_from_dir(dir: &std::path::Path) -> Vec<SkillPackage> {
    let mut packages = Vec::new();
    if !dir.exists() {
        return packages;
    }
    let direct = dir.join("SKILL.md");
    if let Some(package) = load_skill_package(&direct) {
        packages.push(package);
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return packages;
    };
    for entry in entries.flatten() {
        let path = entry.path().join("SKILL.md");
        if let Some(package) = load_skill_package(&path) {
            packages.push(package);
        }
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    packages.dedup_by(|left, right| left.name == right.name);
    packages
}

fn load_skill_package(path: &std::path::Path) -> Option<SkillPackage> {
    let content = fs::read_to_string(path).ok()?;
    let provider_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    parse_skill_markdown(&provider_path.display().to_string(), &content).ok()
}

fn register_configured_subagents(
    config: &CliConfig,
    mut registry: AgentSpecRegistry,
    inherited_model_id: &str,
    inherited_model_settings: Option<&ModelSettings>,
    inherited_model_config: Option<&ModelConfig>,
    inherited_model_config_preset: Option<&str>,
) -> CliResult<AgentSpecRegistry> {
    for dir in &config.subagent_dirs {
        if !dir.exists() {
            continue;
        }
        let specs = load_subagents_from_dir(dir)
            .map_err(|error| CliError::Config(format!("failed to load subagents: {error}")))?;
        for spec in specs {
            if config.disabled_subagents.contains(&spec.name) {
                continue;
            }
            registry = registry.with_subagent(build_subagent_config(
                config,
                &spec,
                inherited_model_id,
                inherited_model_settings,
                inherited_model_config,
                inherited_model_config_preset,
            )?);
        }
    }
    Ok(registry)
}

fn build_subagent_config(
    config: &CliConfig,
    spec: &starweaver_core::SubagentSpec,
    inherited_model_id: &str,
    inherited_model_settings: Option<&ModelSettings>,
    inherited_model_config: Option<&ModelConfig>,
    inherited_model_config_preset: Option<&str>,
) -> CliResult<SubagentConfig> {
    let model_id = match spec.model.as_deref() {
        Some("inherit") | None => inherited_model_id,
        Some(model_id) => model_id,
    };
    let model_config = resolve_subagent_model_config(
        spec.model_config.as_ref(),
        inherited_model_config,
        inherited_model_config_preset,
    )?;
    let model = subagent_model(config, model_id, model_config.preset.as_deref())?;
    let mut agent = AgentBuilder::new(model).instruction(spec.system_prompt.clone());
    if let Some(settings) =
        resolve_subagent_model_settings(spec.model_settings.as_ref(), inherited_model_settings)?
    {
        agent = agent.model_settings(settings);
    }
    if let Some(model_config) = model_config.context {
        agent = agent.model_config(model_config);
    }
    let agent = agent.build();
    let denied_tools = spec
        .metadata
        .get("denied_tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let inheritance =
        SubagentToolInheritancePolicy::new(spec.tools.clone(), spec.optional_tools.clone())
            .with_denied_tools(denied_tools)
            .with_inherit_all_when_empty(spec.tools.is_empty() && spec.optional_tools.is_empty());
    Ok(SubagentConfig::new(spec.name.clone(), Arc::new(agent))
        .with_description(spec.description.clone())
        .with_tool_inheritance(inheritance))
}

struct ResolvedSubagentModelConfig {
    context: Option<ModelConfig>,
    preset: Option<String>,
}

fn resolve_inherited_model_settings(
    model: Option<&starweaver_agent::ModelPreset>,
) -> CliResult<Option<ModelSettings>> {
    let Some(model) = model else {
        return Ok(None);
    };
    let preset_settings = model
        .settings_preset
        .as_deref()
        .map(get_model_settings)
        .transpose()
        .map_err(|error| CliError::Config(error.to_string()))?;
    Ok(match (preset_settings, model.settings.clone()) {
        (Some(base), Some(overlay)) => Some(base.merge(&overlay)),
        (Some(base), None) => Some(base),
        (None, Some(settings)) => Some(settings),
        (None, None) => None,
    })
}

fn resolve_inherited_model_config(
    model_config_preset: Option<&str>,
) -> CliResult<Option<ModelConfig>> {
    model_config_preset
        .map(|preset| {
            get_model_config(preset)
                .map(|config| context_model_config_from_preset(&config))
                .map_err(|error| CliError::Config(error.to_string()))
        })
        .transpose()
}

fn resolve_subagent_model_settings(
    value: Option<&serde_json::Value>,
    inherited: Option<&ModelSettings>,
) -> CliResult<Option<ModelSettings>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(inherited.cloned()),
        Some(serde_json::Value::String(value)) if value == "inherit" => Ok(inherited.cloned()),
        Some(serde_json::Value::String(preset)) => get_model_settings(preset)
            .map(Some)
            .map_err(|error| CliError::Config(error.to_string())),
        Some(serde_json::Value::Object(_)) => serde_json::from_value::<ModelSettings>(
            value.cloned().unwrap_or(serde_json::Value::Null),
        )
        .map(Some)
        .map_err(|error| CliError::Config(format!("invalid subagent model_settings: {error}"))),
        Some(other) => Err(CliError::Config(format!(
            "invalid subagent model_settings: expected 'inherit', preset name, or object, got {other}"
        ))),
    }
}

fn resolve_subagent_model_config(
    value: Option<&serde_json::Value>,
    inherited: Option<&ModelConfig>,
    inherited_preset: Option<&str>,
) -> CliResult<ResolvedSubagentModelConfig> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(ResolvedSubagentModelConfig {
            context: inherited.cloned(),
            preset: inherited_preset.map(str::to_string),
        }),
        Some(serde_json::Value::String(value)) if value == "inherit" => {
            Ok(ResolvedSubagentModelConfig {
                context: inherited.cloned(),
                preset: inherited_preset.map(str::to_string),
            })
        }
        Some(serde_json::Value::String(preset)) => get_model_config(preset)
            .map(|config| ResolvedSubagentModelConfig {
                context: Some(context_model_config_from_preset(&config)),
                preset: Some(preset.clone()),
            })
            .map_err(|error| CliError::Config(error.to_string())),
        Some(serde_json::Value::Object(_)) => {
            serde_json::from_value::<ModelConfig>(value.cloned().unwrap_or(serde_json::Value::Null))
                .map(|context| ResolvedSubagentModelConfig {
                    context: Some(context),
                    preset: None,
                })
                .map_err(|error| CliError::Config(format!("invalid subagent model_cfg: {error}")))
        }
        Some(other) => Err(CliError::Config(format!(
            "invalid subagent model_cfg: expected 'inherit', preset name, or object, got {other}"
        ))),
    }
}

fn context_model_config_from_preset(
    config: &starweaver_model::ModelConfigPresetData,
) -> ModelConfig {
    let mut capabilities = BTreeSet::new();
    if config.profile.supports_image_input {
        capabilities.insert(ModelCapability::Vision);
    }
    if config.profile.supports_video_input {
        capabilities.insert(ModelCapability::VideoUnderstanding);
    }
    if config.profile.supports_audio_input {
        capabilities.insert(ModelCapability::AudioUnderstanding);
    }
    if config.profile.supports_document_input {
        capabilities.insert(ModelCapability::DocumentUnderstanding);
    }
    ModelConfig {
        context_window: Some(u64::from(config.context_window)),
        max_images: usize::try_from(config.max_images).unwrap_or(usize::MAX),
        max_videos: usize::try_from(config.max_videos).unwrap_or(usize::MAX),
        support_gif: config.supports_gif,
        split_large_images: config.split_large_images,
        image_split_max_height: usize::try_from(config.image_split_max_height)
            .unwrap_or(usize::MAX),
        image_split_overlap: usize::try_from(config.image_split_overlap).unwrap_or(usize::MAX),
        capabilities,
        ..ModelConfig::default()
    }
}

fn subagent_model(
    config: &CliConfig,
    model_id: &str,
    model_config_preset: Option<&str>,
) -> CliResult<Arc<dyn starweaver_model::ModelAdapter>> {
    match model_id {
        "local_echo" => Ok(Arc::new(local_echo_model())),
        "approval_model" => Ok(Arc::new(scripted_tool_model("approval_probe"))),
        "deferred_model" => Ok(Arc::new(scripted_tool_model("deferred_probe"))),
        #[cfg(test)]
        "capture_subagent_inheritance" => Ok(Arc::new(capture_subagent_inheritance_model())),
        other => provider_model(config, other, model_config_preset)?
            .ok_or_else(|| CliError::Config(format!("unknown subagent model id {other}"))),
    }
}

fn provider_model(
    config: &CliConfig,
    model_id: &str,
    model_config_preset: Option<&str>,
) -> CliResult<Option<Arc<dyn starweaver_model::ModelAdapter>>> {
    let Some(parsed) = ProviderModelId::parse(model_id) else {
        return Ok(None);
    };
    if parsed.oauth_provider.as_deref() == Some("codex") {
        let codex_config = &config.providers.codex;
        let mut http_config = HttpModelConfig::new(CODEX_BASE_URL, "responses");
        apply_provider_http_config_overrides(&mut http_config, codex_config);
        http_config
            .metadata
            .insert("oauth_provider".to_string(), json!("codex"));
        let token_source =
            create_codex_token_source(Some(OAuthStore::default_store())).map_err(|error| {
                CliError::Config(format!("failed to build Codex OAuth token source: {error}"))
            })?;
        let mut profile =
            provider_model_profile(model_config_preset, ProtocolFamily::OpenAiResponses)?;
        let codex_profile = codex_model_profile();
        profile.supports_thinking = codex_profile.supports_thinking;
        profile.thinking_always_enabled = codex_profile.thinking_always_enabled;
        let client = build_codex_model_with_profile(
            parsed.model_name,
            Arc::new(token_source),
            http_config,
            BTreeMap::new(),
            profile,
        )
        .map_err(|error| CliError::Config(format!("failed to build Codex OAuth model: {error}")))?;
        return Ok(Some(Arc::new(client)));
    }
    let provider_config = parsed.provider_config(config);
    if !provider_config.enabled {
        return Err(CliError::Config(format!(
            "provider {} is disabled for model id {model_id}",
            parsed.provider
        )));
    }
    let mut http_config = match parsed.protocol {
        ProtocolFamily::OpenAiResponses => openai_responses_http_config(provider_api_key(
            config,
            &provider_config,
            &parsed,
            model_id,
        )?),
        ProtocolFamily::OpenAiChatCompletions => openai_chat_http_config(provider_api_key(
            config,
            &provider_config,
            &parsed,
            model_id,
        )?),
        ProtocolFamily::AnthropicMessages => anthropic_http_config(provider_api_key(
            config,
            &provider_config,
            &parsed,
            model_id,
        )?),
        ProtocolFamily::GeminiGenerateContent => {
            google_http_config(config, &provider_config, &parsed, model_id)?
        }
        ProtocolFamily::BedrockConverse => return Ok(None),
    };
    apply_provider_http_config_overrides(&mut http_config, &provider_config);
    let profile = provider_model_profile(model_config_preset, parsed.protocol)?;
    let mut client = ProtocolModelClient::new(
        parsed.provider,
        parsed.model_name,
        profile,
        http_config,
        Arc::new(ReqwestHttpClient::new().map_err(|error| CliError::Config(error.to_string()))?),
    );
    if let Some(stream_transport) = parsed.stream_transport {
        client = client
            .with_default_settings(openai_responses_stream_transport_settings(stream_transport));
    }
    Ok(Some(Arc::new(client)))
}

fn provider_api_key(
    config: &CliConfig,
    provider_config: &ProviderConfig,
    parsed: &ProviderModelId,
    model_id: &str,
) -> CliResult<String> {
    let api_key_env = provider_config
        .api_key_env
        .as_deref()
        .unwrap_or_else(|| parsed.default_api_key_env())
        .trim()
        .to_string();
    if api_key_env.is_empty() {
        return Err(CliError::Config(format!(
            "empty api_key_env for provider {} and model id {model_id}",
            parsed.provider
        )));
    }
    let api_key = config
        .env_value(&api_key_env)
        .ok_or_else(|| missing_provider_key(&api_key_env, model_id))?;
    if api_key.trim().is_empty() {
        return Err(missing_provider_key(&api_key_env, model_id));
    }
    Ok(api_key)
}

fn provider_auth_token(
    config: &CliConfig,
    provider_config: &ProviderConfig,
    parsed: &ProviderModelId,
    model_id: &str,
) -> CliResult<String> {
    let token_env = provider_config
        .auth_token_env
        .as_deref()
        .unwrap_or("GOOGLE_CLOUD_ACCESS_TOKEN")
        .trim()
        .to_string();
    if token_env.is_empty() {
        return Err(CliError::Config(format!(
            "empty auth_token_env for provider {} and model id {model_id}",
            parsed.provider
        )));
    }
    let token = config
        .env_value(&token_env)
        .ok_or_else(|| missing_provider_token(&token_env, model_id))?;
    if token.trim().is_empty() {
        return Err(missing_provider_token(&token_env, model_id));
    }
    Ok(token)
}

fn google_http_config(
    config: &CliConfig,
    provider_config: &ProviderConfig,
    parsed: &ProviderModelId,
    model_id: &str,
) -> CliResult<HttpModelConfig> {
    if parsed.provider == "google-cloud" {
        let project = provider_config
            .project
            .as_deref()
            .map(str::trim)
            .filter(|project| !project.is_empty());
        if let Some(project) = project {
            let location = provider_config
                .location
                .as_deref()
                .map(str::trim)
                .filter(|location| !location.is_empty())
                .unwrap_or("us-central1");
            return Ok(google_cloud_project_http_config(
                provider_auth_token(config, provider_config, parsed, model_id)?,
                parsed.model_name.clone(),
                project,
                location,
            ));
        }
        return Ok(google_cloud_http_config(
            provider_api_key(config, provider_config, parsed, model_id)?,
            parsed.model_name.clone(),
        ));
    }
    Ok(gemini_http_config(
        provider_api_key(config, provider_config, parsed, model_id)?,
        parsed.model_name.clone(),
    ))
}

fn apply_provider_http_config_overrides(
    http_config: &mut HttpModelConfig,
    provider_config: &ProviderConfig,
) {
    if let Some(base_url) = provider_config.base_url.as_ref() {
        http_config.set_base_url(base_url);
    }
    if let Some(endpoint_path) = provider_config.endpoint_path.as_ref() {
        http_config.set_endpoint_path(endpoint_path);
    }
    http_config.max_tokens_parameter = provider_config.max_tokens_parameter;
}

fn openai_responses_stream_transport_settings(
    stream_transport: ResponseStreamTransport,
) -> ModelSettings {
    ModelSettings {
        provider_settings: ProviderSettings {
            openai_responses: Some(OpenAiResponsesSettings {
                stream_transport: Some(stream_transport),
                ..OpenAiResponsesSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    }
}

struct ProviderModelId {
    provider: String,
    model_name: String,
    protocol: ProtocolFamily,
    gateway_name: Option<String>,
    oauth_provider: Option<String>,
    stream_transport: Option<ResponseStreamTransport>,
}

impl ProviderModelId {
    fn parse(model_id: &str) -> Option<Self> {
        let (prefix, model_name) = model_id.split_once(':')?;
        if model_name.trim().is_empty() {
            return None;
        }
        let (gateway_name, provider_prefix) = prefix
            .split_once('@')
            .map_or((None, prefix), |(left, right)| (Some(left), right));
        if gateway_name == Some("oauth") {
            return (provider_prefix == "codex").then(|| Self {
                provider: "codex".to_string(),
                model_name: model_name.to_string(),
                protocol: ProtocolFamily::OpenAiResponses,
                gateway_name: None,
                oauth_provider: Some("codex".to_string()),
                stream_transport: None,
            });
        }
        let (provider, protocol, stream_transport) = match provider_prefix {
            "openai" | "openai-responses" => ("openai", ProtocolFamily::OpenAiResponses, None),
            "openai-responses-ws" => (
                "openai",
                ProtocolFamily::OpenAiResponses,
                Some(ResponseStreamTransport::Auto),
            ),
            "openai-chat" => ("openai", ProtocolFamily::OpenAiChatCompletions, None),
            "anthropic" | "claude" => ("anthropic", ProtocolFamily::AnthropicMessages, None),
            "gemini" => ("gemini", ProtocolFamily::GeminiGenerateContent, None),
            "google" | "google-gla" => ("google", ProtocolFamily::GeminiGenerateContent, None),
            "google-cloud" | "google-vertex" => {
                ("google-cloud", ProtocolFamily::GeminiGenerateContent, None)
            }
            _ => return None,
        };
        Some(Self {
            provider: provider.to_string(),
            model_name: model_name.to_string(),
            protocol,
            gateway_name: gateway_name.map(str::to_string),
            oauth_provider: None,
            stream_transport,
        })
    }

    fn provider_config(&self, config: &CliConfig) -> ProviderConfig {
        if let Some(gateway_name) = self.gateway_name.as_ref() {
            let env_prefix = gateway_name.to_ascii_uppercase().replace('-', "_");
            let base_url_env = format!("{env_prefix}_BASE_URL");
            let mut provider = config
                .providers
                .gateways
                .get(gateway_name)
                .cloned()
                .unwrap_or_default();
            if provider.api_key_env.is_none() {
                provider.api_key_env = Some(format!("{env_prefix}_API_KEY"));
            }
            if provider.base_url.is_none() {
                provider.base_url = config.env_value(&base_url_env);
            }
            return provider;
        }
        match self.provider.as_str() {
            "openai" => config.providers.openai.clone(),
            "anthropic" => config.providers.anthropic.clone(),
            "gemini" | "google" => config.providers.gemini.clone(),
            "google-cloud" => config.providers.google_cloud.clone(),
            _ => ProviderConfig::default(),
        }
    }

    fn default_api_key_env(&self) -> &'static str {
        match self.provider.as_str() {
            "openai" => "OPENAI_API_KEY",
            "anthropic" => "ANTHROPIC_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            "google" | "google-cloud" => "GOOGLE_API_KEY",
            _ => "STARWEAVER_API_KEY",
        }
    }
}

fn provider_model_profile(
    model_config_preset: Option<&str>,
    fallback_protocol: ProtocolFamily,
) -> CliResult<ModelProfile> {
    let Some(preset) = model_config_preset else {
        return Ok(ModelProfile::for_protocol(fallback_protocol));
    };
    let config = get_model_config(preset).map_err(|error| CliError::Config(error.to_string()))?;
    Ok(config.profile)
}

fn missing_provider_key(api_key_env: &str, model_id: &str) -> CliError {
    CliError::Config(format!(
        "missing {api_key_env} for model id {model_id}; run `starweaver-cli config init --global` and export the provider API key"
    ))
}

fn missing_provider_token(token_env: &str, model_id: &str) -> CliError {
    CliError::Config(format!(
        "missing {token_env} for model id {model_id}; export a Google Cloud bearer access token or remove the provider project setting to use API-key Express Mode"
    ))
}

fn spec_model_id(spec: &AgentSpec) -> Option<&str> {
    spec.model
        .as_ref()
        .or(spec.preset.model.as_ref())
        .map(|model| model.model_id.as_str())
}

fn profile_model_id(spec: &AgentSpec) -> String {
    spec_model_id(spec).map_or_else(|| "<missing>".to_string(), ToString::to_string)
}

fn profile_model_settings(spec: &AgentSpec) -> Option<&str> {
    spec.model
        .as_ref()
        .or(spec.preset.model.as_ref())
        .and_then(|model| model.settings_preset.as_deref())
}

fn profile_model_cfg(spec: &AgentSpec) -> Option<&str> {
    spec.model
        .as_ref()
        .or(spec.preset.model.as_ref())
        .and_then(|model| model.config_preset.as_deref())
}

fn profile_context_window(spec: &AgentSpec) -> Option<u64> {
    profile_model_cfg(spec).and_then(model_context_window)
}

fn model_context_window(model_cfg: &str) -> Option<u64> {
    get_model_config(model_cfg)
        .ok()
        .map(|config| u64::from(config.context_window))
}

#[cfg(test)]
mod tests;
