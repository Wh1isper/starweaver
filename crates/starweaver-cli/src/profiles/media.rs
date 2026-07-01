use std::{env, sync::Arc};

use async_trait::async_trait;
use serde_json::Map;
use starweaver_agent::{
    AgentCapability, AgentRunState, CapabilityResult, HostMediaUnderstandingClient,
    HostMediaUnderstandingClientHandle, MediaUnderstandingRequest, MediaUnderstandingResponse,
    ToolContext,
};
use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ContentPart, ModelAdapter, ModelMessage, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ToolCallPart,
};

use super::{local_echo_model, provider_model};
use crate::{CliError, CliResult, config::CliConfig};

#[derive(Clone)]
pub(super) struct CliMediaUnderstandingCapability {
    handle: HostMediaUnderstandingClientHandle,
}

impl CliMediaUnderstandingCapability {
    pub(super) fn new(client: Arc<dyn HostMediaUnderstandingClient>) -> Self {
        Self {
            handle: HostMediaUnderstandingClientHandle::new(client),
        }
    }
}

#[async_trait]
impl AgentCapability for CliMediaUnderstandingCapability {
    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        tool_context: &mut ToolContext,
        _call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        tool_context.dependencies.insert(self.handle.clone());
        Ok(())
    }
}

#[derive(Clone)]
struct CliMediaUnderstandingClient {
    image: Option<CliMediaUnderstandingModel>,
    video: Option<CliMediaUnderstandingModel>,
    audio: Option<CliMediaUnderstandingModel>,
}

#[derive(Clone)]
struct CliMediaUnderstandingModel {
    model_id: String,
    model: Arc<dyn ModelAdapter>,
}

#[async_trait]
impl HostMediaUnderstandingClient for CliMediaUnderstandingClient {
    async fn understand(
        &self,
        request: MediaUnderstandingRequest,
    ) -> Result<MediaUnderstandingResponse, String> {
        let selected = match request.media_kind.as_str() {
            "image" => self.image.as_ref(),
            "video" => self.video.as_ref(),
            "audio" => self.audio.as_ref(),
            other => return Err(format!("unsupported media kind {other}")),
        }
        .ok_or_else(|| {
            format!(
                "missing fallback model for {} understanding",
                request.media_kind
            )
        })?;
        let response = selected
            .model
            .request_stream_final(
                vec![ModelMessage::Request(media_understanding_request(&request))],
                None,
                ModelRequestParameters::default(),
                ModelRequestContext::new(RunId::new(), ConversationId::new()),
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut content = response.text_output();
        if content.trim().is_empty() {
            content = "Media understanding model returned no text output.".to_string();
        }
        Ok(MediaUnderstandingResponse {
            success: true,
            media_kind: request.media_kind,
            url: request.url,
            model_id: selected.model_id.clone(),
            content,
            truncated: false,
            metadata: Map::new(),
        })
    }
}

pub(super) fn configured_media_client(
    config: &CliConfig,
) -> CliResult<Option<Arc<dyn HostMediaUnderstandingClient>>> {
    let image = configured_media_model(config, "STARWEAVER_IMAGE_UNDERSTANDING_MODEL")?;
    let video = configured_media_model(config, "STARWEAVER_VIDEO_UNDERSTANDING_MODEL")?;
    let audio = configured_media_model(config, "STARWEAVER_AUDIO_UNDERSTANDING_MODEL")?;
    if image.is_none() && video.is_none() && audio.is_none() {
        return Ok(None);
    }
    Ok(Some(Arc::new(CliMediaUnderstandingClient {
        image,
        video,
        audio,
    })))
}

fn configured_media_model(
    config: &CliConfig,
    env_name: &str,
) -> CliResult<Option<CliMediaUnderstandingModel>> {
    let Some(model_id) = env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let model = match model_id.as_str() {
        "local_echo" => Arc::new(local_echo_model()) as Arc<dyn ModelAdapter>,
        other => provider_model(config, other, None)?.ok_or_else(|| {
            CliError::Config(format!("unknown media understanding model id {other}"))
        })?,
    };
    Ok(Some(CliMediaUnderstandingModel { model_id, model }))
}

fn media_understanding_request(request: &MediaUnderstandingRequest) -> ModelRequest {
    let source_line = if request.url.starts_with("data:") {
        "Source: attached inline media data URL content part".to_string()
    } else {
        format!("URL: {}", request.url)
    };
    let prompt = request
        .instructions
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || {
                format!(
                    "Analyze this {kind} for the Starweaver CLI user. Return concise, useful observations.\n\n{source_line}",
                    kind = request.media_kind,
                )
            },
            |instructions| {
                format!(
                    "Analyze this {kind} for the Starweaver CLI user. Return concise, useful observations.\n\nFocused instructions:\n{instructions}\n\n{source_line}",
                    kind = request.media_kind,
                )
            },
        );
    let mut content = vec![ContentPart::Text { text: prompt }];
    content.push(match request.media_kind.as_str() {
        "image" => ContentPart::ImageUrl {
            url: request.url.clone(),
        },
        "video" => ContentPart::FileUrl {
            url: request.url.clone(),
            media_type: "video/*".to_string(),
        },
        "audio" => ContentPart::FileUrl {
            url: request.url.clone(),
            media_type: "audio/*".to_string(),
        },
        _ => ContentPart::FileUrl {
            url: request.url.clone(),
            media_type: "application/octet-stream".to_string(),
        },
    });
    ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content,
            name: Some("media_understanding".to_string()),
            metadata: Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }
}
