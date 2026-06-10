use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_context::{AgentContext, AgentContextHandle};
use starweaver_core::Usage;
use starweaver_model::{
    ContentPart, DynModelAdapter, ModelMessage, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelSettings, OutputMode,
};
use starweaver_tools::{ToolApprovalState, ToolContext, ToolError, ToolResult};

use crate::bundles::helpers::tool_execution_error;

const SHELL_REVIEW_HISTORY_LIMIT: usize = 10;

/// Default shell-command review prompt aligned with `ya-mono`.
pub const DEFAULT_SHELL_REVIEW_PROMPT: &str = "You review shell commands before execution.\n\nReturn a risk_level and concise reason. Commands below the configured risk threshold\nexecute directly. Commands at or above the threshold enter the configured approval or deny policy.\n\nRisk heuristics:\n- low: read-only inspection or local developer verification such as tests, lint, type checks,\n  imports, and printing.\n- medium: bounded workspace-local state changes such as file writes/deletes, generated\n  artifact or cache cleanup/generation, chmod/chown, package changes, and local servers.\n- high: untrusted remote code execution, broad destructive workspace changes, writes outside\n  the workspace, sensitive file reads, sudo usage, or system-level package/service changes.\n- extra_high: confirmed credential exfiltration, destructive home/root deletion, explicit\n  privilege escalation, malware-like behavior, or broad external data upload.\n\nReserve extra_high for visible catastrophic or hostile intent. Remote script execution is high by default.\nClassify Python and uv commands by the script's visible effect; `uv run python` alone is a runner. Python compileall writes __pycache__/bytecode and is medium risk. Explicit cache/bytecode generation and outbound network access need approval.\nUse workspace context for path scope. Source/package-tree wildcard deletion is broad destructive workspace change.\nUse previous shell reviews as consistency hints. When an identical or equivalent command was previously approved and the current visible effect has not expanded, lower the risk by at least one level. Repeated approved commands that remain bounded and workspace-local can be low risk.\nWhen a command combines safe and risky operations, classify by the riskiest operation after applying relevant previous approval context.\nReturn a concise reason.\n";

/// Action applied when shell review reaches the configured approval threshold.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewAction {
    /// Defer execution to the runtime HITL approval path.
    #[default]
    Defer,
    /// Block execution immediately and return a structured shell result.
    Deny,
}

/// Shell command review risk levels.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewRiskLevel {
    /// Read-only inspection or local verification.
    #[default]
    Low,
    /// Bounded workspace-local state change.
    Medium,
    /// Broad destructive, privileged, external, or sensitive operation.
    High,
    /// Catastrophic or visibly hostile operation.
    ExtraHigh,
}

impl ShellReviewRiskLevel {
    /// Return a sortable risk rank where higher values require more caution.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::ExtraHigh => 3,
        }
    }
}

/// Shell command safety review configuration.
#[derive(Clone)]
pub struct ShellReviewConfig {
    /// Whether review is enabled.
    pub enabled: bool,
    /// Model adapter used for review.
    pub model: Option<DynModelAdapter>,
    /// Optional model settings for review requests.
    pub model_settings: Option<ModelSettings>,
    /// Action when the risk threshold is reached.
    pub on_needs_approval: ShellReviewAction,
    /// Risk threshold requiring approval/deny handling.
    pub risk_threshold: ShellReviewRiskLevel,
    /// Optional override for the default review prompt.
    pub system_prompt: Option<String>,
}

impl ShellReviewConfig {
    /// Create an enabled shell review configuration using a model adapter.
    #[must_use]
    pub fn enabled(model: DynModelAdapter) -> Self {
        Self {
            enabled: true,
            model: Some(model),
            model_settings: None,
            on_needs_approval: ShellReviewAction::Defer,
            risk_threshold: ShellReviewRiskLevel::High,
            system_prompt: None,
        }
    }

    /// Create a disabled shell review configuration.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            model: None,
            model_settings: None,
            on_needs_approval: ShellReviewAction::Defer,
            risk_threshold: ShellReviewRiskLevel::High,
            system_prompt: None,
        }
    }

    /// Attach model settings.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Set threshold action.
    #[must_use]
    pub const fn with_action(mut self, action: ShellReviewAction) -> Self {
        self.on_needs_approval = action;
        self
    }

    /// Set risk threshold.
    #[must_use]
    pub const fn with_risk_threshold(mut self, threshold: ShellReviewRiskLevel) -> Self {
        self.risk_threshold = threshold;
        self
    }

    /// Override the reviewer system prompt.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
}

