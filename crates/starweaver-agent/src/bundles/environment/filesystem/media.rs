//! Media detection and view handling for filesystem tools.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{Map, Value};
use starweaver_context::{AgentContext, ToolConfig};
use starweaver_environment::{EnvironmentProvider, FileStat};
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use crate::{
    bundles::{
        HostMediaCapabilities, HostMediaUnderstandingClientHandle, MediaUnderstandingRequest,
    },
    media_compression::{compress_image_to_model_limit, raw_budget_for_encoded_limit},
};

use super::{format_size, tool_execution_error, ViewArgs};

#[allow(clippy::too_many_lines)]
pub(super) async fn read_media_file(
    context: &ToolContext,
    provider: &dyn EnvironmentProvider,
    arguments: &ViewArgs,
    stat: FileStat,
    tool_config: &ToolConfig,
) -> Result<ToolResult, ToolError> {
    let media_kind = classify_media_path(&arguments.file_path);
    let max_inline = match media_kind {
        MediaKind::Image => tool_config.view_max_inline_image_bytes,
        MediaKind::Video => tool_config.view_max_inline_video_bytes,
        MediaKind::Audio => tool_config.view_max_inline_audio_bytes,
    };
    if stat.size > max_inline {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "file_path": arguments.file_path,
            "media_kind": media_kind.as_str(),
            "error": format!(
                "{} file is too large to inline ({}). Maximum supported inline size is {}.",
                media_kind.title(),
                format_size(stat.size),
                format_size(max_inline),
            ),
        })));
    }
    let mut data = provider
        .read_bytes(&arguments.file_path, 0, None)
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    let mut media_type = match media_kind {
        MediaKind::Image => detect_image_media_type(&data)
            .or_else(|| image_media_type(&arguments.file_path))
            .unwrap_or("application/octet-stream")
            .to_string(),
        MediaKind::Video => video_media_type(&arguments.file_path).to_string(),
        MediaKind::Audio => audio_media_type(&arguments.file_path).to_string(),
    };
    if media_kind == MediaKind::Image && !is_supported_inline_image(&media_type) {
        return Ok(ToolResult::new(Value::String(format!(
            "Error: unsupported image format '{media_type}' for {}. Supported formats: image/gif, image/jpeg, image/png, image/webp.",
            arguments.file_path
        ))));
    }

    let original_bytes = data.len();
    let mut compressed_for_model = false;
    if media_kind == MediaKind::Image {
        if let Some(agent_context) = context.dependency::<AgentContext>() {
            let max_image_bytes = agent_context.model_config.max_image_bytes;
            if max_image_bytes > 0 {
                match compress_image_to_model_limit(&data, max_image_bytes, &media_type) {
                    Ok(compressed) => {
                        if compressed.data.len() > raw_budget_for_encoded_limit(max_image_bytes) {
                            return Ok(ToolResult::new(serde_json::json!({
                                "success": false,
                                "file_path": arguments.file_path,
                                "media_kind": media_kind.as_str(),
                                "media_type": media_type,
                                "error": format!(
                                    "Image could not be compressed below the {max_image_bytes} byte API limit after accounting for base64 encoding."
                                ),
                                "message": "Try resizing or converting it to a smaller format first.",
                            })));
                        }
                        data = compressed.data;
                        media_type = compressed.media_type;
                        compressed_for_model = compressed.compressed;
                    }
                    Err(error) => {
                        return Ok(ToolResult::new(serde_json::json!({
                            "success": false,
                            "file_path": arguments.file_path,
                            "media_kind": media_kind.as_str(),
                            "media_type": media_type,
                            "error": format!("Image could not be compressed for inline model input: {error}"),
                            "message": "Try resizing or converting it to a smaller format first.",
                        })));
                    }
                }
            }
        }
    }

    let data_url = data_url(&media_type, &data);
    let capabilities = context.dependency::<HostMediaCapabilities>();
    let native_supported = capabilities
        .as_ref()
        .is_some_and(|capabilities| media_capability_supported(capabilities, media_kind));
    if native_supported {
        let message = format!(
            "The {} is attached in a provider-native media message.",
            media_kind.as_str()
        );
        let mut private_metadata = Map::new();
        private_metadata.insert(
            "starweaver_tool_return_content_parts".to_string(),
            serde_json::json!([{
                "kind": "data_url",
                "data_url": data_url,
                "media_type": media_type,
            }]),
        );
        private_metadata.insert(
            "starweaver_tool_return_prompt".to_string(),
            serde_json::json!(media_prompt(
                media_kind,
                &arguments.file_path,
                arguments.instructions.as_deref()
            )),
        );
        return Ok(ToolResult::new(serde_json::json!({
            "success": true,
            "file_path": arguments.file_path,
            "media_kind": media_kind.as_str(),
            "media_type": media_type,
            "native_supported": true,
            "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
            "message": message,
            "instructions": arguments.instructions,
            "compressed": compressed_for_model,
            "original_bytes": original_bytes,
            "inline_bytes": data.len(),
        }))
        .with_private_metadata(private_metadata));
    }

    if let Some(handle) = context.dependency::<HostMediaUnderstandingClientHandle>() {
        let response = handle
            .client
            .understand(MediaUnderstandingRequest {
                media_kind: media_kind.as_str().to_string(),
                url: data_url,
                instructions: arguments.instructions.clone(),
            })
            .await
            .map_err(|error| tool_execution_error("view", error))?;
        return serde_json::to_value(response)
            .map(ToolResult::new)
            .map_err(|error| tool_execution_error("view", error));
    }

    Ok(ToolResult::new(serde_json::json!({
        "success": false,
        "file_path": arguments.file_path,
        "media_kind": media_kind.as_str(),
        "media_type": media_type,
        "native_supported": false,
        "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
        "missing_dependency": "HostMediaUnderstandingClientHandle",
        "message": "The active model does not advertise native support for this local media kind. Configure a HostMediaUnderstandingClientHandle fallback adapter or switch to a media-capable model.",
    })))
}

