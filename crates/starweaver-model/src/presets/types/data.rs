use serde::{Deserialize, Serialize};

use crate::{HttpModelConfig, ModelProfile, ModelSettings, ProtocolFamily, ProviderAlias};

/// Media and context metadata for a configured model family.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelConfigPresetData {
    /// Canonical preset name.
    pub name: String,
    /// Provider protocol family.
    pub protocol: ProtocolFamily,
    /// Context window in tokens.
    pub context_window: u32,
    /// Maximum image count recommended for one request.
    pub max_images: u32,
    /// Maximum video count recommended for one request.
    pub max_videos: u32,
    /// GIF input support.
    pub supports_gif: bool,
    /// Split large images before sending them to the model.
    pub split_large_images: bool,
    /// Maximum image split height.
    pub image_split_max_height: u32,
    /// Image split overlap.
    pub image_split_overlap: u32,
    /// Model profile capabilities.
    pub profile: ModelProfile,
}

/// Complete model preset ready for host profile resolution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelRuntimePreset {
    /// Canonical preset name.
    pub name: String,
    /// Provider alias/model id.
    pub model_id: String,
    /// Provider display name.
    pub provider_name: String,
    /// Provider model name sent on the wire.
    pub model_name: String,
    /// Protocol family.
    pub protocol: ProtocolFamily,
    /// Default model settings.
    pub settings: ModelSettings,
    /// Capability/config preset.
    pub config: ModelConfigPresetData,
}

impl ModelRuntimePreset {
    /// Convert this runtime preset into a provider alias using the supplied HTTP config.
    #[must_use]
    pub fn provider_alias(&self, http: HttpModelConfig) -> ProviderAlias {
        ProviderAlias::new(
            self.model_id.clone(),
            self.provider_name.clone(),
            self.model_name.clone(),
            self.protocol,
            http,
        )
        .with_profile(self.config.profile.clone())
        .with_default_settings(self.settings.clone())
    }
}