impl Default for ShellReviewConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

impl std::fmt::Debug for ShellReviewConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShellReviewConfig")
            .field("enabled", &self.enabled)
            .field(
                "model",
                &self.model.as_ref().map(|model| model.model_name()),
            )
            .field("model_settings", &self.model_settings)
            .field("on_needs_approval", &self.on_needs_approval)
            .field("risk_threshold", &self.risk_threshold)
            .field(
                "system_prompt",
                &self.system_prompt.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

/// `AgentContext` dependency carrying shell review config and short-term history.
#[derive(Clone, Debug)]
pub struct ShellReviewHandle {
    config: ShellReviewConfig,
    records: Arc<Mutex<VecDeque<ShellReviewRecord>>>,
}

impl ShellReviewHandle {
    /// Create a shell review handle.
    #[must_use]
    pub fn new(config: ShellReviewConfig) -> Self {
        Self {
            config,
            records: Arc::new(Mutex::new(VecDeque::with_capacity(
                SHELL_REVIEW_HISTORY_LIMIT,
            ))),
        }
    }

    /// Return the review configuration.
    #[must_use]
    pub const fn config(&self) -> &ShellReviewConfig {
        &self.config
    }

    /// Return a snapshot of previous records.
    #[must_use]
    pub fn records(&self) -> Vec<ShellReviewRecord> {
        self.records
            .lock()
            .map_or_else(|_| Vec::new(), |records| records.iter().cloned().collect())
    }

    fn push_record(&self, record: ShellReviewRecord) {
        if let Ok(mut records) = self.records.lock() {
            if records.len() >= SHELL_REVIEW_HISTORY_LIMIT {
                records.pop_front();
            }
            records.push_back(record);
        }
    }

    fn update_last_matching_approval(
        &self,
        tool_call_id: Option<&str>,
        fingerprint: &ShellReviewFingerprint,
    ) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        if let Some(tool_call_id) = tool_call_id {
            if let Some(record) = records
                .iter_mut()
                .rev()
                .find(|record| record.tool_call_id.as_deref() == Some(tool_call_id))
            {
                record.approved = true;
                return;
            }
        }
        if let Some(record) = records
            .iter_mut()
            .rev()
            .find(|record| record.request.command_fingerprint() == *fingerprint)
        {
            record.approved = true;
        }
    }
}

/// Attach shell command review to an `AgentContext`.
pub fn attach_shell_review(context: &mut AgentContext, config: ShellReviewConfig) {
    context.dependencies.insert(ShellReviewHandle::new(config));
}

/// Attach a shared shell review handle to an `AgentContext`.
pub fn attach_shell_review_handle(context: &mut AgentContext, handle: ShellReviewHandle) {
    context.dependencies.insert(handle);
}

/// Structured shell review decision.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewDecision {
    /// Review risk level.
    #[serde(default)]
    pub risk_level: ShellReviewRiskLevel,
    /// Concise reason for the decision.
    #[serde(default)]
    pub reason: String,
}

impl ShellReviewDecision {
    /// Return whether this decision reaches the configured threshold.
    #[must_use]
    pub const fn requires_approval(&self, config: &ShellReviewConfig) -> bool {
        config.enabled && self.risk_level.rank() >= config.risk_threshold.rank()
    }

    /// Return whether this decision should defer through HITL.
    #[must_use]
    pub fn requires_defer(&self, config: &ShellReviewConfig) -> bool {
        self.requires_approval(config) && config.on_needs_approval == ShellReviewAction::Defer
    }

    /// Return whether this decision should deny execution.
    #[must_use]
    pub fn requires_deny(&self, config: &ShellReviewConfig) -> bool {
        self.requires_approval(config) && config.on_needs_approval == ShellReviewAction::Deny
    }
}

/// Previous review decision rendered into the reviewer prompt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewPreviousDecision {
    /// Whether the previous command execution was approved.
    pub approved: bool,
    /// Previous risk level.
    pub risk_level: ShellReviewRiskLevel,
    /// Previous reason.
    #[serde(default)]
    pub reason: String,
    /// Previous command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Previous working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Execution context submitted to shell review.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewContextSnapshot {
    /// Shell timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Runtime tool call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Whether this tool call was already approved by a host.
    #[serde(default)]
    pub tool_call_approved: bool,
    /// Default shell working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_cwd: Option<String>,
    /// Provider-visible allowed paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,
    /// Shell platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_platform: Option<String>,
    /// Shell executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_executable: Option<String>,
}

