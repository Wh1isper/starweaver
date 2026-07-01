//! Media upload filtering.

use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use starweaver_context::{AgentContext, ModelCapability};
use starweaver_model::{
    ContentPart, MediaPolicy, MediaPreflight, ModelMessage, ModelRequestPart, parse_data_url,
};
use starweaver_runtime::AgentRunState;

use crate::filters::message::request_metadata_mut;

use super::policy::media_policy_from_state_and_context;

/// Media upload request passed to upload adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaUploadRequest {
    /// Binary payload to upload.
    pub data: Vec<u8>,
    /// Corrected media type.
    pub media_type: String,
    /// Original media preflight evidence.
    pub preflight: MediaPreflight,
}

/// Media upload adapter used by the SDK `media_upload` filter.
#[async_trait]
pub trait MediaUploader: Send + Sync {
    /// Upload one media payload and return a resource or URL-backed content part.
    ///
    /// # Errors
    ///
    /// Returns a message when the adapter cannot upload the payload. The processor keeps the
    /// original content and records the failure in request metadata.
    async fn upload(&self, request: MediaUploadRequest) -> Result<ContentPart, String>;
}

pub(in crate::filters) async fn media_upload_filter(
    state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
    uploader: Option<&Arc<dyn MediaUploader>>,
) -> Vec<ModelMessage> {
    let Some(media_uploader) = uploader else {
        return messages;
    };
    let targets = UploadTargets::from_context(context);
    if !targets.any() {
        return messages;
    }

    let policy = media_policy_from_state_and_context(state, context);
    let mut upload_count = 0usize;
    let mut failures = Vec::new();
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        for item in content {
                            match upload_content_part(item, &policy, targets, media_uploader).await
                            {
                                UploadOutcome::Uploaded(replacement) => {
                                    *item = replacement;
                                    upload_count += 1;
                                }
                                UploadOutcome::Failed(error) => failures.push(json!(error)),
                                UploadOutcome::Skipped => {}
                            }
                        }
                    }
                    ModelRequestPart::ToolReturn(tool_return) => {
                        let outcome = upload_tool_value(
                            &mut tool_return.content,
                            &policy,
                            targets,
                            media_uploader,
                        )
                        .await;
                        upload_count += outcome.uploaded;
                        failures.extend(outcome.failures);
                    }
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::RetryPrompt { .. }
                    | ModelRequestPart::Instruction { .. } => {}
                }
            }
        }
    }
    if upload_count > 0 {
        request_metadata_mut(&mut messages)
            .insert("starweaver_media_uploaded".to_string(), json!(upload_count));
    }
    if !failures.is_empty() {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_upload_failures".to_string(),
            Value::Array(failures),
        );
    }
    messages
}

async fn upload_content_part(
    item: &ContentPart,
    policy: &MediaPolicy,
    targets: UploadTargets,
    uploader: &Arc<dyn MediaUploader>,
) -> UploadOutcome {
    match item {
        ContentPart::Binary { data, media_type } => {
            let preflight = MediaPreflight::inspect_with_policy(data, Some(media_type), policy);
            if should_upload(&preflight, media_type, targets) {
                let upload_media_type = upload_media_type(&preflight, media_type);
                upload_payload(data.clone(), upload_media_type, preflight, uploader).await
            } else {
                UploadOutcome::Skipped
            }
        }
        ContentPart::DataUrl {
            data_url,
            media_type,
        } => match parse_data_url(data_url) {
            Ok(parsed) => {
                let preflight =
                    MediaPreflight::inspect_with_policy(&parsed.data, Some(media_type), policy);
                if should_upload(&preflight, media_type, targets) {
                    let upload_media_type = upload_media_type(&preflight, media_type);
                    upload_payload(parsed.data, upload_media_type, preflight, uploader).await
                } else {
                    UploadOutcome::Skipped
                }
            }
            Err(error) => UploadOutcome::Failed(error),
        },
        ContentPart::Text { .. }
        | ContentPart::ImageUrl { .. }
        | ContentPart::FileUrl { .. }
        | ContentPart::ResourceRef { .. } => UploadOutcome::Skipped,
    }
}

