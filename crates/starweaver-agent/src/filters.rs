//! Named SDK history filter presets for ya-agent-sdk parity.

use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_model::{
    parse_data_url, ContentPart, MediaPolicy, MediaPreflight, ModelMessage, ModelRequest,
    ModelRequestPart, ModelResponsePart, ToolArguments, ToolReturnPart,
};
use starweaver_runtime::{
    AgentRunState, HistoryProcessor, HistoryProcessorError, HistoryProcessorResult,
    ReinjectSystemPromptProcessor, StaticCapabilityBundle,
};

/// Ordered default filter names for ya-agent-sdk behavioral parity.
pub const DEFAULT_FILTER_ORDER: &[&str] = &[
    "cold_start",
    "capability",
    "media_preflight",
    "media_upload",
    "compact",
    "handoff",
    "auto_load_files",
    "background_shell",
    "bus_message",
    "environment_instructions",
    "runtime_instructions",
    "system_prompt",
    "tool_args",
    "reasoning_normalize",
];

const FILTER_ORDER_METADATA: &str = "starweaver_filter_order";
const MEDIA_POLICY_METADATA: &str = "starweaver_media_policy";
const AUTO_LOAD_METADATA: &str = "starweaver_auto_load_files";
const BACKGROUND_SHELL_METADATA: &str = "starweaver_background_shell";
const BUS_MESSAGE_METADATA: &str = "starweaver_bus_messages";
const ENVIRONMENT_INSTRUCTIONS_METADATA: &str = "starweaver_environment_instructions";
const RUNTIME_INSTRUCTIONS_METADATA: &str = "starweaver_runtime_instructions";
const HANDOFF_METADATA: &str = "starweaver_handoff";
const COMPACT_KEEP_MESSAGES_METADATA: &str = "starweaver_compact_keep_messages";
const COLD_START_TOOL_RETURN_LIMIT_METADATA: &str = "starweaver_cold_start_tool_return_limit";

/// Build the default named filter bundle.
#[must_use]
pub fn default_filter_bundle() -> StaticCapabilityBundle {
    let mut bundle = StaticCapabilityBundle::new("ya-agent-sdk-filter-parity");
    for name in DEFAULT_FILTER_ORDER {
        bundle = bundle.with_history_processor(Arc::new(NamedFilterProcessor::new(name)));
    }
    bundle
}

/// Build named filter processors in default parity order.
#[must_use]
pub fn default_filter_processors() -> Vec<Arc<dyn HistoryProcessor>> {
    DEFAULT_FILTER_ORDER
        .iter()
        .map(|name| Arc::new(NamedFilterProcessor::new(name)) as Arc<dyn HistoryProcessor>)
        .collect()
}

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

/// Named SDK filter processor with concrete parity behavior.
#[derive(Clone)]
pub struct NamedFilterProcessor {
    name: &'static str,
    uploader: Option<Arc<dyn MediaUploader>>,
}

impl std::fmt::Debug for NamedFilterProcessor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamedFilterProcessor")
            .field("name", &self.name)
            .field("has_uploader", &self.uploader.is_some())
            .finish()
    }
}

impl NamedFilterProcessor {
    /// Create a named filter processor.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            uploader: None,
        }
    }

    /// Create a media upload processor with an adapter.
    #[must_use]
    pub fn media_upload(uploader: Arc<dyn MediaUploader>) -> Self {
        Self {
            name: "media_upload",
            uploader: Some(uploader),
        }
    }

    /// Return processor name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

