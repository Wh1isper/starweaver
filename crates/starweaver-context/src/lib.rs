//! Agent context, state, event bus, and message bus primitives for Starweaver.

use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use starweaver_core::AgentId;
use starweaver_core::{ConversationId, Metadata, RunId, TraceContext, Usage, XmlWriter};
use starweaver_model::ModelMessage;

/// In-memory state store for context domains.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateStore {
    domains: BTreeMap<String, Value>,
}

impl StateStore {
    /// Create an empty state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a domain value.
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.domains.insert(key.into(), value);
    }

    /// Get a domain value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.domains.get(key)
    }

    /// Remove a domain value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.domains.remove(key)
    }

    /// Return all domains.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn domains(&self) -> &BTreeMap<String, Value> {
        &self.domains
    }
}

/// Runtime event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvent {
    /// Event type.
    pub kind: String,
    /// Event payload.
    #[serde(default)]
    pub payload: Value,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentEvent {
    /// Create an event.
    #[must_use]
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            metadata: Metadata::default(),
        }
    }

    /// Attach event metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Append-only in-memory event bus.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventBus {
    events: Vec<AgentEvent>,
}

impl EventBus {
    /// Create an empty event bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish one event.
    pub fn publish(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    /// Return the number of retained events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Return whether the event bus is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Return all events.
    #[must_use]
    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    /// Drain all events.
    pub fn drain(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.events)
    }
}

/// Steering or coordination message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BusMessage {
    /// Message topic.
    pub topic: String,
    /// Message payload.
    #[serde(default)]
    pub payload: Value,
    /// Message metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl BusMessage {
    /// Create a bus message.
    #[must_use]
    pub fn new(topic: impl Into<String>, payload: Value) -> Self {
        Self {
            topic: topic.into(),
            payload,
            metadata: Metadata::default(),
        }
    }
}

/// FIFO message bus for steering active and future runs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageBus {
    messages: VecDeque<BusMessage>,
}

impl MessageBus {
    /// Create an empty message bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue one message.
    pub fn enqueue(&mut self, message: BusMessage) {
        self.messages.push_back(message);
    }

    /// Dequeue one message.
    pub fn dequeue(&mut self) -> Option<BusMessage> {
        self.messages.pop_front()
    }

    /// Return number of queued messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Return whether any queued message has the provided topic.
    #[must_use]
    pub fn has_topic(&self, topic: &str) -> bool {
        self.messages.iter().any(|message| message.topic == topic)
    }

    /// Return whether the bus has no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Serializable note store carried by context state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct NoteStore {
    notes: BTreeMap<String, String>,
}

impl NoteStore {
    /// Create an empty note store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a note value.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.notes.insert(key.into(), value.into());
    }

    /// Get a note value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.notes.get(key).map(String::as_str)
    }

    /// Delete a note value and return whether it existed.
    pub fn delete(&mut self, key: &str) -> bool {
        self.notes.remove(key).is_some()
    }

    /// Return all notes sorted by key.
    #[must_use]
    pub fn list_all(&self) -> Vec<(String, String)> {
        self.notes
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Return a serializable copy of all notes.
    #[must_use]
    pub fn export_notes(&self) -> BTreeMap<String, String> {
        self.notes.clone()
    }

    /// Restore notes from exported data.
    #[must_use]
    pub const fn from_exported(notes: BTreeMap<String, String>) -> Self {
        Self { notes }
    }

    /// Return whether the store has no notes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
}

/// Serializable state used to restore an agent context.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumableState {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier when exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
    /// Accumulated usage.
    #[serde(default)]
    pub usage: Usage,
    /// Model/runtime configuration used for injected runtime context and tool policies.
    #[serde(default, skip_serializing_if = "RuntimeModelConfig::is_default")]
    pub model_config: RuntimeModelConfig,
    /// Tool-level configuration used by first-party and host tools.
    #[serde(default, skip_serializing_if = "ToolConfig::is_default")]
    pub tool_config: ToolConfig,
    /// Security-related runtime configuration.
    #[serde(default, skip_serializing_if = "SecurityConfig::is_empty")]
    pub security: SecurityConfig,
    /// Context creation time used for elapsed runtime context.
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
    /// State domains.
    #[serde(default)]
    pub state: StateStore,
    /// Persisted notes.
    #[serde(default, skip_serializing_if = "NoteStore::is_empty")]
    pub notes: NoteStore,
    /// Pending bus messages.
    #[serde(default)]
    pub message_bus: MessageBus,
    /// Trace correlation snapshot.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_snapshot: TraceContext,
    /// Run metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Type-indexed dependency container for runtime and tool contexts.
