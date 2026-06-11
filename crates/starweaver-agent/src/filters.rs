//! Named SDK history filter presets for ya-agent-sdk parity.

use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_context::{AgentContext, AgentEvent};
use starweaver_model::{
    parse_data_url, ContentPart, MediaPolicy, MediaPreflight, ModelAdapter, ModelMessage,
    ModelRequest, ModelRequestContext, ModelRequestParameters, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelSettings, ToolArguments, ToolReturnPart,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, CapabilityError, CapabilityOrdering, CapabilityResult,
    CapabilitySpec, StaticCapabilityBundle,
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
const COMPACT_DEPTH_METADATA: &str = "starweaver_compact_depth";
const DEFAULT_AUTO_COMPACT_KEEP_MESSAGES: usize = 12;
const COLD_START_TOOL_RETURN_LIMIT_METADATA: &str = "starweaver_cold_start_tool_return_limit";
const CACHE_FRIENDLY_COMPACT_INSTRUCTION: &str = "Generate a compact continuation summary for the conversation history.
Return only the summary text. Do not call tools.
Use this exact Markdown structure:

## Condensed conversation summary

### Analysis

[Brief analysis of the conversation and what matters for continuation.]

### Context

1. Primary Request and Intent:
   [User's explicit requests and intent]

2. Key Technical Concepts:
   - [Concepts, technologies, APIs, and architecture points]

3. Files and Code Sections:
   - [Files examined, edited, or created, with important details]

4. Problem Solving:
   [Problems solved and ongoing troubleshooting]

5. Pending Tasks:
   - [Explicit pending tasks]

6. Current Work:
   [Precise current work immediately before compaction]

7. Optional Next Step:
   [Direct next step aligned with the current work]

8. Past Interactions:
   - [Key interactions already completed, including actions and outcomes]

9. Skills Documentation:
   [If any /skills/ documentation was accessed, list the relevant skill files and remind the next agent to re-read them]

10. Auto-load Files:
   [List only file paths that should be auto-loaded when resuming]
";
const CACHE_FRIENDLY_COMPACT_PROMPT: &str = "Compact the conversation history into the requested continuation summary format. Focus on details needed to continue the user's work accurately after older messages are removed. Return only the summary text.";
const COMPACT_LIMIT_PROMPT: &str = "You have exceeded the maximum token limit for this conversation. Please provide a summary of the conversation so far and what you should work on next and I'll resume the conversation.";
const PROJECT_GUIDANCE_TAG: &str = "project-guidance";
const USER_RULES_TAG: &str = "user-rules";

/// Build the default named filter bundle.
#[must_use]
pub fn default_filter_bundle() -> StaticCapabilityBundle {
    let mut bundle = StaticCapabilityBundle::new("ya-agent-sdk-filter-parity");
    for capability in default_filter_capabilities(None) {
        bundle = bundle.with_hook(capability);
    }
    bundle
}

/// Build named filter capabilities in default parity order.
#[must_use]
pub fn default_filter_capabilities(
    compact_model: Option<&Arc<dyn ModelAdapter>>,
) -> Vec<Arc<dyn AgentCapability>> {
    default_filter_capabilities_with_config(compact_model, None, None)
}

/// Build named filter capabilities with inherited compactor configuration.
#[must_use]
pub fn default_filter_capabilities_with_config(
    compact_model: Option<&Arc<dyn ModelAdapter>>,
    compact_model_settings: Option<&ModelSettings>,
    compact_request_params: Option<&ModelRequestParameters>,
) -> Vec<Arc<dyn AgentCapability>> {
    DEFAULT_FILTER_ORDER
        .iter()
        .map(|name| {
            if *name == "compact" {
                let mut capability = CacheFriendlyCompactCapability::new(compact_model.cloned());
                if let Some(settings) = compact_model_settings.cloned() {
                    capability = capability.with_model_settings(settings);
                }
                if let Some(params) = compact_request_params.cloned() {
                    capability = capability.with_request_params(params);
                }
                Arc::new(capability) as Arc<dyn AgentCapability>
            } else {
                Arc::new(NamedFilterCapability::new(name)) as Arc<dyn AgentCapability>
            }
        })
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

/// Named SDK filter capability with concrete parity behavior.
#[derive(Clone)]
pub struct NamedFilterCapability {
    name: &'static str,
    uploader: Option<Arc<dyn MediaUploader>>,
}

impl std::fmt::Debug for NamedFilterCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamedFilterCapability")
            .field("name", &self.name)
            .field("has_uploader", &self.uploader.is_some())
            .finish()
    }
}

impl NamedFilterCapability {
    /// Create a named filter capability.
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

/// Cache-friendly compaction capability that mirrors ya-mono automatic compaction.
#[derive(Clone)]
pub struct CacheFriendlyCompactCapability {
    model: Option<Arc<dyn ModelAdapter>>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
}

impl CacheFriendlyCompactCapability {
    /// Create a compaction capability using the current agent model when available.
    #[must_use]
    pub fn new(model: Option<Arc<dyn ModelAdapter>>) -> Self {
        Self {
            model,
            model_settings: None,
            request_params: ModelRequestParameters::default(),
        }
    }

    /// Inherit model settings from the parent agent.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Inherit request parameters from the parent agent.
    #[must_use]
    pub fn with_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = params;
        self
    }
}

#[async_trait]
impl AgentCapability for CacheFriendlyCompactCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(filter_capability_id("compact"))
            .with_ordering(filter_capability_ordering("compact"))
    }

    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let mut compacted = if let Some(keep) = manual_compact_keep(state) {
            build_trimmed_compact_messages(state, &messages, keep)
        } else if need_auto_compact(context, &messages) {
            self.compact_with_model(state, context, &messages).await?
        } else {
            messages
        };
        record_filter_order(&mut compacted, "compact");
        let changed = compacted != state.message_history;
        if changed {
            state.message_history.clone_from(&compacted);
            context.message_history.clone_from(&compacted);
        }
        Ok(compacted)
    }
}