impl ShellReviewContextSnapshot {
    /// Build approval-safe metadata.
    #[must_use]
    pub fn to_metadata(&self) -> Value {
        serde_json::json!({
            "timeout_seconds": self.timeout_seconds,
            "tool_call_id": self.tool_call_id,
            "tool_call_approved": self.tool_call_approved,
            "default_cwd": self.default_cwd,
            "allowed_paths": self.allowed_paths,
            "shell_platform": self.shell_platform,
            "shell_executable": self.shell_executable,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShellReviewFingerprint {
    command: String,
    cwd: Option<String>,
    background: bool,
    environment_keys: Vec<String>,
}

/// Shell command context submitted to the reviewer.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewRequest {
    /// Command string.
    pub command: String,
    /// Optional working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Whether command runs in background.
    #[serde(default)]
    pub background: bool,
    /// Environment variable keys visible to the command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_keys: Vec<String>,
    /// Execution context snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snapshot: Option<ShellReviewContextSnapshot>,
    /// Previous shell reviews for consistency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_reviews: Vec<ShellReviewPreviousDecision>,
}

impl ShellReviewRequest {
    fn command_fingerprint(&self) -> ShellReviewFingerprint {
        let mut environment_keys = self.environment_keys.clone();
        environment_keys.sort();
        ShellReviewFingerprint {
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            background: self.background,
            environment_keys,
        }
    }

    /// Render the model prompt for this review request.
    #[must_use]
    pub fn to_prompt(&self) -> String {
        let mut lines = vec![
            "Review this shell command.".to_string(),
            String::new(),
            "<command>".to_string(),
            self.command.clone(),
            "</command>".to_string(),
            String::new(),
            "<execution_context>".to_string(),
            format!("cwd: {}", self.cwd.as_deref().unwrap_or("<default>")),
            format!("background: {}", py_bool(self.background)),
            format!(
                "environment_keys: {}",
                py_string_list(&self.environment_keys)
            ),
        ];
        if let Some(snapshot) = self.context_snapshot.as_ref() {
            lines.extend([
                format!(
                    "timeout_seconds: {}",
                    py_option_u64(snapshot.timeout_seconds)
                ),
                format!(
                    "tool_call_approved: {}",
                    py_bool(snapshot.tool_call_approved)
                ),
            ]);
        }
        lines.extend(["</execution_context>".to_string(), String::new()]);

        if let Some(snapshot) = self.context_snapshot.as_ref() {
            lines.extend([
                "<workspace_context>".to_string(),
                format!(
                    "default_cwd: {}",
                    snapshot.default_cwd.as_deref().unwrap_or("<unknown>")
                ),
                format!("allowed_paths: {}", py_string_list(&snapshot.allowed_paths)),
                format!(
                    "shell_platform: {}",
                    snapshot.shell_platform.as_deref().unwrap_or("<unknown>")
                ),
                format!(
                    "shell_executable: {}",
                    snapshot.shell_executable.as_deref().unwrap_or("<unknown>")
                ),
                "</workspace_context>".to_string(),
                String::new(),
            ]);
        }

        if !self.previous_reviews.is_empty() {
            lines.push("<previous_shell_reviews>".to_string());
            for (index, review) in self.previous_reviews.iter().enumerate() {
                lines.extend([
                    format!("review_{}:", index + 1),
                    format!("  approved: {}", py_bool(review.approved)),
                    format!("  risk_level: {}", risk_level_name(review.risk_level)),
                    format!("  reason: {}", review.reason),
                    format!(
                        "  command: {}",
                        review.command.as_deref().unwrap_or("<unknown>")
                    ),
                    format!("  cwd: {}", review.cwd.as_deref().unwrap_or("<default>")),
                ]);
            }
            lines.extend(["</previous_shell_reviews>".to_string(), String::new()]);
        }

        lines.join("\n")
    }

    /// Build HITL metadata for this request and decision.
    #[must_use]
    pub fn to_approval_metadata(&self, decision: &ShellReviewDecision) -> Value {
        let mut metadata = Map::new();
        metadata.insert(
            "reviewer".to_string(),
            Value::String("shell_command_reviewer".to_string()),
        );
        metadata.insert("reason".to_string(), Value::String(decision.reason.clone()));
        metadata.insert(
            "risk_level".to_string(),
            Value::String(risk_level_name(decision.risk_level).to_string()),
        );
        metadata.insert("command".to_string(), Value::String(self.command.clone()));
        metadata.insert("cwd".to_string(), option_string_value(self.cwd.as_deref()));
        metadata.insert("background".to_string(), Value::Bool(self.background));
        if let Some(snapshot) = self.context_snapshot.as_ref() {
            metadata.insert("context".to_string(), snapshot.to_metadata());
        }
        if !self.previous_reviews.is_empty() {
            metadata.insert(
                "previous_shell_reviews".to_string(),
                serde_json::to_value(&self.previous_reviews)
                    .unwrap_or_else(|_| Value::Array(Vec::new())),
            );
        }
        Value::Object(metadata)
    }
}

/// Stored shell review result for short-term reviewer context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewRecord {
    /// Original request.
    pub request: ShellReviewRequest,
    /// Review decision.
    pub decision: ShellReviewDecision,
    /// Runtime tool call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Whether execution was approved.
    #[serde(default)]
    pub approved: bool,
    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl ShellReviewRecord {
    fn new(
        request: ShellReviewRequest,
        decision: ShellReviewDecision,
        tool_call_id: Option<String>,
    ) -> Self {
        Self {
            request,
            decision,
            tool_call_id,
            approved: false,
            created_at: Utc::now(),
        }
    }
}

/// Review a shell command and return a blocked result when policy denies execution.
pub async fn review_shell_command_or_block(
    context: &ToolContext,
    command: &str,
    cwd: Option<&str>,
    background: bool,
    mut environment_keys: Vec<String>,
    timeout_seconds: u64,
    mut snapshot: ShellReviewContextSnapshot,
) -> Result<Option<ToolResult>, ToolError> {
    let Some(agent_context) = context.dependency::<AgentContext>() else {
        return Ok(None);
    };
    let Some(handle) = agent_context.dependencies.get::<ShellReviewHandle>() else {
        return Ok(None);
    };
    let tool_call_id = tool_call_id(context);
    let tool_call_approved = matches!(context.approval, Some(ToolApprovalState::Approved { .. }));
    if snapshot.timeout_seconds.is_none() {
        snapshot.timeout_seconds = Some(timeout_seconds);
    }
    snapshot.tool_call_id.clone_from(&tool_call_id);
    snapshot.tool_call_approved = tool_call_approved;
    environment_keys.sort();

    let mut request = ShellReviewRequest {
        command: command.to_string(),
        cwd: cwd.map(str::to_string),
        background,
        environment_keys,
        context_snapshot: Some(snapshot),
        previous_reviews: Vec::new(),
    };

    let fingerprint = request.command_fingerprint();
    if tool_call_approved {
        handle.update_last_matching_approval(tool_call_id.as_deref(), &fingerprint);
        return Ok(None);
    }

    request.previous_reviews = previous_shell_reviews(&handle, &request, tool_call_id.as_deref());
    let decision = review_shell_command(context, &handle, &request).await?;
    let mut record =
        ShellReviewRecord::new(request.clone(), decision.clone(), tool_call_id.clone());
    if !decision.requires_approval(handle.config()) {
        record.approved = true;
        handle.push_record(record);
        return Ok(None);
    }
    handle.push_record(record);

    let metadata = request.to_approval_metadata(&decision);
    if decision.requires_defer(handle.config()) {
        return Err(ToolError::ApprovalRequired {
            tool: "shell_exec".to_string(),
            metadata,
        });
    }
    if decision.requires_deny(handle.config()) {
        return Ok(Some(ToolResult::new(serde_json::json!({
            "stdout": "",
            "stderr": "",
            "return_code": 1,
            "error": format!("Shell command blocked by review: {}", decision.reason),
            "shell_review": decision,
        }))));
    }
    Ok(None)
}

fn previous_shell_reviews(
    handle: &ShellReviewHandle,
    request: &ShellReviewRequest,
    tool_call_id: Option<&str>,
) -> Vec<ShellReviewPreviousDecision> {
    let records = handle.records();
    let fingerprint = request.command_fingerprint();
    let mut previous = Vec::new();
    let mut seen = Vec::<usize>::new();
    for pass in 0..3 {
        for (index, record) in records.iter().enumerate().rev() {
            if seen.contains(&index) {
                continue;
            }
            let matches = match pass {
                0 => tool_call_id.is_some() && record.tool_call_id.as_deref() == tool_call_id,
                1 => record.request.command_fingerprint() == fingerprint,
                _ => true,
            };
            if !matches {
                continue;
            }
            previous.push(ShellReviewPreviousDecision {
                approved: record.approved,
                risk_level: record.decision.risk_level,
                reason: record.decision.reason.clone(),
                command: Some(record.request.command.clone()),
                cwd: record.request.cwd.clone(),
            });
            seen.push(index);
        }
    }
    previous
}

async fn review_shell_command(
    context: &ToolContext,
    handle: &ShellReviewHandle,
    request: &ShellReviewRequest,
) -> Result<ShellReviewDecision, ToolError> {
    let config = handle.config();
    let Some(model) = config.model.as_ref().filter(|_| config.enabled) else {
        return Ok(ShellReviewDecision {
            risk_level: ShellReviewRiskLevel::Low,
            reason: "Shell review is disabled.".to_string(),
        });
    };

    let response = model
        .request_stream_final(
            vec![ModelMessage::Request(shell_review_model_request(
                config, request,
            ))],
            config.model_settings.clone(),
            shell_review_request_params(),
            ModelRequestContext::new(context.run_id.clone(), context.conversation_id.clone()),
        )
        .await
        .map_err(|error| {
            tool_execution_error("shell_exec", format!("Shell review failed: {error}"))
        })?;
    record_shell_review_usage(context, &response);
    parse_shell_review_decision(&response).ok_or_else(|| {
        tool_execution_error(
            "shell_exec",
            format!(
                "Shell review model returned an invalid decision: {}",
                response.text_output()
            ),
        )
    })
}

fn shell_review_model_request(
    config: &ShellReviewConfig,
    request: &ShellReviewRequest,
) -> ModelRequest {
    ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: config
                    .system_prompt
                    .clone()
                    .unwrap_or_else(|| DEFAULT_SHELL_REVIEW_PROMPT.to_string()),
                metadata: Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: request.to_prompt(),
                }],
                name: Some("shell_review".to_string()),
                metadata: Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }
}

