use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use super::{
    builtin::builtin_profile_specs, default_toolsets, load_profile_spec,
    load_skill_packages_from_dir, mcp_transport_error, model_context_window,
    profile_context_window, profile_model_cfg, profile_model_id, profile_model_settings,
    McpDoctorFinding, McpSummary, ProfileSource, ProfileSummary, SkillSummary, SubagentSummary,
    ToolSummary,
};
use crate::{
    config::{mcp_servers, tool_need_approval, CliConfig, CliModelProfile},
    error::io_error,
    CliError, CliResult,
};

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
        yaml_serde::to_string(&spec).map_err(|error| CliError::Config(error.to_string()))?;
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