pub(super) enum ViewFileKind {
    Text,
    Image,
    Video,
    Audio,
    Pdf,
    Office,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaKind {
    Image,
    Video,
    Audio,
}

impl MediaKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }
}

pub(super) fn classify_view_path(path: &str) -> ViewFileKind {
    match extension(path).as_deref() {
        Some("jpg" | "jpeg" | "png" | "gif" | "bmp" | "ico" | "webp") => ViewFileKind::Image,
        Some(
            "mp4" | "webm" | "mov" | "avi" | "flv" | "wmv" | "mpg" | "mpeg" | "3gp" | "mkv" | "m4v"
            | "ogv",
        ) => ViewFileKind::Video,
        Some("mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "wma" | "opus" | "aiff" | "aif") => {
            ViewFileKind::Audio
        }
        Some("pdf") => ViewFileKind::Pdf,
        Some("doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "epub") => ViewFileKind::Office,
        Some(
            "txt" | "md" | "json" | "xml" | "csv" | "html" | "htm" | "rs" | "py" | "js" | "ts"
            | "tsx" | "jsx" | "toml" | "yaml" | "yml",
        ) => ViewFileKind::Text,
        _ => ViewFileKind::Unknown,
    }
}

fn classify_media_path(path: &str) -> MediaKind {
    match classify_view_path(path) {
        ViewFileKind::Image => MediaKind::Image,
        ViewFileKind::Video => MediaKind::Video,
        ViewFileKind::Audio => MediaKind::Audio,
        ViewFileKind::Text | ViewFileKind::Pdf | ViewFileKind::Office | ViewFileKind::Unknown => {
            MediaKind::Image
        }
    }
}

const fn media_capability_supported(capabilities: &HostMediaCapabilities, kind: MediaKind) -> bool {
    match kind {
        MediaKind::Image => capabilities.supports_image_url,
        MediaKind::Video => capabilities.supports_video_url,
        MediaKind::Audio => capabilities.supports_audio_url,
    }
}

fn extension(path: &str) -> Option<String> {
    let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
    filename
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, ext)| ext.to_ascii_lowercase())
}

fn image_media_type(path: &str) -> Option<&'static str> {
    match extension(path).as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("ico") => Some("image/x-icon"),
        _ => None,
    }
}

fn video_media_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        Some("flv") => "video/x-flv",
        Some("wmv") => "video/x-ms-wmv",
        Some("mpg" | "mpeg") => "video/mpeg",
        Some("3gp") => "video/3gpp",
        Some("mkv") => "video/x-matroska",
        Some("m4v") => "video/x-m4v",
        Some("ogv") => "video/ogg",
        _ => "video/mp4",
    }
}

fn audio_media_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wma") => "audio/x-ms-wma",
        Some("opus") => "audio/opus",
        Some("aiff" | "aif") => "audio/aiff",
        _ => "audio/mpeg",
    }
}

fn detect_image_media_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn is_supported_inline_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    )
}

fn media_prompt(kind: MediaKind, file_path: &str, instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "The view tool loaded local {kind} file `{file_path}` through the active environment. Inspect the attached media and answer accordingly.",
        kind = kind.as_str(),
    );
    if let Some(instructions) = instructions.filter(|value| !value.trim().is_empty()) {
        prompt.push_str(
            "

Analysis instructions:
",
        );
        prompt.push_str(instructions.trim());
    }
    prompt
}

fn data_url(media_type: &str, data: &[u8]) -> String {
    format!("data:{media_type};base64,{}", STANDARD.encode(data))
}