fn upload_tool_value<'a>(
    value: &'a mut Value,
    policy: &'a MediaPolicy,
    targets: UploadTargets,
    uploader: &'a Arc<dyn MediaUploader>,
) -> Pin<Box<dyn Future<Output = ToolUploadOutcome> + Send + 'a>> {
    Box::pin(async move {
        let mut outcome = ToolUploadOutcome::default();
        match value {
            Value::Array(items) => {
                for item in items {
                    outcome.merge(upload_tool_value(item, policy, targets, uploader).await);
                }
            }
            Value::Object(object) => {
                if let Some(data_url) = object.get("data_url").and_then(Value::as_str) {
                    match parse_data_url(data_url) {
                        Ok(parsed) => {
                            let media_type = object
                                .get("media_type")
                                .and_then(Value::as_str)
                                .unwrap_or(parsed.media_type.as_str())
                                .to_string();
                            let preflight = MediaPreflight::inspect_with_policy(
                                &parsed.data,
                                Some(media_type.as_str()),
                                policy,
                            );
                            if should_upload(&preflight, &media_type, targets) {
                                let upload_media_type = upload_media_type(&preflight, &media_type);
                                match upload_payload(
                                    parsed.data,
                                    upload_media_type,
                                    preflight,
                                    uploader,
                                )
                                .await
                                {
                                    UploadOutcome::Uploaded(part) => {
                                        *value = content_part_json(&part);
                                        outcome.uploaded += 1;
                                    }
                                    UploadOutcome::Failed(error) => {
                                        outcome.failures.push(json!(error));
                                    }
                                    UploadOutcome::Skipped => {}
                                }
                            }
                        }
                        Err(error) => outcome.failures.push(json!(error)),
                    }
                    return outcome;
                }
                for item in object.values_mut() {
                    outcome.merge(upload_tool_value(item, policy, targets, uploader).await);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
        outcome
    })
}

async fn upload_payload(
    data: Vec<u8>,
    media_type: String,
    preflight: MediaPreflight,
    uploader: &Arc<dyn MediaUploader>,
) -> UploadOutcome {
    match uploader
        .upload(MediaUploadRequest {
            data,
            media_type,
            preflight,
        })
        .await
    {
        Ok(part) => UploadOutcome::Uploaded(part),
        Err(error) => UploadOutcome::Failed(error),
    }
}

fn should_upload(
    preflight: &MediaPreflight,
    declared_media_type: &str,
    targets: UploadTargets,
) -> bool {
    let media_type = preflight
        .corrected_media_type
        .as_deref()
        .unwrap_or(declared_media_type);
    (targets.images && media_type.starts_with("image/"))
        || (targets.videos && media_type.starts_with("video/"))
}

fn upload_media_type(preflight: &MediaPreflight, fallback: &str) -> String {
    preflight
        .corrected_media_type
        .clone()
        .unwrap_or_else(|| fallback.to_string())
}

fn content_part_json(part: &ContentPart) -> Value {
    serde_json::to_value(part).unwrap_or_else(|_| json!({}))
}

#[derive(Clone, Copy)]
struct UploadTargets {
    images: bool,
    videos: bool,
}

impl UploadTargets {
    fn from_context(context: &AgentContext) -> Self {
        Self {
            images: context
                .model_config
                .capabilities
                .contains(&ModelCapability::ImageUrl),
            videos: context
                .model_config
                .capabilities
                .contains(&ModelCapability::VideoUrl),
        }
    }

    const fn any(self) -> bool {
        self.images || self.videos
    }
}

enum UploadOutcome {
    Uploaded(ContentPart),
    Failed(String),
    Skipped,
}

#[derive(Default)]
struct ToolUploadOutcome {
    uploaded: usize,
    failures: Vec<Value>,
}

impl ToolUploadOutcome {
    fn merge(&mut self, other: Self) {
        self.uploaded += other.uploaded;
        self.failures.extend(other.failures);
    }
}
