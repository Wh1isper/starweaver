use std::fs;

use starweaver_model::MaxTokensParameter;
use toml::Value;

use super::{
    CliConfig, ProviderConfig, parse_hitl_policy, parse_output_mode, validate_shell_review_action,
    validate_shell_review_risk,
};
use crate::{
    CliError, CliResult,
    args::{HitlPolicy, OutputMode, TuiRenderMode},
    error::io_error,
};

/// Config write scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigScope {
    /// Global config file.
    Global,
    /// Project config file.
    Project,
}

/// Set a config value.
pub fn set_config_value(
    config: &CliConfig,
    scope: ConfigScope,
    key: &str,
    value: &str,
) -> CliResult<()> {
    let parsed_value = parse_config_value(key, value)?;
    let root_dir = match scope {
        ConfigScope::Global => &config.global_dir,
        ConfigScope::Project => &config.project_dir,
    };
    let path = root_dir.join("config.toml");
    fs::create_dir_all(root_dir).map_err(|error| io_error(root_dir, error))?;
    let mut root = if path.exists() {
        let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
        content
            .parse::<Value>()
            .map_err(|error| CliError::Config(error.to_string()))?
            .as_table()
            .cloned()
            .unwrap_or_default()
    } else {
        toml::map::Map::new()
    };
    if let Some(field) = key.strip_prefix("security.shell_review.") {
        let security_root = root
            .entry("security".to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let security_root_table = security_root
            .as_table_mut()
            .ok_or_else(|| CliError::Usage("config section security is not a table".to_string()))?;
        let shell_review = security_root_table
            .entry("shell_review".to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let shell_review_table = shell_review.as_table_mut().ok_or_else(|| {
            CliError::Usage("config section security.shell_review is not a table".to_string())
        })?;
        shell_review_table.insert(field.to_string(), parsed_value);
    } else if let Some((provider, field)) = split_provider_config_key(key) {
        let provider_root = root
            .entry("providers".to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let provider_root_table = provider_root.as_table_mut().ok_or_else(|| {
            CliError::Usage("config section providers is not a table".to_string())
        })?;
        let selected_provider = provider_root_table
            .entry(provider.to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let selected_provider_table = selected_provider.as_table_mut().ok_or_else(|| {
            CliError::Usage(format!(
                "config section providers.{provider} is not a table"
            ))
        })?;
        selected_provider_table.insert(field.to_string(), parsed_value);
    } else {
        let (section, field) = split_config_key(key)?;
        let section_value = root
            .entry(section.to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let section_table = section_value
            .as_table_mut()
            .ok_or_else(|| CliError::Usage(format!("config section {section} is not a table")))?;
        section_table.insert(field.to_string(), parsed_value);
    }
    let temp = path.with_extension("toml.tmp");
    fs::write(&temp, toml::to_string_pretty(&Value::Table(root))?)
        .map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| io_error(&path, error))?;
    Ok(())
}

/// Return a config value by key.
#[allow(clippy::too_many_lines)]
pub fn get_config_value(config: &CliConfig, key: &str) -> CliResult<String> {
    let value = match key {
        "general.default_profile" => config.default_profile.clone(),
        "general.default_output" => output_mode_name(config.default_output).to_string(),
        "general.default_hitl" => hitl_policy_name(config.default_hitl).to_string(),
        "general.max_goal_iterations" => config.max_goal_iterations.to_string(),
        "skills.dirs" => config
            .skill_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":"),
        "subagents.dirs" => config
            .subagent_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":"),
        "subagents.disabled" => config.disabled_subagents.join(","),
        "storage.database_path" => config.database_path.display().to_string(),
        "storage.file_store_path" => config.file_store_path.display().to_string(),
        "environment.workspace_root" => config.workspace_root.display().to_string(),
        "environment.provider" => config.environment_provider.clone(),
        "environment.files_policy" => config.files_policy.clone(),
        "environment.shell_enabled" => config.shell_enabled.to_string(),
        "security.shell_review.enabled" => config.shell_review.enabled.to_string(),
        "security.shell_review.model" => config.shell_review.model.clone().unwrap_or_default(),
        "security.shell_review.model_settings" => config
            .shell_review
            .model_settings
            .clone()
            .unwrap_or_default(),
        "security.shell_review.on_needs_approval" => config.shell_review.on_needs_approval.clone(),
        "security.shell_review.risk_threshold" => config.shell_review.risk_threshold.clone(),
        "security.shell_review.system_prompt" => config
            .shell_review
            .system_prompt
            .clone()
            .unwrap_or_default(),
        "update.channel" => config.update_channel.clone(),
        "oauth_refresh.enabled" => config.oauth_refresh.enabled.to_string(),
        "oauth_refresh.interval_seconds" => config.oauth_refresh.interval_seconds.to_string(),
        "oauth_refresh.failure_retry_seconds" => {
            config.oauth_refresh.failure_retry_seconds.to_string()
        }
        "oauth_refresh.refresh_on_startup" => config.oauth_refresh.refresh_on_startup.to_string(),
        "general.model" | "model.default.model" => config
            .default_model
            .as_ref()
            .map(|profile| profile.model_id.clone())
            .unwrap_or_default(),
        "general.model_settings" | "model.default.model_settings" => config
            .default_model
            .as_ref()
            .and_then(|profile| profile.model_settings.clone())
            .unwrap_or_default(),
        "general.model_cfg" | "model.default.model_cfg" => config
            .default_model
            .as_ref()
            .and_then(|profile| profile.model_cfg.clone())
            .unwrap_or_default(),
        "model.profiles" => serde_json::to_string(&config.model_profiles)?,
        "envd.profiles" => serde_json::to_string(&config.envd_profiles)?,
        "env" => serde_json::to_string(&config.env_vars)?,
        "providers.openai.enabled" => config.providers.openai.enabled.to_string(),
        "providers.openai.api_key_env" => config
            .providers
            .openai
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.openai.base_url" => config.providers.openai.base_url.clone().unwrap_or_default(),
        "providers.openai.endpoint_path" => config
            .providers
            .openai
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.openai.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.openai.max_tokens_parameter).to_string()
        }
        "providers.openai.ready" => provider_ready(config, &config.providers.openai).to_string(),
        "providers.codex.enabled" => config.providers.codex.enabled.to_string(),
        "providers.codex.base_url" => config.providers.codex.base_url.clone().unwrap_or_default(),
        "providers.codex.endpoint_path" => config
            .providers
            .codex
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.codex.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.codex.max_tokens_parameter).to_string()
        }
        "providers.anthropic.enabled" => config.providers.anthropic.enabled.to_string(),
        "providers.anthropic.api_key_env" => config
            .providers
            .anthropic
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.anthropic.base_url" => config
            .providers
            .anthropic
            .base_url
            .clone()
            .unwrap_or_default(),
        "providers.anthropic.endpoint_path" => config
            .providers
            .anthropic
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.anthropic.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.anthropic.max_tokens_parameter).to_string()
        }
        "providers.anthropic.ready" => {
            provider_ready(config, &config.providers.anthropic).to_string()
        }
        "providers.gemini.enabled" => config.providers.gemini.enabled.to_string(),
        "providers.gemini.api_key_env" => config
            .providers
            .gemini
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.gemini.base_url" => config.providers.gemini.base_url.clone().unwrap_or_default(),
        "providers.gemini.endpoint_path" => config
            .providers
            .gemini
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.gemini.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.gemini.max_tokens_parameter).to_string()
        }
        "providers.gemini.ready" => provider_ready(config, &config.providers.gemini).to_string(),
        "providers.google-cloud.enabled" => config.providers.google_cloud.enabled.to_string(),
        "providers.google-cloud.api_key_env" => config
            .providers
            .google_cloud
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.google-cloud.auth_token_env" => config
            .providers
            .google_cloud
            .auth_token_env
            .clone()
            .unwrap_or_default(),
        "providers.google-cloud.project" => config
            .providers
            .google_cloud
            .project
            .clone()
            .unwrap_or_default(),
        "providers.google-cloud.location" => config
            .providers
            .google_cloud
            .location
            .clone()
            .unwrap_or_default(),
        "providers.google-cloud.base_url" => config
            .providers
            .google_cloud
            .base_url
            .clone()
            .unwrap_or_default(),
        "providers.google-cloud.endpoint_path" => config
            .providers
            .google_cloud
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.google-cloud.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.google_cloud.max_tokens_parameter)
                .to_string()
        }
        "providers.google-cloud.ready" => {
            provider_ready(config, &config.providers.google_cloud).to_string()
        }
        "tui.render_mode" => tui_render_mode_name(config.tui_render_mode).to_string(),
        "trim.auto_after_run" => config.auto_trim.to_string(),
        "trim.current_session_keep_recent_runs" => {
            config.current_session_keep_recent_runs.to_string()
        }
        "trim.all_sessions_keep_recent_runs" => config.all_sessions_keep_recent_runs.to_string(),
        "trim.all_sessions_keep_days" => config.all_sessions_keep_days.to_string(),
        "trim.all_sessions_interval_hours" => config.all_sessions_interval_hours.to_string(),
        "metadata.tools" => serde_json::to_string(&config.tools_config)?,
        "metadata.mcp" => serde_json::to_string(&config.mcp_config)?,
        "metadata.unmapped" => serde_json::to_string(&config.unmapped_metadata)?,
        other => {
            if let Some((provider, field)) = split_provider_config_key(other) {
                if let Some(provider_config) = provider_config_by_name(config, provider) {
                    provider_config_value(config, provider_config, field)?
                } else {
                    return Err(CliError::NotFound(other.to_string()));
                }
            } else {
                return Err(CliError::NotFound(other.to_string()));
            }
        }
    };
    Ok(format!("{value}\n"))
}

