//! CLI `AgentSpec` profile resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
    sync::Arc,
};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Map};
use starweaver_agent::{
    core_toolsets, load_subagents_from_dir, parse_skill_markdown, string_tool, AgentBuilder,
    AgentCapability, AgentRunState, AgentSpec, AgentSpecRegistry, ApprovalRequiredToolset,
    CapabilityResult, DynToolset, FunctionModel, HostMediaUnderstandingClient,
    HostMediaUnderstandingClientHandle, McpServerSpec, McpToolSpec, McpToolset, McpToolsetConfig,
    McpTransport, MediaUnderstandingRequest, MediaUnderstandingResponse, ShellReviewAction,
    ShellReviewConfig, ShellReviewHandle, ShellReviewRiskLevel, SkillPackage, SkillRegistry,
    StaticToolset, SubagentConfig, SubagentToolInheritancePolicy, ToolContext, ToolError,
    ToolProxyToolset, ToolResult,
};
use starweaver_model::{
    anthropic_http_config, build_codex_model_with_profile, codex_model_profile, gemini_http_config,
    get_model_config, get_model_settings, openai_chat_http_config, openai_responses_http_config,
    ContentPart, HttpModelConfig, ModelAdapter, ModelMessage, ModelProfile, ModelRequest,
    ModelRequestContext, ModelRequestParameters, ModelRequestPart, ModelResponse,
    ModelResponsePart, ProtocolFamily, ProtocolModelClient, ReqwestHttpClient, ToolCallPart,
};

use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, RunId};

use crate::{
    config::{mcp_servers, tool_need_approval, CliConfig, CliModelProfile, ProviderConfig},
    error::io_error,
    oauth::{create_codex_token_source, OAuthStore, CODEX_BASE_URL},
    CliError, CliResult,
};

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
    pub fn build_agent(&self) -> CliResult<starweaver_runtime::Agent> {
        let mut builder = self
            .spec
            .builder(&self.registry)
            .map_err(|error| CliError::Config(error.to_string()))?;
        if let Some(client) = self.media_client.as_ref() {
            builder = builder.capability(Arc::new(CliMediaUnderstandingCapability {
                handle: HostMediaUnderstandingClientHandle::new(client.clone()),
            }));
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

/// List built-in and configured profiles.
pub fn list_profiles(config: &CliConfig) -> Vec<ProfileSummary> {
    let mut profiles = BTreeMap::<String, ProfileSummary>::new();
    for (name, spec) in builtin_profile_specs() {
        profiles.insert(
            name.to_string(),
            ProfileSummary {
                name: name.to_string(),
                label: None,
                source: ProfileSource::BuiltIn.kind().to_string(),
                model_id: profile_model_id(&spec),
                model_settings: profile_model_settings(&spec).map(ToString::to_string),
                model_cfg: profile_model_cfg(&spec).map(ToString::to_string),
                context_window: profile_context_window(&spec),
                path: None,
            },
        );
    }
    for profile in list_config_model_profiles(config) {
        profiles.insert(profile.name.clone(), profile);
    }
    profiles.into_values().collect()
}

/// List model profiles explicitly configured in config.toml.
///
/// This is the source used by client model selectors such as the TUI `/model`
/// picker. Built-in SDK profiles are intentionally omitted so local/test models
/// only appear when the user explicitly adds them to config.
pub fn list_config_model_profiles(config: &CliConfig) -> Vec<ProfileSummary> {
    let mut profiles = Vec::new();
    let default_profile = config
        .model_profiles
        .get("default_model")
        .or(config.default_model.as_ref());
    if let Some(profile) = default_profile {
        profiles.push(config_model_profile_summary("default_model", profile));
    }
    profiles.extend(
        config
            .model_profiles
            .iter()
            .filter(|(name, _)| name.as_str() != "default_model")
            .map(|(name, profile)| config_model_profile_summary(name, profile)),
    );
    profiles
}

fn config_model_profile_summary(name: &str, profile: &CliModelProfile) -> ProfileSummary {
    ProfileSummary {
        name: name.to_string(),
        label: profile.label.clone(),
        source: ProfileSource::Config.kind().to_string(),
        model_id: profile.model_id.clone(),
        model_settings: profile.model_settings.clone(),
        model_cfg: profile.model_cfg.clone(),
        context_window: profile.model_cfg.as_deref().and_then(model_context_window),
        path: None,
    }
}

/// Render a built-in or file-backed profile as YAML.
pub fn show_profile(config: &CliConfig, requested: &str) -> CliResult<String> {
    let (spec, source) = load_profile_spec(config, requested)?;
    let mut yaml =
        serde_yaml::to_string(&spec).map_err(|error| CliError::Config(error.to_string()))?;
    yaml.push_str(&source.render_comment());
    Ok(yaml)
}

/// List configured skills.
pub fn list_skills(config: &CliConfig) -> Vec<SkillSummary> {
    let mut packages = BTreeMap::new();
    for dir in &config.skill_dirs {
        for package in load_skill_packages_from_dir(dir) {
            packages.insert(package.name.clone(), package);
        }
    }
    packages
        .into_values()
        .map(|package| SkillSummary {
            name: package.name,
            description: package.description,
            path: package.path,
        })
        .collect()
}

/// Show one configured skill package.
pub fn show_skill(config: &CliConfig, name: &str) -> CliResult<String> {
    for dir in &config.skill_dirs {
        for package in load_skill_packages_from_dir(dir) {
            if package.name == name {
                let summary = package.summary_line();
                return Ok(package.body.unwrap_or_else(|| format!("{summary}\n")));
            }
        }
    }
    Err(CliError::NotFound(format!("skill {name}")))
}

/// List configured subagents.
pub fn list_subagents(config: &CliConfig) -> Vec<SubagentSummary> {
    let mut summaries = BTreeMap::new();
    for dir in &config.subagent_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|extension| extension == "md") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(spec) = starweaver_agent::parse_subagent_markdown(&content) else {
                continue;
            };
            if config.disabled_subagents.contains(&spec.name) {
                continue;
            }
            summaries.insert(
                spec.name.clone(),
                SubagentSummary {
                    name: spec.name,
                    description: spec.description,
                    model: spec.model.unwrap_or_else(|| "inherit".to_string()),
                    path: path.display().to_string(),
                    tools: spec.tools,
                    optional_tools: spec.optional_tools,
                },
            );
        }
    }
    summaries.into_values().collect()
}