fn shell_review_request_params() -> ModelRequestParameters {
    ModelRequestParameters {
        output_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "risk_level": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "extra_high"]
                },
                "reason": {"type": "string"}
            },
            "required": ["risk_level", "reason"],
            "additionalProperties": false
        })),
        output_mode: Some(OutputMode::Prompted),
        ..ModelRequestParameters::default()
    }
}

fn record_shell_review_usage(context: &ToolContext, response: &ModelResponse) {
    if response.usage == Usage::default() {
        return;
    }
    if let Some(handle) = context.dependency::<AgentContextHandle>() {
        let mut snapshot = handle.snapshot();
        snapshot.add_usage(&response.usage);
        handle.replace(snapshot);
    }
}

fn parse_shell_review_decision(response: &ModelResponse) -> Option<ShellReviewDecision> {
    parse_decision_value(&response.text_output())
}

fn parse_decision_value(text: &str) -> Option<ShellReviewDecision> {
    let trimmed = strip_json_fence(text.trim());
    serde_json::from_str::<ShellReviewDecision>(trimmed)
        .ok()
        .or_else(|| extract_json_object(trimmed).and_then(|json| serde_json::from_str(&json).ok()))
}

fn strip_json_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
    rest.strip_suffix("```").map_or(rest, str::trim)
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}