fn provider_config_by_name<'a>(
    config: &'a CliConfig,
    provider: &str,
) -> Option<&'a ProviderConfig> {
    match provider {
        "openai" => Some(&config.providers.openai),
        "codex" => Some(&config.providers.codex),
        "anthropic" => Some(&config.providers.anthropic),
        "gemini" | "google" | "google-gla" => Some(&config.providers.gemini),
        "google-cloud" | "google_cloud" | "google-vertex" => Some(&config.providers.google_cloud),
        gateway => config.providers.gateways.get(gateway),
    }
}

fn provider_config_value(
    config: &CliConfig,
    provider: &ProviderConfig,
    field: &str,
) -> CliResult<String> {
    let value = match field {
        "enabled" => provider.enabled.to_string(),
        "api_key_env" => provider.api_key_env.clone().unwrap_or_default(),
        "auth_token_env" => provider.auth_token_env.clone().unwrap_or_default(),
        "project" => provider.project.clone().unwrap_or_default(),
        "location" => provider.location.clone().unwrap_or_default(),
        "base_url" => provider.base_url.clone().unwrap_or_default(),
        "endpoint_path" => provider.endpoint_path.clone().unwrap_or_default(),
        "max_tokens_parameter" => {
            max_tokens_parameter_name(provider.max_tokens_parameter).to_string()
        }
        "ready" => provider_ready(config, provider).to_string(),
        other => return Err(CliError::NotFound(other.to_string())),
    };
    Ok(value)
}

