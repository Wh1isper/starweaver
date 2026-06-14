use thiserror::Error;

/// Preset lookup failure.
#[derive(Debug, Error)]
pub enum ModelPresetError {
    /// The requested preset name is unknown.
    #[error("unknown model preset: {name}. available: {available:?}")]
    UnknownPreset {
        /// Requested preset name.
        name: String,
        /// Available canonical names and aliases.
        available: Vec<String>,
    },
    /// The requested model config preset name is unknown.
    #[error("unknown model config preset: {name}. available: {available:?}")]
    UnknownModelConfig {
        /// Requested preset name.
        name: String,
        /// Available canonical names and aliases.
        available: Vec<String>,
    },
}
