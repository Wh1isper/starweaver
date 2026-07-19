#![allow(missing_docs)]

use std::{collections::BTreeSet, path::PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{ProtocolIdentity, RunId, SessionId};
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, DeferredToolRecord, InputPart, RunRecord, SessionRecord,
};
use starweaver_stream::{DisplayMessage, ReplayCursor, ReplayEvent, ReplayScope};

use crate::{EnvironmentAttachmentRef, RunOutputItem, StreamPayloadFormat};

/// Empty named params object used by methods that take no arguments.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCapabilities {
    pub sessions: bool,
    pub runs: bool,
    pub management: bool,
    pub profiles: bool,
    pub client_model_selection: bool,
    pub blocking_run_start: bool,
    pub blocking_run_prompt: bool,
    pub non_blocking_run_start: bool,
    pub live_display: bool,
    pub stream_replay: bool,
    pub stream_subscribe: bool,
    pub cancel: bool,
    pub steering: bool,
    pub attach: bool,
    pub environment_attachments: bool,
    pub environment_active_mounts: bool,
    pub default_stream_payload: StreamPayloadFormat,
    pub approvals: bool,
    pub deferred: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_search: Option<crate::SessionSearchFeatureCapabilities>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostInitializeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_dir: Option<PathBuf>,
    pub project_dir: PathBuf,
    pub default_profile: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostInitializeResult {
    pub protocol: ProtocolIdentity,
    pub server_info: ServerInfo,
    pub capabilities: HostCapabilities,
    pub config: HostInitializeConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownResult {
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsGetResult {
    pub sdk: String,
    pub version: String,
    pub config_path: PathBuf,
    pub database_path: PathBuf,
    pub state_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub default_profile: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientStateParams {
    #[serde(default, alias = "client", skip_serializing_if = "Option::is_none")]
    pub client_state_scope: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub model_id: String,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolsets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    pub client_state_scope: String,
    pub selected_profile: String,
    pub model_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileListResult {
    pub profiles: Vec<ProfileSummary>,
    pub current: ModelSelection,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileGetParams {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileGetResult {
    pub name: String,
    pub profile: ProfileConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelectParams {
    pub profile: String,
    #[serde(default, alias = "client", skip_serializing_if = "Option::is_none")]
    pub client_state_scope: Option<String>,
}

pub type ModelCurrentResult = ModelSelection;
pub type ModelSelectResult = ModelSelection;
pub type ModelListResult = ProfileListResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigGetParams {
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigGetResult {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateResult {
    pub session: SessionRecord,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionListResult {
    pub sessions: Vec<SessionRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGetParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionGetResult {
    pub session: SessionRecord,
    pub runs: Vec<RunRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCurrentSetParams {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCurrentResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDeleteParams {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDeleteResult {
    pub session_id: SessionId,
    pub deleted: bool,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunInputDto {
    pub parts: Vec<InputPart>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<RunInputDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(
        default,
        alias = "modelProfile",
        skip_serializing_if = "Option::is_none"
    )]
    pub profile: Option<String>,
    #[serde(default, alias = "client", skip_serializing_if = "Option::is_none")]
    pub client_state_scope: Option<String>,
    #[serde(default, alias = "runId", skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_attachments: Vec<EnvironmentAttachmentRef>,
    #[serde(default)]
    pub continuation_mode: crate::ContinuationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

pub type RunPromptParams = RunStartParams;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunStartResult {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub status: String,
    pub idempotent_replay: bool,
    pub payload_format: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_attachments: Vec<EnvironmentAttachmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<crate::AgentMaterialization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<crate::ContinuationAssessment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPromptResult {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_attachments: Vec<EnvironmentAttachmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<crate::AgentMaterialization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<crate::ContinuationAssessment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunIdentityParams {
    pub session_id: SessionId,
    pub run_id: RunId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunAwaitParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostRunStatus {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Fail-closed effect recovery projection when a started continuation lost its host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_effect: Option<starweaver_session::ContinuationEffectState>,
}

impl HostRunStatus {
    #[must_use]
    pub fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunStatusResult {
    pub status: HostRunStatus,
}

pub type RunAwaitResult = RunStatusResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCancelParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCancelResult {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub cancelled: bool,
    pub control_id: String,
    pub receipt_id: String,
    pub fencing_generation: u64,
    pub idempotent: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSteerParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSteerResult {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub steering_id: String,
    pub queued: bool,
    pub receipt_id: String,
    pub fencing_generation: u64,
    pub idempotent: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamReplayParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

pub type RunAttachParams = StreamReplayParams;
pub type SessionOutputParams = StreamReplayParams;
pub type SessionReplayParams = StreamReplayParams;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunAttachmentResult {
    pub session_id: SessionId,
    #[serde(default)]
    pub run_id: Option<RunId>,
    pub active: bool,
    pub payload_format: StreamPayloadFormat,
    pub events: Vec<RunOutputItem>,
}

pub type SessionOutputResult = RunAttachmentResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamReplayResult {
    pub session_id: SessionId,
    #[serde(default)]
    pub run_id: Option<RunId>,
    pub scope: ReplayScope,
    #[serde(default)]
    pub latest_cursor: Option<ReplayCursor>,
    pub next_sequence: usize,
    pub events: Vec<ReplayEvent>,
    pub messages: Vec<DisplayMessage>,
}

pub type SessionReplayResult = StreamReplayResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamSubscribeParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub payload_format: StreamPayloadFormat,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamSubscribeResult {
    pub subscription_id: String,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub active: bool,
    pub payload_format: StreamPayloadFormat,
    pub events: Vec<RunOutputItem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamUnsubscribeParams {
    pub subscription_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamUnsubscribeResult {
    pub subscription_id: String,
    pub closed: bool,
    pub was_active: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HitlListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
}

pub type ApprovalListParams = HitlListParams;
pub type DeferredListParams = HitlListParams;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalListResult {
    pub approvals: Vec<ApprovalRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalShowParams {
    pub approval_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResult {
    pub approval: ApprovalRecord,
}

pub type ApprovalShowResult = ApprovalResult;
pub type ApprovalDecideResult = ApprovalResult;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    #[serde(alias = "approve")]
    Approved,
    #[serde(alias = "reject", alias = "rejected")]
    Denied,
}

impl From<ApprovalDecision> for ApprovalStatus {
    fn from(value: ApprovalDecision) -> Self {
        match value {
            ApprovalDecision::Approved => Self::Approved,
            ApprovalDecision::Denied => Self::Denied,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecideParams {
    pub approval_id: String,
    pub status: ApprovalDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeferredListResult {
    pub deferred: Vec<DeferredToolRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredShowParams {
    pub deferred_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeferredResult {
    pub deferred: DeferredToolRecord,
}

pub type DeferredShowResult = DeferredResult;
pub type DeferredCompleteResult = DeferredResult;
pub type DeferredFailResult = DeferredResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredCompleteParams {
    pub deferred_id: String,
    #[serde(default)]
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredFailParams {
    pub deferred_id: String,
    pub error: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionReadyParams {
    pub subscription_id: String,
    pub scope: ReplayScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionClosedReason {
    Terminal,
    Unsubscribed,
    TransportClosed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionClosedParams {
    pub subscription_id: String,
    pub scope: ReplayScope,
    pub reason: SubscriptionClosedReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamEventParams {
    pub subscription_id: String,
    pub scope: ReplayScope,
    pub cursor: ReplayCursor,
    pub item: RunOutputItem,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticNotificationParams {
    pub level: DiagnosticLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Typed union of every host v1 notification payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "method", content = "params")]
pub enum HostNotificationKind {
    #[serde(rename = "subscription.ready")]
    SubscriptionReady(SubscriptionReadyParams),
    #[serde(rename = "subscription.closed")]
    SubscriptionClosed(SubscriptionClosedParams),
    #[serde(rename = "stream.event")]
    StreamEvent(Box<StreamEventParams>),
    #[serde(rename = "run.status")]
    RunStatus(HostRunStatus),
    #[serde(rename = "diagnostic")]
    Diagnostic(DiagnosticNotificationParams),
}

/// JSON-RPC 2.0 notification envelope with a typed host v1 payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostNotification {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub notification: HostNotificationKind,
}

impl HostNotification {
    #[must_use]
    pub fn new(notification: HostNotificationKind) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            notification,
        }
    }
}

/// Serialize one typed host notification to a transport frame.
#[must_use]
pub fn typed_notification(notification: HostNotificationKind) -> Value {
    serde_json::json!(HostNotification::new(notification))
}

/// Strictly decode one canonical v1 notification frame.
///
/// # Errors
///
/// Returns an error for envelope fields outside `jsonrpc`, `method`, and `params`, a non-2.0
/// version, an unknown notification method, or a payload that does not match its concrete DTO.
pub fn validate_v1_notification(value: &Value) -> Result<(), String> {
    validate_exact_object_fields(value, &["jsonrpc", "method", "params"], "notification")?;
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("invalid notification: jsonrpc must be 2.0".to_string());
    }
    serde_json::from_value::<HostNotification>(value.clone())
        .map(|_| ())
        .map_err(|error| format!("invalid notification: {error}"))
}

/// Strictly decode one v1 JSON-RPC error response for the expected stable code.
///
/// # Errors
///
/// Returns an error for a malformed envelope, invalid id, unknown fields, or a code mismatch.
pub fn validate_v1_error_response(expected_code: i64, value: &Value) -> Result<(), String> {
    validate_exact_object_fields(value, &["jsonrpc", "id", "error"], "error response")?;
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("invalid error response: jsonrpc must be 2.0".to_string());
    }
    match value.get("id") {
        Some(Value::Null | Value::String(_)) => {}
        Some(Value::Number(number)) if number.as_i64().is_some() || number.as_u64().is_some() => {}
        _ => {
            return Err(
                "invalid error response: id must be a string, integer, or null".to_string(),
            );
        }
    }
    let error = value
        .get("error")
        .ok_or_else(|| "invalid error response: missing error".to_string())?;
    validate_exact_object_fields(error, &["code", "message"], "error object")?;
    if error.get("code").and_then(Value::as_i64) != Some(expected_code) {
        return Err(format!(
            "invalid error response: expected code {expected_code}"
        ));
    }
    if error.get("message").and_then(Value::as_str).is_none() {
        return Err("invalid error response: message must be a string".to_string());
    }
    Ok(())
}

fn validate_exact_object_fields(
    value: &Value,
    expected: &[&str],
    context: &str,
) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("invalid {context}: expected object"))?;
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "invalid {context}: expected fields {expected:?}, got {actual:?}"
        ))
    }
}

/// One method entry in the machine-readable v1 conformance catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V1MethodContract {
    pub name: &'static str,
    pub params: &'static str,
    pub result: &'static str,
}

/// Every method currently dispatched by the v1 standalone host, including compatibility aliases.
pub const V1_METHOD_CONTRACTS: &[V1MethodContract] = &[
    V1MethodContract {
        name: "initialize",
        params: "HostInitializeParams",
        result: "HostInitializeResult",
    },
    V1MethodContract {
        name: "shutdown",
        params: "EmptyParams",
        result: "ShutdownResult",
    },
    V1MethodContract {
        name: "diagnostics.get",
        params: "EmptyParams",
        result: "DiagnosticsGetResult",
    },
    V1MethodContract {
        name: "profile.list",
        params: "ClientStateParams",
        result: "ProfileListResult",
    },
    V1MethodContract {
        name: "model.list",
        params: "ClientStateParams",
        result: "ModelListResult",
    },
    V1MethodContract {
        name: "profile.get",
        params: "ProfileGetParams",
        result: "ProfileGetResult",
    },
    V1MethodContract {
        name: "model.current",
        params: "ClientStateParams",
        result: "ModelCurrentResult",
    },
    V1MethodContract {
        name: "model.select",
        params: "ModelSelectParams",
        result: "ModelSelectResult",
    },
    V1MethodContract {
        name: "config.get",
        params: "ConfigGetParams",
        result: "ConfigGetResult",
    },
    V1MethodContract {
        name: "storage.importLegacy",
        params: "StorageImportLegacyParams",
        result: "StorageImportLegacyResult",
    },
    V1MethodContract {
        name: "session.create",
        params: "SessionCreateParams",
        result: "SessionCreateResult",
    },
    V1MethodContract {
        name: "session.list",
        params: "SessionListParams",
        result: "SessionListResult",
    },
    V1MethodContract {
        name: "session.search",
        params: "SessionSearchParams",
        result: "SessionSearchResult",
    },
    V1MethodContract {
        name: "session.get",
        params: "SessionGetParams",
        result: "SessionGetResult",
    },
    V1MethodContract {
        name: "session.current.get",
        params: "EmptyParams",
        result: "SessionCurrentResult",
    },
    V1MethodContract {
        name: "session.current.set",
        params: "SessionCurrentSetParams",
        result: "SessionCurrentResult",
    },
    V1MethodContract {
        name: "session.delete",
        params: "SessionDeleteParams",
        result: "SessionDeleteResult",
    },
    V1MethodContract {
        name: "run.start",
        params: "RunStartParams",
        result: "RunStartResult",
    },
    V1MethodContract {
        name: "run.resume",
        params: "RunResumeParams",
        result: "RunResumeResult",
    },
    V1MethodContract {
        name: "run.prompt",
        params: "RunPromptParams",
        result: "RunPromptResult",
    },
    V1MethodContract {
        name: "run.status",
        params: "RunIdentityParams",
        result: "RunStatusResult",
    },
    V1MethodContract {
        name: "run.await",
        params: "RunAwaitParams",
        result: "RunAwaitResult",
    },
    V1MethodContract {
        name: "run.cancel",
        params: "RunCancelParams",
        result: "RunCancelResult",
    },
    V1MethodContract {
        name: "run.steer",
        params: "RunSteerParams",
        result: "RunSteerResult",
    },
    V1MethodContract {
        name: "run.attach",
        params: "RunAttachParams",
        result: "RunAttachmentResult",
    },
    V1MethodContract {
        name: "session.output",
        params: "SessionOutputParams",
        result: "SessionOutputResult",
    },
    V1MethodContract {
        name: "stream.replay",
        params: "StreamReplayParams",
        result: "StreamReplayResult",
    },
    V1MethodContract {
        name: "session.replay",
        params: "SessionReplayParams",
        result: "SessionReplayResult",
    },
    V1MethodContract {
        name: "approval.list",
        params: "ApprovalListParams",
        result: "ApprovalListResult",
    },
    V1MethodContract {
        name: "approval.show",
        params: "ApprovalShowParams",
        result: "ApprovalShowResult",
    },
    V1MethodContract {
        name: "approval.decide",
        params: "ApprovalDecideParams",
        result: "ApprovalDecideResult",
    },
    V1MethodContract {
        name: "deferred.list",
        params: "DeferredListParams",
        result: "DeferredListResult",
    },
    V1MethodContract {
        name: "deferred.show",
        params: "DeferredShowParams",
        result: "DeferredShowResult",
    },
    V1MethodContract {
        name: "deferred.complete",
        params: "DeferredCompleteParams",
        result: "DeferredCompleteResult",
    },
    V1MethodContract {
        name: "deferred.fail",
        params: "DeferredFailParams",
        result: "DeferredFailResult",
    },
    V1MethodContract {
        name: "environment.attach",
        params: "EnvironmentAttachParams",
        result: "EnvironmentAttachResult",
    },
    V1MethodContract {
        name: "environment.detach",
        params: "EnvironmentDetachParams",
        result: "EnvironmentDetachResult",
    },
    V1MethodContract {
        name: "environment.list",
        params: "EnvironmentListParams",
        result: "EnvironmentListResult",
    },
    V1MethodContract {
        name: "environment.health",
        params: "EnvironmentHealthParams",
        result: "EnvironmentHealthResult",
    },
    V1MethodContract {
        name: "environment.active_mount",
        params: "EnvironmentActiveMountParams",
        result: "EnvironmentActiveMountResult",
    },
    V1MethodContract {
        name: "environment.active_unmount",
        params: "EnvironmentActiveUnmountParams",
        result: "EnvironmentActiveUnmountResult",
    },
    V1MethodContract {
        name: "environment.active_list",
        params: "EnvironmentActiveListParams",
        result: "EnvironmentActiveListResult",
    },
    V1MethodContract {
        name: "stream.subscribe",
        params: "StreamSubscribeParams",
        result: "StreamSubscribeResult",
    },
    V1MethodContract {
        name: "stream.unsubscribe",
        params: "StreamUnsubscribeParams",
        result: "StreamUnsubscribeResult",
    },
];

pub const V1_NOTIFICATION_METHODS: &[&str] = &[
    "subscription.ready",
    "stream.event",
    "run.status",
    "diagnostic",
    "subscription.closed",
];

/// Look up one implemented v1 method contract by its exact wire method name.
#[must_use]
pub fn v1_method_contract(method: &str) -> Option<&'static V1MethodContract> {
    V1_METHOD_CONTRACTS
        .iter()
        .find(|contract| contract.name == method)
}

fn validate_dto<T: DeserializeOwned>(
    method: &str,
    direction: &str,
    value: &Value,
) -> Result<(), String> {
    serde_json::from_value::<T>(value.clone())
        .map(|_| ())
        .map_err(|error| format!("invalid {method} {direction}: {error}"))
}

/// Deserialize params through the concrete DTO registered for an implemented v1 method.
///
/// # Errors
///
/// Returns a stable validation message when the method is unknown or its params do not match the
/// registered DTO.
pub fn validate_v1_method_params(method: &str, value: &Value) -> Result<(), String> {
    macro_rules! params {
        ($ty:ty) => {
            validate_dto::<$ty>(method, "params", value)
        };
    }
    match method {
        "initialize" => {
            let params = serde_json::from_value::<crate::HostInitializeParams>(value.clone())
                .map_err(|error| format!("invalid {method} params: {error}"))?;
            crate::validate_host_initialize(&params)
                .map_err(|error| format!("invalid {method} params: {}", error.message))
        }
        "shutdown" | "diagnostics.get" | "session.current.get" => params!(EmptyParams),
        "profile.list" | "model.list" | "model.current" => params!(ClientStateParams),
        "profile.get" => params!(ProfileGetParams),
        "model.select" => params!(ModelSelectParams),
        "config.get" => params!(ConfigGetParams),
        "storage.importLegacy" => params!(crate::StorageImportLegacyParams),
        "session.create" => params!(SessionCreateParams),
        "session.list" => params!(SessionListParams),
        "session.search" => params!(crate::SessionSearchParams),
        "session.get" => params!(SessionGetParams),
        "session.current.set" => params!(SessionCurrentSetParams),
        "session.delete" => params!(SessionDeleteParams),
        "run.start" | "run.prompt" => params!(RunStartParams),
        "run.resume" => params!(crate::RunResumeParams),
        "run.status" => params!(RunIdentityParams),
        "run.await" => params!(RunAwaitParams),
        "run.cancel" => params!(RunCancelParams),
        "run.steer" => params!(RunSteerParams),
        "run.attach" | "session.output" => params!(RunAttachParams),
        "stream.replay" | "session.replay" => params!(StreamReplayParams),
        "approval.list" => params!(ApprovalListParams),
        "approval.show" => params!(ApprovalShowParams),
        "approval.decide" => params!(ApprovalDecideParams),
        "deferred.list" => params!(DeferredListParams),
        "deferred.show" => params!(DeferredShowParams),
        "deferred.complete" => params!(DeferredCompleteParams),
        "deferred.fail" => params!(DeferredFailParams),
        "environment.attach" => params!(crate::EnvironmentAttachParams),
        "environment.detach" => params!(crate::EnvironmentDetachParams),
        "environment.list" => params!(crate::EnvironmentListParams),
        "environment.health" => params!(crate::EnvironmentHealthParams),
        "environment.active_mount" => params!(crate::EnvironmentActiveMountParams),
        "environment.active_unmount" => params!(crate::EnvironmentActiveUnmountParams),
        "environment.active_list" => params!(crate::EnvironmentActiveListParams),
        "stream.subscribe" => params!(StreamSubscribeParams),
        "stream.unsubscribe" => params!(StreamUnsubscribeParams),
        _ => Err(format!("unknown v1 method: {method}")),
    }
}

/// Deserialize a result through the concrete DTO registered for an implemented v1 method.
///
/// # Errors
///
/// Returns a stable validation message when the method is unknown or its result does not match the
/// registered DTO.
pub fn validate_v1_method_result(method: &str, value: &Value) -> Result<(), String> {
    macro_rules! result {
        ($ty:ty) => {
            validate_dto::<$ty>(method, "result", value)
        };
    }
    match method {
        "initialize" => result!(HostInitializeResult),
        "shutdown" => result!(ShutdownResult),
        "diagnostics.get" => result!(DiagnosticsGetResult),
        "profile.list" | "model.list" => result!(ProfileListResult),
        "profile.get" => result!(ProfileGetResult),
        "model.current" | "model.select" => result!(ModelSelection),
        "config.get" => result!(ConfigGetResult),
        "storage.importLegacy" => result!(crate::StorageImportLegacyResult),
        "session.create" => result!(SessionCreateResult),
        "session.list" => result!(SessionListResult),
        "session.search" => result!(crate::SessionSearchResult),
        "session.get" => result!(SessionGetResult),
        "session.current.get" | "session.current.set" => result!(SessionCurrentResult),
        "session.delete" => result!(SessionDeleteResult),
        "run.start" => result!(RunStartResult),
        "run.resume" => result!(crate::RunResumeResult),
        "run.prompt" => result!(RunPromptResult),
        "run.status" | "run.await" => result!(RunStatusResult),
        "run.cancel" => result!(RunCancelResult),
        "run.steer" => result!(RunSteerResult),
        "run.attach" | "session.output" => result!(RunAttachmentResult),
        "stream.replay" | "session.replay" => result!(StreamReplayResult),
        "approval.list" => result!(ApprovalListResult),
        "approval.show" | "approval.decide" => result!(ApprovalResult),
        "deferred.list" => result!(DeferredListResult),
        "deferred.show" | "deferred.complete" | "deferred.fail" => result!(DeferredResult),
        "environment.attach" => result!(crate::EnvironmentAttachResult),
        "environment.detach" => result!(crate::EnvironmentDetachResult),
        "environment.list" => result!(crate::EnvironmentListResult),
        "environment.health" => result!(crate::EnvironmentHealthResult),
        "environment.active_mount" => result!(crate::EnvironmentActiveMountResult),
        "environment.active_unmount" => result!(crate::EnvironmentActiveUnmountResult),
        "environment.active_list" => result!(crate::EnvironmentActiveListResult),
        "stream.subscribe" => result!(StreamSubscribeResult),
        "stream.unsubscribe" => result!(StreamUnsubscribeResult),
        _ => Err(format!("unknown v1 method: {method}")),
    }
}