impl CacheFriendlyCompactCapability {
    async fn compact_with_model(
        &self,
        state: &AgentRunState,
        context: &mut AgentContext,
        messages: &[ModelMessage],
    ) -> CapabilityResult<Vec<ModelMessage>> {
        if context
            .metadata
            .get(COMPACT_DEPTH_METADATA)
            .and_then(Value::as_u64)
            .unwrap_or_default()
            > 0
        {
            return Ok(messages.to_vec());
        }
        let Some(model) = &self.model else {
            return Ok(build_trimmed_compact_messages(
                state,
                messages,
                DEFAULT_AUTO_COMPACT_KEEP_MESSAGES,
            ));
        };
        context
            .metadata
            .insert(COMPACT_DEPTH_METADATA.to_string(), json!(1));
        let event_id = format!("{}-{}", state.run_id.as_str(), state.run_step);
        context.publish_event(AgentEvent::new(
            "compact_start",
            json!({"event_id": event_id, "message_count": messages.len()}),
        ));
        let compact_messages = build_compact_summary_request(messages);
        let request_context =
            ModelRequestContext::new(state.run_id.clone(), state.conversation_id.clone())
                .with_trace_context(context.trace_context.clone());
        let response = match model
            .request_stream_final(
                compact_messages,
                compact_model_settings(model.default_settings(), self.model_settings.as_ref()),
                compact_request_params(&self.request_params),
                request_context,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                context.metadata.remove(COMPACT_DEPTH_METADATA);
                context.publish_event(AgentEvent::new(
                    "compact_failed",
                    json!({"event_id": event_id, "message": error.to_string()}),
                ));
                return Ok(messages.to_vec());
            }
        };
        context.metadata.remove(COMPACT_DEPTH_METADATA);
        context.add_usage(&response.usage);
        let summary = response.text_output();
        let compacted = build_cache_friendly_compacted_messages(state, messages, summary);
        context.publish_event(AgentEvent::new(
            "compact_complete",
            json!({
                "event_id": event_id,
                "message_count_before": messages.len(),
                "message_count_after": compacted.len(),
            }),
        ));
        Ok(compacted)
    }
}