#[derive(Clone, Default)]
pub struct DependencyStore {
    values: BTreeMap<String, Arc<dyn Any + Send + Sync>>,
    type_keys: BTreeMap<TypeId, String>,
}

impl DependencyStore {
    /// Create an empty dependency store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a dependency using its Rust type as the lookup key.
    pub fn insert<T>(&mut self, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.insert_named(std::any::type_name::<T>(), value);
    }

    /// Insert a dependency with a caller-provided stable name.
    pub fn insert_named<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        let name = name.into();
        self.type_keys.insert(TypeId::of::<T>(), name.clone());
        self.values.insert(name, Arc::new(value));
    }

    /// Get a dependency by Rust type.
    #[must_use]
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.type_keys
            .get(&TypeId::of::<T>())
            .and_then(|name| self.get_named(name))
    }

    /// Get a dependency by stable name.
    #[must_use]
    pub fn get_named<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.values
            .get(name)
            .cloned()
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Return all named dependency keys.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    /// Return whether the store has no dependencies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl std::fmt::Debug for DependencyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DependencyStore")
            .field("keys", &self.keys())
            .finish()
    }
}

/// Fixed-point ratio stored as parts per thousand.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Ratio {
    parts_per_thousand: u16,
}

impl Ratio {
    /// Create a ratio from parts per thousand.
    #[must_use]
    pub const fn from_parts_per_thousand(parts_per_thousand: u16) -> Self {
        Self { parts_per_thousand }
    }

    /// Return the ratio as parts per thousand.
    #[must_use]
    pub const fn parts_per_thousand(self) -> u16 {
        self.parts_per_thousand
    }

    /// Return the ratio as a floating point value for calculations.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.parts_per_thousand) / 1000.0
    }
}

impl Default for Ratio {
    fn default() -> Self {
        Self::from_parts_per_thousand(1000)
    }
}

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
    ReasoningForeignIncompatible,
}

/// Runtime model configuration stored on [`AgentContext`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelConfig {
    /// Context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Ratio where proactive context reminders should begin.
    #[serde(default = "default_proactive_context_management_threshold")]
    pub proactive_context_management_threshold: Option<Ratio>,
    /// Ratio where compacting becomes urgent.
    #[serde(default = "default_compact_threshold")]
    pub compact_threshold: Ratio,
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
    /// Maximum image bytes before image compression/splitting policy applies.
    #[serde(default = "default_model_max_image_bytes")]
    pub max_image_bytes: usize,
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

    /// Return whether no model-facing runtime config should be rendered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.context_window.is_none()
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
        self.split_large_images = other.split_large_images;
        self.image_split_max_height = other.image_split_max_height;
        self.image_split_overlap = other.image_split_overlap;
        if !other.capabilities.is_empty() {
            self.capabilities = other.capabilities;
        }
    }
}

/// Backwards-compatible alias for the context model configuration.
pub type RuntimeModelConfig = ModelConfig;

/// Shell review action for commands that require policy intervention.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewAction {
    /// Defer command execution for external approval.
    #[default]
    Defer,
    /// Deny commands that need approval.
    Deny,
}

/// Shell review risk threshold.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewRiskLevel {
    /// Low risk.
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    #[default]
    High,
    /// Extra high risk.
    ExtraHigh,
}

/// Shell command safety review configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewConfig {
    /// Whether shell review is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Model identifier used for shell review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Action when a command needs approval.
    #[serde(default)]
    pub on_needs_approval: ShellReviewAction,
    /// Risk level where intervention begins.
    #[serde(default)]
    pub risk_threshold: ShellReviewRiskLevel,
    /// Optional custom system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// Security-related runtime configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecurityConfig {
    /// Optional shell command safety review configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_review: Option<ShellReviewConfig>,
}