fn provider_ready(config: &CliConfig, provider: &ProviderConfig) -> bool {
    provider.enabled
        && (config.env_value_present(provider.api_key_env.as_deref())
            || config.env_value_present(provider.auth_token_env.as_deref()))
}

const fn output_mode_name(output: OutputMode) -> &'static str {
    match output {
        OutputMode::Text => "text",
        OutputMode::DisplayJsonl => "display-jsonl",
        OutputMode::AguiJsonl => "agui-jsonl",
        OutputMode::Json => "json",
        OutputMode::Silent => "silent",
    }
}

const fn max_tokens_parameter_name(parameter: MaxTokensParameter) -> &'static str {
    match parameter {
        MaxTokensParameter::Default => "default",
        MaxTokensParameter::MaxTokens => "max_tokens",
        MaxTokensParameter::MaxOutputTokens => "max_output_tokens",
        MaxTokensParameter::MaxCompletionTokens => "max_completion_tokens",
        MaxTokensParameter::Omit => "omit",
    }
}

fn validated_max_tokens_parameter(value: &str) -> CliResult<MaxTokensParameter> {
    match value.trim() {
        "default" => Ok(MaxTokensParameter::Default),
        "max_tokens" => Ok(MaxTokensParameter::MaxTokens),
        "max_output_tokens" => Ok(MaxTokensParameter::MaxOutputTokens),
        "max_completion_tokens" => Ok(MaxTokensParameter::MaxCompletionTokens),
        "omit" => Ok(MaxTokensParameter::Omit),
        other => Err(CliError::Usage(format!(
            "invalid max_tokens_parameter: {other}; expected default, max_tokens, max_output_tokens, max_completion_tokens, or omit"
        ))),
    }
}