#[async_trait]
impl AgentCapability for NamedFilterCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(filter_capability_id(self.name))
            .with_ordering(filter_capability_ordering(self.name))
    }

    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let mut messages = match self.name {
            "cold_start" => cold_start_filter(state, messages),
            "capability" => capability_filter(state, messages),
            "media_preflight" => media_preflight_filter(state, messages),
            "media_upload" => media_upload_filter(state, messages, self.uploader.as_ref()).await,
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
            "system_prompt" => system_prompt_filter(state, messages),
            "tool_args" => tool_args_filter(messages),
            "reasoning_normalize" => reasoning_normalize_filter(messages),
            other => {
                return Err(CapabilityError::Failed(format!(
                    "unknown SDK filter '{other}'"
                )));
            }
        };
        record_filter_order(&mut messages, self.name);
        Ok(messages)
    }
}

fn filter_capability_id(name: &str) -> String {
    format!("starweaver.filter.{name}")
}

fn filter_capability_ordering(name: &str) -> CapabilityOrdering {
    let Some(index) = DEFAULT_FILTER_ORDER
        .iter()
        .position(|candidate| *candidate == name)
    else {
        return CapabilityOrdering::default();
    };
    let mut ordering = CapabilityOrdering::default();
    if let Some(previous) = index
        .checked_sub(1)
        .and_then(|idx| DEFAULT_FILTER_ORDER.get(idx))
    {
        ordering = ordering.after(filter_capability_id(previous));
    }
    ordering
}

fn cold_start_filter(state: &AgentRunState, mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let limit = state
        .metadata
        .get(COLD_START_TOOL_RETURN_LIMIT_METADATA)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(500);
    let trim_end = messages
        .iter()
        .rposition(|message| matches!(message, ModelMessage::Response(_)))
        .unwrap_or(messages.len());
    let mut truncated = 0usize;
    for message in messages.iter_mut().take(trim_end) {
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

fn need_auto_compact(context: &AgentContext, messages: &[ModelMessage]) -> bool {
    let Some(context_window) = context.model_config.context_window else {
        return false;
    };
    let Some(current_tokens) = latest_request_total_tokens(messages) else {
        return false;
    };
    let threshold = context_window.saturating_mul(u64::from(
        context.model_config.compact_threshold.parts_per_thousand(),
    )) / 1000;
    current_tokens >= threshold
}

fn latest_request_total_tokens(messages: &[ModelMessage]) -> Option<u64> {
    messages.iter().rev().find_map(|message| {
        let ModelMessage::Response(response) = message else {
            return None;
        };
        (response.usage.total_tokens > 0).then_some(response.usage.total_tokens)
    })
}

fn build_compact_summary_request(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut compact_messages = messages
        .iter()
        .filter_map(|message| trim_message_for_compact(message.clone()))
        .collect::<Vec<_>>();
    compact_messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: CACHE_FRIENDLY_COMPACT_INSTRUCTION.to_string(),
                metadata: Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: CACHE_FRIENDLY_COMPACT_PROMPT.to_string(),
                }],
                name: None,
                metadata: Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
    compact_messages
}

fn compact_model_settings(
    defaults: Option<&ModelSettings>,
    inherited: Option<&ModelSettings>,
) -> Option<ModelSettings> {
    let mut settings = match (defaults, inherited) {
        (Some(defaults), Some(inherited)) => defaults.merge(inherited),
        (Some(defaults), None) => defaults.clone(),
        (None, Some(inherited)) => inherited.clone(),
        (None, None) => return None,
    };
    strip_compact_model_settings(&mut settings);
    Some(settings)
}