impl SecurityConfig {
    /// Return whether no security config is active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Tool-level configuration stored on [`AgentContext`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolConfig {
    /// Skip SSRF URL verification for URL-fetching tools.
    #[serde(default = "default_true")]
    pub skip_url_verification: bool,
    /// Enable document URL parsing in load-style document tools.
    #[serde(default)]
    pub enable_load_document: bool,
    /// Model used for image understanding fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_understanding_model: Option<String>,
    /// Model used for video understanding fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_understanding_model: Option<String>,
    /// Model used for audio understanding fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_understanding_model: Option<String>,
    /// Google Custom Search API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_search_api_key: Option<String>,
    /// Google Custom Search Engine id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_search_cx: Option<String>,
    /// Tavily API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tavily_api_key: Option<String>,
    /// Brave Search API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brave_search_api_key: Option<String>,
    /// Pixabay API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixabay_api_key: Option<String>,
    /// `RapidAPI` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rapidapi_api_key: Option<String>,
    /// Firecrawl API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firecrawl_api_key: Option<String>,
    /// Maximum text file size the view tool will inspect.
    #[serde(default = "default_view_max_text_file_size")]
    pub view_max_text_file_size: u64,
    /// Static relaxed text view path patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub view_relaxed_text_patterns: Vec<String>,
    /// Runtime relaxed text view patterns keyed by source id.
    #[serde(default, skip)]
    pub view_relaxed_text_dynamic_patterns: BTreeMap<String, Vec<String>>,
    /// Maximum text file size for relaxed text paths.
    #[serde(default = "default_view_relaxed_text_file_size")]
    pub view_relaxed_text_file_size: u64,
    /// Default line limit for relaxed text paths.
    #[serde(default = "default_view_relaxed_line_limit")]
    pub view_relaxed_line_limit: usize,
    /// Default max line length for relaxed text paths.
    #[serde(default = "default_view_relaxed_max_line_length")]
    pub view_relaxed_max_line_length: usize,
    /// Maximum returned text characters for normal view output.
    #[serde(default = "default_view_max_content_chars")]
    pub view_max_content_chars: usize,
    /// Maximum returned text characters for relaxed view output.
    #[serde(default = "default_view_relaxed_max_content_chars")]
    pub view_relaxed_max_content_chars: usize,
    /// Maximum file size `edit` and `multi_edit` will process.
    #[serde(default = "default_edit_max_file_size")]
    pub edit_max_file_size: u64,
    /// Maximum file size grep will read per file.
    #[serde(default = "default_grep_max_file_size")]
    pub grep_max_file_size: u64,
    /// Maximum image size view will inline.
    #[serde(default = "default_view_max_inline_image_bytes")]
    pub view_max_inline_image_bytes: u64,
    /// Maximum video size view will inline.
    #[serde(default = "default_view_max_inline_video_bytes")]
    pub view_max_inline_video_bytes: u64,
    /// Maximum audio size view will inline.
    #[serde(default = "default_view_max_inline_audio_bytes")]
    pub view_max_inline_audio_bytes: u64,
    /// Chunk size for streamed HTTP reads.
    #[serde(default = "default_fetch_stream_chunk_size")]
    pub fetch_stream_chunk_size: usize,
    /// Maximum binary response size fetch will inline.
    #[serde(default = "default_fetch_max_inline_binary_bytes")]
    pub fetch_max_inline_binary_bytes: u64,
    /// Maximum concurrent downloads.
    #[serde(default = "default_download_max_concurrency")]
    pub download_max_concurrency: usize,
    /// Maximum file size for document conversion tools.
    #[serde(default = "default_document_max_file_size")]
    pub document_max_file_size: u64,
    /// Filesystem generic output truncation limit.
    #[serde(default = "default_filesystem_output_truncate_limit")]
    pub filesystem_output_truncate_limit: usize,
    /// Grep soft truncation threshold.
    #[serde(default = "default_grep_truncation_threshold")]
    pub grep_truncation_threshold: usize,
    /// Grep matching line max characters after truncation.
    #[serde(default = "default_grep_truncated_line_max")]
    pub grep_truncated_line_max: usize,
    /// Shell output truncation limit.
    #[serde(default = "default_shell_output_truncate_limit")]
    pub shell_output_truncate_limit: usize,
    /// Cold-start history tool return character limit.
    #[serde(default = "default_cold_start_tool_return_limit")]
    pub cold_start_tool_return_limit: usize,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            skip_url_verification: true,
            enable_load_document: false,
            image_understanding_model: None,
            video_understanding_model: None,
            audio_understanding_model: None,
            google_search_api_key: None,
            google_search_cx: None,
            tavily_api_key: None,
            brave_search_api_key: None,
            pixabay_api_key: None,
            rapidapi_api_key: None,
            firecrawl_api_key: None,
            view_max_text_file_size: default_view_max_text_file_size(),
            view_relaxed_text_patterns: Vec::new(),
            view_relaxed_text_dynamic_patterns: BTreeMap::new(),
            view_relaxed_text_file_size: default_view_relaxed_text_file_size(),
            view_relaxed_line_limit: default_view_relaxed_line_limit(),
            view_relaxed_max_line_length: default_view_relaxed_max_line_length(),
            view_max_content_chars: default_view_max_content_chars(),
            view_relaxed_max_content_chars: default_view_relaxed_max_content_chars(),
            edit_max_file_size: default_edit_max_file_size(),
            grep_max_file_size: default_grep_max_file_size(),
            view_max_inline_image_bytes: default_view_max_inline_image_bytes(),
            view_max_inline_video_bytes: default_view_max_inline_video_bytes(),
            view_max_inline_audio_bytes: default_view_max_inline_audio_bytes(),
            fetch_stream_chunk_size: default_fetch_stream_chunk_size(),
            fetch_max_inline_binary_bytes: default_fetch_max_inline_binary_bytes(),
            download_max_concurrency: default_download_max_concurrency(),
            document_max_file_size: default_document_max_file_size(),
            filesystem_output_truncate_limit: default_filesystem_output_truncate_limit(),
            grep_truncation_threshold: default_grep_truncation_threshold(),
            grep_truncated_line_max: default_grep_truncated_line_max(),
            shell_output_truncate_limit: default_shell_output_truncate_limit(),
            cold_start_tool_return_limit: default_cold_start_tool_return_limit(),
        }
    }
}