/// Show one configured subagent markdown file.
pub fn show_subagent(config: &CliConfig, name: &str) -> CliResult<String> {
    for dir in &config.subagent_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|extension| extension == "md") {
                continue;
            }
            let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
            let Ok(spec) = starweaver_agent::parse_subagent_markdown(&content) else {
                continue;
            };
            if spec.name == name {
                return Ok(content);
            }
        }
    }
    Err(CliError::NotFound(format!("subagent {name}")))
}

/// List configured MCP servers.
pub fn list_mcp_servers(config: &CliConfig) -> Vec<McpSummary> {
    mcp_servers(config)
        .into_iter()
        .map(|(name, value)| McpSummary {
            transport: value
                .get("transport")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("stdio")
                .to_string(),
            name,
            config: value,
        })
        .collect()
}

/// Show one configured MCP server JSON object.
pub fn show_mcp_server(config: &CliConfig, name: &str) -> CliResult<String> {
    mcp_servers(config)
        .remove(name)
        .ok_or_else(|| CliError::NotFound(format!("mcp server {name}")))
        .and_then(|value| serde_json::to_string_pretty(&value).map_err(CliError::from))
        .map(|json| format!("{json}\n"))
}

/// Validate configured MCP servers.
pub fn doctor_mcp_servers(config: &CliConfig) -> Vec<McpDoctorFinding> {
    mcp_servers(config)
        .into_iter()
        .map(|(name, value)| {
            let transport = value
                .get("transport")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("stdio")
                .to_string();
            match mcp_transport_error(&value) {
                Some(error) => McpDoctorFinding {
                    name,
                    status: "error".to_string(),
                    transport,
                    error: Some(error),
                },
                None => McpDoctorFinding {
                    name,
                    status: "ok".to_string(),
                    transport,
                    error: None,
                },
            }
        })
        .collect()
}

