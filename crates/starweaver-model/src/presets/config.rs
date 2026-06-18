//! Built-in model capability/config preset resolution.

use crate::{ModelProfile, ProtocolFamily};

use super::types::ModelConfigPresetData;

#[allow(clippy::match_same_arms, clippy::too_many_lines)]
pub(super) fn model_config_by_name(name: &str) -> Option<ModelConfigPresetData> {
    match name {
        "claude_200k" => Some(config_data(
            name,
            ProtocolFamily::AnthropicMessages,
            200_000,
            20,
            0,
            true,
            true,
        )),
        "claude_400k" => Some(config_data(
            name,
            ProtocolFamily::AnthropicMessages,
            400_000,
            20,
            0,
            true,
            true,
        )),
        "claude_1m" => Some(config_data(
            name,
            ProtocolFamily::AnthropicMessages,
            1_000_000,
            20,
            0,
            true,
            true,
        )),
        "gpt5_270k" => Some(config_data(
            name,
            ProtocolFamily::OpenAiResponses,
            270_000,
            20,
            0,
            false,
            true,
        )),
        "gpt5_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiResponses,
            922_000,
            20,
            0,
            false,
            true,
        )),
        "deepseek_v4_400k" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            400_000,
            0,
            0,
            false,
            false,
        )),
        "deepseek_v4_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            1_000_000,
            0,
            0,
            false,
            false,
        )),
        "mimo_v2_5_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            1_000_000,
            0,
            0,
            false,
            false,
        )),
        "mimo_v2_5_pro_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            1_000_000,
            0,
            0,
            false,
            false,
        )),
        "gemini_200k" => Some(config_data(
            name,
            ProtocolFamily::GeminiGenerateContent,
            200_000,
            20,
            1,
            true,
            true,
        )),
        "gemini_1m" => Some(config_data(
            name,
            ProtocolFamily::GeminiGenerateContent,
            1_000_000,
            20,
            1,
            true,
            true,
        )),
        _ => None,
    }
}

fn config_data(
    name: &str,
    protocol: ProtocolFamily,
    context_window: u32,
    max_images: u32,
    max_videos: u32,
    supports_gif: bool,
    split_large_images: bool,
) -> ModelConfigPresetData {
    let mut profile = ModelProfile::for_protocol(protocol);
    profile.supports_image_input = max_images > 0;
    profile.supports_image_output =
        matches!(protocol, ProtocolFamily::OpenAiResponses) && name.starts_with("gpt5_");
    profile.supports_video_input = max_videos > 0;
    profile.supports_audio_input = matches!(protocol, ProtocolFamily::GeminiGenerateContent);
    profile.supports_document_input = matches!(
        protocol,
        ProtocolFamily::AnthropicMessages | ProtocolFamily::GeminiGenerateContent
    );
    ModelConfigPresetData {
        name: name.to_string(),
        protocol,
        context_window,
        max_images,
        max_videos,
        supports_gif,
        split_large_images,
        image_split_max_height: 4096,
        image_split_overlap: 50,
        profile,
    }
}
