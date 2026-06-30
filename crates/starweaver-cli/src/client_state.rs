//! Frontend-specific local state helpers.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::{args::TuiRenderMode, config::CliConfig, CliError, CliResult};

/// Read the selected model/profile name for a frontend client.
pub fn read_selected_profile(config: &CliConfig, client: &str) -> CliResult<Option<String>> {
    let value = read_client_state(config, client)?;
    Ok(value
        .get("selected_profile")
        .or_else(|| value.get("selectedProfile"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

/// Persist the selected model/profile name for a frontend client.
pub fn write_selected_profile(config: &CliConfig, client: &str, profile: &str) -> CliResult<()> {
    update_client_state(config, client, |value| {
        value["selected_profile"] = json!(profile);
        value["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
    })
}

/// Read the TUI transcript rendering mode for a frontend client.
pub fn read_render_mode(config: &CliConfig, client: &str) -> CliResult<Option<TuiRenderMode>> {
    let value = read_client_state(config, client)?;
    let Some(raw) = value
        .get("render_mode")
        .or_else(|| value.get("renderMode"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    match raw {
        "normal" => Ok(Some(TuiRenderMode::Normal)),
        "concise" => Ok(Some(TuiRenderMode::Concise)),
        "debug" => Ok(Some(TuiRenderMode::Debug)),
        other => Err(CliError::Config(format!(
            "invalid saved TUI render mode: {other}; expected normal, concise, or debug"
        ))),
    }
}

/// Persist the TUI transcript rendering mode for a frontend client.
pub fn write_render_mode(
    config: &CliConfig,
    client: &str,
    render_mode: TuiRenderMode,
) -> CliResult<()> {
    update_client_state(config, client, |value| {
        value["render_mode"] = json!(render_mode_name(render_mode));
        value["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
    })
}

fn read_client_state(config: &CliConfig, client: &str) -> CliResult<Value> {
    let path = client_state_dir(config, client)?.join("state.json");
    if !path.exists() {
        return Ok(json!({}));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|error| crate::error::io_error(&path, error))?;
    Ok(serde_json::from_str::<Value>(&content)?)
}

fn update_client_state(
    config: &CliConfig,
    client: &str,
    update: impl FnOnce(&mut Value),
) -> CliResult<()> {
    let dir = client_state_dir(config, client)?;
    std::fs::create_dir_all(&dir).map_err(|error| crate::error::io_error(&dir, error))?;
    let path = dir.join("state.json");
    let temp = dir.join(format!("state.{}.json.tmp", std::process::id()));
    let mut value = read_client_state(config, client)?;
    if !value.is_object() {
        value = json!({});
    }
    update(&mut value);
    std::fs::write(&temp, serde_json::to_vec_pretty(&value)?)
        .map_err(|error| crate::error::io_error(&temp, error))?;
    std::fs::rename(&temp, &path).map_err(|error| crate::error::io_error(&path, error))?;
    Ok(())
}

const fn render_mode_name(render_mode: TuiRenderMode) -> &'static str {
    match render_mode {
        TuiRenderMode::Normal => "normal",
        TuiRenderMode::Concise => "concise",
        TuiRenderMode::Debug => "debug",
    }
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