/// List default first-party CLI tools.
pub fn list_default_tools(config: &CliConfig) -> Vec<ToolSummary> {
    let approval = tool_need_approval(config)
        .into_iter()
        .collect::<BTreeSet<_>>();
    default_toolsets(config)
        .into_iter()
        .flat_map(|toolset| {
            let toolset_name = toolset
                .id()
                .map_or_else(|| toolset.name().to_string(), str::to_string);
            let approval = approval.clone();
            toolset.get_tools().into_iter().map(move |tool| {
                let metadata = tool.metadata();
                let approval_configured = approval.iter().any(|entry| {
                    entry == tool.name()
                        || entry == &toolset_name
                        || metadata
                            .get("bundle")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|bundle| entry == bundle)
                });
                ToolSummary {
                    name: tool.name().to_string(),
                    toolset: toolset_name.clone(),
                    description: tool.description().map(str::to_string),
                    metadata,
                    approval_configured,
                }
            })
        })
        .collect()
}

fn load_profile_spec(config: &CliConfig, requested: &str) -> CliResult<(AgentSpec, ProfileSource)> {
    if requested == "default_model" {
        if let Some(profile) = config.default_model.as_ref() {
            return Ok((
                config_model_spec("default_model", profile),
                ProfileSource::Config,
            ));
        }
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

fn builtin_profile_specs() -> Vec<(&'static str, AgentSpec)> {
    vec![
        ("general", default_spec("general")),
        ("default", default_spec("default")),
        ("coding", coding_spec()),
        ("research", research_spec()),
        ("workspace", workspace_spec()),
        (
            "approval_model",
            scripted_spec("approval_model", "approval_model"),
        ),
        (
            "deferred_model",
            scripted_spec("deferred_model", "deferred_model"),
        ),
    ]
}

fn builtin_spec(name: &str) -> Option<AgentSpec> {
    match name {
        "general" | "default" => Some(default_spec(name)),
        "coding" => Some(coding_spec()),
        "research" => Some(research_spec()),
        "workspace" => Some(workspace_spec()),
        "approval_model" => Some(scripted_spec("approval_model", "approval_model")),
        "deferred_model" => Some(scripted_spec("deferred_model", "deferred_model")),
        _ => None,
    }
}

fn default_spec(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec!["You are Starweaver CLI, a helpful local assistant.".to_string()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "local_echo".to_string(),
            settings_preset: None,
            config_preset: None,
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn coding_spec() -> AgentSpec {
    AgentSpec {
        name: "coding".to_string(),
        instructions: vec![
            "You are Starweaver CLI, a coding assistant focused on concise implementation help."
                .to_string(),
        ],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "openai:gpt-5".to_string(),
            settings_preset: Some("openai_responses_medium".to_string()),
            config_preset: Some("gpt5_270k".to_string()),
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn research_spec() -> AgentSpec {
    AgentSpec {
        name: "research".to_string(),
        instructions: vec![
            "You are Starweaver CLI, a research assistant that cites evidence and tracks assumptions."
                .to_string(),
        ],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "anthropic:claude-sonnet-4-5".to_string(),
            settings_preset: Some("anthropic_default".to_string()),
            config_preset: Some("claude_200k".to_string()),
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn workspace_spec() -> AgentSpec {
    AgentSpec {
        name: "workspace".to_string(),
        instructions: vec![
            "You are Starweaver CLI, a workspace assistant with file and shell tools governed by local policy."
                .to_string(),
        ],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "local_echo".to_string(),
            settings_preset: None,
            config_preset: None,
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn config_model_spec(name: &str, profile: &CliModelProfile) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec!["You are Starweaver CLI, a helpful local assistant.".to_string()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: normalize_model_id(&profile.model_id),
            settings_preset: profile.model_settings.clone(),
            config_preset: profile.model_cfg.clone(),
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn normalize_model_id(model_id: &str) -> String {
    model_id.trim().to_string()
}

fn scripted_spec(name: &str, model_id: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec!["Exercise CLI control-flow handling deterministically.".to_string()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: model_id.to_string(),
            settings_preset: None,
            config_preset: None,
            settings: None,
        }),
        toolsets: vec!["cli_control_flow".to_string()],
        ..AgentSpec::default()
    }
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
    for toolset in default_toolsets(config) {
        let name = toolset.name().to_string();
        registry = registry.with_toolset(toolset.clone());
        match name.as_str() {
            "filesystem" => {
                registry = registry.with_toolset_alias("filesystem", toolset.clone());
                registry = registry.with_toolset_alias("environment", toolset);
            }
            "shell" => {
                registry = registry.with_toolset_alias("shell", toolset);
            }
            "host_operations" => {
                registry = registry.with_toolset_alias("tools", toolset);
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
    for server in configured_mcp_server_specs(config) {
        registry = registry.with_mcp_server(server.name.clone(), server);
    }
    let inherited_model_id = spec_model_id(spec).unwrap_or("local_echo");
    let inherited_model_config = spec
        .model
        .as_ref()
        .and_then(|model| model.config_preset.as_deref());
    if let Some(model) = provider_model(config, inherited_model_id, inherited_model_config)? {
        registry = registry.with_model(inherited_model_id, model);
    }
    registry = register_configured_subagents(config, registry, inherited_model_id)?;
    Ok(registry)
}

fn default_toolsets(config: &CliConfig) -> Vec<DynToolset> {
    let approval = tool_need_approval(config);
    let shell_review_approval = shell_review_adjusted_approval(config, &approval);
    let mut toolsets = vec![Arc::new(control_flow_toolset()) as DynToolset];
    let mut filesystem = None;
    let mut shell = None;
    for toolset in core_toolsets() {
        match toolset.name() {
            "filesystem" => filesystem = Some(toolset.clone()),
            "shell" => shell = Some(toolset.clone()),
            _ => {}
        }
        let selected_approval = if toolset.name() == "shell" {
            &shell_review_approval
        } else {
            &approval
        };
        toolsets.push(policy_toolset(toolset, selected_approval));
    }
    if let (Some(filesystem), Some(shell)) = (filesystem, shell) {
        toolsets.push(policy_toolset(
            environment_toolset(&filesystem, &shell),
            &shell_review_approval,
        ));
    }
    toolsets.push(policy_toolset(configured_skill_toolset(config), &approval));
    if let Some(mcp_proxy) = configured_mcp_proxy_toolset(config) {
        toolsets.push(policy_toolset(mcp_proxy, &approval));
    }
    toolsets
}

#[derive(Clone)]
struct CliMediaUnderstandingCapability {
    handle: HostMediaUnderstandingClientHandle,
}

#[async_trait]
impl AgentCapability for CliMediaUnderstandingCapability {
    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        tool_context: &mut ToolContext,
        _call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        tool_context.dependencies.insert(self.handle.clone());
        Ok(())
    }
}

#[derive(Clone)]
struct CliMediaUnderstandingClient {
    image: Option<CliMediaUnderstandingModel>,
    video: Option<CliMediaUnderstandingModel>,
    audio: Option<CliMediaUnderstandingModel>,
}

#[derive(Clone)]
struct CliMediaUnderstandingModel {
    model_id: String,
    model: Arc<dyn ModelAdapter>,
}

#[async_trait]
impl HostMediaUnderstandingClient for CliMediaUnderstandingClient {
    async fn understand(
        &self,
        request: MediaUnderstandingRequest,
    ) -> Result<MediaUnderstandingResponse, String> {
        let selected = match request.media_kind.as_str() {
            "image" => self.image.as_ref(),
            "video" => self.video.as_ref(),
            "audio" => self.audio.as_ref(),
            other => return Err(format!("unsupported media kind {other}")),
        }
        .ok_or_else(|| {
            format!(
                "missing fallback model for {} understanding",
                request.media_kind
            )
        })?;
        let response = selected
            .model
            .request_stream_final(
                vec![ModelMessage::Request(media_understanding_request(&request))],
                None,
                ModelRequestParameters::default(),
                ModelRequestContext::new(RunId::new(), ConversationId::new()),
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut content = response.text_output();
        if content.trim().is_empty() {
            content = "Media understanding model returned no text output.".to_string();
        }
        Ok(MediaUnderstandingResponse {
            success: true,
            media_kind: request.media_kind,
            url: request.url,
            model_id: selected.model_id.clone(),
            content,
            truncated: false,
            metadata: Map::new(),
        })
    }
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

fn configured_media_client(
    config: &CliConfig,
) -> CliResult<Option<Arc<dyn HostMediaUnderstandingClient>>> {
    let image = configured_media_model(config, "STARWEAVER_IMAGE_UNDERSTANDING_MODEL")?;
    let video = configured_media_model(config, "STARWEAVER_VIDEO_UNDERSTANDING_MODEL")?;
    let audio = configured_media_model(config, "STARWEAVER_AUDIO_UNDERSTANDING_MODEL")?;
    if image.is_none() && video.is_none() && audio.is_none() {
        return Ok(None);
    }
    Ok(Some(Arc::new(CliMediaUnderstandingClient {
        image,
        video,
        audio,
    })))
}

fn configured_media_model(
    config: &CliConfig,
    env_name: &str,
) -> CliResult<Option<CliMediaUnderstandingModel>> {
    let Some(model_id) = env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let model = match model_id.as_str() {
        "local_echo" => Arc::new(local_echo_model()) as Arc<dyn ModelAdapter>,
        other => provider_model(config, other, None)?.ok_or_else(|| {
            CliError::Config(format!("unknown media understanding model id {other}"))
        })?,
    };
    Ok(Some(CliMediaUnderstandingModel { model_id, model }))
}

fn media_understanding_request(request: &MediaUnderstandingRequest) -> ModelRequest {
    let source_line = if request.url.starts_with("data:") {
        "Source: attached inline media data URL content part".to_string()
    } else {
        format!("URL: {}", request.url)
    };
    let prompt = request
        .instructions
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || {
                format!(
                    "Analyze this {kind} for the Starweaver CLI user. Return concise, useful observations.\n\n{source_line}",
                    kind = request.media_kind,
                )
            },
            |instructions| {
                format!(
                    "Analyze this {kind} for the Starweaver CLI user. Return concise, useful observations.\n\nFocused instructions:\n{instructions}\n\n{source_line}",
                    kind = request.media_kind,
                )
            },
        );
    let mut content = vec![ContentPart::Text { text: prompt }];
    content.push(match request.media_kind.as_str() {
        "image" => ContentPart::ImageUrl {
            url: request.url.clone(),
        },
        "video" => ContentPart::FileUrl {
            url: request.url.clone(),
            media_type: "video/*".to_string(),
        },
        "audio" => ContentPart::FileUrl {
            url: request.url.clone(),
            media_type: "audio/*".to_string(),
        },
        _ => ContentPart::FileUrl {
            url: request.url.clone(),
            media_type: "application/octet-stream".to_string(),
        },
    });
    ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content,
            name: Some("media_understanding".to_string()),
            metadata: Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
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

fn configured_skill_toolset(config: &CliConfig) -> DynToolset {
    configured_skill_registry(config).toolset()
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

fn configured_mcp_server_specs(config: &CliConfig) -> Vec<McpServerSpec> {
    mcp_servers(config)
        .into_iter()
        .filter_map(|(name, value)| {
            let transport = parse_mcp_transport(&value)?;
            let metadata = value.as_object().cloned().unwrap_or_default();
            Some(McpServerSpec {
                name,
                transport: transport.kind().to_string(),
                metadata,
            })
        })
        .collect()
}

fn configured_mcp_proxy_toolset(config: &CliConfig) -> Option<DynToolset> {
    let mut descriptions = BTreeMap::new();
    let toolsets = configured_mcp_toolsets(config, &mut descriptions);
    if toolsets.is_empty() {
        return None;
    }
    let proxy = ToolProxyToolset::new(toolsets)
        .try_with_prefix("mcp")
        .ok()?
        .with_namespace_descriptions(descriptions);
    Some(Arc::new(proxy))
}

fn configured_mcp_toolsets(
    config: &CliConfig,
    descriptions: &mut BTreeMap<String, String>,
) -> Vec<DynToolset> {
    mcp_servers(config)
        .into_iter()
        .filter_map(|(name, value)| {
            let transport = parse_mcp_transport(&value)?;
            let toolset_id = format!("mcp_{name}");
            let mut toolset_config = McpToolsetConfig::new(toolset_id.clone(), transport);
            if let Some(prefix) = value.get("tool_prefix").and_then(serde_json::Value::as_str) {
                toolset_config = toolset_config.with_tool_prefix(prefix);
            }
            if value
                .get("include_instructions")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                toolset_config = toolset_config.with_include_instructions(true);
            }
            if let Some(instructions) = value
                .get("instructions")
                .and_then(serde_json::Value::as_str)
            {
                toolset_config = toolset_config.with_instructions(instructions);
            }
            let description = mcp_namespace_description(&value);
            for tool in parse_mcp_tools(&value) {
                toolset_config = toolset_config.with_tool(tool);
            }
            if let Some(description) = description {
                descriptions.insert(toolset_id, description);
            }
            Some(Arc::new(McpToolset::new(toolset_config)) as DynToolset)
        })
        .collect()
}

fn mcp_namespace_description(value: &serde_json::Value) -> Option<String> {
    value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("instructions")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
}

fn mcp_transport_error(value: &serde_json::Value) -> Option<String> {
    let transport = value
        .get("transport")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio");
    match transport {
        "stdio" => value
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| command.trim().is_empty())
            .map_or_else(
                || Some("stdio transport requires command".to_string()),
                |empty| empty.then(|| "stdio transport requires command".to_string()),
            ),
        "streamable_http" | "http" | "sse" => value
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(|url| url.trim().is_empty())
            .map_or_else(
                || Some(format!("{transport} transport requires url")),
                |empty| empty.then(|| format!("{transport} transport requires url")),
            ),
        other => Some(format!("unknown MCP transport {other}")),
    }
}

fn parse_mcp_transport(value: &serde_json::Value) -> Option<McpTransport> {
    let transport = value
        .get("transport")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio");
    match transport {
        "stdio" => {
            let command = value.get("command").and_then(serde_json::Value::as_str)?;
            let args = value
                .get("args")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            let mut parsed = McpTransport::stdio(command).with_args(args);
            if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
                parsed = parsed.with_cwd(cwd);
            }
            if let Some(env) = value.get("env").and_then(serde_json::Value::as_object) {
                parsed = parsed.with_env(env.clone());
            }
            Some(parsed)
        }
        "streamable_http" | "http" => {
            let url = value.get("url").and_then(serde_json::Value::as_str)?;
            let mut parsed = McpTransport::streamable_http(url);
            if let Some(headers) = value.get("headers").and_then(serde_json::Value::as_object) {
                parsed = parsed.with_headers(headers.clone());
            }
            Some(parsed)
        }
        "sse" => {
            let url = value.get("url").and_then(serde_json::Value::as_str)?;
            let mut parsed = McpTransport::sse(url);
            if let Some(headers) = value.get("headers").and_then(serde_json::Value::as_object) {
                parsed = parsed.with_headers(headers.clone());
            }
            Some(parsed)
        }
        _ => None,
    }
}

fn parse_mcp_tools(value: &serde_json::Value) -> Vec<McpToolSpec> {
    value
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(serde_json::Value::as_str)?;
            let parameters = tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            let mut spec = McpToolSpec::new(name, parameters);
            if let Some(description) = tool.get("description").and_then(serde_json::Value::as_str) {
                spec = spec.with_description(description);
            }
            if let Some(task) = tool.get("task").and_then(serde_json::Value::as_bool) {
                spec = spec.with_task(task);
            }
            if let Some(metadata) = tool.get("metadata").and_then(serde_json::Value::as_object) {
                spec = spec.with_metadata(metadata.clone());
            }
            Some(spec)
        })
        .collect()
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
            registry =
                registry.with_subagent(build_subagent_config(config, &spec, inherited_model_id)?);
        }
    }
    Ok(registry)
}

fn build_subagent_config(
    config: &CliConfig,
    spec: &starweaver_core::SubagentSpec,
    inherited_model_id: &str,
) -> CliResult<SubagentConfig> {
    let model_id = match spec.model.as_deref() {
        Some("inherit") | None => inherited_model_id,
        Some(model_id) => model_id,
    };
    let model = subagent_model(config, model_id)?;
    let agent = AgentBuilder::new(model)
        .instruction(spec.system_prompt.clone())
        .build();
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

fn subagent_model(
    config: &CliConfig,
    model_id: &str,
) -> CliResult<Arc<dyn starweaver_model::ModelAdapter>> {
    match model_id {
        "local_echo" => Ok(Arc::new(local_echo_model())),
        "approval_model" => Ok(Arc::new(scripted_tool_model("approval_probe"))),
        "deferred_model" => Ok(Arc::new(scripted_tool_model("deferred_probe"))),
        other => provider_model(config, other, None)?
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
        let mut http_config = HttpModelConfig::new(
            codex_config.base_url.as_deref().unwrap_or(CODEX_BASE_URL),
            codex_config.endpoint_path.as_deref().unwrap_or("responses"),
        );
        http_config.max_tokens_parameter = codex_config.max_tokens_parameter;
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
    let api_key =
        env::var(&api_key_env).map_err(|_| missing_provider_key(&api_key_env, model_id))?;
    if api_key.trim().is_empty() {
        return Err(missing_provider_key(&api_key_env, model_id));
    }
    let mut http_config = match parsed.protocol {
        ProtocolFamily::OpenAiResponses => openai_responses_http_config(api_key),
        ProtocolFamily::OpenAiChatCompletions => openai_chat_http_config(api_key),
        ProtocolFamily::AnthropicMessages => anthropic_http_config(api_key),
        ProtocolFamily::GeminiGenerateContent => {
            gemini_http_config(api_key, parsed.model_name.clone())
        }
        ProtocolFamily::BedrockConverse => return Ok(None),
    };
    if let Some(base_url) = provider_config.base_url.as_ref() {
        http_config.base_url.clone_from(base_url);
    }
    if let Some(endpoint_path) = provider_config.endpoint_path.as_ref() {
        http_config.endpoint_path.clone_from(endpoint_path);
    }
    http_config.max_tokens_parameter = provider_config.max_tokens_parameter;
    let profile = provider_model_profile(model_config_preset, parsed.protocol)?;
    let client = ProtocolModelClient::new(
        parsed.provider,
        parsed.model_name,
        profile,
        http_config,
        Arc::new(ReqwestHttpClient::new().map_err(|error| CliError::Config(error.to_string()))?),
    );
    Ok(Some(Arc::new(client)))
}

struct ProviderModelId {
    provider: String,
    model_name: String,
    protocol: ProtocolFamily,
    gateway_name: Option<String>,
    oauth_provider: Option<String>,
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
            });
        }
        let protocol = match provider_prefix {
            "openai" | "openai-responses" => ProtocolFamily::OpenAiResponses,
            "openai-chat" => ProtocolFamily::OpenAiChatCompletions,
            "anthropic" | "claude" => ProtocolFamily::AnthropicMessages,
            "gemini" | "google" | "google-vertex" | "google-cloud" | "google-gla" => {
                ProtocolFamily::GeminiGenerateContent
            }
            _ => return None,
        };
        let provider = match protocol {
            ProtocolFamily::OpenAiResponses | ProtocolFamily::OpenAiChatCompletions => "openai",
            ProtocolFamily::AnthropicMessages => "anthropic",
            ProtocolFamily::GeminiGenerateContent => "gemini",
            ProtocolFamily::BedrockConverse => "bedrock",
        };
        Some(Self {
            provider: provider.to_string(),
            model_name: model_name.to_string(),
            protocol,
            gateway_name: gateway_name.map(str::to_string),
            oauth_provider: None,
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
                provider.base_url = env::var(&base_url_env).ok();
            }
            return provider;
        }
        match self.provider.as_str() {
            "openai" => config.providers.openai.clone(),
            "anthropic" => config.providers.anthropic.clone(),
            "gemini" => config.providers.gemini.clone(),
            _ => ProviderConfig::default(),
        }
    }

    fn default_api_key_env(&self) -> &'static str {
        match self.provider.as_str() {
            "openai" => "OPENAI_API_KEY",
            "anthropic" => "ANTHROPIC_API_KEY",
            "gemini" => "GEMINI_API_KEY",
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

fn local_echo_model() -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
        let prompt = latest_user_prompt(&messages).unwrap_or_default();
        Ok(ModelResponse::text(format!("local echo: {prompt}")))
    })
}

fn scripted_tool_model(tool_name: &'static str) -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
        if messages.iter().any(message_has_tool_return) {
            return Ok(ModelResponse::text(format!("{tool_name} handled")));
        }
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: format!("{tool_name}_call"),
                name: tool_name.to_string(),
                arguments: json!({"action": tool_name}).into(),
            })],
            usage: starweaver_core::Usage::default(),
            model_name: Some(tool_name.to_string()),
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::default(),
        })
    })
}

