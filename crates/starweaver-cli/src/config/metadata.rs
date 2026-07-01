use std::{collections::BTreeMap, fs, path::Path};

use toml::Value;

use crate::{CliError, CliResult, config::CliConfig, error::io_error};

/// Return tool policy entries requiring approval.
#[must_use]
pub fn tool_need_approval(config: &CliConfig) -> Vec<String> {
    let values = config
        .tools_config
        .get("tools")
        .and_then(|tools| tools.get("need_approval"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    values.unwrap_or_else(default_need_approval)
}

const fn default_need_approval() -> Vec<String> {
    Vec::new()
}

/// Return merged configured MCP server map.
#[must_use]
pub fn mcp_servers(config: &CliConfig) -> BTreeMap<String, serde_json::Value> {
    config
        .mcp_config
        .get("servers")
        .and_then(serde_json::Value::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn read_tools_config(
    global_dir: &Path,
    project_dir: &Path,
) -> CliResult<serde_json::Value> {
    let mut value = serde_json::json!({});
    merge_toml_metadata(&mut value, &global_dir.join("tools.toml"))?;
    merge_toml_metadata(&mut value, &project_dir.join("tools.toml"))?;
    Ok(value)
}

pub(super) fn read_mcp_config(
    global_dir: &std::path::Path,
    project_dir: &std::path::Path,
) -> CliResult<serde_json::Value> {
    let mut value = serde_json::json!({});
    merge_json_metadata(&mut value, &global_dir.join("mcp.json"))?;
    merge_json_metadata(&mut value, &project_dir.join("mcp.json"))?;
    Ok(value)
}

fn merge_toml_metadata(target: &mut serde_json::Value, path: &std::path::Path) -> CliResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let parsed = content
        .parse::<Value>()
        .map_err(|error| CliError::Config(error.to_string()))?;
    let json = serde_json::to_value(parsed)?;
    merge_json_value(target, json);
    Ok(())
}

fn merge_json_metadata(target: &mut serde_json::Value, path: &std::path::Path) -> CliResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let json = serde_json::from_str::<serde_json::Value>(&content)?;
    merge_json_value(target, json);
    Ok(())
}

pub(super) fn merge_json_value(target: &mut serde_json::Value, overlay: serde_json::Value) {
    match (target, overlay) {
        (serde_json::Value::Object(target), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_json_value(target.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (target, overlay) => *target = overlay,
    }
}