impl ToolConfig {
    /// Return whether the persisted config only contains default values.
    ///
    /// Runtime-only dynamic relaxed patterns do not make the config persistent.
    #[must_use]
    pub fn is_default(&self) -> bool {
        let mut persisted = self.clone();
        persisted.view_relaxed_text_dynamic_patterns.clear();
        persisted == Self::default()
    }

    /// Return all static and runtime relaxed text patterns in deterministic order.
    #[must_use]
    pub fn view_relaxed_text_patterns(&self) -> Vec<&str> {
        let mut patterns = self
            .view_relaxed_text_patterns
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        for source_patterns in self.view_relaxed_text_dynamic_patterns.values() {
            patterns.extend(source_patterns.iter().map(String::as_str));
        }
        patterns
    }

    /// Register runtime relaxed text patterns for a source id.
    pub fn register_view_relaxed_text_patterns(
        &mut self,
        source: impl Into<String>,
        patterns: impl IntoIterator<Item = String>,
    ) {
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect::<Vec<_>>();
        let source = source.into();
        if patterns.is_empty() {
            self.view_relaxed_text_dynamic_patterns.remove(&source);
        } else {
            self.view_relaxed_text_dynamic_patterns
                .insert(source, patterns);
        }
    }

    /// Remove runtime relaxed text patterns for a source id.
    pub fn unregister_view_relaxed_text_patterns(&mut self, source: &str) {
        self.view_relaxed_text_dynamic_patterns.remove(source);
    }

    /// Normalize concurrency fields after deserialization or direct mutation.
    pub fn normalize(&mut self) {
        self.download_max_concurrency = self.download_max_concurrency.max(1);
    }
}

const fn default_true() -> bool {
    true
}

#[allow(clippy::unnecessary_wraps)]
const fn default_proactive_context_management_threshold() -> Option<Ratio> {
    Some(Ratio::from_parts_per_thousand(650))
}

