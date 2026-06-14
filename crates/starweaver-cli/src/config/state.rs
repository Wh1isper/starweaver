use std::{fs, path::PathBuf, process, thread};

use crate::{config::CliConfig, error::io_error, CliResult};

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
    let path = config.project_dir.join("state.json");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
    let value = serde_json::from_str::<serde_json::Value>(&content)?;
    Ok(value
        .get("current_session_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

/// Write current session pointer into project state through atomic rename.
pub fn write_current_session(config: &CliConfig, session_id: &str) -> CliResult<()> {
    fs::create_dir_all(&config.project_dir)
        .map_err(|error| io_error(&config.project_dir, error))?;
    let path = config.project_dir.join("state.json");
    let temp = state_temp_path(config);
    let value = serde_json::json!({
        "current_session_id": session_id,
        "database_path": config.database_path,
        "profile": config.default_profile,
    });
    fs::write(&temp, serde_json::to_vec_pretty(&value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| io_error(&path, error))?;
    Ok(())
}

/// Clear the current session pointer without deleting other project state fields.
pub fn clear_current_session(config: &CliConfig) -> CliResult<()> {
    fs::create_dir_all(&config.project_dir)
        .map_err(|error| io_error(&config.project_dir, error))?;
    let path = config.project_dir.join("state.json");
    let mut value = if path.exists() {
        let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
        serde_json::from_str::<serde_json::Value>(&content)?
    } else {
        serde_json::json!({})
    };
    if !value.is_object() {
        value = serde_json::json!({});
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("current_session_id");
        object.insert(
            "database_path".to_string(),
            serde_json::json!(config.database_path),
        );
        object.insert(
            "profile".to_string(),
            serde_json::json!(config.default_profile),
        );
        object.insert(
            "cleared_at".to_string(),
            serde_json::json!(chrono::Utc::now()),
        );
    }
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