fn latest_user_prompt(messages: &[ModelMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().rev().find_map(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => {
                content.iter().find_map(|part| match part {
                    starweaver_model::ContentPart::Text { text } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        }),
        ModelMessage::Response(_) => None,
    })
}

fn message_has_tool_return(message: &ModelMessage) -> bool {
    match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_))),
        ModelMessage::Response(_) => false,
    }
}

fn control_flow_toolset() -> StaticToolset {
    let approval_tool = string_tool(
        "approval_probe",
        Some("Deterministic approval probe".to_string()),
        json!({"type": "object"}),
        |_context, arguments| async move {
            Err(ToolError::ApprovalRequired {
                tool: "approval_probe".to_string(),
                metadata: json!({"arguments": arguments, "reason": "cli approval probe"}),
            })
        },
    );
    let deferred_tool = string_tool(
        "deferred_probe",
        Some("Deterministic deferred-call probe".to_string()),
        json!({"type": "object"}),
        |_context, arguments| async move {
            Err(ToolError::CallDeferred {
                tool: "deferred_probe".to_string(),
                metadata: json!({"arguments": arguments, "reason": "cli deferred probe"}),
            })
        },
    );
    StaticToolset::new("cli_control_flow")
        .with_id("cli_control_flow")
        .with_tool(Arc::new(approval_tool))
        .with_tool(Arc::new(deferred_tool))
}