const fn default_compact_threshold() -> Ratio {
    Ratio::from_parts_per_thousand(900)
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

const fn default_image_split_max_height() -> usize {
    4096
}

const fn default_image_split_overlap() -> usize {
    50
}

const fn default_view_max_text_file_size() -> u64 {
    10 * 1024 * 1024
}

const fn default_view_relaxed_text_file_size() -> u64 {
    50 * 1024 * 1024
}

const fn default_view_relaxed_line_limit() -> usize {
    5000
}

const fn default_view_relaxed_max_line_length() -> usize {
    20_000
}

const fn default_view_max_content_chars() -> usize {
    60_000
}

const fn default_view_relaxed_max_content_chars() -> usize {
    250_000
}

const fn default_edit_max_file_size() -> u64 {
    20 * 1024 * 1024
}

const fn default_grep_max_file_size() -> u64 {
    10 * 1024 * 1024
}

const fn default_view_max_inline_image_bytes() -> u64 {
    20 * 1024 * 1024
}

const fn default_view_max_inline_video_bytes() -> u64 {
    50 * 1024 * 1024
}

const fn default_view_max_inline_audio_bytes() -> u64 {
    50 * 1024 * 1024
}

const fn default_fetch_stream_chunk_size() -> usize {
    64 * 1024
}

const fn default_fetch_max_inline_binary_bytes() -> u64 {
    30 * 1024 * 1024
}

const fn default_download_max_concurrency() -> usize {
    4
}

const fn default_document_max_file_size() -> u64 {
    200 * 1024 * 1024
}

const fn default_filesystem_output_truncate_limit() -> usize {
    20_000
}

const fn default_grep_truncation_threshold() -> usize {
    30_000
}

const fn default_grep_truncated_line_max() -> usize {
    300
}

const fn default_shell_output_truncate_limit() -> usize {
    20_000
}

const fn default_cold_start_tool_return_limit() -> usize {
    500
}

/// Lifecycle-wide agent context.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentContext {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
    /// Accumulated usage.
    #[serde(default)]
    pub usage: Usage,
    /// Model/runtime configuration used for injected runtime context and tool policies.
    #[serde(default, skip_serializing_if = "RuntimeModelConfig::is_default")]
    pub model_config: RuntimeModelConfig,
    /// Tool-level configuration used by first-party and host tools.
    #[serde(default, skip_serializing_if = "ToolConfig::is_default")]
    pub tool_config: ToolConfig,
    /// Security-related runtime configuration.
    #[serde(default, skip_serializing_if = "SecurityConfig::is_empty")]
    pub security: SecurityConfig,
    /// Context creation time used for elapsed runtime context.
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
    /// State store.
    #[serde(default)]
    pub state: StateStore,
    /// Event bus.
    #[serde(default)]
    pub events: EventBus,
    /// Persisted notes.
    #[serde(default, skip_serializing_if = "NoteStore::is_empty")]
    pub notes: NoteStore,
    /// Message bus.
    #[serde(default)]
    pub messages: MessageBus,
    /// Trace correlation context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Context metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Typed dependencies, skipped from serialization.
    #[serde(skip)]
    pub dependencies: DependencyStore,
}