const fn hitl_policy_name(hitl: HitlPolicy) -> &'static str {
    match hitl {
        HitlPolicy::Deny => "deny",
        HitlPolicy::Defer => "defer",
        HitlPolicy::Fail => "fail",
        HitlPolicy::Prompt => "prompt",
    }
}

const fn tui_render_mode_name(mode: TuiRenderMode) -> &'static str {
    match mode {
        TuiRenderMode::Normal => "normal",
        TuiRenderMode::Concise => "concise",
        TuiRenderMode::Debug => "debug",
    }
}

fn validated_tui_render_mode(value: &str) -> CliResult<&'static str> {
    match value.trim() {
        "normal" => Ok("normal"),
        "concise" => Ok("concise"),
        "debug" => Ok("debug"),
        other => Err(CliError::Usage(format!(
            "invalid tui.render_mode: {other}; expected normal, concise, or debug"
        ))),
    }
}

fn split_provider_config_key(key: &str) -> Option<(&str, &str)> {
    let mut parts = key.split('.');
    let section = parts.next()?;
    let provider = parts.next()?;
    let field = parts.next()?;
    if parts.next().is_some() || section != "providers" || !valid_provider_config_name(provider) {
        return None;
    }
    match field {
        "enabled"
        | "api_key_env"
        | "auth_token_env"
        | "project"
        | "location"
        | "base_url"
        | "endpoint_path"
        | "max_tokens_parameter"
        | "ready" => Some((provider, field)),
        _ => None,
    }
}

fn valid_provider_config_name(provider: &str) -> bool {
    !provider.is_empty()
        && provider
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn split_config_key(key: &str) -> CliResult<(&str, &str)> {
    if let Some((section, field)) = key.split_once('.') {
        match (section, field) {
            (
                "general",
                "default_profile"
                | "default_output"
                | "default_hitl"
                | "model"
                | "model_settings"
                | "model_cfg"
                | "max_goal_iterations",
            )
            | ("skills", "dirs" | "additional_dirs")
            | ("subagents", "dirs" | "additional_dirs" | "disabled" | "disabled_builtins")
            | ("storage", "database_path" | "file_store_path")
            | ("environment", "workspace_root" | "provider" | "files_policy" | "shell_enabled")
            | (
                "security",
                "shell_review.enabled"
                | "shell_review.model"
                | "shell_review.model_settings"
                | "shell_review.on_needs_approval"
                | "shell_review.risk_threshold"
                | "shell_review.system_prompt",
            )
            | ("update", "channel")
            | ("tui", "render_mode")
            | (
                "oauth_refresh",
                "enabled" | "interval_seconds" | "failure_retry_seconds" | "refresh_on_startup",
            )
            | (
                "trim",
                "auto_after_run"
                | "current_session_keep_recent_runs"
                | "all_sessions_keep_recent_runs"
                | "all_sessions_keep_days"
                | "all_sessions_interval_hours",
            ) => return Ok((section, field)),
            _ => {}
        }
    }
    Err(CliError::NotFound(key.to_string()))
}

fn validated_output_mode(value: &str) -> CliResult<&'static str> {
    match parse_output_mode(value.trim()) {
        Some(OutputMode::Text) => Ok("text"),
        Some(OutputMode::DisplayJsonl) => Ok("display-jsonl"),
        Some(OutputMode::AguiJsonl) => Ok("agui-jsonl"),
        Some(OutputMode::Json) => Ok("json"),
        Some(OutputMode::Silent) => Ok("silent"),
        None => Err(CliError::Usage(format!(
            "invalid general.default_output: {value}; expected text, display-jsonl, agui-jsonl, json, or silent"
        ))),
    }
}

