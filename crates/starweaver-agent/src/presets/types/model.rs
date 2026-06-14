use serde::{Deserialize, Serialize};
use starweaver_model::ModelSettings;

/// Model configuration selected by a serializable agent spec.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelPreset {
    /// Logical model id resolved by [`AgentSpecRegistry`].
    pub model_id: String,
    /// Built-in model settings preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_preset: Option<String>,
    /// Built-in model capability/config preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_preset: Option<String>,
    /// Default model settings overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ModelSettings>,
}