impl AgentContext {
    /// Create a fresh context.
    #[must_use]
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            run_id: None,
            conversation_id: ConversationId::new(),
            message_history: Vec::new(),
            usage: Usage::default(),
            model_config: RuntimeModelConfig::default(),
            tool_config: ToolConfig::default(),
            security: SecurityConfig::default(),
            started_at: Utc::now(),
            state: StateStore::new(),
            events: EventBus::new(),
            notes: NoteStore::new(),
            messages: MessageBus::new(),
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
            dependencies: DependencyStore::new(),
        }
    }

    /// Restore a context from serialized state.
    #[must_use]
    pub fn from_state(state: ResumableState) -> Self {
        Self {
            agent_id: state.agent_id,
            run_id: state.run_id,
            conversation_id: state.conversation_id.unwrap_or_default(),
            message_history: state.message_history,
            usage: state.usage,
            model_config: state.model_config,
            tool_config: state.tool_config,
            security: state.security,
            started_at: state.started_at,
            state: state.state,
            events: EventBus::new(),
            notes: state.notes,
            messages: state.message_bus,
            trace_context: state.trace_snapshot,
            metadata: state.metadata,
            dependencies: DependencyStore::new(),
        }
    }

    /// Export context state for session restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        ResumableState {
            agent_id: self.agent_id.clone(),
            run_id: self.run_id.clone(),
            conversation_id: Some(self.conversation_id.clone()),
            message_history: self.message_history.clone(),
            usage: self.usage.clone(),
            model_config: self.model_config.clone(),
            tool_config: self.tool_config.clone(),
            security: self.security.clone(),
            started_at: self.started_at,
            state: self.state.clone(),
            notes: self.notes.clone(),
            message_bus: self.messages.clone(),
            trace_snapshot: self.trace_context.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// Replace context with serialized state.
    pub fn restore_state(&mut self, state: ResumableState) {
        *self = Self::from_state(state);
    }

    /// Create a child context for subagent execution.
    ///
    /// The child receives long-lived runtime state needed for delegation: the parent
    /// conversation id, accumulated usage, state domains, notes, and typed dependencies.
    /// Per-run queues and histories start empty so delegated runs have an isolated model
    /// history and do not duplicate pending parent steering messages.
    #[must_use]
    pub fn subagent_context(&self, agent_id: impl Into<String>) -> Self {
        let mut metadata = self.metadata.clone();
        metadata.insert(
            "parent_agent_id".to_string(),
            serde_json::json!(self.agent_id.as_str()),
        );
        if let Some(run_id) = &self.run_id {
            metadata.insert(
                "parent_run_id".to_string(),
                serde_json::json!(run_id.as_str()),
            );
        }
        Self {
            agent_id: AgentId::from_string(agent_id),
            run_id: None,
            conversation_id: self.conversation_id.clone(),
            message_history: Vec::new(),
            usage: self.usage.clone(),
            model_config: self.model_config.clone(),
            tool_config: self.tool_config.clone(),
            security: self.security.clone(),
            started_at: Utc::now(),
            state: self.state.clone(),
            events: EventBus::new(),
            notes: self.notes.clone(),
            messages: MessageBus::new(),
            trace_context: self.trace_context.clone(),
            metadata,
            dependencies: self.dependencies.clone(),
        }
    }

    /// Absorb child context state that should survive successful subagent execution.
    pub fn absorb_subagent_context(&mut self, child: &Self) {
        self.usage = child.usage.clone();
        self.notes = child.notes.clone();
    }

    /// Attach trace correlation context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }

    /// Replace trace correlation context.
    pub fn set_trace_context(&mut self, trace_context: TraceContext) {
        self.trace_context = trace_context;
    }

    /// Record a model message in context history.
    pub fn push_message(&mut self, message: ModelMessage) {
        self.message_history.push(message);
    }

    /// Record usage in the context ledger.
    pub fn add_usage(&mut self, usage: &Usage) {
        self.usage.add_assign(usage);
    }

    /// Return the latest model request token usage reported by the provider.
    #[must_use]
    pub fn latest_request_total_tokens(&self) -> Option<u64> {
        self.message_history.iter().rev().find_map(|message| {
            let ModelMessage::Response(response) = message else {
                return None;
            };
            (response.usage.total_tokens > 0).then_some(response.usage.total_tokens)
        })
    }

    /// Publish an event.
    pub fn publish_event(&mut self, event: AgentEvent) {
        self.events.publish(event);
    }

    /// Enqueue a message.
    pub fn enqueue_message(&mut self, message: BusMessage) {
        self.messages.enqueue(message);
    }

    /// Insert a typed dependency.
    pub fn insert_dependency<T>(&mut self, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.insert(value);
    }

    /// Insert a named typed dependency.
    pub fn insert_named_dependency<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.insert_named(name, value);
    }

    /// Get a typed dependency.
    #[must_use]
    pub fn dependency<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get::<T>()
    }

    /// Get a named typed dependency.
    #[must_use]
    pub fn named_dependency<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get_named::<T>(name)
    }

    /// Set the context window exposed in model-facing runtime context.
    pub const fn set_context_window(&mut self, context_window: Option<u64>) {
        self.model_config.context_window = context_window;
    }

    /// Merge runtime model defaults into this context.
    pub fn merge_model_config(&mut self, model_config: ModelConfig) {
        self.model_config.merge_from(model_config);
    }

    /// Replace the tool config for this context.
    pub fn set_tool_config(&mut self, mut tool_config: ToolConfig) {
        tool_config.normalize();
        self.tool_config = tool_config;
    }

    /// Merge runtime tool defaults into this context.
    pub fn merge_tool_config(&mut self, mut tool_config: ToolConfig) {
        tool_config.normalize();
        let existing_dynamic_patterns = self.tool_config.view_relaxed_text_dynamic_patterns.clone();
        for (source, patterns) in existing_dynamic_patterns {
            tool_config
                .view_relaxed_text_dynamic_patterns
                .entry(source)
                .or_insert(patterns);
        }
        self.tool_config = tool_config;
    }

    /// Render runtime context instructions for model-facing requests.
    #[must_use]
    pub fn inject_runtime_context(&self, is_user_prompt: bool) -> Option<String> {
        let now = Utc::now();
        let elapsed_milliseconds = (now - self.started_at).num_milliseconds().max(0);
        let elapsed_tenths = (elapsed_milliseconds + 50) / 100;
        let elapsed = format!("{}.{:01}s", elapsed_tenths / 10, elapsed_tenths % 10);
        let mut xml = XmlWriter::new();
        xml.open("runtime-context")
            .text_element("agent-id", self.agent_id.as_str())
            .text_element("current-time", now.to_rfc3339())
            .text_element("elapsed-time", elapsed);

        if let Some(context_window) = self.model_config.context_window {
            xml.open("model-config")
                .text_element("context-window", context_window.to_string())
                .close("model-config");
        }

        let latest_total_tokens = self.latest_request_total_tokens();
        if let Some(total_tokens) = latest_total_tokens {
            xml.open("token-usage")
                .text_element("total-tokens", total_tokens.to_string())
                .close("token-usage");
        }

        if is_user_prompt && !self.notes.is_empty() {
            let entries = self.notes.list_all();
            let count = entries.len().to_string();
            xml.open_attrs("notes", [("count", count.as_str())]);
            for (key, _value) in entries {
                xml.empty_element_attrs("note", [("key", key.as_str())]);
            }
            xml.close("notes");
        }

        xml.close("runtime-context");
        let mut output = xml.finish();
        if let Some(reminder) = self.context_pressure_reminder(latest_total_tokens) {
            let mut reminder_xml = XmlWriter::new();
            reminder_xml
                .open("system-reminder")
                .text_element("item", reminder)
                .close("system-reminder");
            output.push_str("\n\n");
            output.push_str(&reminder_xml.finish());
        }
        Some(output)
    }

    fn context_pressure_reminder(&self, latest_total_tokens: Option<u64>) -> Option<String> {
        let total_tokens = latest_total_tokens?;
        let context_window = self.model_config.context_window?;
        if context_window == 0 {
            return None;
        }
        let threshold = self.model_config.proactive_context_management_threshold?;
        if total_tokens.saturating_mul(1000)
            < context_window.saturating_mul(u64::from(threshold.parts_per_thousand()))
        {
            return None;
        }
        let usage_pct = total_tokens.saturating_mul(100) / context_window;
        let compact_pct = u64::from(self.model_config.compact_threshold.parts_per_thousand())
            .saturating_mul(100)
            / 1000;
        let mut reminder = format!(
            "Context usage is at {usage_pct}% ({} / {} tokens). Configured compact threshold is {compact_pct}%. Please summarize progress and continue with a smaller context when appropriate.",
            format_u64_with_commas(total_tokens),
            format_u64_with_commas(context_window),
        );
        if !self.notes.is_empty() {
            reminder.push_str(
                " Review note keys, read needed values, and delete stale or oversized notes before summarizing.",
            );
        }
        Some(reminder)
    }

    /// Render context instructions for model-facing user prompts.
    #[must_use]
    pub fn context_instructions(&self, is_user_prompt: bool) -> Option<String> {
        self.inject_runtime_context(is_user_prompt)
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new(AgentId::default())
    }
}