fn strip_compact_model_settings(settings: &mut ModelSettings) {
    settings.thinking = None;
    strip_compact_incompatible_body(&mut settings.extra_body);
    strip_incompatible_beta_header(&mut settings.extra_headers);
    if let Some(Value::Object(provider_options)) = &mut settings.provider_options {
        strip_compact_incompatible_body(provider_options);
    }
}

fn compact_request_params(inherited: &ModelRequestParameters) -> ModelRequestParameters {
    let mut params = inherited.clone();
    params.output_schema = None;
    params.output_mode = None;
    params.thinking = None;
    params.allow_text_output = Some(true);
    strip_compact_incompatible_body(&mut params.extra_body);
    strip_compact_incompatible_body(&mut params.http.extra_body);
    strip_incompatible_beta_header(&mut params.http.headers);
    params
}

fn strip_compact_incompatible_body(body: &mut Map<String, Value>) {
    for key in [
        "anthropic_cache_tool_definitions",
        "anthropic_cache_instructions",
        "anthropic_cache_messages",
        "anthropic_cache",
        "thinking",
        "anthropic_thinking",
        "anthropic_effort",
    ] {
        body.remove(key);
    }
    strip_clear_thinking_edits(body);
}

fn strip_incompatible_beta_header(headers: &mut std::collections::BTreeMap<String, String>) {
    let Some(beta_header) = headers.get("anthropic-beta").cloned() else {
        return;
    };
    let filtered = beta_header
        .split(',')
        .map(str::trim)
        .filter(|beta| !beta.is_empty() && *beta != "interleaved-thinking-2025-05-14")
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        headers.remove("anthropic-beta");
    } else {
        headers.insert("anthropic-beta".to_string(), filtered.join(","));
    }
}

fn strip_clear_thinking_edits(body: &mut Map<String, Value>) {
    let Some(Value::Object(context_management)) = body.get_mut("context_management") else {
        return;
    };
    let Some(Value::Array(edits)) = context_management.get_mut("edits") else {
        return;
    };
    edits.retain(|edit| {
        !edit
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.contains("clear_thinking"))
    });
    if edits.is_empty() {
        context_management.remove("edits");
    }
    if context_management.is_empty() {
        body.remove("context_management");
    }
}

fn build_cache_friendly_compacted_messages(
    state: &AgentRunState,
    messages: &[ModelMessage],
    summary: String,
) -> Vec<ModelMessage> {
    let mut summary_response = ModelResponse::text(summary);
    summary_response
        .metadata
        .insert("keep".to_string(), json!("compact"));
    let mut request_parts = instruction_parts(messages);
    if request_parts.is_empty() {
        request_parts.push(ModelRequestPart::SystemPrompt {
            text: "Placeholder system prompt".to_string(),
            metadata: Map::new(),
        });
    }
    request_parts.push(ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text {
            text: COMPACT_LIMIT_PROMPT.to_string(),
        }],
        name: None,
        metadata: Map::new(),
    });
    vec![
        ModelMessage::Request(ModelRequest {
            parts: request_parts,
            timestamp: None,
            instructions: None,
            run_id: Some(state.run_id.clone()),
            conversation_id: Some(state.conversation_id.clone()),
            metadata: Map::new(),
        }),
        ModelMessage::Response(summary_response),
        context_restored_request(state),
    ]
}

fn system_prompt_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let source_parts = instruction_parts(&state.message_history);
    if source_parts.is_empty() {
        return messages;
    }
    let existing = instruction_parts(&messages);
    let has_all = source_parts.iter().all(|part| existing.contains(part));
    if has_all {
        return messages;
    }
    messages.insert(
        0,
        ModelMessage::Request(ModelRequest {
            parts: source_parts,
            timestamp: None,
            instructions: None,
            run_id: Some(state.run_id.clone()),
            conversation_id: Some(state.conversation_id.clone()),
            metadata: Map::new(),
        }),
    );
    messages
}