fn validated_hitl_policy(value: &str) -> CliResult<&'static str> {
    match parse_hitl_policy(value.trim()) {
        Some(HitlPolicy::Deny) => Ok("deny"),
        Some(HitlPolicy::Defer) => Ok("defer"),
        Some(HitlPolicy::Fail) => Ok("fail"),
        Some(HitlPolicy::Prompt) => Ok("prompt"),
        None => Err(CliError::Usage(format!(
            "invalid general.default_hitl: {value}; expected deny, defer, fail, or prompt"
        ))),
    }
}

fn validated_environment_provider(value: &str) -> CliResult<&str> {
    match value.trim() {
        "local" => Ok("local"),
        "virtual" => Ok("virtual"),
        other => Err(CliError::Usage(format!(
            "invalid environment.provider: {other}; expected local or virtual"
        ))),
    }
}

fn validated_files_policy(value: &str) -> CliResult<&str> {
    match value.trim() {
        "read_only" | "read-only" => Ok("read_only"),
        "read_write" | "read-write" => Ok("read_write"),
        "none" | "disabled" => Ok("none"),
        other => Err(CliError::Usage(format!(
            "invalid environment.files_policy: {other}; expected read_only, read_write, or none"
        ))),
    }
}

fn validated_non_empty<'a>(key: &str, value: &'a str) -> CliResult<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::Usage(format!(
            "invalid {key}: value cannot be empty"
        )));
    }
    Ok(trimmed)
}

fn validated_positive_u64(key: &str, value: &str) -> CliResult<u64> {
    let parsed = value
        .trim()
        .parse::<u64>()
        .map_err(|error| CliError::Usage(format!("invalid {key}: {error}")))?;
    if parsed == 0 {
        return Err(CliError::Usage(format!(
            "invalid {key}: value must be positive"
        )));
    }
    Ok(parsed)
}