fn format_u64_with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    let first_group_len = digits.len() % 3;
    let mut index = 0usize;
    if first_group_len > 0 {
        output.push_str(&digits[..first_group_len]);
        index = first_group_len;
        if index < digits.len() {
            output.push(',');
        }
    }
    while index < digits.len() {
        output.push_str(&digits[index..index + 3]);
        index += 3;
        if index < digits.len() {
            output.push(',');
        }
    }
    output
}

/// Shared context snapshot handle for tools that need to report context mutations.
#[derive(Clone)]
pub struct AgentContextHandle {
    inner: Arc<Mutex<AgentContext>>,
}

impl AgentContextHandle {
    /// Create a handle from a context snapshot.
    #[must_use]
    pub fn new(context: AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(context)),
        }
    }

    /// Return the latest context snapshot held by this handle.
    #[must_use]
    pub fn snapshot(&self) -> AgentContext {
        match self.inner.lock() {
            Ok(context) => context.clone(),
            Err(error) => error.into_inner().clone(),
        }
    }

    /// Replace the context snapshot held by this handle.
    pub fn replace(&self, context: AgentContext) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = context,
            Err(error) => {
                let mut guard = error.into_inner();
                *guard = context;
            }
        }
    }
}

impl Default for AgentContextHandle {
    fn default() -> Self {
        Self::new(AgentContext::default())
    }
}

impl std::fmt::Debug for AgentContextHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentContextHandle")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}
