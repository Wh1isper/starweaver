//! Built-in model presets for common provider configurations.

mod config;
mod http;
mod registry;
mod settings;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod types;

pub use http::{
    anthropic_http_config, gemini_http_config, google_cloud_http_config,
    google_cloud_project_http_config, openai_chat_http_config, openai_responses_http_config,
};
pub use types::{
    ModelConfigPreset, ModelConfigPresetData, ModelPresetError, ModelRuntimePreset,
    ModelSettingsPreset,
};

use crate::ModelSettings;

use config::model_config_by_name;
use registry::{
    MODEL_CONFIG_ALIASES, MODEL_CONFIG_PRESETS, MODEL_SETTINGS_ALIASES, MODEL_SETTINGS_PRESETS,
    model_config_alias, model_settings_alias,
};
use settings::model_settings_by_name;

/// Resolve a built-in model settings preset by name or alias.
///
/// # Errors
///
/// Returns an error when the preset name is unknown.
pub fn get_model_settings(name: &str) -> Result<ModelSettings, ModelPresetError> {
    let canonical = model_settings_alias(name);
    model_settings_by_name(canonical).ok_or_else(|| ModelPresetError::UnknownPreset {
        name: name.to_string(),
        available: list_model_settings_presets(),
    })
}

/// Return a built-in model config preset by name or alias.
///
/// # Errors
///
/// Returns an error when the preset name is unknown.
pub fn get_model_config(name: &str) -> Result<ModelConfigPresetData, ModelPresetError> {
    let canonical = model_config_alias(name);
    model_config_by_name(canonical).ok_or_else(|| ModelPresetError::UnknownModelConfig {
        name: name.to_string(),
        available: list_model_config_presets(),
    })
}

/// Return all built-in model settings preset names and aliases.
#[must_use]
pub fn list_model_settings_presets() -> Vec<String> {
    let mut names = MODEL_SETTINGS_PRESETS
        .iter()
        .copied()
        .chain(MODEL_SETTINGS_ALIASES.iter().map(|(alias, _)| *alias))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// Return all built-in model config preset names and aliases.
#[must_use]
pub fn list_model_config_presets() -> Vec<String> {
    let mut names = MODEL_CONFIG_PRESETS
        .iter()
        .copied()
        .chain(MODEL_CONFIG_ALIASES.iter().map(|(alias, _)| *alias))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// Build a complete runtime preset from model id, settings preset, and config preset.
///
/// # Errors
///
/// Returns an error when a preset name is unknown.
pub fn model_runtime_preset(
    model_id: impl Into<String>,
    provider_name: impl Into<String>,
    model_name: impl Into<String>,
    settings_preset: &str,
    config_preset: &str,
) -> Result<ModelRuntimePreset, ModelPresetError> {
    let settings = get_model_settings(settings_preset)?;
    let config = get_model_config(config_preset)?;
    Ok(ModelRuntimePreset {
        name: format!("{}+{}", model_settings_alias(settings_preset), config.name),
        model_id: model_id.into(),
        provider_name: provider_name.into(),
        model_name: model_name.into(),
        protocol: config.protocol,
        settings,
        config,
    })
}