#[async_trait]
impl HistoryProcessor for NamedFilterProcessor {
    async fn process(
        &self,
        state: &AgentRunState,
        messages: Vec<ModelMessage>,
    ) -> HistoryProcessorResult<Vec<ModelMessage>> {
        let mut messages = match self.name {
            "cold_start" => cold_start_filter(state, messages),
            "capability" => capability_filter(state, messages),
            "media_preflight" => media_preflight_filter(state, messages),
            "media_upload" => media_upload_filter(state, messages, self.uploader.as_ref()).await,
            "compact" => compact_filter(state, messages),
            "handoff" => {
                inject_instruction_from_metadata(state, messages, HANDOFF_METADATA, "handoff")
            }
            "auto_load_files" => auto_load_files_filter(state, messages),
            "background_shell" => background_shell_filter(state, messages),
            "bus_message" => bus_message_filter(state, messages),
            "environment_instructions" => inject_instruction_from_metadata(
                state,
                messages,
                ENVIRONMENT_INSTRUCTIONS_METADATA,
                "environment",
            ),
            "runtime_instructions" => inject_instruction_from_metadata(
                state,
                messages,
                RUNTIME_INSTRUCTIONS_METADATA,
                "runtime",
            ),
            "system_prompt" => {
                ReinjectSystemPromptProcessor::new()
                    .process(state, messages)
                    .await?
            }
            "tool_args" => tool_args_filter(messages),
            "reasoning_normalize" => reasoning_normalize_filter(messages),
            other => {
                return Err(HistoryProcessorError::failed(format!(
                    "unknown SDK filter '{other}'"
                )));
            }
        };
        record_filter_order(&mut messages, self.name);
        Ok(messages)
    }
}

fn cold_start_filter(state: &AgentRunState, mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let limit = state
        .metadata
        .get(COLD_START_TOOL_RETURN_LIMIT_METADATA)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(4096);
    let mut truncated = 0usize;
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                if let ModelRequestPart::ToolReturn(tool_return) = part {
                    if truncate_tool_return(tool_return, limit) {
                        truncated += 1;
                    }
                }
            }
        }
    }
    if truncated > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_cold_start_truncated_tool_returns".to_string(),
            json!(truncated),
        );
    }
    if !state.idle_messages.is_empty() {
        push_user_text(
            &mut messages,
            format!("Cold-start context: {}", state.idle_messages.join("\n")),
            "cold_start",
        );
    }
    messages
}

fn capability_filter(state: &AgentRunState, mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let policy = media_policy_from_state(state);
    let mut replaced = 0usize;
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                if let ModelRequestPart::UserPrompt { content, .. } = part {
                    for item in content {
                        if let Some(reason) = content_policy_reason(item, &policy) {
                            *item = ContentPart::Text {
                                text: format!(
                                    "System reminder: media part was removed by capability policy: {reason}."
                                ),
                            };
                            replaced += 1;
                        }
                    }
                }
            }
        }
    }
    if replaced > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_capability_replacements".to_string(),
            json!(replaced),
        );
    }
    messages
}

fn media_preflight_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let policy = media_policy_from_state(state);
    let mut reports = Vec::new();
    let mut replacements = 0usize;
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        for item in content {
                            let result = preflight_content_part(item, &policy);
                            if result.replaced {
                                replacements += 1;
                            }
                            reports.extend(result.reports);
                        }
                    }
                    ModelRequestPart::ToolReturn(tool_return) => {
                        let result = preflight_tool_value(&mut tool_return.content, &policy);
                        if result.replaced {
                            replacements += 1;
                        }
                        reports.extend(result.reports);
                    }
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::RetryPrompt { .. }
                    | ModelRequestPart::Instruction { .. } => {}
                }
            }
        }
    }
    let limits = enforce_media_count_limits(&mut messages, &policy);
    replacements += limits;
    if !reports.is_empty() {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_preflight".to_string(),
            Value::Array(reports),
        );
    }
    if replacements > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_replacements".to_string(),
            json!(replacements),
        );
    }
    messages
}