fn instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    messages
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request
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
                .collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .collect()
}

fn manual_compact_keep(state: &AgentRunState) -> Option<usize> {
    state
        .metadata
        .get(COMPACT_KEEP_MESSAGES_METADATA)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn build_trimmed_compact_messages(
    state: &AgentRunState,
    messages: &[ModelMessage],
    keep: usize,
) -> Vec<ModelMessage> {
    if keep == 0 || messages.len() <= keep {
        return messages.to_vec();
    }

    let mut compacted = Vec::new();
    for message in messages.iter().take(messages.len().saturating_sub(keep)) {
        if has_keep_tag(message) {
            compacted.push(message.clone());
        }
    }
    compacted.extend(
        messages
            .iter()
            .skip(messages.len().saturating_sub(keep))
            .filter_map(|message| trim_message_for_compact(message.clone())),
    );

    if !has_context_restored_marker(&compacted) {
        compacted.push(context_restored_request(state));
    }
    request_metadata_mut(&mut compacted).insert("starweaver_compacted".to_string(), json!(true));
    compacted
}

fn trim_message_for_compact(message: ModelMessage) -> Option<ModelMessage> {
    match message {
        ModelMessage::Response(_) => Some(message),
        ModelMessage::Request(mut request) => {
            let mut parts = Vec::new();
            for part in request.parts {
                match part {
                    ModelRequestPart::ToolReturn(mut tool_return) => {
                        trim_tool_return_for_compact(&mut tool_return);
                        parts.push(ModelRequestPart::ToolReturn(tool_return));
                    }
                    ModelRequestPart::UserPrompt {
                        content,
                        name,
                        metadata,
                    } => {
                        let content = content
                            .into_iter()
                            .filter_map(trim_content_for_compact)
                            .collect::<Vec<_>>();
                        if !content.is_empty() {
                            parts.push(ModelRequestPart::UserPrompt {
                                content,
                                name,
                                metadata,
                            });
                        }
                    }
                    other => parts.push(other),
                }
            }
            if parts.is_empty() {
                return None;
            }
            request.parts = parts;
            Some(ModelMessage::Request(request))
        }
    }
}

fn trim_content_for_compact(content: ContentPart) -> Option<ContentPart> {
    match content {
        ContentPart::Text { text } => {
            strip_injected_context_text(&text).map(|text| ContentPart::Text { text })
        }
        ContentPart::ImageUrl { url } => Some(ContentPart::Text {
            text: format!("[image: {url}]"),
        }),
        ContentPart::FileUrl { url, media_type } => Some(ContentPart::Text {
            text: format!("[{media_type}: {url}]"),
        }),
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => Some(ContentPart::Text {
            text: format!("[{media_type} content removed]"),
        }),
    }
}

fn strip_injected_context_text(text: &str) -> Option<String> {
    let mut cleaned = text.to_string();
    for tag in [
        "runtime-context",
        "environment-context",
        PROJECT_GUIDANCE_TAG,
        USER_RULES_TAG,
    ] {
        cleaned = strip_xml_tag_blocks(&cleaned, tag);
    }
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn strip_xml_tag_blocks(text: &str, tag: &str) -> String {
    let mut remaining = text;
    let mut output = String::new();
    let open_prefix = format!("<{tag}");
    let close_tag = format!("</{tag}>");
    while let Some(start) = remaining.find(&open_prefix) {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start..];
        let Some(open_end) = after_start.find('>') else {
            output.push_str(after_start);
            return output;
        };
        let after_open = &after_start[open_end + 1..];
        if let Some(close_start) = after_open.find(&close_tag) {
            remaining = &after_open[close_start + close_tag.len()..];
        } else {
            remaining = after_open;
            break;
        }
    }
    output.push_str(remaining);
    output
}

fn trim_tool_return_for_compact(tool_return: &mut ToolReturnPart) {
    if let Some(text) = truncate_compact_text(&tool_return.content.to_string()) {
        tool_return.content = json!(text);
        tool_return
            .metadata
            .insert("starweaver_compact_trimmed".to_string(), json!(true));
    }
    if let Some(user_content) = &mut tool_return.user_content {
        if let Some(text) = truncate_compact_text(&user_content.to_string()) {
            *user_content = json!(text);
        }
    }
}

fn truncate_compact_text(text: &str) -> Option<String> {
    const MAX: usize = 500;
    const HEAD: usize = 200;
    const TAIL: usize = 200;
    if text.chars().count() <= MAX {
        return None;
    }
    let head = text.chars().take(HEAD).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = text.chars().count().saturating_sub(HEAD + TAIL);
    Some(format!(
        "{head}\n[... {truncated} chars truncated ...]\n{tail}"
    ))
}

fn has_keep_tag(message: &ModelMessage) -> bool {
    match message {
        ModelMessage::Request(request) => keep_tag_value(&request.metadata).is_some(),
        ModelMessage::Response(response) => keep_tag_value(&response.metadata).is_some(),
    }
}

fn keep_tag_value(metadata: &Map<String, Value>) -> Option<&str> {
    metadata
        .get("keep")
        .or_else(|| metadata.get("ya_keep"))
        .and_then(Value::as_str)
}

fn has_context_restored_marker(messages: &[ModelMessage]) -> bool {
    messages.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => content.iter().any(|item| match item {
                ContentPart::Text { text } => text.contains("<context-restored>"),
                _ => false,
            }),
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    })
}

