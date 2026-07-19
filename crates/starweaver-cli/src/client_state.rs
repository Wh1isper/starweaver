//! TUI local state helpers.

use serde_json::{Value, json};

use crate::{CliError, CliResult, args::TuiRenderMode, config::CliConfig};

/// Read the selected model/profile name for the TUI.
pub fn read_selected_profile(config: &CliConfig) -> CliResult<Option<String>> {
    let value = read_tui_state(config)?;
    Ok(value
        .get("selected_profile")
        .or_else(|| value.get("selectedProfile"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

/// Persist the selected model/profile name for the TUI.
pub fn write_selected_profile(config: &CliConfig, profile: &str) -> CliResult<()> {
    update_tui_state(config, |value| {
        value["selected_profile"] = json!(profile);
        value["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
    })
}

/// Read the TUI transcript rendering mode.
pub fn read_render_mode(config: &CliConfig) -> CliResult<Option<TuiRenderMode>> {
    let value = read_tui_state(config)?;
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

/// Persist the TUI transcript rendering mode.
pub fn write_render_mode(config: &CliConfig, render_mode: TuiRenderMode) -> CliResult<()> {
    update_tui_state(config, |value| {
        value["render_mode"] = json!(render_mode_name(render_mode));
        value["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
    })
}

fn read_tui_state(config: &CliConfig) -> CliResult<Value> {
    let path = config.tui_state_dir.join("state.json");
    if !path.exists() {
        return Ok(json!({}));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|error| crate::error::io_error(&path, error))?;
    Ok(serde_json::from_str::<Value>(&content)?)
}

fn update_tui_state(config: &CliConfig, update: impl FnOnce(&mut Value)) -> CliResult<()> {
    let dir = &config.tui_state_dir;
    std::fs::create_dir_all(dir).map_err(|error| crate::error::io_error(dir, error))?;
    let path = dir.join("state.json");
    let mut value = read_tui_state(config)?;
    if !value.is_object() {
        value = json!({});
    }
    update(&mut value);
    let payload = serde_json::to_vec_pretty(&value)?;
    crate::atomic_file::replace(&path, &payload)
        .map_err(|error| crate::error::io_error(&path, error))?;
    Ok(())
}

const fn render_mode_name(render_mode: TuiRenderMode) -> &'static str {
    match render_mode {
        TuiRenderMode::Normal => "normal",
        TuiRenderMode::Concise => "concise",
        TuiRenderMode::Debug => "debug",
    }
}
