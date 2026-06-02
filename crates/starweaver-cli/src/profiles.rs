//! CLI `AgentSpec` profile resolution.

use std::{collections::BTreeMap, env, fs, path::PathBuf, sync::Arc};

use serde::Serialize;
use serde_json::json;
use starweaver_agent::{
    environment_toolsets, load_subagents_from_dir, parse_skill_markdown, skill_tools, string_tool,
    AgentBuilder, AgentSpec, AgentSpecRegistry, FunctionModel, SkillPackage, StaticToolset,
    SubagentConfig, SubagentToolInheritancePolicy, ToolError, ToolResult,
};
use starweaver_model::{
    anthropic_http_config, gemini_http_config, openai_chat_http_config,
    openai_responses_http_config, HttpModelConfig, ModelMessage, ModelProfile, ModelRequestPart,
    ModelResponse, ModelResponsePart, ProtocolFamily, ProtocolModelClient, ReqwestHttpClient,
    ToolCallPart,
};

use crate::{
    config::{CliConfig, CliModelProfile, ProviderConfig},
    error::io_error,
    oauth::{CodexOAuthHttpClient, CODEX_BASE_URL},
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
        self.spec
            .builder(&self.registry)
            .map_err(|error| CliError::Config(error.to_string()))
            .map(starweaver_agent::AgentBuilder::build)
    }
}

/// Profile listing item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProfileSummary {
    /// Profile name.
    pub name: String,
    /// Profile source kind.
    pub source: String,
    /// Default model id.
    pub model_id: String,
    /// Profile path for file-backed profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Resolve an `AgentSpec` by name or YAML path.
pub fn resolve_profile(config: &CliConfig, requested: Option<&str>) -> CliResult<ResolvedProfile> {
    let requested = requested.unwrap_or(&config.default_profile);
    let (spec, source) = load_profile_spec(config, requested)?;
    let name = spec.name.clone();
    let registry = default_registry(config, &spec)?;
    Ok(ResolvedProfile {
        name,
        source,
        spec,
        registry,
    })
}

/// List built-in and configured profiles.
pub fn list_profiles(config: &CliConfig) -> CliResult<Vec<ProfileSummary>> {
    let mut profiles = builtin_profile_specs()
        .into_iter()
        .map(|(name, spec)| ProfileSummary {
            name: name.to_string(),
            source: ProfileSource::BuiltIn.kind().to_string(),
            model_id: profile_model_id(&spec),
            path: None,
        })
        .collect::<Vec<_>>();
    if let Some(profile) = config.default_model.as_ref() {
        profiles.push(ProfileSummary {
            name: "default_model".to_string(),
            source: ProfileSource::Config.kind().to_string(),
            model_id: profile.model_id.clone(),
            path: None,
        });
    }
    profiles.extend(
        config
            .model_profiles
            .iter()
            .map(|(name, profile)| ProfileSummary {
                name: name.clone(),
                source: ProfileSource::Config.kind().to_string(),
                model_id: profile.model_id.clone(),
                path: None,
            }),
    );
    for root in &config.profile_search_paths {
        if !root.exists() {
            continue;
        }
        for entry in fs::read_dir(root).map_err(|error| io_error(root, error))? {
            let entry = entry.map_err(|error| io_error(root, error))?;
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                continue;
            };
            if !matches!(extension, "yaml" | "yml") {
                continue;
            }
            let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
            let spec = AgentSpec::from_yaml(&content)
                .map_err(|error| CliError::Config(error.to_string()))?;
            profiles.push(ProfileSummary {
                name: spec.name.clone(),
                source: ProfileSource::File(path.clone()).kind().to_string(),
                model_id: profile_model_id(&spec),
                path: Some(path.display().to_string()),
            });
        }
    }
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

/// Render a built-in or file-backed profile as YAML.
pub fn show_profile(config: &CliConfig, requested: &str) -> CliResult<String> {
    let (spec, source) = load_profile_spec(config, requested)?;
    let mut yaml =
        serde_yaml::to_string(&spec).map_err(|error| CliError::Config(error.to_string()))?;
    yaml.push_str(&source.render_comment());
    Ok(yaml)
}

fn load_profile_spec(config: &CliConfig, requested: &str) -> CliResult<(AgentSpec, ProfileSource)> {
    if let Some(spec) = builtin_spec(requested) {
        return Ok((spec, ProfileSource::BuiltIn));
    }
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
    config.profile_search_paths.iter().find_map(|root| {
        [
            root.join(requested),
            root.join(format!("{requested}.yaml")),
            root.join(format!("{requested}.yml")),
        ]
        .into_iter()
        .find(|path| path.exists())
    })
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
        toolsets: vec!["cli_control_flow".to_string(), "environment".to_string()],
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
        toolsets: vec![
            "environment".to_string(),
            "filesystem".to_string(),
            "shell".to_string(),
        ],
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
        toolsets: vec!["environment".to_string()],
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
        toolsets: vec!["environment".to_string(), "filesystem".to_string(), "shell".to_string()],
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
        toolsets: vec!["cli_control_flow".to_string(), "environment".to_string()],
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
        )
        .with_toolset_alias("cli_control_flow", Arc::new(control_flow_toolset()));
    for toolset in environment_toolsets() {
        let name = toolset.name().to_string();
        registry = registry.with_toolset(toolset.clone());
        registry = registry.with_toolset_alias("environment", toolset.clone());
        match name.as_str() {
            "file" | "files" | "filesystem" | "environment_file" => {
                registry = registry.with_toolset_alias("filesystem", toolset);
            }
            "shell" | "environment_shell" => {
                registry = registry.with_toolset_alias("shell", toolset);
            }
            _ => {}
        }
    }
    registry = registry.with_toolset_alias("skills", configured_skill_toolset(config));
    let inherited_model_id = spec_model_id(spec).unwrap_or("local_echo");
    if let Some(model) = provider_model(config, inherited_model_id)? {
        registry = registry.with_model(inherited_model_id, model);
    }
    registry = register_configured_subagents(config, registry, inherited_model_id)?;
    Ok(registry)
}

fn configured_skill_toolset(config: &CliConfig) -> starweaver_tools::DynToolset {
    let mut packages = BTreeMap::new();
    for dir in &config.skill_dirs {
        for package in load_skill_packages_from_dir(dir) {
            packages.insert(package.name.clone(), package);
        }
    }
    skill_tools(packages.into_values())
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
    parse_skill_markdown(&path.display().to_string(), &content).ok()
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
            .with_denied_tools(denied_tools);
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
        other => provider_model(config, other)?
            .ok_or_else(|| CliError::Config(format!("unknown subagent model id {other}"))),
    }
}

fn provider_model(
    config: &CliConfig,
    model_id: &str,
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
        let client = ProtocolModelClient::new(
            "codex",
            parsed.model_name,
            ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
            http_config,
            Arc::new(CodexOAuthHttpClient::new().map_err(|error| {
                CliError::Config(format!("failed to build Codex OAuth client: {error}"))
            })?),
        );
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
    let client = ProtocolModelClient::new(
        parsed.provider,
        parsed.model_name,
        ModelProfile::for_protocol(parsed.protocol),
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
                arguments: json!({"action": tool_name}),
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