fn parse_config_value(key: &str, value: &str) -> CliResult<Value> {
    if let Some((_provider, field)) = split_provider_config_key(key) {
        return parse_provider_config_value(key, field, value);
    }
    let parsed = match key {
        "general.default_profile"
        | "general.model"
        | "general.model_settings"
        | "general.model_cfg"
        | "storage.database_path"
        | "storage.file_store_path"
        | "environment.workspace_root"
        | "providers.openai.base_url"
        | "providers.openai.endpoint_path"
        | "providers.codex.base_url"
        | "providers.codex.endpoint_path"
        | "providers.anthropic.base_url"
        | "providers.anthropic.endpoint_path"
        | "providers.gemini.base_url"
        | "providers.gemini.endpoint_path"
        | "security.shell_review.model"
        | "security.shell_review.model_settings"
        | "security.shell_review.system_prompt"
        | "update.channel" => Value::String(value.to_string()),
        "general.default_output" => Value::String(validated_output_mode(value)?.to_string()),
        "general.default_hitl" => Value::String(validated_hitl_policy(value)?.to_string()),
        "general.max_goal_iterations" => Value::Integer(
            value
                .trim()
                .parse::<usize>()
                .map_err(|error| CliError::Usage(error.to_string()))
                .and_then(|parsed| {
                    if parsed == 0 {
                        Err(CliError::Usage(
                            "invalid general.max_goal_iterations: value must be positive"
                                .to_string(),
                        ))
                    } else {
                        Ok(parsed)
                    }
                })?
                .try_into()
                .map_err(|error: std::num::TryFromIntError| CliError::Usage(error.to_string()))?,
        ),
        "environment.provider" => Value::String(validated_environment_provider(value)?.to_string()),
        "environment.files_policy" => Value::String(validated_files_policy(value)?.to_string()),
        "security.shell_review.on_needs_approval" => {
            Value::String(validate_shell_review_action(value)?.to_string())
        }
        "security.shell_review.risk_threshold" => {
            Value::String(validate_shell_review_risk(value)?.to_string())
        }
        "skills.dirs"
        | "skills.additional_dirs"
        | "subagents.dirs"
        | "subagents.additional_dirs" => Value::Array(
            value
                .split(':')
                .filter(|path| !path.trim().is_empty())
                .map(|path| Value::String(path.to_string()))
                .collect(),
        ),
        "subagents.disabled" | "subagents.disabled_builtins" => Value::Array(
            value
                .split(',')
                .filter(|name| !name.trim().is_empty())
                .map(|name| Value::String(name.trim().to_string()))
                .collect(),
        ),
        "tui.render_mode" => Value::String(validated_tui_render_mode(value)?.to_string()),
        "trim.auto_after_run"
        | "trim.current_session_keep_recent_runs"
        | "trim.all_sessions_keep_recent_runs"
        | "trim.all_sessions_keep_days"
        | "trim.all_sessions_interval_hours" => parse_trim_config_value(key, value)?,
        "environment.shell_enabled"
        | "security.shell_review.enabled"
        | "oauth_refresh.enabled"
        | "oauth_refresh.refresh_on_startup" => value
            .parse::<bool>()
            .map(Value::Boolean)
            .map_err(|error| CliError::Usage(error.to_string()))?,
        "oauth_refresh.interval_seconds" | "oauth_refresh.failure_retry_seconds" => Value::Integer(
            validated_positive_u64(key, value)?
                .try_into()
                .map_err(|error: std::num::TryFromIntError| CliError::Usage(error.to_string()))?,
        ),
        _ => return Err(CliError::NotFound(key.to_string())),
    };
    Ok(parsed)
}

fn parse_trim_config_value(key: &str, value: &str) -> CliResult<Value> {
    match key {
        "trim.auto_after_run" => value
            .parse::<bool>()
            .map(Value::Boolean)
            .map_err(|error| CliError::Usage(error.to_string())),
        "trim.current_session_keep_recent_runs" | "trim.all_sessions_keep_recent_runs" => {
            Ok(Value::Integer(
                value
                    .parse::<usize>()
                    .map_err(|error| CliError::Usage(error.to_string()))?
                    .try_into()
                    .map_err(|error: std::num::TryFromIntError| {
                        CliError::Usage(error.to_string())
                    })?,
            ))
        }
        "trim.all_sessions_keep_days" | "trim.all_sessions_interval_hours" => Ok(Value::Integer(
            value
                .parse::<u64>()
                .map_err(|error| CliError::Usage(error.to_string()))?
                .try_into()
                .map_err(|error: std::num::TryFromIntError| CliError::Usage(error.to_string()))?,
        )),
        _ => Err(CliError::NotFound(key.to_string())),
    }
}

fn parse_provider_config_value(key: &str, field: &str, value: &str) -> CliResult<Value> {
    let parsed = match field {
        "enabled" => value
            .parse::<bool>()
            .map(Value::Boolean)
            .map_err(|error| CliError::Usage(error.to_string()))?,
        "api_key_env" | "auth_token_env" | "project" | "location" => {
            Value::String(validated_non_empty(key, value)?.to_string())
        }
        "base_url" | "endpoint_path" => Value::String(value.to_string()),
        "max_tokens_parameter" => Value::String(
            max_tokens_parameter_name(validated_max_tokens_parameter(value)?).to_string(),
        ),
        "ready" => {
            return Err(CliError::Usage(format!(
                "{key} is read-only; set api_key_env and export the API key"
            )));
        }
        other => return Err(CliError::NotFound(other.to_string())),
    };
    Ok(parsed)
}
