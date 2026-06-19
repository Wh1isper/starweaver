//! Frontend-specific local state helpers.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::{config::CliConfig, CliError, CliResult};

/// Read the selected model/profile name for a frontend client.
pub fn read_selected_profile(config: &CliConfig, client: &str) -> CliResult<Option<String>> {
    let path = client_state_dir(config, client)?.join("state.json");
    if !path.exists() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|error| crate::error::io_error(&path, error))?;
    let value = serde_json::from_str::<Value>(&content)?;
    Ok(value
        .get("selected_profile")
        .or_else(|| value.get("selectedProfile"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

/// Persist the selected model/profile name for a frontend client.
pub fn write_selected_profile(config: &CliConfig, client: &str, profile: &str) -> CliResult<()> {
    let dir = client_state_dir(config, client)?;
    std::fs::create_dir_all(&dir).map_err(|error| crate::error::io_error(&dir, error))?;
    let path = dir.join("state.json");
    let temp = dir.join(format!("state.{}.json.tmp", std::process::id()));
    let value = json!({
        "selected_profile": profile,
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(&temp, serde_json::to_vec_pretty(&value)?)
        .map_err(|error| crate::error::io_error(&temp, error))?;
    std::fs::rename(&temp, &path).map_err(|error| crate::error::io_error(&path, error))?;
    Ok(())
}

fn client_state_dir(config: &CliConfig, client: &str) -> CliResult<PathBuf> {
    match client {
        "tui" => Ok(config.tui_state_dir.clone()),
        "desktop" => Ok(config.desktop_state_dir.clone()),
        other => Err(CliError::Usage(format!(
            "unknown client state scope: {other}; expected tui or desktop"
        ))),
    }
}
