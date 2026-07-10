use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::PerThousandRatio;

/// Model capabilities that influence tool and media behavior.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    /// Model can process images.
    Vision,
    /// Model can process video content.
    VideoUnderstanding,
    /// Model can process documents.
    DocumentUnderstanding,
    /// Model can process audio content.
    AudioUnderstanding,
    /// Model supports images by URL.
    ImageUrl,
    /// Model supports videos by URL.
    VideoUrl,
    /// Provider requires reasoning content in assistant messages.
    ReasoningRequired,
    /// Provider rejects foreign-provider reasoning content.
    ReasoningForeignUnsupported,
}

/// Runtime model configuration stored on [`crate::AgentContext`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelConfig {
    /// Context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Parts-per-thousand threshold where proactive context reminders should begin.
    #[serde(default = "default_proactive_context_management_threshold")]
    pub proactive_context_management_threshold: Option<PerThousandRatio>,
    /// Parts-per-thousand threshold where compacting becomes urgent.
    #[serde(default = "default_compact_threshold")]
    pub compact_threshold: PerThousandRatio,
    /// Cold-start gap in seconds before older tool returns are aggressively trimmed.
    #[serde(default = "default_cold_start_trim_seconds")]
    pub cold_start_trim_seconds: u64,
    /// Whether stream retry recovery should resume after provider stream errors.
    #[serde(default)]
    pub stream_resume_on_error: bool,
    /// Maximum stream retry resume attempts.
    #[serde(default = "default_stream_resume_max_attempts")]
    pub stream_resume_max_attempts: usize,
    /// Optional prompt used when resuming a failed stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_resume_prompt: Option<String>,
    /// Maximum number of images retained for model input.
    #[serde(default = "default_model_max_images")]
    pub max_images: usize,
    /// Maximum number of videos retained for model input.
    #[serde(default = "default_model_max_videos")]
    pub max_videos: usize,
    /// Whether GIF input is supported.
    #[serde(default = "default_true")]
    pub support_gif: bool,
    /// Maximum image bytes after base64 encoding before compression applies.
    ///
    /// A value of `0` disables the byte limit without disabling the independent
    /// image dimension limit.
    #[serde(default = "default_model_max_image_bytes")]
    pub max_image_bytes: usize,
    /// Maximum width or height accepted for model image input.
    ///
    /// A value of `0` disables the dimension limit without disabling the
    /// independent encoded-byte limit.
    #[serde(default = "default_model_max_image_dimension")]
    pub max_image_dimension: usize,
    /// Whether large images should be split where supported.
    #[serde(default = "default_true")]
    pub split_large_images: bool,
    /// Maximum split image height.
    #[serde(default = "default_image_split_max_height")]
    pub image_split_max_height: usize,
    /// Pixel overlap between split image segments.
    #[serde(default = "default_image_split_overlap")]
    pub image_split_overlap: usize,
    /// Explicit model capabilities.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<ModelCapability>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            context_window: None,
            proactive_context_management_threshold: default_proactive_context_management_threshold(
            ),
            compact_threshold: default_compact_threshold(),
            cold_start_trim_seconds: default_cold_start_trim_seconds(),
            stream_resume_on_error: false,
            stream_resume_max_attempts: default_stream_resume_max_attempts(),
            stream_resume_prompt: None,
            max_images: default_model_max_images(),
            max_videos: default_model_max_videos(),
            support_gif: true,
            max_image_bytes: default_model_max_image_bytes(),
            max_image_dimension: default_model_max_image_dimension(),
            split_large_images: true,
            image_split_max_height: default_image_split_max_height(),
            image_split_overlap: default_image_split_overlap(),
            capabilities: BTreeSet::new(),
        }
    }
}

impl ModelConfig {
    /// Return whether the config only contains default values.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Return whether a context-window value should be rendered into runtime context.
    #[must_use]
    pub const fn has_runtime_context_window(&self) -> bool {
        self.context_window.is_some()
    }

    /// Return whether a capability is present.
    #[must_use]
    pub fn has_capability(&self, capability: &ModelCapability) -> bool {
        self.capabilities.contains(capability)
    }

    /// Return whether the model supports vision.
    #[must_use]
    pub fn has_vision(&self) -> bool {
        self.has_capability(&ModelCapability::Vision)
    }

    /// Return whether the model supports video understanding.
    #[must_use]
    pub fn has_video_understanding(&self) -> bool {
        self.has_capability(&ModelCapability::VideoUnderstanding)
    }

    /// Return whether the model supports audio understanding.
    #[must_use]
    pub fn has_audio_understanding(&self) -> bool {
        self.has_capability(&ModelCapability::AudioUnderstanding)
    }

    /// Return whether the model supports document understanding.
    #[must_use]
    pub fn has_document_understanding(&self) -> bool {
        self.has_capability(&ModelCapability::DocumentUnderstanding)
    }

    /// Merge non-empty values from another config into this one.
    pub fn merge_from(&mut self, other: Self) {
        if other.context_window.is_some() {
            self.context_window = other.context_window;
        }
        self.proactive_context_management_threshold = other.proactive_context_management_threshold;
        self.compact_threshold = other.compact_threshold;
        self.cold_start_trim_seconds = other.cold_start_trim_seconds;
        self.stream_resume_on_error = other.stream_resume_on_error;
        self.stream_resume_max_attempts = other.stream_resume_max_attempts;
        if other.stream_resume_prompt.is_some() {
            self.stream_resume_prompt = other.stream_resume_prompt;
        }
        self.max_images = other.max_images;
        self.max_videos = other.max_videos;
        self.support_gif = other.support_gif;
        self.max_image_bytes = other.max_image_bytes;
        self.max_image_dimension = other.max_image_dimension;
        self.split_large_images = other.split_large_images;
        self.image_split_max_height = other.image_split_max_height;
        self.image_split_overlap = other.image_split_overlap;
        if !other.capabilities.is_empty() {
            self.capabilities = other.capabilities;
        }
    }
}

const fn default_true() -> bool {
    true
}

#[allow(clippy::unnecessary_wraps)]
const fn default_proactive_context_management_threshold() -> Option<PerThousandRatio> {
    Some(PerThousandRatio::from_per_thousand(650))
}

const fn default_compact_threshold() -> PerThousandRatio {
    PerThousandRatio::from_per_thousand(900)
}

const fn default_cold_start_trim_seconds() -> u64 {
    3600
}

const fn default_stream_resume_max_attempts() -> usize {
    3
}

const fn default_model_max_images() -> usize {
    20
}

const fn default_model_max_videos() -> usize {
    1
}

const fn default_model_max_image_bytes() -> usize {
    5 * 1024 * 1024
}

const fn default_model_max_image_dimension() -> usize {
    8000
}

const fn default_image_split_max_height() -> usize {
    4096
}

const fn default_image_split_overlap() -> usize {
    50
}
