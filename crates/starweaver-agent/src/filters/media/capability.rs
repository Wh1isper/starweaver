//! Model capability media filtering.

use serde_json::{json, Value};
use starweaver_context::{AgentContext, ModelCapability};
use starweaver_model::{ContentPart, ModelMessage, ModelRequestPart};
use starweaver_runtime::AgentRunState;

use crate::filters::message::request_metadata_mut;

pub(in crate::filters) fn capability_filter(
    _state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let capabilities = &context.model_config.capabilities;
    let has_vision = capabilities.contains(&ModelCapability::Vision);
    let has_video = capabilities.contains(&ModelCapability::VideoUnderstanding);
    let has_document = capabilities.contains(&ModelCapability::DocumentUnderstanding);
    if has_vision && has_video && has_document {
        return messages;
    }

    let mut removed = 0usize;
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        removed +=
                            filter_content_parts(content, has_vision, has_video, has_document);
                    }
                    ModelRequestPart::ToolReturn(tool_return) => {
                        let outcome = filter_tool_value(
                            &mut tool_return.content,
                            has_vision,
                            has_video,
                            has_document,
                        );
                        removed += outcome.removed_count();
                        if let Some(user_content) = &mut tool_return.user_content {
                            let outcome = filter_tool_value(
                                user_content,
                                has_vision,
                                has_video,
                                has_document,
                            );
                            removed += outcome.removed_count();
                        }
                        if let Some(content_parts) = tool_return
                            .private_metadata
                            .get_mut("starweaver_tool_return_content_parts")
                        {
                            let outcome = filter_tool_value(
                                content_parts,
                                has_vision,
                                has_video,
                                has_document,
                            );
                            removed += outcome.removed_count();
                        }
                    }
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::RetryPrompt { .. }
                    | ModelRequestPart::Instruction { .. } => {}
                }
            }
        }
    }
    if removed > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_capability_replacements".to_string(),
            json!(removed),
        );
    }
    messages
}

fn filter_content_parts(
    content: &mut Vec<ContentPart>,
    has_vision: bool,
    has_video: bool,
    has_document: bool,
) -> usize {
    let mut removed = RemovalOutcome::default();
    content.retain(|item| {
        unsupported_kind(
            content_part_media_kind(item),
            has_vision,
            has_video,
            has_document,
        )
        .map_or(true, |kind| {
            removed.mark(kind);
            false
        })
    });
    content.extend(
        removed
            .reminders()
            .into_iter()
            .map(|text| ContentPart::Text { text }),
    );
    removed.removed_count()
}

fn filter_tool_value(
    value: &mut Value,
    has_vision: bool,
    has_video: bool,
    has_document: bool,
) -> RemovalOutcome {
    if let Some(kind) = value_media_kind(value) {
        if let Some(kind) = unsupported_kind(Some(kind), has_vision, has_video, has_document) {
            let mut removed = RemovalOutcome::default();
            removed.mark(kind);
            *value = Value::String(removal_reminder(kind).to_string());
            return removed;
        }
        return RemovalOutcome::default();
    }

    match value {
        Value::Array(items) => {
            let mut removed = RemovalOutcome::default();
            let mut filtered = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                let child_removed =
                    filter_tool_value(&mut item, has_vision, has_video, has_document);
                if matches!(item, Value::String(_)) && child_removed.removed_count() > 0 {
                    removed.merge(&child_removed);
                    continue;
                }
                removed.merge(&child_removed);
                filtered.push(item);
            }
            filtered.extend(removed.reminders().into_iter().map(Value::String));
            *items = filtered;
            removed
        }
        Value::Object(object) => {
            let mut removed = RemovalOutcome::default();
            for item in object.values_mut() {
                removed.merge(&filter_tool_value(
                    item,
                    has_vision,
                    has_video,
                    has_document,
                ));
            }
            removed
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            RemovalOutcome::default()
        }
    }
}

const fn unsupported_kind(
    kind: Option<FilteredMediaKind>,
    has_vision: bool,
    has_video: bool,
    has_document: bool,
) -> Option<FilteredMediaKind> {
    match kind {
        Some(FilteredMediaKind::Image) if !has_vision => Some(FilteredMediaKind::Image),
        Some(FilteredMediaKind::Video) if !has_video => Some(FilteredMediaKind::Video),
        Some(FilteredMediaKind::Document) if !has_document => Some(FilteredMediaKind::Document),
        Some(_) | None => None,
    }
}

fn content_part_media_kind(item: &ContentPart) -> Option<FilteredMediaKind> {
    match item {
        ContentPart::ImageUrl { .. } => Some(FilteredMediaKind::Image),
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => media_type_kind(media_type),
        ContentPart::Text { .. } => None,
    }
}

fn value_media_kind(value: &Value) -> Option<FilteredMediaKind> {
    if let Ok(part) = serde_json::from_value::<ContentPart>(value.clone()) {
        return content_part_media_kind(&part);
    }
    let object = value.as_object()?;
    if object
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "image_url")
    {
        return Some(FilteredMediaKind::Image);
    }
    object
        .get("media_type")
        .and_then(Value::as_str)
        .and_then(media_type_kind)
}

fn media_type_kind(media_type: &str) -> Option<FilteredMediaKind> {
    let normalized = media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase();
    if normalized.starts_with("image/") {
        Some(FilteredMediaKind::Image)
    } else if normalized.starts_with("video/") {
        Some(FilteredMediaKind::Video)
    } else if is_document_media_type(&normalized) {
        Some(FilteredMediaKind::Document)
    } else {
        None
    }
}

fn is_document_media_type(media_type: &str) -> bool {
    media_type == "application/pdf"
        || media_type == "application/epub+zip"
        || media_type == "application/msword"
        || media_type == "application/vnd.ms-excel"
        || media_type == "application/vnd.ms-powerpoint"
        || media_type.starts_with("application/vnd.openxmlformats-officedocument")
}

const fn removal_reminder(kind: FilteredMediaKind) -> &'static str {
    match kind {
        FilteredMediaKind::Image => {
            "<filtered-content type='image'>Image content has been filtered out as the current model does not support vision.</filtered-content>"
        }
        FilteredMediaKind::Video => {
            "<filtered-content type='video'>Video content has been filtered out as the current model does not support video understanding.</filtered-content>"
        }
        FilteredMediaKind::Document => {
            "<filtered-content type='document'>Document content has been filtered out as the current model does not support document understanding.</filtered-content>"
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilteredMediaKind {
    Image,
    Video,
    Document,
}

#[derive(Default)]
struct RemovalOutcome {
    images: usize,
    videos: usize,
    documents: usize,
}

impl RemovalOutcome {
    fn mark(&mut self, kind: FilteredMediaKind) {
        match kind {
            FilteredMediaKind::Image => self.images += 1,
            FilteredMediaKind::Video => self.videos += 1,
            FilteredMediaKind::Document => self.documents += 1,
        }
    }

    const fn removed_count(&self) -> usize {
        self.images + self.videos + self.documents
    }

    fn reminders(&self) -> Vec<String> {
        let mut reminders = Vec::new();
        if self.images > 0 {
            reminders.push(removal_reminder(FilteredMediaKind::Image).to_string());
        }
        if self.videos > 0 {
            reminders.push(removal_reminder(FilteredMediaKind::Video).to_string());
        }
        if self.documents > 0 {
            reminders.push(removal_reminder(FilteredMediaKind::Document).to_string());
        }
        reminders
    }

    fn merge(&mut self, other: &Self) {
        self.images += other.images;
        self.videos += other.videos;
        self.documents += other.documents;
    }
}