fn context_restored_request(state: &AgentRunState) -> ModelMessage {
    let mut parts = Vec::new();
    if let Some(original) = metadata_text(&state.metadata, "starweaver_original_request") {
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: "<original-request>Below is the user's original request from the start of the conversation:</original-request>".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        });
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text { text: original }],
            name: None,
            metadata: Map::new(),
        });
    }
    if let Some(steering) = metadata_text(&state.metadata, "starweaver_user_steering") {
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: format!("<user-steering>Below are messages the user sent during your previous work session:</user-steering>\n{steering}"),
            }],
            name: None,
            metadata: Map::new(),
        });
    }
    parts.push(ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text {
            text: "<context-restored>Context was compacted from a long conversation. The summary above is the most authoritative source for current state. Synthesize the summary, original request, and any user steering messages to resume work. Do NOT repeat questions, confirmations, or actions documented in the summary. If the summary records a user decision, respect it without re-asking.</context-restored>".to_string(),
        }],
        name: None,
        metadata: Map::new(),
    });
    ModelMessage::Request(ModelRequest {
        parts,
        timestamp: None,
        instructions: None,
        run_id: Some(state.run_id.clone()),
        conversation_id: Some(state.conversation_id.clone()),
        metadata: Map::from_iter([("keep".to_string(), json!("compact"))]),
    })
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
    let content_text = value_model_response_text(&tool_return.content);
    if content_text.chars().count() <= limit {
        return false;
    }
    let original_chars = content_text.chars().count();
    tool_return.content = json!(truncate_head_tail(&content_text, limit));
    if let Some(user_content) = &mut tool_return.user_content {
        let user_text = value_model_response_text(user_content);
        if user_text.chars().count() > limit {
            *user_content = json!(truncate_head_tail(&user_text, limit));
        }
    }
    tool_return
        .metadata
        .insert("starweaver_truncated".to_string(), json!(true));
    tool_return.metadata.insert(
        "starweaver_original_chars".to_string(),
        json!(original_chars),
    );
    true
}

fn value_model_response_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn truncate_head_tail(text: &str, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    let head_len = 200.min(limit / 2);
    let tail_len = 200.min(limit.saturating_sub(head_len));
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = total.saturating_sub(head_len + tail_len);
    format!("{head}\n[... {truncated} chars truncated ...]\n{tail}")
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
