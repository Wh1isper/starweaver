use std::{env, path::PathBuf};

use super::{expand_path, validate_shell_review_action, validate_shell_review_risk, CliConfig};
use crate::args::{Cli, CliCommand, HitlPolicy, OutputMode};

#[allow(clippy::too_many_lines)]
pub(super) fn apply_env(config: &mut CliConfig) {
    for (key, value) in &config.env_vars {
        if env::var_os(key).is_none() {
            env::set_var(key, value);
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_PROFILE") {
        config.default_profile = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_SKILL_DIRS") {
        config.skill_dirs = env::split_paths(&value).collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_SUBAGENT_DIRS") {
        config.subagent_dirs = env::split_paths(&value).collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_DISABLED_SUBAGENTS") {
        config.disabled_subagents = value
            .to_string_lossy()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_SESSION_DB") {
        config.database_path = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_FILE_STORE") {
        config.file_store_path = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_WORKSPACE_ROOT") {
        config.workspace_root = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_ENV_PROVIDER") {
        config.environment_provider = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_FILES_POLICY") {
        config.files_policy = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_ENABLED") {
        config.shell_enabled = env_bool(&value.to_string_lossy());
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_REVIEW_ENABLED") {
        config.shell_review.enabled = env_bool(&value.to_string_lossy());
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_REVIEW_MODEL") {
        config.shell_review.model = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_REVIEW_MODEL_SETTINGS") {
        config.shell_review.model_settings = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_REVIEW_ACTION") {
        let value = value.to_string_lossy().to_string();
        if let Ok(action) = validate_shell_review_action(&value) {
            config.shell_review.on_needs_approval = action.to_string();
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_REVIEW_RISK_THRESHOLD") {
        let value = value.to_string_lossy().replace('-', "_");
        if let Ok(risk) = validate_shell_review_risk(&value) {
            config.shell_review.risk_threshold = risk.to_string();
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_OUTPUT") {
        if let Some(output) = parse_output_mode(&value.to_string_lossy()) {
            config.default_output = output;
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_HITL") {
        if let Some(hitl) = parse_hitl_policy(&value.to_string_lossy()) {
            config.default_hitl = hitl;
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_UPDATE_CHANNEL") {
        config.update_channel = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_OAUTH_REFRESH_ENABLED") {
        config.oauth_refresh.enabled = env_bool(&value.to_string_lossy());
    }
    if let Some(value) = env::var_os("STARWEAVER_OAUTH_REFRESH_INTERVAL_SECONDS") {
        if let Ok(seconds) = value.to_string_lossy().parse::<u64>() {
            if seconds > 0 {
                config.oauth_refresh.interval_seconds = seconds;
            }
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_OAUTH_REFRESH_FAILURE_RETRY_SECONDS") {
        if let Ok(seconds) = value.to_string_lossy().parse::<u64>() {
            if seconds > 0 {
                config.oauth_refresh.failure_retry_seconds = seconds;
            }
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_OAUTH_REFRESH_ON_STARTUP") {
        config.oauth_refresh.refresh_on_startup = env_bool(&value.to_string_lossy());
    }
    if let Some(value) = env::var_os("STARWEAVER_OPENAI_BASE_URL") {
        config.providers.openai.base_url = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_ANTHROPIC_BASE_URL") {
        config.providers.anthropic.base_url = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_GEMINI_BASE_URL") {
        config.providers.gemini.base_url = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_OPENAI_API_KEY_ENV") {
        config.providers.openai.api_key_env = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_ANTHROPIC_API_KEY_ENV") {
        config.providers.anthropic.api_key_env = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_GEMINI_API_KEY_ENV") {
        config.providers.gemini.api_key_env = Some(value.to_string_lossy().to_string());
    }
    if env::var_os("STARWEAVER_NO_AUTO_TRIM").is_some() {
        config.auto_trim = false;
    }
}

pub(super) fn apply_cli_overrides(
    config: &mut CliConfig,
    cli: &Cli,
    project_dir: &std::path::Path,
) {
    if let Some(store) = cli.store.as_ref() {
        config.database_path = expand_path(store, project_dir);
    }
    if let Some(profile) = top_level_profile(cli) {
        config.default_profile = profile;
    }
    if let Some(output) = top_level_output(cli) {
        config.default_output = output;
    }
    if let Some(hitl) = top_level_hitl(cli) {
        config.default_hitl = hitl;
    }
}

fn top_level_profile(cli: &Cli) -> Option<String> {
    cli.command
        .as_ref()
        .and_then(|command| match command {
            CliCommand::Run(run) => run.profile.clone(),
            _ => None,
        })
        .or_else(|| cli.profile.clone())
}

fn top_level_output(cli: &Cli) -> Option<OutputMode> {
    cli.command
        .as_ref()
        .and_then(|command| match command {
            CliCommand::Run(run) => run.output,
            _ => None,
        })
        .or(cli.output)
}

fn top_level_hitl(cli: &Cli) -> Option<HitlPolicy> {
    cli.command
        .as_ref()
        .and_then(|command| match command {
            CliCommand::Run(run) => run.hitl,
            _ => None,
        })
        .or(cli.hitl)
}

fn env_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "on")
}

pub(super) fn parse_output_mode(value: &str) -> Option<OutputMode> {
    match value {
        "text" | "Text" => Some(OutputMode::Text),
        "display-jsonl" | "display_jsonl" | "DisplayJsonl" => Some(OutputMode::DisplayJsonl),
        "agui-jsonl" | "agui_jsonl" | "AguiJsonl" | "starweaver" => Some(OutputMode::AguiJsonl),
        "json" | "Json" => Some(OutputMode::Json),
        "silent" | "Silent" => Some(OutputMode::Silent),
        _ => None,
    }
}

pub(super) fn parse_hitl_policy(value: &str) -> Option<HitlPolicy> {
    match value {
        "deny" | "Deny" => Some(HitlPolicy::Deny),
        "defer" | "Defer" => Some(HitlPolicy::Defer),
        "fail" | "Fail" => Some(HitlPolicy::Fail),
        "prompt" | "Prompt" => Some(HitlPolicy::Prompt),
        _ => None,
    }
}