#[allow(dead_code)]
fn ok_tool_result(value: serde_json::Value) -> ToolResult {
    ToolResult::new(value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::config::ConfigResolver;

    fn test_config() -> CliConfig {
        let temp = tempfile::tempdir().unwrap();
        let cli =
            crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();
        ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap()
    }

    #[test]
    fn shell_review_adjusted_approval_removes_shell_entries_only_when_enabled() {
        let mut config = test_config();
        let approval = vec![
            "shell".to_string(),
            "shell_exec".to_string(),
            "write".to_string(),
            "*".to_string(),
        ];

        assert_eq!(shell_review_adjusted_approval(&config, &approval), approval);

        config.shell_review.enabled = true;
        assert_eq!(
            shell_review_adjusted_approval(&config, &approval),
            vec!["write".to_string(), "*".to_string()]
        );
    }

    #[test]
    fn resolve_profile_builds_configured_shell_review_handle() {
        let mut config = test_config();
        config.shell_review.enabled = true;
        config.shell_review.model = Some("local_echo".to_string());
        config.shell_review.on_needs_approval = "deny".to_string();
        config.shell_review.risk_threshold = "medium".to_string();
        config.shell_review.system_prompt = Some("Custom shell review prompt".to_string());

        let profile = resolve_profile(&config, Some("general")).unwrap();
        let Some(handle) = profile.shell_review else {
            panic!("shell review handle");
        };

        assert!(handle.config().enabled);
        assert_eq!(handle.config().on_needs_approval, ShellReviewAction::Deny);
        assert_eq!(handle.config().risk_threshold, ShellReviewRiskLevel::Medium);
        assert_eq!(
            handle.config().system_prompt.as_deref(),
            Some("Custom shell review prompt")
        );
        assert!(handle.config().model.is_some());
    }
}