async fn media_upload_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
    uploader: Option<&Arc<dyn MediaUploader>>,
) -> Vec<ModelMessage> {
    let Some(media_uploader) = uploader else {
        return messages;
    };
    let policy = media_policy_from_state(state);
    let mut upload_count = 0usize;
    let mut failures = Vec::new();
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        for item in content {
                            match upload_content_part(item, &policy, media_uploader).await {
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
                        let outcome =
                            upload_tool_value(&mut tool_return.content, &policy, media_uploader)
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

fn compact_filter(state: &AgentRunState, messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let Some(keep) = state
        .metadata
        .get(COMPACT_KEEP_MESSAGES_METADATA)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return messages;
    };
    if keep == 0 || messages.len() <= keep {
        return messages;
    }
    let mut instructions = Vec::new();
    for message in &messages {
        if let ModelMessage::Request(request) = message {
            let parts = request
                .parts
                .iter()
                .filter(|part| {
                    matches!(
                        part,
                        ModelRequestPart::SystemPrompt { .. }
                            | ModelRequestPart::Instruction { .. }
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            if !parts.is_empty() {
                instructions.push(ModelMessage::Request(ModelRequest {
                    parts,
                    timestamp: request.timestamp,
                    instructions: None,
                    run_id: request.run_id.clone(),
                    conversation_id: request.conversation_id.clone(),
                    metadata: Map::new(),
                }));
            }
        }
    }
    let mut compacted = messages.into_iter().rev().take(keep).collect::<Vec<_>>();
    compacted.reverse();
    for instruction in instructions.into_iter().rev() {
        if !compacted.contains(&instruction) {
            compacted.insert(0, instruction);
        }
    }
    request_metadata_mut(&mut compacted).insert("starweaver_compacted".to_string(), json!(true));
    compacted
}

fn inject_instruction_from_metadata(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
    metadata_key: &str,
    instruction_type: &str,
) -> Vec<ModelMessage> {
    let Some(text) = metadata_text(&state.metadata, metadata_key) else {
        return messages;
    };
    let part = ModelRequestPart::Instruction {
        text,
        metadata: Map::from_iter([(
            "starweaver_instruction_type".to_string(),
            json!(instruction_type),
        )]),
    };
    insert_request_part_before_latest_user(&mut messages, part);
    messages
}

fn auto_load_files_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let Some(files) = state
        .metadata
        .get(AUTO_LOAD_METADATA)
        .and_then(Value::as_array)
    else {
        return messages;
    };
    let mut loaded = Vec::new();
    for file in files {
        let path = file
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let content = file
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        loaded.push(format!("## {path}\n{content}"));
    }
    if loaded.is_empty() {
        return messages;
    }
    push_user_text(
        &mut messages,
        format!("Auto-loaded files:\n{}", loaded.join("\n\n")),
        "auto_load_files",
    );
    messages
}

fn background_shell_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let Some(processes) = state
        .metadata
        .get(BACKGROUND_SHELL_METADATA)
        .and_then(Value::as_array)
    else {
        return messages;
    };
    if processes.is_empty() {
        return messages;
    }
    push_user_text(
        &mut messages,
        format!(
            "Background shell updates: {}",
            Value::Array(processes.clone())
        ),
        "background_shell",
    );
    messages
}

fn bus_message_filter(state: &AgentRunState, mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let Some(bus_messages) = state
        .metadata
        .get(BUS_MESSAGE_METADATA)
        .and_then(Value::as_array)
    else {
        return messages;
    };
    if bus_messages.is_empty() {
        return messages;
    }
    push_user_text(
        &mut messages,
        format!(
            "Message bus updates: {}",
            Value::Array(bus_messages.clone())
        ),
        "bus_message",
    );
    messages
}

fn tool_args_filter(mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let mut repaired = 0usize;
    for message in &mut messages {
        if let ModelMessage::Response(response) = message {
            for part in &mut response.parts {
                if let ModelResponsePart::ToolCall(call) = part {
                    if let Some(repaired_args) = repair_tool_arguments(&call.arguments) {
                        call.arguments = repaired_args;
                        repaired += 1;
                    }
                }
            }
        }
    }
    if repaired > 0 {
        request_metadata_mut(&mut messages)
            .insert("starweaver_tool_args_repaired".to_string(), json!(repaired));
    }
    messages
}

fn reasoning_normalize_filter(mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let mut removed = 0usize;
    for message in &mut messages {
        if let ModelMessage::Response(response) = message {
            let before = response.parts.len();
            response.parts.retain(|part| match part {
                ModelResponsePart::Thinking { text, .. } => !text.trim().is_empty(),
                _ => true,
            });
            removed += before.saturating_sub(response.parts.len());
            for part in &mut response.parts {
                if let ModelResponsePart::Thinking { text, signature } = part {
                    *text = normalize_reasoning_text(text);
                    if signature.as_deref().is_some_and(str::is_empty) {
                        *signature = None;
                    }
                }
            }
        }
    }
    if removed > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_reasoning_removed_empty".to_string(),
            json!(removed),
        );
    }
    messages
}

fn truncate_tool_return(tool_return: &mut ToolReturnPart, limit: usize) -> bool {
    let Ok(serialized) = serde_json::to_string(&tool_return.content) else {
        return false;
    };
    if serialized.len() <= limit {
        return false;
    }
    tool_return.content = json!({
        "starweaver_truncated": true,
        "original_bytes": serialized.len(),
        "preview": serialized.chars().take(limit).collect::<String>(),
    });
    tool_return
        .metadata
        .insert("starweaver_truncated".to_string(), json!(true));
    true
}

fn preflight_content_part(item: &mut ContentPart, policy: &MediaPolicy) -> PreflightOutcome {
    match item {
        ContentPart::Binary { data, media_type } => {
            let preflight = MediaPreflight::inspect_with_policy(data, Some(media_type), policy);
            let report = preflight_report(&preflight);
            if let Some(corrected) = preflight.corrected_media_type.clone() {
                *media_type = corrected;
            }
            if preflight.corrupt || !preflight.allowed_by_policy {
                *item = ContentPart::Text {
                    text: replacement_text(&preflight),
                };
                PreflightOutcome::replaced(report)
            } else {
                PreflightOutcome::reported(report)
            }
        }
        ContentPart::DataUrl {
            data_url,
            media_type,
        } => match parse_data_url(data_url) {
            Ok(parsed) => {
                let preflight = MediaPreflight::inspect_with_policy(
                    &parsed.data,
                    Some(parsed.media_type.as_str()),
                    policy,
                );
                let report = preflight_report(&preflight);
                if let Some(corrected) = preflight.corrected_media_type.clone() {
                    *media_type = corrected;
                }
                if preflight.corrupt || !preflight.allowed_by_policy {
                    *item = ContentPart::Text {
                        text: replacement_text(&preflight),
                    };
                    PreflightOutcome::replaced(report)
                } else {
                    PreflightOutcome::reported(report)
                }
            }
            Err(error) => {
                *item = ContentPart::Text {
                    text: format!("System reminder: data URL media was removed: {error}."),
                };
                PreflightOutcome::replaced(json!({ "error": error }))
            }
        },
        ContentPart::ImageUrl { .. }
        | ContentPart::FileUrl { .. }
        | ContentPart::ResourceRef { .. }
        | ContentPart::Text { .. } => PreflightOutcome::default(),
    }
}

fn preflight_tool_value(value: &mut Value, policy: &MediaPolicy) -> PreflightOutcome {
    let mut outcome = PreflightOutcome::default();
    match value {
        Value::Array(items) => {
            for item in items {
                outcome.merge(preflight_tool_value(item, policy));
            }
        }
        Value::Object(object) => {
            if let Some(data_url) = object.get("data_url").and_then(Value::as_str) {
                match parse_data_url(data_url) {
                    Ok(parsed) => {
                        let preflight = MediaPreflight::inspect_with_policy(
                            &parsed.data,
                            object
                                .get("media_type")
                                .and_then(Value::as_str)
                                .or(Some(parsed.media_type.as_str())),
                            policy,
                        );
                        outcome.reports.push(preflight_report(&preflight));
                        if let Some(media_type) = preflight.corrected_media_type.clone() {
                            object.insert("media_type".to_string(), json!(media_type));
                        }
                        if preflight.corrupt || !preflight.allowed_by_policy {
                            *value = json!({ "type": "system_reminder", "text": replacement_text(&preflight) });
                            outcome.replaced = true;
                        }
                    }
                    Err(error) => {
                        *value = json!({ "type": "system_reminder", "text": format!("System reminder: data URL media was removed: {error}.") });
                        outcome.replaced = true;
                    }
                }
                return outcome;
            }
            for item in object.values_mut() {
                outcome.merge(preflight_tool_value(item, policy));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    outcome
}

async fn upload_content_part(
    item: &ContentPart,
    policy: &MediaPolicy,
    uploader: &Arc<dyn MediaUploader>,
) -> UploadOutcome {
    match item {
        ContentPart::Binary { data, media_type } => {
            let preflight = MediaPreflight::inspect_with_policy(data, Some(media_type), policy);
            if should_upload(&preflight) {
                upload_payload(data.clone(), media_type.clone(), preflight, uploader).await
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
                if should_upload(&preflight) {
                    upload_payload(parsed.data, media_type.clone(), preflight, uploader).await
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
    uploader: &'a Arc<dyn MediaUploader>,
) -> Pin<Box<dyn Future<Output = ToolUploadOutcome> + Send + 'a>> {
    Box::pin(async move {
        let mut outcome = ToolUploadOutcome::default();
        match value {
            Value::Array(items) => {
                for item in items {
                    outcome.merge(upload_tool_value(item, policy, uploader).await);
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
                            if should_upload(&preflight) {
                                match upload_payload(parsed.data, media_type, preflight, uploader)
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
                    outcome.merge(upload_tool_value(item, policy, uploader).await);
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

const fn should_upload(preflight: &MediaPreflight) -> bool {
    preflight.over_base64_budget || preflight.detected_kind.is_video()
}

fn enforce_media_count_limits(messages: &mut [ModelMessage], policy: &MediaPolicy) -> usize {
    let mut image_count = 0usize;
    let mut video_count = 0usize;
    let mut replaced = 0usize;
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            for part in request.parts.iter_mut().rev() {
                if let ModelRequestPart::UserPrompt { content, .. } = part {
                    for item in content.iter_mut().rev() {
                        if is_image_content(item) {
                            image_count += 1;
                            if policy.max_images.is_some_and(|limit| image_count > limit) {
                                *item = ContentPart::Text {
                                    text: "System reminder: older image omitted because the model image count limit was reached.".to_string(),
                                };
                                replaced += 1;
                            }
                        } else if is_video_content(item) {
                            video_count += 1;
                            if policy.max_videos.is_some_and(|limit| video_count > limit) {
                                *item = ContentPart::Text {
                                    text: "System reminder: older video omitted because the model video count limit was reached.".to_string(),
                                };
                                replaced += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    replaced
}

fn is_image_content(item: &ContentPart) -> bool {
    match item {
        ContentPart::ImageUrl { .. } => true,
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => media_type.starts_with("image/"),
        ContentPart::Text { .. } => false,
    }
}

fn is_video_content(item: &ContentPart) -> bool {
    match item {
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => media_type.starts_with("video/"),
        ContentPart::ImageUrl { .. } | ContentPart::Text { .. } => false,
    }
}

fn content_policy_reason(item: &ContentPart, policy: &MediaPolicy) -> Option<String> {
    match item {
        ContentPart::ImageUrl { .. } if !policy.allow_images => {
            Some("image media is disabled".to_string())
        }
        ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::Binary { media_type, .. } => {
            if media_type.starts_with("image/") && !policy.allow_images {
                Some("image media is disabled".to_string())
            } else if media_type.starts_with("video/") && !policy.allow_videos {
                Some("video media is disabled".to_string())
            } else if !media_type.starts_with("image/")
                && !media_type.starts_with("video/")
                && !policy.allow_documents
            {
                Some("document media is disabled".to_string())
            } else if media_type == "image/gif" && !policy.allow_gif {
                Some("gif media is disabled".to_string())
            } else if media_type == "image/webp" && !policy.allow_webp {
                Some("webp media is disabled".to_string())
            } else {
                None
            }
        }
        ContentPart::Text { .. } | ContentPart::ImageUrl { .. } => None,
    }
}

fn repair_tool_arguments(arguments: &ToolArguments) -> Option<ToolArguments> {
    match arguments {
        ToolArguments::RawJsonString(text) => serde_json::from_str::<Value>(text)
            .ok()
            .map(ToolArguments::parsed),
        ToolArguments::Invalid { raw, .. } => Some(ToolArguments::parsed(json!({
            "starweaver_argument_repair": "invalid_json_string",
            "raw": raw,
        }))),
        ToolArguments::Parsed(Value::String(text)) => serde_json::from_str::<Value>(text)
            .ok()
            .map(ToolArguments::parsed)
            .or_else(|| {
                Some(ToolArguments::parsed(json!({
                    "starweaver_argument_repair": "invalid_json_string",
                    "raw": text,
                })))
            }),
        ToolArguments::Parsed(Value::Null) => Some(ToolArguments::parsed(json!({}))),
        ToolArguments::Parsed(Value::Object(_)) => None,
        ToolArguments::Parsed(other) => Some(ToolArguments::parsed(json!({
            "starweaver_argument_repair": "non_object_arguments",
            "value": other,
        }))),
    }
}

fn normalize_reasoning_text(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn media_policy_from_state(state: &AgentRunState) -> MediaPolicy {
    state
        .metadata
        .get(MEDIA_POLICY_METADATA)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn metadata_text(metadata: &Map<String, Value>, key: &str) -> Option<String> {
    match metadata.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        other => Some(other.to_string()),
    }
    .filter(|text| !text.trim().is_empty())
}

fn push_user_text(messages: &mut Vec<ModelMessage>, text: String, source: &str) {
    let request = ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text { text }],
            name: None,
            metadata: Map::from_iter([("starweaver_filter_source".to_string(), json!(source))]),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    };
    messages.push(ModelMessage::Request(request));
}

fn insert_request_part_before_latest_user(
    messages: &mut Vec<ModelMessage>,
    part: ModelRequestPart,
) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            request.parts.insert(0, part);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![part],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
}

fn request_metadata_mut(messages: &mut Vec<ModelMessage>) -> &mut Map<String, Value> {
    let needs_request = !matches!(messages.last(), Some(ModelMessage::Request(_)));
    if needs_request {
        messages.push(ModelMessage::Request(ModelRequest {
            parts: Vec::new(),
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }));
    }
    match messages.last_mut() {
        Some(ModelMessage::Request(request)) => &mut request.metadata,
        Some(ModelMessage::Response(_)) | None => unreachable!("request metadata ensured"),
    }
}

fn record_filter_order(messages: &mut Vec<ModelMessage>, name: &str) {
    let entry = request_metadata_mut(messages)
        .entry(FILTER_ORDER_METADATA.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut() {
        items.push(Value::String(name.to_string()));
    }
}

fn preflight_report(preflight: &MediaPreflight) -> Value {
    serde_json::to_value(preflight).unwrap_or_else(|_| json!({}))
}

fn replacement_text(preflight: &MediaPreflight) -> String {
    preflight.corruption_reason.as_ref().map_or_else(
        || {
            preflight.policy_reason.as_ref().map_or_else(
                || "System reminder: media payload was removed by media preflight.".to_string(),
                |reason| {
                    format!("System reminder: media payload was removed by media policy: {reason}.")
                },
            )
        },
        |reason| {
            format!("System reminder: media payload was removed because it is corrupt: {reason}.")
        },
    )
}

fn content_part_json(part: &ContentPart) -> Value {
    serde_json::to_value(part).unwrap_or_else(|_| json!({}))
}

#[derive(Default)]
struct PreflightOutcome {
    reports: Vec<Value>,
    replaced: bool,
}

impl PreflightOutcome {
    fn reported(report: Value) -> Self {
        Self {
            reports: vec![report],
            replaced: false,
        }
    }

    fn replaced(report: Value) -> Self {
        Self {
            reports: vec![report],
            replaced: true,
        }
    }

    fn merge(&mut self, other: Self) {
        self.reports.extend(other.reports);
        self.replaced |= other.replaced;
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
