use std::{fs, path::PathBuf, process, thread};

use chrono::{DateTime, Utc};

use crate::{CliResult, config::CliConfig, error::io_error};

const LAST_RETENTION_MAINTENANCE_KEY: &str = "last_retention_maintenance_at";

/// Ensure local config directories exist.
pub fn ensure_config_dirs(config: &CliConfig) -> CliResult<()> {
    fs::create_dir_all(&config.file_store_path)
        .map_err(|error| io_error(&config.file_store_path, error))?;
    if let Some(parent) = config.database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    Ok(())
}

/// Read current session pointer from project state.
pub fn read_current_session(config: &CliConfig) -> CliResult<Option<String>> {
    let value = read_project_state(config)?;
    Ok(value
        .get("current_session_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

/// Read the last automatic retention maintenance timestamp.
pub fn read_last_retention_maintenance(config: &CliConfig) -> CliResult<Option<DateTime<Utc>>> {
    let value = read_project_state(config)?;
    value
        .get(LAST_RETENTION_MAINTENANCE_KEY)
        .and_then(serde_json::Value::as_str)
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|parsed| parsed.with_timezone(&Utc))
                .map_err(|error| crate::CliError::Config(error.to_string()))
        })
        .transpose()
}

/// Write current session pointer into project state through atomic rename.
pub fn write_current_session(config: &CliConfig, session_id: &str) -> CliResult<()> {
    update_project_state(config, |value| {
        value["current_session_id"] = serde_json::json!(session_id);
        value["database_path"] = serde_json::json!(config.database_path);
        value["profile"] = serde_json::json!(config.default_profile);
    })
}

/// Write the last automatic retention maintenance timestamp.
pub fn write_last_retention_maintenance(
    config: &CliConfig,
    timestamp: DateTime<Utc>,
) -> CliResult<()> {
    update_project_state(config, |value| {
        value[LAST_RETENTION_MAINTENANCE_KEY] = serde_json::json!(timestamp.to_rfc3339());
        value["database_path"] = serde_json::json!(config.database_path);
    })
}

/// Clear the current session pointer without deleting other project state fields.
pub fn clear_current_session(config: &CliConfig) -> CliResult<()> {
    update_project_state(config, |value| {
        if let Some(object) = value.as_object_mut() {
            object.remove("current_session_id");
        }
        value["database_path"] = serde_json::json!(config.database_path);
        value["profile"] = serde_json::json!(config.default_profile);
        value["cleared_at"] = serde_json::json!(Utc::now());
    })
}

fn read_project_state(config: &CliConfig) -> CliResult<serde_json::Value> {
    let path = config.project_dir.join("state.json");
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
    let mut value = serde_json::from_str::<serde_json::Value>(&content)?;
    if !value.is_object() {
        value = serde_json::json!({});
    }
    Ok(value)
}

fn update_project_state(
    config: &CliConfig,
    update: impl FnOnce(&mut serde_json::Value),
) -> CliResult<()> {
    fs::create_dir_all(&config.project_dir)
        .map_err(|error| io_error(&config.project_dir, error))?;
    let path = config.project_dir.join("state.json");
    let mut value = read_project_state(config)?;
    update(&mut value);
    let temp = state_temp_path(config);
    fs::write(&temp, serde_json::to_vec_pretty(&value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| io_error(&path, error))?;
    Ok(())
}

fn state_temp_path(config: &CliConfig) -> PathBuf {
    config.project_dir.join(format!(
        "state.{}.{}.json.tmp",
        process::id(),
        format_thread_id(thread::current().id())
    ))
}

fn format_thread_id(id: thread::ThreadId) -> String {
    format!("{id:?}")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}