fn tool_call_id(context: &ToolContext) -> Option<String> {
    context
        .metadata
        .get("tool_call_id")
        .or_else(|| context.metadata.get("starweaver.tool_call_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

const fn risk_level_name(level: ShellReviewRiskLevel) -> &'static str {
    match level {
        ShellReviewRiskLevel::Low => "low",
        ShellReviewRiskLevel::Medium => "medium",
        ShellReviewRiskLevel::High => "high",
        ShellReviewRiskLevel::ExtraHigh => "extra_high",
    }
}

const fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

fn py_option_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "None".to_string(), |value| value.to_string())
}

fn py_string_list(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn option_string_value(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use starweaver_core::{ConversationId, RunId};
    use starweaver_model::{ModelMessage, ModelRequestPart, TestModel, ToolCallPart};

    use super::*;

    fn review_tool_context(handle: &ShellReviewHandle, tool_call_id: &str) -> ToolContext {
        let mut agent_context = AgentContext::default();
        attach_shell_review_handle(&mut agent_context, handle.clone());
        let mut dependencies = agent_context.dependencies.clone();
        dependencies.insert(agent_context);
        let mut context = ToolContext::new(
            RunId::from_string("run_shell_review"),
            ConversationId::from_string("conversation_shell_review"),
            0,
        )
        .with_dependencies(dependencies);
        context
            .metadata
            .insert("tool_call_id".to_string(), serde_json::json!(tool_call_id));
        context
    }

    fn review_snapshot() -> ShellReviewContextSnapshot {
        ShellReviewContextSnapshot {
            timeout_seconds: Some(30),
            tool_call_id: None,
            tool_call_approved: false,
            default_cwd: Some("/workspace".to_string()),
            allowed_paths: vec!["/workspace".to_string()],
            shell_platform: Some("linux".to_string()),
            shell_executable: Some("/bin/bash".to_string()),
        }
    }

    fn user_prompt_text(messages: &[ModelMessage]) -> Option<String> {
        messages.iter().rev().find_map(|message| match message {
            ModelMessage::Request(request) => {
                request.parts.iter().rev().find_map(|part| match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        content.iter().find_map(|part| match part {
                            ContentPart::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                    }
                    _ => None,
                })
            }
            ModelMessage::Response(_) => None,
        })
    }

    #[test]
    fn prompt_includes_context_and_previous_reviews_without_tool_call_id() {
        let request = ShellReviewRequest {
            command: "cargo test -p starweaver-agent".to_string(),
            cwd: Some("crates/starweaver-agent".to_string()),
            background: true,
            environment_keys: vec!["RUST_LOG".to_string(), "CI".to_string()],
            context_snapshot: Some(ShellReviewContextSnapshot {
                timeout_seconds: Some(120),
                tool_call_id: Some("call-secret".to_string()),
                tool_call_approved: true,
                default_cwd: Some("/workspace".to_string()),
                allowed_paths: vec!["/workspace".to_string(), "/tmp/shared".to_string()],
                shell_platform: Some("linux".to_string()),
                shell_executable: Some("/bin/bash".to_string()),
            }),
            previous_reviews: vec![ShellReviewPreviousDecision {
                approved: true,
                risk_level: ShellReviewRiskLevel::Medium,
                reason: "bounded workspace-local write".to_string(),
                command: Some("cargo test".to_string()),
                cwd: Some("crates/starweaver-agent".to_string()),
            }],
        };

        let prompt = request.to_prompt();

        assert!(prompt.contains("<command>\ncargo test -p starweaver-agent\n</command>"));
        assert!(prompt.contains("cwd: crates/starweaver-agent"));
        assert!(prompt.contains("background: True"));
        assert!(prompt.contains("environment_keys: ['RUST_LOG', 'CI']"));
        assert!(prompt.contains("timeout_seconds: 120"));
        assert!(prompt.contains("tool_call_approved: True"));
        assert!(prompt.contains("default_cwd: /workspace"));
        assert!(prompt.contains("allowed_paths: ['/workspace', '/tmp/shared']"));
        assert!(prompt.contains("shell_platform: linux"));
        assert!(prompt.contains("shell_executable: /bin/bash"));
        assert!(prompt.contains("<previous_shell_reviews>"));
        assert!(prompt.contains("approved: True"));
        assert!(prompt.contains("risk_level: medium"));
        assert!(!prompt.contains("tool_call_id:"));
        assert!(!prompt.contains("call-secret"));

        let metadata = request.to_approval_metadata(&ShellReviewDecision {
            risk_level: ShellReviewRiskLevel::High,
            reason: "needs approval".to_string(),
        });
        assert_eq!(metadata["reviewer"], "shell_command_reviewer");
        assert_eq!(metadata["command"], "cargo test -p starweaver-agent");
        assert_eq!(metadata["context"]["tool_call_id"], "call-secret");
        assert_eq!(metadata["context"]["tool_call_approved"], true);
        assert_eq!(metadata["previous_shell_reviews"][0]["approved"], true);
    }

    #[test]
    fn review_decision_threshold_respects_defer_and_deny_actions() {
        let high = ShellReviewDecision {
            risk_level: ShellReviewRiskLevel::High,
            reason: "broad change".to_string(),
        };
        let medium = ShellReviewDecision {
            risk_level: ShellReviewRiskLevel::Medium,
            reason: "bounded change".to_string(),
        };
        let defer_config = ShellReviewConfig::disabled()
            .with_action(ShellReviewAction::Defer)
            .with_risk_threshold(ShellReviewRiskLevel::High);
        let defer_config = ShellReviewConfig {
            enabled: true,
            ..defer_config
        };
        let deny_config = defer_config.clone().with_action(ShellReviewAction::Deny);

        assert!(high.requires_approval(&defer_config));
        assert!(high.requires_defer(&defer_config));
        assert!(!high.requires_deny(&defer_config));
        assert!(high.requires_deny(&deny_config));
        assert!(!medium.requires_approval(&defer_config));
    }

    #[tokio::test]
    async fn review_defer_records_previous_reviews_and_metadata() {
        let review_model = TestModel::with_responses(vec![
            ModelResponse::text(r#"{"risk_level":"low","reason":"read-only verification"}"#),
            ModelResponse::text(
                "```json\n{\"risk_level\":\"high\",\"reason\":\"writes outside workspace\"}\n```",
            ),
        ]);
        let handle =
            ShellReviewHandle::new(ShellReviewConfig::enabled(Arc::new(review_model.clone())));

        let first = review_shell_command_or_block(
            &review_tool_context(&handle, "call-1"),
            "cargo test -p starweaver-agent",
            Some("crates/starweaver-agent"),
            false,
            vec!["RUST_LOG".to_string()],
            30,
            review_snapshot(),
        )
        .await
        .unwrap();
        assert!(first.is_none());
        assert_eq!(handle.records().len(), 1);
        assert!(handle.records()[0].approved);

        let error = review_shell_command_or_block(
            &review_tool_context(&handle, "call-2"),
            "cargo test -p starweaver-agent",
            Some("crates/starweaver-agent"),
            false,
            vec!["RUST_LOG".to_string()],
            30,
            review_snapshot(),
        )
        .await
        .unwrap_err();

        let ToolError::ApprovalRequired { tool, metadata } = error else {
            panic!("expected approval required error");
        };
        assert_eq!(tool, "shell_exec");
        assert_eq!(metadata["reviewer"], "shell_command_reviewer");
        assert_eq!(metadata["risk_level"], "high");
        assert_eq!(metadata["context"]["tool_call_id"], "call-2");
        assert_eq!(metadata["context"]["timeout_seconds"], 30);
        assert_eq!(metadata["previous_shell_reviews"][0]["approved"], true);
        assert_eq!(metadata["previous_shell_reviews"][0]["risk_level"], "low");

        let captured_messages = review_model.captured_messages();
        assert_eq!(captured_messages.len(), 2);
        let prompt = user_prompt_text(&captured_messages[1]).unwrap();
        assert!(prompt.contains("<previous_shell_reviews>"));
        assert!(prompt.contains("approved: True"));
        assert!(prompt.contains("risk_level: low"));
        assert!(!prompt.contains("tool_call_id:"));
        assert!(!prompt.contains("call-2"));

        let captured_params = review_model.captured_params();
        assert_eq!(captured_params.len(), 2);
        assert!(captured_params[1].output_schema.is_some());
        assert_eq!(captured_params[1].output_mode, Some(OutputMode::Prompted));
    }

    #[tokio::test]
    async fn review_uses_streaming_model_request() {
        let review_model = TestModel::with_stream_events(vec![vec![
            starweaver_model::ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text(
                r#"{"risk_level":"low","reason":"streamed decision"}"#,
            ))),
        ]]);
        let handle =
            ShellReviewHandle::new(ShellReviewConfig::enabled(Arc::new(review_model.clone())));

        let result = review_shell_command_or_block(
            &review_tool_context(&handle, "call-stream-review"),
            "git status --short",
            Some("/workspace"),
            false,
            Vec::new(),
            30,
            review_snapshot(),
        )
        .await
        .unwrap();

        assert!(result.is_none());
        assert_eq!(handle.records().len(), 1);
        assert!(handle.records()[0].approved);
        assert_eq!(handle.records()[0].decision.reason, "streamed decision");
        assert_eq!(review_model.captured_messages().len(), 1);
    }

    #[tokio::test]
    async fn review_deny_returns_structured_shell_result() {
        let review_model = TestModel::with_json(&serde_json::json!({
            "risk_level": "medium",
            "reason": "bounded generated files"
        }));
        let handle = ShellReviewHandle::new(
            ShellReviewConfig::enabled(Arc::new(review_model))
                .with_action(ShellReviewAction::Deny)
                .with_risk_threshold(ShellReviewRiskLevel::Medium),
        );

        let blocked = review_shell_command_or_block(
            &review_tool_context(&handle, "call-deny"),
            "python -m compileall .",
            None,
            false,
            Vec::new(),
            30,
            ShellReviewContextSnapshot::default(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(blocked.content["return_code"], 1);
        assert_eq!(blocked.content["stdout"], "");
        assert_eq!(blocked.content["stderr"], "");
        assert!(blocked.content["error"]
            .as_str()
            .unwrap()
            .contains("Shell command blocked by review"));
        assert_eq!(blocked.content["shell_review"]["risk_level"], "medium");
    }

    #[tokio::test]
    async fn shell_exec_runs_review_before_provider_execution() {
        use starweaver_core::Metadata;
        use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};
        use starweaver_tools::ToolRegistry;

        let provider = Arc::new(VirtualEnvironmentProvider::new("test").with_shell_output(
            "echo ok",
            ShellOutput {
                status: 0,
                stdout: "ok\n".to_string(),
                stderr: String::new(),
                metadata: Metadata::default(),
            },
        ));
        let review_model = TestModel::with_responses(vec![
            ModelResponse::text(r#"{"risk_level":"high","reason":"dangerous"}"#),
            ModelResponse::text(r#"{"risk_level":"low","reason":"read-only"}"#),
        ]);
        let handle = ShellReviewHandle::new(
            ShellReviewConfig::enabled(Arc::new(review_model))
                .with_action(ShellReviewAction::Deny)
                .with_risk_threshold(ShellReviewRiskLevel::High),
        );
        let mut agent_context = AgentContext::default();
        crate::bundles::environment::attach_environment(&mut agent_context, provider);
        attach_shell_review_handle(&mut agent_context, handle);
        let mut dependencies = agent_context.dependencies.clone();
        dependencies.insert(agent_context);
        let context = ToolContext::new(
            RunId::from_string("run_shell_exec_review"),
            ConversationId::from_string("conversation_shell_exec_review"),
            0,
        )
        .with_dependencies(dependencies);
        let mut registry = ToolRegistry::new();
        registry.insert_toolset(&crate::bundles::environment::shell_tools());

        let denied = registry
            .execute_call(
                context.clone(),
                &ToolCallPart {
                    id: "call-denied".to_string(),
                    name: "shell_exec".to_string(),
                    arguments: serde_json::json!({"command": "echo missing"}).into(),
                },
            )
            .await;
        assert!(!denied.is_error);
        assert_eq!(denied.content["return_code"], 1);
        assert!(denied.content["error"]
            .as_str()
            .unwrap()
            .contains("Shell command blocked by review"));

        let allowed = registry
            .execute_call(
                context,
                &ToolCallPart {
                    id: "call-allowed".to_string(),
                    name: "shell_exec".to_string(),
                    arguments: serde_json::json!({"command": "echo ok"}).into(),
                },
            )
            .await;
        assert!(!allowed.is_error);
        assert_eq!(allowed.content["stdout"], "ok\n");
    }

    #[tokio::test]
    async fn approved_tool_call_bypasses_reviewer_and_marks_history_approved() {
        let review_model = TestModel::with_json(&serde_json::json!({
            "risk_level": "high",
            "reason": "needs approval"
        }));
        let handle =
            ShellReviewHandle::new(ShellReviewConfig::enabled(Arc::new(review_model.clone())));
        let context = review_tool_context(&handle, "call-approved");

        let error = review_shell_command_or_block(
            &context,
            "rm -rf target",
            None,
            false,
            Vec::new(),
            30,
            review_snapshot(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ToolError::ApprovalRequired { .. }));
        assert_eq!(review_model.captured_messages().len(), 1);
        assert!(!handle.records()[0].approved);

        let mut approved_context = review_tool_context(&handle, "call-approved");
        approved_context.approve();
        let allowed = review_shell_command_or_block(
            &approved_context,
            "rm -rf target",
            None,
            false,
            Vec::new(),
            30,
            review_snapshot(),
        )
        .await
        .unwrap();

        assert!(allowed.is_none());
        assert_eq!(review_model.captured_messages().len(), 1);
        assert!(handle.records()[0].approved);
    }
}
