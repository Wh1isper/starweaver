//! Generated renderer-safe Desktop host bindings. Do not edit.
#![allow(missing_docs)]

pub const DESKTOP_HOST_PROTOCOL_DIGEST: &str =
    "sha256:92ebe8f13baf1e3aced0f0edcae3b2a9a23e2e5b64718e615bb0c6812b01c2bf";

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_rpc_core::generated as host;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopHostEventScope {
    pub session_id: SessionId,
    pub run_id: RunId,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DesktopHostEventAcknowledgementToken(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DesktopHostOperationAcknowledgementToken(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DesktopOperationId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DesktopPageToken(pub String);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPage {
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<DesktopPageToken>,
}

/// Backend-owned value for one supervisor-only wire field. Renderer input can never construct this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SupervisorFieldValue(Value);
impl SupervisorFieldValue {
    /// Converts one trusted backend-owned value into its wire representation.
    ///
    /// # Errors
    ///
    /// Returns an error when the value cannot be represented as JSON.
    pub fn from_serializable<T: Serialize>(value: T) -> Result<Self, serde_json::Error> {
        serde_json::to_value(value).map(Self)
    }
}

/// Privileged per-operation values that cannot be derived from renderer intent or fixed policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SupervisorDynamicFields(BTreeMap<String, SupervisorFieldValue>);
impl SupervisorDynamicFields {
    /// Inserts one backend-owned value.
    ///
    /// # Errors
    ///
    /// Returns an error when the value cannot be represented as JSON.
    pub fn insert<T: Serialize>(&mut self, field: &str, value: T) -> Result<(), serde_json::Error> {
        self.0.insert(
            field.to_string(),
            SupervisorFieldValue::from_serializable(value)?,
        );
        Ok(())
    }
    fn required(&self, field: &str) -> Result<SupervisorFieldValue, SupervisorFieldsError> {
        self.0.get(field).cloned().ok_or(SupervisorFieldsError)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ApprovalId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct AttachmentId(pub String);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClarificationAnswer {
    pub free_text: Option<String>,
    pub question: String,
    pub selected_options: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ClarificationId(pub String);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigDocument {
    pub default_profile: String,
    pub profiles: Vec<LaunchProfile>,
    pub providers: Vec<ConfigProvider>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ConfigEtag(pub String);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigProvider {
    pub base_url: Option<String>,
    pub enabled: bool,
    pub endpoint_path: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum ConfigReloadMode {
    #[serde(rename = "dry_run")]
    DryRun,
    #[serde(rename = "commit")]
    Commit,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum ContinuationMode {
    #[serde(rename = "preserve")]
    Preserve,
    #[serde(rename = "compatible")]
    Compatible,
    #[serde(rename = "switch")]
    Switch,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DecimalU64(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DeferredId(pub String);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind")]
pub enum InputPart {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LaunchProfile {
    pub instructions: Vec<String>,
    pub model_config: Option<String>,
    pub model_id: String,
    pub model_settings: Option<String>,
    pub name: String,
    pub toolsets: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RunId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum SessionSearchMode {
    #[serde(rename = "literal")]
    Literal,
    #[serde(rename = "hybrid")]
    Hybrid,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum SessionStatus {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "archived")]
    Archived,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "deleted")]
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TextInputPart {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub String);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalDecideIntent {
    pub approval_id: ApprovalId,
    pub decision: String,
    pub expected_revision: DecimalU64,
    pub reason: Option<String>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct ApprovalDecideSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecideCompleteParams {
    pub approval_id: ApprovalId,
    pub decision: String,
    pub expected_revision: DecimalU64,
    pub idempotency_key: SupervisorFieldValue,
    pub reason: Option<String>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecideResult {
    pub approval: Value,
    pub receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalListIntent {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub state: Option<String>,
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct ApprovalListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListResult {
    pub approvals: Value,
    pub page: DesktopPage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalShowIntent {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct ApprovalShowSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalShowCompleteParams {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalShowResult {
    pub approval: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CatalogListIntent {}

#[derive(Clone, Debug)]
pub struct CatalogListSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogListCompleteParams {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogListResult {
    pub profiles: Value,
    pub selection: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClarificationListIntent {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub state: Option<String>,
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct ClarificationListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationListResult {
    pub clarifications: Value,
    pub page: DesktopPage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClarificationResolveIntent {
    pub answers: Vec<ClarificationAnswer>,
    pub clarification_id: ClarificationId,
    pub expected_revision: DecimalU64,
    pub response: Option<String>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct ClarificationResolveSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationResolveCompleteParams {
    pub answers: Vec<ClarificationAnswer>,
    pub clarification_id: ClarificationId,
    pub expected_revision: DecimalU64,
    pub idempotency_key: SupervisorFieldValue,
    pub response: Option<String>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationResolveResult {
    pub clarification: Value,
    pub receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClarificationShowIntent {
    pub clarification_id: ClarificationId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct ClarificationShowSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationShowCompleteParams {
    pub clarification_id: ClarificationId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationShowResult {
    pub clarification: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigActivateIntent {
    pub desired_etag: ConfigEtag,
}

#[derive(Clone, Debug)]
pub struct ConfigActivateSupervisorFields {
    pub activation_id: SupervisorFieldValue,
    pub authorization: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigActivateCompleteParams {
    pub activation_id: SupervisorFieldValue,
    pub authorization: SupervisorFieldValue,
    pub desired_etag: ConfigEtag,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigActivateResult {
    pub receipt: Value,
    pub status: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigDiscardIntent {
    pub desired_etag: ConfigEtag,
}

#[derive(Clone, Debug)]
pub struct ConfigDiscardSupervisorFields {
    pub authorization: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDiscardCompleteParams {
    pub authorization: SupervisorFieldValue,
    pub desired_etag: ConfigEtag,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDiscardResult {
    pub receipt: Value,
    pub status: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigGetIntent {}

#[derive(Clone, Debug)]
pub struct ConfigGetSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGetCompleteParams {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGetResult {
    pub config: Value,
    pub status: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigReloadIntent {
    pub candidate_etag: Option<ConfigEtag>,
    pub expected_active_etag: ConfigEtag,
    pub mode: ConfigReloadMode,
}

#[derive(Clone, Debug)]
pub struct ConfigReloadSupervisorFields {
    pub authorization: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReloadCompleteParams {
    pub authorization: SupervisorFieldValue,
    pub candidate_etag: Option<ConfigEtag>,
    pub expected_active_etag: ConfigEtag,
    pub idempotency_key: SupervisorFieldValue,
    pub mode: ConfigReloadMode,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReloadResult {
    pub candidate_etag: Value,
    pub receipt: Value,
    pub status: Value,
    pub validation: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigUpdateIntent {
    pub candidate: ConfigDocument,
    pub expected_active_etag: ConfigEtag,
}

#[derive(Clone, Debug)]
pub struct ConfigUpdateSupervisorFields {
    pub authorization: SupervisorFieldValue,
    pub candidate_fingerprint: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateCompleteParams {
    pub authorization: SupervisorFieldValue,
    pub candidate: ConfigDocument,
    pub candidate_fingerprint: SupervisorFieldValue,
    pub expected_active_etag: ConfigEtag,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateResult {
    pub receipt: Value,
    pub status: Value,
    pub validation: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigValidateIntent {
    pub candidate: ConfigDocument,
}

#[derive(Clone, Debug)]
pub struct ConfigValidateSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigValidateCompleteParams {
    pub candidate: ConfigDocument,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigValidateResult {
    pub validation: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeferredCompleteIntent {
    pub deferred_id: DeferredId,
    pub expected_revision: DecimalU64,
    pub result_text: String,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct DeferredCompleteSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredCompleteCompleteParams {
    pub deferred_id: DeferredId,
    pub expected_revision: DecimalU64,
    pub idempotency_key: SupervisorFieldValue,
    pub result_text: String,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredCompleteResult {
    pub deferred: Value,
    pub receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeferredFailIntent {
    pub deferred_id: DeferredId,
    pub error: String,
    pub expected_revision: DecimalU64,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct DeferredFailSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredFailCompleteParams {
    pub deferred_id: DeferredId,
    pub error: String,
    pub expected_revision: DecimalU64,
    pub idempotency_key: SupervisorFieldValue,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredFailResult {
    pub deferred: Value,
    pub receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeferredListIntent {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub state: Option<String>,
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct DeferredListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredListResult {
    pub deferred: Value,
    pub page: DesktopPage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeferredShowIntent {
    pub deferred_id: DeferredId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct DeferredShowSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredShowCompleteParams {
    pub deferred_id: DeferredId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredShowResult {
    pub deferred: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentDetachIntent {
    pub attachment_id: AttachmentId,
}

#[derive(Clone, Debug)]
pub struct EnvironmentDetachSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDetachCompleteParams {
    pub attachment_id: AttachmentId,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDetachResult {
    pub attachment: Value,
    pub receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentHealthIntent {
    pub attachment_id: AttachmentId,
}

#[derive(Clone, Debug)]
pub struct EnvironmentHealthSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHealthCompleteParams {
    pub attachment_id: AttachmentId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHealthResult {
    pub attachment: Value,
    pub checked_at: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentListIntent {
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct EnvironmentListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentListResult {
    pub attachments: Value,
    pub page: DesktopPage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelSelectIntent {
    pub profile: String,
}

#[derive(Clone, Debug)]
pub struct ModelSelectSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectCompleteParams {
    pub idempotency_key: SupervisorFieldValue,
    pub profile: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectResult {
    pub receipt: Value,
    pub selection: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelSelectionGetIntent {}

#[derive(Clone, Debug)]
pub struct ModelSelectionGetSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionGetCompleteParams {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionGetResult {
    pub selection: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProfileGetIntent {
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct ProfileGetSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileGetCompleteParams {
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileGetResult {
    pub profile: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunInterruptIntent {
    pub reason: Option<String>,
    pub run_id: RunId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct RunInterruptSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInterruptCompleteParams {
    pub idempotency_key: SupervisorFieldValue,
    pub reason: Option<String>,
    pub run_id: RunId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInterruptResult {
    pub receipt: Value,
    pub run: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunListIntent {
    pub session_id: SessionId,
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct RunListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunListResult {
    pub page: DesktopPage,
    pub runs: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunResumeIntent {
    pub continuation_mode: ContinuationMode,
    pub profile: Option<String>,
    pub run_id: RunId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct RunResumeSupervisorFields {
    pub environment_attachments: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResumeCompleteParams {
    pub continuation_mode: ContinuationMode,
    pub environment_attachments: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
    pub profile: Option<String>,
    pub run_id: RunId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResumeResult {
    pub receipt: Value,
    pub run: Value,
    pub source_run_id: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunStartIntent {
    pub continuation_mode: ContinuationMode,
    pub input: Vec<InputPart>,
    pub profile: Option<String>,
    pub restore_from_run_id: Option<RunId>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct RunStartSupervisorFields {
    pub environment_attachments: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartCompleteParams {
    pub continuation_mode: ContinuationMode,
    pub environment_attachments: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
    pub input: Vec<InputPart>,
    pub profile: Option<String>,
    pub restore_from_run_id: Option<RunId>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartResult {
    pub receipt: Value,
    pub run: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunStatusIntent {
    pub run_id: RunId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct RunStatusSupervisorFields {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatusCompleteParams {
    pub run_id: RunId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatusResult {
    pub controllable_by_current_host: Value,
    pub run: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSteerIntent {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct RunSteerSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSteerCompleteParams {
    pub idempotency_key: SupervisorFieldValue,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSteerResult {
    pub accepted: Value,
    pub receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionCreateIntent {
    pub profile: Option<String>,
    pub title: Option<String>,
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug)]
pub struct SessionCreateSupervisorFields {
    pub deferred_tools: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateCompleteParams {
    pub deferred_tools: SupervisorFieldValue,
    pub idempotency_key: SupervisorFieldValue,
    pub profile: Option<String>,
    pub title: Option<String>,
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateResult {
    pub receipt: Value,
    pub session: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionDeleteIntent {
    pub expected_revision: DecimalU64,
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct SessionDeleteSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDeleteCompleteParams {
    pub expected_revision: DecimalU64,
    pub idempotency_key: SupervisorFieldValue,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDeleteResult {
    pub receipt: Value,
    pub session: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionForkIntent {
    pub session_id: SessionId,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SessionForkSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkCompleteParams {
    pub idempotency_key: SupervisorFieldValue,
    pub session_id: SessionId,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkResult {
    pub receipt: Value,
    pub session: Value,
    pub source_run_id: Value,
    pub source_session_id: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionGetIntent {
    pub session_id: SessionId,
}

#[derive(Clone, Debug)]
pub struct SessionGetSupervisorFields {
    pub run_limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGetCompleteParams {
    pub run_limit: SupervisorFieldValue,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGetResult {
    pub runs: Value,
    pub session: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionListIntent {
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct SessionListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListResult {
    pub page: DesktopPage,
    pub sessions: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionSearchIntent {
    pub mode: SessionSearchMode,
    pub profile: Option<String>,
    pub query: Option<String>,
    pub status: Option<SessionStatus>,
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct SessionSearchSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
    pub mode: SessionSearchMode,
    pub profile: Option<String>,
    pub query: Option<String>,
    pub status: Option<SessionStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    pub hits: Value,
    pub page: DesktopPage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceListIntent {
    pub page_token: Option<DesktopPageToken>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceListSupervisorFields {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListCompleteParams {
    pub cursor: SupervisorFieldValue,
    pub limit: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListResult {
    pub page: DesktopPage,
    pub workspaces: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceRegisterIntent {
    pub display_label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRegisterSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
    pub root: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRegisterCompleteParams {
    pub display_label: Option<String>,
    pub idempotency_key: SupervisorFieldValue,
    pub root: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRegisterResult {
    pub receipt: Value,
    pub workspace: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceRemoveIntent {
    pub expected_revision: DecimalU64,
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRemoveSupervisorFields {
    pub idempotency_key: SupervisorFieldValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRemoveCompleteParams {
    pub expected_revision: DecimalU64,
    pub idempotency_key: SupervisorFieldValue,
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRemoveResult {
    pub receipt: Value,
    pub workspace: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "input", deny_unknown_fields)]
pub enum DesktopHostOperation {
    #[serde(rename = "approval.decide")]
    ApprovalDecide(ApprovalDecideIntent),
    #[serde(rename = "approval.list")]
    ApprovalList(ApprovalListIntent),
    #[serde(rename = "approval.show")]
    ApprovalShow(ApprovalShowIntent),
    #[serde(rename = "catalog.list")]
    CatalogList(CatalogListIntent),
    #[serde(rename = "clarification.list")]
    ClarificationList(ClarificationListIntent),
    #[serde(rename = "clarification.resolve")]
    ClarificationResolve(ClarificationResolveIntent),
    #[serde(rename = "clarification.show")]
    ClarificationShow(ClarificationShowIntent),
    #[serde(rename = "config.activate")]
    ConfigActivate(ConfigActivateIntent),
    #[serde(rename = "config.discard")]
    ConfigDiscard(ConfigDiscardIntent),
    #[serde(rename = "config.get")]
    ConfigGet(ConfigGetIntent),
    #[serde(rename = "config.reload")]
    ConfigReload(ConfigReloadIntent),
    #[serde(rename = "config.update")]
    ConfigUpdate(ConfigUpdateIntent),
    #[serde(rename = "config.validate")]
    ConfigValidate(ConfigValidateIntent),
    #[serde(rename = "deferred.complete")]
    DeferredComplete(DeferredCompleteIntent),
    #[serde(rename = "deferred.fail")]
    DeferredFail(DeferredFailIntent),
    #[serde(rename = "deferred.list")]
    DeferredList(DeferredListIntent),
    #[serde(rename = "deferred.show")]
    DeferredShow(DeferredShowIntent),
    #[serde(rename = "environment.detach")]
    EnvironmentDetach(EnvironmentDetachIntent),
    #[serde(rename = "environment.health")]
    EnvironmentHealth(EnvironmentHealthIntent),
    #[serde(rename = "environment.list")]
    EnvironmentList(EnvironmentListIntent),
    #[serde(rename = "model.select")]
    ModelSelect(ModelSelectIntent),
    #[serde(rename = "model.selection.get")]
    ModelSelectionGet(ModelSelectionGetIntent),
    #[serde(rename = "profile.get")]
    ProfileGet(ProfileGetIntent),
    #[serde(rename = "run.interrupt")]
    RunInterrupt(RunInterruptIntent),
    #[serde(rename = "run.list")]
    RunList(RunListIntent),
    #[serde(rename = "run.resume")]
    RunResume(RunResumeIntent),
    #[serde(rename = "run.start")]
    RunStart(RunStartIntent),
    #[serde(rename = "run.status")]
    RunStatus(RunStatusIntent),
    #[serde(rename = "run.steer")]
    RunSteer(RunSteerIntent),
    #[serde(rename = "session.create")]
    SessionCreate(SessionCreateIntent),
    #[serde(rename = "session.delete")]
    SessionDelete(SessionDeleteIntent),
    #[serde(rename = "session.fork")]
    SessionFork(SessionForkIntent),
    #[serde(rename = "session.get")]
    SessionGet(SessionGetIntent),
    #[serde(rename = "session.list")]
    SessionList(SessionListIntent),
    #[serde(rename = "session.search")]
    SessionSearch(SessionSearchIntent),
    #[serde(rename = "workspace.list")]
    WorkspaceList(WorkspaceListIntent),
    #[serde(rename = "workspace.register")]
    WorkspaceRegister(WorkspaceRegisterIntent),
    #[serde(rename = "workspace.remove")]
    WorkspaceRemove(WorkspaceRemoveIntent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopHostInvocation {
    pub operation_id: DesktopOperationId,
    pub operation: DesktopHostOperation,
}

impl DesktopHostOperation {
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub const fn page_token(&self) -> Option<&DesktopPageToken> {
        match self {
            Self::ApprovalDecide(_) => None,
            Self::ApprovalList(intent) => intent.page_token.as_ref(),
            Self::ApprovalShow(_) => None,
            Self::CatalogList(_) => None,
            Self::ClarificationList(intent) => intent.page_token.as_ref(),
            Self::ClarificationResolve(_) => None,
            Self::ClarificationShow(_) => None,
            Self::ConfigActivate(_) => None,
            Self::ConfigDiscard(_) => None,
            Self::ConfigGet(_) => None,
            Self::ConfigReload(_) => None,
            Self::ConfigUpdate(_) => None,
            Self::ConfigValidate(_) => None,
            Self::DeferredComplete(_) => None,
            Self::DeferredFail(_) => None,
            Self::DeferredList(intent) => intent.page_token.as_ref(),
            Self::DeferredShow(_) => None,
            Self::EnvironmentDetach(_) => None,
            Self::EnvironmentHealth(_) => None,
            Self::EnvironmentList(intent) => intent.page_token.as_ref(),
            Self::ModelSelect(_) => None,
            Self::ModelSelectionGet(_) => None,
            Self::ProfileGet(_) => None,
            Self::RunInterrupt(_) => None,
            Self::RunList(intent) => intent.page_token.as_ref(),
            Self::RunResume(_) => None,
            Self::RunStart(_) => None,
            Self::RunStatus(_) => None,
            Self::RunSteer(_) => None,
            Self::SessionCreate(_) => None,
            Self::SessionDelete(_) => None,
            Self::SessionFork(_) => None,
            Self::SessionGet(_) => None,
            Self::SessionList(intent) => intent.page_token.as_ref(),
            Self::SessionSearch(intent) => intent.page_token.as_ref(),
            Self::WorkspaceList(intent) => intent.page_token.as_ref(),
            Self::WorkspaceRegister(_) => None,
            Self::WorkspaceRemove(_) => None,
        }
    }

    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub const fn requires_idempotency(&self) -> bool {
        match self {
            Self::ApprovalDecide(_) => true,
            Self::ApprovalList(_) => false,
            Self::ApprovalShow(_) => false,
            Self::CatalogList(_) => false,
            Self::ClarificationList(_) => false,
            Self::ClarificationResolve(_) => true,
            Self::ClarificationShow(_) => false,
            Self::ConfigActivate(_) => true,
            Self::ConfigDiscard(_) => true,
            Self::ConfigGet(_) => false,
            Self::ConfigReload(_) => true,
            Self::ConfigUpdate(_) => true,
            Self::ConfigValidate(_) => false,
            Self::DeferredComplete(_) => true,
            Self::DeferredFail(_) => true,
            Self::DeferredList(_) => false,
            Self::DeferredShow(_) => false,
            Self::EnvironmentDetach(_) => true,
            Self::EnvironmentHealth(_) => false,
            Self::EnvironmentList(_) => false,
            Self::ModelSelect(_) => true,
            Self::ModelSelectionGet(_) => false,
            Self::ProfileGet(_) => false,
            Self::RunInterrupt(_) => true,
            Self::RunList(_) => false,
            Self::RunResume(_) => true,
            Self::RunStart(_) => true,
            Self::RunStatus(_) => false,
            Self::RunSteer(_) => true,
            Self::SessionCreate(_) => true,
            Self::SessionDelete(_) => true,
            Self::SessionFork(_) => true,
            Self::SessionGet(_) => false,
            Self::SessionList(_) => false,
            Self::SessionSearch(_) => false,
            Self::WorkspaceList(_) => false,
            Self::WorkspaceRegister(_) => true,
            Self::WorkspaceRemove(_) => true,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SupervisorHostFields {
    ApprovalDecide(ApprovalDecideSupervisorFields),
    ApprovalList(ApprovalListSupervisorFields),
    ApprovalShow(ApprovalShowSupervisorFields),
    CatalogList(CatalogListSupervisorFields),
    ClarificationList(ClarificationListSupervisorFields),
    ClarificationResolve(ClarificationResolveSupervisorFields),
    ClarificationShow(ClarificationShowSupervisorFields),
    ConfigActivate(ConfigActivateSupervisorFields),
    ConfigDiscard(ConfigDiscardSupervisorFields),
    ConfigGet(ConfigGetSupervisorFields),
    ConfigReload(ConfigReloadSupervisorFields),
    ConfigUpdate(ConfigUpdateSupervisorFields),
    ConfigValidate(ConfigValidateSupervisorFields),
    DeferredComplete(DeferredCompleteSupervisorFields),
    DeferredFail(DeferredFailSupervisorFields),
    DeferredList(DeferredListSupervisorFields),
    DeferredShow(DeferredShowSupervisorFields),
    EnvironmentDetach(EnvironmentDetachSupervisorFields),
    EnvironmentHealth(EnvironmentHealthSupervisorFields),
    EnvironmentList(EnvironmentListSupervisorFields),
    ModelSelect(ModelSelectSupervisorFields),
    ModelSelectionGet(ModelSelectionGetSupervisorFields),
    ProfileGet(ProfileGetSupervisorFields),
    RunInterrupt(RunInterruptSupervisorFields),
    RunList(RunListSupervisorFields),
    RunResume(RunResumeSupervisorFields),
    RunStart(RunStartSupervisorFields),
    RunStatus(RunStatusSupervisorFields),
    RunSteer(RunSteerSupervisorFields),
    SessionCreate(SessionCreateSupervisorFields),
    SessionDelete(SessionDeleteSupervisorFields),
    SessionFork(SessionForkSupervisorFields),
    SessionGet(SessionGetSupervisorFields),
    SessionList(SessionListSupervisorFields),
    SessionSearch(SessionSearchSupervisorFields),
    WorkspaceList(WorkspaceListSupervisorFields),
    WorkspaceRegister(WorkspaceRegisterSupervisorFields),
    WorkspaceRemove(WorkspaceRemoveSupervisorFields),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisorFieldsError;

/// Constructs all non-renderer host fields from fixed policy and privileged dynamic evidence.
///
/// # Errors
///
/// Returns an error when a policy value cannot be represented or required privileged evidence is absent.
#[allow(clippy::too_many_lines)]
pub fn build_supervisor_fields(
    operation: &DesktopHostOperation,
    idempotency_key: &str,
    wire_cursor: Option<&str>,
    dynamic_fields: &SupervisorDynamicFields,
) -> Result<SupervisorHostFields, SupervisorFieldsError> {
    match operation {
        DesktopHostOperation::ApprovalDecide(_) => Ok(SupervisorHostFields::ApprovalDecide(
            ApprovalDecideSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ApprovalList(_) => Ok(SupervisorHostFields::ApprovalList(
            ApprovalListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ApprovalShow(_) => Ok(SupervisorHostFields::ApprovalShow(
            ApprovalShowSupervisorFields {},
        )),
        DesktopHostOperation::CatalogList(_) => Ok(SupervisorHostFields::CatalogList(
            CatalogListSupervisorFields {},
        )),
        DesktopHostOperation::ClarificationList(_) => Ok(SupervisorHostFields::ClarificationList(
            ClarificationListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ClarificationResolve(_) => Ok(
            SupervisorHostFields::ClarificationResolve(ClarificationResolveSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            }),
        ),
        DesktopHostOperation::ClarificationShow(_) => Ok(SupervisorHostFields::ClarificationShow(
            ClarificationShowSupervisorFields {},
        )),
        DesktopHostOperation::ConfigActivate(_) => Ok(SupervisorHostFields::ConfigActivate(
            ConfigActivateSupervisorFields {
                activation_id: dynamic_fields.required("activationId")?,
                authorization: dynamic_fields.required("authorization")?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ConfigDiscard(_) => Ok(SupervisorHostFields::ConfigDiscard(
            ConfigDiscardSupervisorFields {
                authorization: dynamic_fields.required("authorization")?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ConfigGet(_) => Ok(SupervisorHostFields::ConfigGet(
            ConfigGetSupervisorFields {},
        )),
        DesktopHostOperation::ConfigReload(_) => Ok(SupervisorHostFields::ConfigReload(
            ConfigReloadSupervisorFields {
                authorization: dynamic_fields.required("authorization")?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ConfigUpdate(_) => Ok(SupervisorHostFields::ConfigUpdate(
            ConfigUpdateSupervisorFields {
                authorization: dynamic_fields.required("authorization")?,
                candidate_fingerprint: dynamic_fields.required("candidateFingerprint")?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ConfigValidate(_) => Ok(SupervisorHostFields::ConfigValidate(
            ConfigValidateSupervisorFields {},
        )),
        DesktopHostOperation::DeferredComplete(_) => Ok(SupervisorHostFields::DeferredComplete(
            DeferredCompleteSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::DeferredFail(_) => Ok(SupervisorHostFields::DeferredFail(
            DeferredFailSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::DeferredList(_) => Ok(SupervisorHostFields::DeferredList(
            DeferredListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::DeferredShow(_) => Ok(SupervisorHostFields::DeferredShow(
            DeferredShowSupervisorFields {},
        )),
        DesktopHostOperation::EnvironmentDetach(_) => Ok(SupervisorHostFields::EnvironmentDetach(
            EnvironmentDetachSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::EnvironmentHealth(_) => Ok(SupervisorHostFields::EnvironmentHealth(
            EnvironmentHealthSupervisorFields {},
        )),
        DesktopHostOperation::EnvironmentList(_) => Ok(SupervisorHostFields::EnvironmentList(
            EnvironmentListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ModelSelect(_) => Ok(SupervisorHostFields::ModelSelect(
            ModelSelectSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::ModelSelectionGet(_) => Ok(SupervisorHostFields::ModelSelectionGet(
            ModelSelectionGetSupervisorFields {},
        )),
        DesktopHostOperation::ProfileGet(_) => Ok(SupervisorHostFields::ProfileGet(
            ProfileGetSupervisorFields {},
        )),
        DesktopHostOperation::RunInterrupt(_) => Ok(SupervisorHostFields::RunInterrupt(
            RunInterruptSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::RunList(_) => {
            Ok(SupervisorHostFields::RunList(RunListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            }))
        }
        DesktopHostOperation::RunResume(_) => {
            Ok(SupervisorHostFields::RunResume(RunResumeSupervisorFields {
                environment_attachments: SupervisorFieldValue::from_serializable(
                    Vec::<String>::new(),
                )
                .map_err(|_| SupervisorFieldsError)?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            }))
        }
        DesktopHostOperation::RunStart(_) => {
            Ok(SupervisorHostFields::RunStart(RunStartSupervisorFields {
                environment_attachments: SupervisorFieldValue::from_serializable(
                    Vec::<String>::new(),
                )
                .map_err(|_| SupervisorFieldsError)?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            }))
        }
        DesktopHostOperation::RunStatus(_) => Ok(SupervisorHostFields::RunStatus(
            RunStatusSupervisorFields {},
        )),
        DesktopHostOperation::RunSteer(_) => {
            Ok(SupervisorHostFields::RunSteer(RunSteerSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            }))
        }
        DesktopHostOperation::SessionCreate(_) => Ok(SupervisorHostFields::SessionCreate(
            SessionCreateSupervisorFields {
                deferred_tools: SupervisorFieldValue::from_serializable(Vec::<String>::new())
                    .map_err(|_| SupervisorFieldsError)?,
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::SessionDelete(_) => Ok(SupervisorHostFields::SessionDelete(
            SessionDeleteSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::SessionFork(_) => Ok(SupervisorHostFields::SessionFork(
            SessionForkSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::SessionGet(_) => Ok(SupervisorHostFields::SessionGet(
            SessionGetSupervisorFields {
                run_limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::SessionList(_) => Ok(SupervisorHostFields::SessionList(
            SessionListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::SessionSearch(_) => Ok(SupervisorHostFields::SessionSearch(
            SessionSearchSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::WorkspaceList(_) => Ok(SupervisorHostFields::WorkspaceList(
            WorkspaceListSupervisorFields {
                cursor: SupervisorFieldValue::from_serializable(wire_cursor)
                    .map_err(|_| SupervisorFieldsError)?,
                limit: SupervisorFieldValue::from_serializable(100_u32)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
        DesktopHostOperation::WorkspaceRegister(_) => Ok(SupervisorHostFields::WorkspaceRegister(
            WorkspaceRegisterSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
                root: dynamic_fields.required("root")?,
            },
        )),
        DesktopHostOperation::WorkspaceRemove(_) => Ok(SupervisorHostFields::WorkspaceRemove(
            WorkspaceRemoveSupervisorFields {
                idempotency_key: SupervisorFieldValue::from_serializable(idempotency_key)
                    .map_err(|_| SupervisorFieldsError)?,
            },
        )),
    }
}

#[derive(Clone, Debug)]
pub enum CompleteHostParams {
    ApprovalDecide(ApprovalDecideCompleteParams),
    ApprovalList(ApprovalListCompleteParams),
    ApprovalShow(ApprovalShowCompleteParams),
    CatalogList(CatalogListCompleteParams),
    ClarificationList(ClarificationListCompleteParams),
    ClarificationResolve(ClarificationResolveCompleteParams),
    ClarificationShow(ClarificationShowCompleteParams),
    ConfigActivate(ConfigActivateCompleteParams),
    ConfigDiscard(ConfigDiscardCompleteParams),
    ConfigGet(ConfigGetCompleteParams),
    ConfigReload(ConfigReloadCompleteParams),
    ConfigUpdate(ConfigUpdateCompleteParams),
    ConfigValidate(ConfigValidateCompleteParams),
    DeferredComplete(DeferredCompleteCompleteParams),
    DeferredFail(DeferredFailCompleteParams),
    DeferredList(DeferredListCompleteParams),
    DeferredShow(DeferredShowCompleteParams),
    EnvironmentDetach(EnvironmentDetachCompleteParams),
    EnvironmentHealth(EnvironmentHealthCompleteParams),
    EnvironmentList(EnvironmentListCompleteParams),
    ModelSelect(ModelSelectCompleteParams),
    ModelSelectionGet(ModelSelectionGetCompleteParams),
    ProfileGet(ProfileGetCompleteParams),
    RunInterrupt(RunInterruptCompleteParams),
    RunList(RunListCompleteParams),
    RunResume(RunResumeCompleteParams),
    RunStart(RunStartCompleteParams),
    RunStatus(RunStatusCompleteParams),
    RunSteer(RunSteerCompleteParams),
    SessionCreate(SessionCreateCompleteParams),
    SessionDelete(SessionDeleteCompleteParams),
    SessionFork(SessionForkCompleteParams),
    SessionGet(SessionGetCompleteParams),
    SessionList(SessionListCompleteParams),
    SessionSearch(SessionSearchCompleteParams),
    WorkspaceList(WorkspaceListCompleteParams),
    WorkspaceRegister(WorkspaceRegisterCompleteParams),
    WorkspaceRemove(WorkspaceRemoveCompleteParams),
}

#[derive(Clone, Debug)]
pub struct SupervisorRequestContext {
    pub request_id: String,
    pub execution_domain: String,
}
#[derive(Clone, Debug)]
pub struct CompleteHostRequest {
    pub execution_domain: String,
    pub request: host::HostRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildHostRequestError {
    AuthorityMismatch,
    InvalidGeneratedRequest,
}

/// Builds an exact generated host call from disjoint renderer and supervisor authority.
///
/// # Errors
///
/// Returns an error when authority variants mismatch or the completed generated params violate the canonical host schema.
#[allow(clippy::too_many_lines)]
pub fn build_complete_host_request(
    operation: DesktopHostOperation,
    supervisor: SupervisorHostFields,
    context: SupervisorRequestContext,
) -> Result<CompleteHostRequest, BuildHostRequestError> {
    let params = match (operation, supervisor) {
        (
            DesktopHostOperation::ApprovalDecide(intent),
            SupervisorHostFields::ApprovalDecide(supervisor),
        ) => CompleteHostParams::ApprovalDecide(ApprovalDecideCompleteParams {
            approval_id: intent.approval_id,
            decision: intent.decision,
            expected_revision: intent.expected_revision,
            idempotency_key: supervisor.idempotency_key,
            reason: intent.reason,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::ApprovalList(intent),
            SupervisorHostFields::ApprovalList(supervisor),
        ) => CompleteHostParams::ApprovalList(ApprovalListCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
            run_id: intent.run_id,
            session_id: intent.session_id,
            state: intent.state,
        }),
        (
            DesktopHostOperation::ApprovalShow(intent),
            SupervisorHostFields::ApprovalShow(_supervisor),
        ) => CompleteHostParams::ApprovalShow(ApprovalShowCompleteParams {
            approval_id: intent.approval_id,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::CatalogList(_intent),
            SupervisorHostFields::CatalogList(_supervisor),
        ) => CompleteHostParams::CatalogList(CatalogListCompleteParams {}),
        (
            DesktopHostOperation::ClarificationList(intent),
            SupervisorHostFields::ClarificationList(supervisor),
        ) => CompleteHostParams::ClarificationList(ClarificationListCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
            run_id: intent.run_id,
            session_id: intent.session_id,
            state: intent.state,
        }),
        (
            DesktopHostOperation::ClarificationResolve(intent),
            SupervisorHostFields::ClarificationResolve(supervisor),
        ) => CompleteHostParams::ClarificationResolve(ClarificationResolveCompleteParams {
            answers: intent.answers,
            clarification_id: intent.clarification_id,
            expected_revision: intent.expected_revision,
            idempotency_key: supervisor.idempotency_key,
            response: intent.response,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::ClarificationShow(intent),
            SupervisorHostFields::ClarificationShow(_supervisor),
        ) => CompleteHostParams::ClarificationShow(ClarificationShowCompleteParams {
            clarification_id: intent.clarification_id,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::ConfigActivate(intent),
            SupervisorHostFields::ConfigActivate(supervisor),
        ) => CompleteHostParams::ConfigActivate(ConfigActivateCompleteParams {
            activation_id: supervisor.activation_id,
            authorization: supervisor.authorization,
            desired_etag: intent.desired_etag,
            idempotency_key: supervisor.idempotency_key,
        }),
        (
            DesktopHostOperation::ConfigDiscard(intent),
            SupervisorHostFields::ConfigDiscard(supervisor),
        ) => CompleteHostParams::ConfigDiscard(ConfigDiscardCompleteParams {
            authorization: supervisor.authorization,
            desired_etag: intent.desired_etag,
            idempotency_key: supervisor.idempotency_key,
        }),
        (
            DesktopHostOperation::ConfigGet(_intent),
            SupervisorHostFields::ConfigGet(_supervisor),
        ) => CompleteHostParams::ConfigGet(ConfigGetCompleteParams {}),
        (
            DesktopHostOperation::ConfigReload(intent),
            SupervisorHostFields::ConfigReload(supervisor),
        ) => CompleteHostParams::ConfigReload(ConfigReloadCompleteParams {
            authorization: supervisor.authorization,
            candidate_etag: intent.candidate_etag,
            expected_active_etag: intent.expected_active_etag,
            idempotency_key: supervisor.idempotency_key,
            mode: intent.mode,
        }),
        (
            DesktopHostOperation::ConfigUpdate(intent),
            SupervisorHostFields::ConfigUpdate(supervisor),
        ) => CompleteHostParams::ConfigUpdate(ConfigUpdateCompleteParams {
            authorization: supervisor.authorization,
            candidate: intent.candidate,
            candidate_fingerprint: supervisor.candidate_fingerprint,
            expected_active_etag: intent.expected_active_etag,
            idempotency_key: supervisor.idempotency_key,
        }),
        (
            DesktopHostOperation::ConfigValidate(intent),
            SupervisorHostFields::ConfigValidate(_supervisor),
        ) => CompleteHostParams::ConfigValidate(ConfigValidateCompleteParams {
            candidate: intent.candidate,
        }),
        (
            DesktopHostOperation::DeferredComplete(intent),
            SupervisorHostFields::DeferredComplete(supervisor),
        ) => CompleteHostParams::DeferredComplete(DeferredCompleteCompleteParams {
            deferred_id: intent.deferred_id,
            expected_revision: intent.expected_revision,
            idempotency_key: supervisor.idempotency_key,
            result_text: intent.result_text,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::DeferredFail(intent),
            SupervisorHostFields::DeferredFail(supervisor),
        ) => CompleteHostParams::DeferredFail(DeferredFailCompleteParams {
            deferred_id: intent.deferred_id,
            error: intent.error,
            expected_revision: intent.expected_revision,
            idempotency_key: supervisor.idempotency_key,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::DeferredList(intent),
            SupervisorHostFields::DeferredList(supervisor),
        ) => CompleteHostParams::DeferredList(DeferredListCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
            run_id: intent.run_id,
            session_id: intent.session_id,
            state: intent.state,
        }),
        (
            DesktopHostOperation::DeferredShow(intent),
            SupervisorHostFields::DeferredShow(_supervisor),
        ) => CompleteHostParams::DeferredShow(DeferredShowCompleteParams {
            deferred_id: intent.deferred_id,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::EnvironmentDetach(intent),
            SupervisorHostFields::EnvironmentDetach(supervisor),
        ) => CompleteHostParams::EnvironmentDetach(EnvironmentDetachCompleteParams {
            attachment_id: intent.attachment_id,
            idempotency_key: supervisor.idempotency_key,
        }),
        (
            DesktopHostOperation::EnvironmentHealth(intent),
            SupervisorHostFields::EnvironmentHealth(_supervisor),
        ) => CompleteHostParams::EnvironmentHealth(EnvironmentHealthCompleteParams {
            attachment_id: intent.attachment_id,
        }),
        (
            DesktopHostOperation::EnvironmentList(_intent),
            SupervisorHostFields::EnvironmentList(supervisor),
        ) => CompleteHostParams::EnvironmentList(EnvironmentListCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
        }),
        (
            DesktopHostOperation::ModelSelect(intent),
            SupervisorHostFields::ModelSelect(supervisor),
        ) => CompleteHostParams::ModelSelect(ModelSelectCompleteParams {
            idempotency_key: supervisor.idempotency_key,
            profile: intent.profile,
        }),
        (
            DesktopHostOperation::ModelSelectionGet(_intent),
            SupervisorHostFields::ModelSelectionGet(_supervisor),
        ) => CompleteHostParams::ModelSelectionGet(ModelSelectionGetCompleteParams {}),
        (
            DesktopHostOperation::ProfileGet(intent),
            SupervisorHostFields::ProfileGet(_supervisor),
        ) => CompleteHostParams::ProfileGet(ProfileGetCompleteParams { name: intent.name }),
        (
            DesktopHostOperation::RunInterrupt(intent),
            SupervisorHostFields::RunInterrupt(supervisor),
        ) => CompleteHostParams::RunInterrupt(RunInterruptCompleteParams {
            idempotency_key: supervisor.idempotency_key,
            reason: intent.reason,
            run_id: intent.run_id,
            session_id: intent.session_id,
        }),
        (DesktopHostOperation::RunList(intent), SupervisorHostFields::RunList(supervisor)) => {
            CompleteHostParams::RunList(RunListCompleteParams {
                cursor: supervisor.cursor,
                limit: supervisor.limit,
                session_id: intent.session_id,
            })
        }
        (DesktopHostOperation::RunResume(intent), SupervisorHostFields::RunResume(supervisor)) => {
            CompleteHostParams::RunResume(RunResumeCompleteParams {
                continuation_mode: intent.continuation_mode,
                environment_attachments: supervisor.environment_attachments,
                idempotency_key: supervisor.idempotency_key,
                profile: intent.profile,
                run_id: intent.run_id,
                session_id: intent.session_id,
            })
        }
        (DesktopHostOperation::RunStart(intent), SupervisorHostFields::RunStart(supervisor)) => {
            CompleteHostParams::RunStart(RunStartCompleteParams {
                continuation_mode: intent.continuation_mode,
                environment_attachments: supervisor.environment_attachments,
                idempotency_key: supervisor.idempotency_key,
                input: intent.input,
                profile: intent.profile,
                restore_from_run_id: intent.restore_from_run_id,
                session_id: intent.session_id,
            })
        }
        (DesktopHostOperation::RunStatus(intent), SupervisorHostFields::RunStatus(_supervisor)) => {
            CompleteHostParams::RunStatus(RunStatusCompleteParams {
                run_id: intent.run_id,
                session_id: intent.session_id,
            })
        }
        (DesktopHostOperation::RunSteer(intent), SupervisorHostFields::RunSteer(supervisor)) => {
            CompleteHostParams::RunSteer(RunSteerCompleteParams {
                idempotency_key: supervisor.idempotency_key,
                run_id: intent.run_id,
                session_id: intent.session_id,
                text: intent.text,
            })
        }
        (
            DesktopHostOperation::SessionCreate(intent),
            SupervisorHostFields::SessionCreate(supervisor),
        ) => CompleteHostParams::SessionCreate(SessionCreateCompleteParams {
            deferred_tools: supervisor.deferred_tools,
            idempotency_key: supervisor.idempotency_key,
            profile: intent.profile,
            title: intent.title,
            workspace_id: intent.workspace_id,
        }),
        (
            DesktopHostOperation::SessionDelete(intent),
            SupervisorHostFields::SessionDelete(supervisor),
        ) => CompleteHostParams::SessionDelete(SessionDeleteCompleteParams {
            expected_revision: intent.expected_revision,
            idempotency_key: supervisor.idempotency_key,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::SessionFork(intent),
            SupervisorHostFields::SessionFork(supervisor),
        ) => CompleteHostParams::SessionFork(SessionForkCompleteParams {
            idempotency_key: supervisor.idempotency_key,
            session_id: intent.session_id,
            title: intent.title,
        }),
        (
            DesktopHostOperation::SessionGet(intent),
            SupervisorHostFields::SessionGet(supervisor),
        ) => CompleteHostParams::SessionGet(SessionGetCompleteParams {
            run_limit: supervisor.run_limit,
            session_id: intent.session_id,
        }),
        (
            DesktopHostOperation::SessionList(_intent),
            SupervisorHostFields::SessionList(supervisor),
        ) => CompleteHostParams::SessionList(SessionListCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
        }),
        (
            DesktopHostOperation::SessionSearch(intent),
            SupervisorHostFields::SessionSearch(supervisor),
        ) => CompleteHostParams::SessionSearch(SessionSearchCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
            mode: intent.mode,
            profile: intent.profile,
            query: intent.query,
            status: intent.status,
        }),
        (
            DesktopHostOperation::WorkspaceList(_intent),
            SupervisorHostFields::WorkspaceList(supervisor),
        ) => CompleteHostParams::WorkspaceList(WorkspaceListCompleteParams {
            cursor: supervisor.cursor,
            limit: supervisor.limit,
        }),
        (
            DesktopHostOperation::WorkspaceRegister(intent),
            SupervisorHostFields::WorkspaceRegister(supervisor),
        ) => CompleteHostParams::WorkspaceRegister(WorkspaceRegisterCompleteParams {
            display_label: intent.display_label,
            idempotency_key: supervisor.idempotency_key,
            root: supervisor.root,
        }),
        (
            DesktopHostOperation::WorkspaceRemove(intent),
            SupervisorHostFields::WorkspaceRemove(supervisor),
        ) => CompleteHostParams::WorkspaceRemove(WorkspaceRemoveCompleteParams {
            expected_revision: intent.expected_revision,
            idempotency_key: supervisor.idempotency_key,
            workspace_id: intent.workspace_id,
        }),
        _ => return Err(BuildHostRequestError::AuthorityMismatch),
    };
    let (method, params) = match &params {
        CompleteHostParams::ApprovalDecide(params) => {
            ("approval.decide", serde_json::to_value(params))
        }
        CompleteHostParams::ApprovalList(params) => ("approval.list", serde_json::to_value(params)),
        CompleteHostParams::ApprovalShow(params) => ("approval.show", serde_json::to_value(params)),
        CompleteHostParams::CatalogList(params) => ("catalog.list", serde_json::to_value(params)),
        CompleteHostParams::ClarificationList(params) => {
            ("clarification.list", serde_json::to_value(params))
        }
        CompleteHostParams::ClarificationResolve(params) => {
            ("clarification.resolve", serde_json::to_value(params))
        }
        CompleteHostParams::ClarificationShow(params) => {
            ("clarification.show", serde_json::to_value(params))
        }
        CompleteHostParams::ConfigActivate(params) => {
            ("config.activate", serde_json::to_value(params))
        }
        CompleteHostParams::ConfigDiscard(params) => {
            ("config.discard", serde_json::to_value(params))
        }
        CompleteHostParams::ConfigGet(params) => ("config.get", serde_json::to_value(params)),
        CompleteHostParams::ConfigReload(params) => ("config.reload", serde_json::to_value(params)),
        CompleteHostParams::ConfigUpdate(params) => ("config.update", serde_json::to_value(params)),
        CompleteHostParams::ConfigValidate(params) => {
            ("config.validate", serde_json::to_value(params))
        }
        CompleteHostParams::DeferredComplete(params) => {
            ("deferred.complete", serde_json::to_value(params))
        }
        CompleteHostParams::DeferredFail(params) => ("deferred.fail", serde_json::to_value(params)),
        CompleteHostParams::DeferredList(params) => ("deferred.list", serde_json::to_value(params)),
        CompleteHostParams::DeferredShow(params) => ("deferred.show", serde_json::to_value(params)),
        CompleteHostParams::EnvironmentDetach(params) => {
            ("environment.detach", serde_json::to_value(params))
        }
        CompleteHostParams::EnvironmentHealth(params) => {
            ("environment.health", serde_json::to_value(params))
        }
        CompleteHostParams::EnvironmentList(params) => {
            ("environment.list", serde_json::to_value(params))
        }
        CompleteHostParams::ModelSelect(params) => ("model.select", serde_json::to_value(params)),
        CompleteHostParams::ModelSelectionGet(params) => {
            ("model.selection.get", serde_json::to_value(params))
        }
        CompleteHostParams::ProfileGet(params) => ("profile.get", serde_json::to_value(params)),
        CompleteHostParams::RunInterrupt(params) => ("run.interrupt", serde_json::to_value(params)),
        CompleteHostParams::RunList(params) => ("run.list", serde_json::to_value(params)),
        CompleteHostParams::RunResume(params) => ("run.resume", serde_json::to_value(params)),
        CompleteHostParams::RunStart(params) => ("run.start", serde_json::to_value(params)),
        CompleteHostParams::RunStatus(params) => ("run.status", serde_json::to_value(params)),
        CompleteHostParams::RunSteer(params) => ("run.steer", serde_json::to_value(params)),
        CompleteHostParams::SessionCreate(params) => {
            ("session.create", serde_json::to_value(params))
        }
        CompleteHostParams::SessionDelete(params) => {
            ("session.delete", serde_json::to_value(params))
        }
        CompleteHostParams::SessionFork(params) => ("session.fork", serde_json::to_value(params)),
        CompleteHostParams::SessionGet(params) => ("session.get", serde_json::to_value(params)),
        CompleteHostParams::SessionList(params) => ("session.list", serde_json::to_value(params)),
        CompleteHostParams::SessionSearch(params) => {
            ("session.search", serde_json::to_value(params))
        }
        CompleteHostParams::WorkspaceList(params) => {
            ("workspace.list", serde_json::to_value(params))
        }
        CompleteHostParams::WorkspaceRegister(params) => {
            ("workspace.register", serde_json::to_value(params))
        }
        CompleteHostParams::WorkspaceRemove(params) => {
            ("workspace.remove", serde_json::to_value(params))
        }
    };
    let params = params.map_err(|_| BuildHostRequestError::InvalidGeneratedRequest)?;
    let frame = serde_json::to_vec(&serde_json::json!({"jsonrpc":"2.0","id":context.request_id,"method":method,"params":params})).map_err(|_| BuildHostRequestError::InvalidGeneratedRequest)?;
    let request = host::decode_request_frame(&frame)
        .map_err(|_| BuildHostRequestError::InvalidGeneratedRequest)?;
    Ok(CompleteHostRequest {
        execution_domain: context.execution_domain,
        request,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum DesktopHostResult {
    ApprovalDecide(ApprovalDecideResult),
    ApprovalList(ApprovalListResult),
    ApprovalShow(ApprovalShowResult),
    CatalogList(CatalogListResult),
    ClarificationList(ClarificationListResult),
    ClarificationResolve(ClarificationResolveResult),
    ClarificationShow(ClarificationShowResult),
    ConfigActivate(ConfigActivateResult),
    ConfigDiscard(ConfigDiscardResult),
    ConfigGet(ConfigGetResult),
    ConfigReload(ConfigReloadResult),
    ConfigUpdate(ConfigUpdateResult),
    ConfigValidate(ConfigValidateResult),
    DeferredComplete(DeferredCompleteResult),
    DeferredFail(DeferredFailResult),
    DeferredList(DeferredListResult),
    DeferredShow(DeferredShowResult),
    EnvironmentDetach(EnvironmentDetachResult),
    EnvironmentHealth(EnvironmentHealthResult),
    EnvironmentList(EnvironmentListResult),
    ModelSelect(ModelSelectResult),
    ModelSelectionGet(ModelSelectionGetResult),
    ProfileGet(ProfileGetResult),
    RunInterrupt(RunInterruptResult),
    RunList(RunListResult),
    RunResume(RunResumeResult),
    RunStart(RunStartResult),
    RunStatus(RunStatusResult),
    RunSteer(RunSteerResult),
    SessionCreate(SessionCreateResult),
    SessionDelete(SessionDeleteResult),
    SessionFork(SessionForkResult),
    SessionGet(SessionGetResult),
    SessionList(SessionListResult),
    SessionSearch(SessionSearchResult),
    WorkspaceList(WorkspaceListResult),
    WorkspaceRegister(WorkspaceRegisterResult),
    WorkspaceRemove(WorkspaceRemoveResult),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    UnsupportedOperation,
    InvalidGeneratedValue,
    UnauthorizedEventClass,
}

/// Projects one typed host result into the reviewed renderer-safe result sum.
///
/// # Errors
///
/// Returns an error for operations outside the renderer manifest or invalid generated values.
#[allow(clippy::too_many_lines)]
pub fn project_host_result(
    result: host::HostResult,
    next_page_token: Option<DesktopPageToken>,
) -> Result<DesktopHostResult, ProjectionError> {
    match result {
        host::HostResult::ApprovalDecide(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ApprovalDecide(ApprovalDecideResult {
                approval: project_field(object, "approval")?,
                receipt: project_field(object, "receipt")?,
            }))
        }
        host::HostResult::ApprovalList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ApprovalList(ApprovalListResult {
                approvals: project_field(object, "approvals")?,
                page: project_page(object, next_page_token)?,
            }))
        }
        host::HostResult::ApprovalShow(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ApprovalShow(ApprovalShowResult {
                approval: project_field(object, "approval")?,
            }))
        }
        host::HostResult::CatalogList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::CatalogList(CatalogListResult {
                profiles: project_field(object, "profiles")?,
                selection: project_field(object, "selection")?,
            }))
        }
        host::HostResult::ClarificationList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ClarificationList(
                ClarificationListResult {
                    clarifications: project_field(object, "clarifications")?,
                    page: project_page(object, next_page_token)?,
                },
            ))
        }
        host::HostResult::ClarificationResolve(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ClarificationResolve(
                ClarificationResolveResult {
                    clarification: project_field(object, "clarification")?,
                    receipt: project_field(object, "receipt")?,
                },
            ))
        }
        host::HostResult::ClarificationShow(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ClarificationShow(
                ClarificationShowResult {
                    clarification: project_field(object, "clarification")?,
                },
            ))
        }
        host::HostResult::ConfigActivate(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ConfigActivate(ConfigActivateResult {
                receipt: project_field(object, "receipt")?,
                status: project_field(object, "status")?,
            }))
        }
        host::HostResult::ConfigDiscard(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ConfigDiscard(ConfigDiscardResult {
                receipt: project_field(object, "receipt")?,
                status: project_field(object, "status")?,
            }))
        }
        host::HostResult::ConfigGet(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ConfigGet(ConfigGetResult {
                config: project_field(object, "config")?,
                status: project_field(object, "status")?,
            }))
        }
        host::HostResult::ConfigReload(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ConfigReload(ConfigReloadResult {
                candidate_etag: project_field(object, "candidateEtag")?,
                receipt: project_field(object, "receipt")?,
                status: project_field(object, "status")?,
                validation: project_field(object, "validation")?,
            }))
        }
        host::HostResult::ConfigUpdate(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ConfigUpdate(ConfigUpdateResult {
                receipt: project_field(object, "receipt")?,
                status: project_field(object, "status")?,
                validation: project_field(object, "validation")?,
            }))
        }
        host::HostResult::ConfigValidate(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ConfigValidate(ConfigValidateResult {
                validation: project_field(object, "validation")?,
            }))
        }
        host::HostResult::DeferredComplete(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::DeferredComplete(
                DeferredCompleteResult {
                    deferred: project_field(object, "deferred")?,
                    receipt: project_field(object, "receipt")?,
                },
            ))
        }
        host::HostResult::DeferredFail(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::DeferredFail(DeferredFailResult {
                deferred: project_field(object, "deferred")?,
                receipt: project_field(object, "receipt")?,
            }))
        }
        host::HostResult::DeferredList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::DeferredList(DeferredListResult {
                deferred: project_field(object, "deferred")?,
                page: project_page(object, next_page_token)?,
            }))
        }
        host::HostResult::DeferredShow(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::DeferredShow(DeferredShowResult {
                deferred: project_field(object, "deferred")?,
            }))
        }
        host::HostResult::EnvironmentDetach(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::EnvironmentDetach(
                EnvironmentDetachResult {
                    attachment: project_field(object, "attachment")?,
                    receipt: project_field(object, "receipt")?,
                },
            ))
        }
        host::HostResult::EnvironmentHealth(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::EnvironmentHealth(
                EnvironmentHealthResult {
                    attachment: project_field(object, "attachment")?,
                    checked_at: project_field(object, "checkedAt")?,
                },
            ))
        }
        host::HostResult::EnvironmentList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::EnvironmentList(EnvironmentListResult {
                attachments: project_field(object, "attachments")?,
                page: project_page(object, next_page_token)?,
            }))
        }
        host::HostResult::ModelSelect(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ModelSelect(ModelSelectResult {
                receipt: project_field(object, "receipt")?,
                selection: project_field(object, "selection")?,
            }))
        }
        host::HostResult::ModelSelectionGet(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ModelSelectionGet(
                ModelSelectionGetResult {
                    selection: project_field(object, "selection")?,
                },
            ))
        }
        host::HostResult::ProfileGet(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::ProfileGet(ProfileGetResult {
                profile: project_field(object, "profile")?,
            }))
        }
        host::HostResult::RunInterrupt(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::RunInterrupt(RunInterruptResult {
                receipt: project_field(object, "receipt")?,
                run: project_field(object, "run")?,
            }))
        }
        host::HostResult::RunList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::RunList(RunListResult {
                page: project_page(object, next_page_token)?,
                runs: project_field(object, "runs")?,
            }))
        }
        host::HostResult::RunResume(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::RunResume(RunResumeResult {
                receipt: project_field(object, "receipt")?,
                run: project_field(object, "run")?,
                source_run_id: project_field(object, "sourceRunId")?,
            }))
        }
        host::HostResult::RunStart(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::RunStart(RunStartResult {
                receipt: project_field(object, "receipt")?,
                run: project_field(object, "run")?,
            }))
        }
        host::HostResult::RunStatus(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::RunStatus(RunStatusResult {
                controllable_by_current_host: project_field(object, "controllableByCurrentHost")?,
                run: project_field(object, "run")?,
            }))
        }
        host::HostResult::RunSteer(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::RunSteer(RunSteerResult {
                accepted: project_field(object, "accepted")?,
                receipt: project_field(object, "receipt")?,
            }))
        }
        host::HostResult::SessionCreate(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::SessionCreate(SessionCreateResult {
                receipt: project_field(object, "receipt")?,
                session: project_field(object, "session")?,
            }))
        }
        host::HostResult::SessionDelete(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::SessionDelete(SessionDeleteResult {
                receipt: project_field(object, "receipt")?,
                session: project_field(object, "session")?,
            }))
        }
        host::HostResult::SessionFork(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::SessionFork(SessionForkResult {
                receipt: project_field(object, "receipt")?,
                session: project_field(object, "session")?,
                source_run_id: project_field(object, "sourceRunId")?,
                source_session_id: project_field(object, "sourceSessionId")?,
            }))
        }
        host::HostResult::SessionGet(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::SessionGet(SessionGetResult {
                runs: project_field(object, "runs")?,
                session: project_field(object, "session")?,
            }))
        }
        host::HostResult::SessionList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::SessionList(SessionListResult {
                page: project_page(object, next_page_token)?,
                sessions: project_field(object, "sessions")?,
            }))
        }
        host::HostResult::SessionSearch(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::SessionSearch(SessionSearchResult {
                hits: project_field(object, "hits")?,
                page: project_page(object, next_page_token)?,
            }))
        }
        host::HostResult::WorkspaceList(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::WorkspaceList(WorkspaceListResult {
                page: project_page(object, next_page_token)?,
                workspaces: project_field(object, "workspaces")?,
            }))
        }
        host::HostResult::WorkspaceRegister(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::WorkspaceRegister(
                WorkspaceRegisterResult {
                    receipt: project_field(object, "receipt")?,
                    workspace: project_field(object, "workspace")?,
                },
            ))
        }
        host::HostResult::WorkspaceRemove(value) => {
            let value =
                serde_json::to_value(value).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
            let object = value
                .as_object()
                .ok_or(ProjectionError::InvalidGeneratedValue)?;
            Ok(DesktopHostResult::WorkspaceRemove(WorkspaceRemoveResult {
                receipt: project_field(object, "receipt")?,
                workspace: project_field(object, "workspace")?,
            }))
        }
        _ => Err(ProjectionError::UnsupportedOperation),
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHostOperationDelivery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledgement_token: Option<DesktopHostOperationAcknowledgementToken>,
    pub result: DesktopHostResult,
}

fn project_page(
    object: &serde_json::Map<String, Value>,
    next_page_token: Option<DesktopPageToken>,
) -> Result<DesktopPage, ProjectionError> {
    let page = object
        .get("page")
        .and_then(Value::as_object)
        .ok_or(ProjectionError::InvalidGeneratedValue)?;
    let has_more = page
        .get("hasMore")
        .and_then(Value::as_bool)
        .ok_or(ProjectionError::InvalidGeneratedValue)?;
    if has_more != next_page_token.is_some() {
        return Err(ProjectionError::InvalidGeneratedValue);
    }
    Ok(DesktopPage {
        has_more,
        next_page_token,
    })
}
fn project_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Value, ProjectionError> {
    object
        .get(field)
        .cloned()
        .map(project_value)
        .ok_or(ProjectionError::InvalidGeneratedValue)
}
fn project_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(project_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .filter(|(key, _)| !is_forbidden_projection_key(key))
                .map(|(key, value)| (key, project_value(value)))
                .collect(),
        ),
        value => value,
    }
}
fn is_forbidden_projection_key(key: &str) -> bool {
    matches!(
        key,
        "acceptedCursor"
            | "cursor"
            | "deliverySequence"
            | "diagnosticRef"
            | "fenceCursor"
            | "fingerprint"
            | "idempotencyKey"
            | "nextCursor"
            | "nextDeliverySequence"
            | "scope"
            | "subscriptionId"
    )
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafeHostEvent {
    pub delivery: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHostEventDelivery {
    pub acknowledgement_token: DesktopHostEventAcknowledgementToken,
    pub event: SafeHostEvent,
}

/// Projects an admitted conversation notification without transport-owned subscription or cursor fields.
///
/// # Errors
///
/// Returns an error for internal notifications or event classes outside the reviewed Desktop profile.
pub fn project_host_notification(
    notification: host::HostNotification,
) -> Result<SafeHostEvent, ProjectionError> {
    let host::HostNotificationParams::HostEvent(params) = notification.params else {
        return Err(ProjectionError::UnsupportedOperation);
    };
    if !matches!(
        &params.delivery.record.event,
        host::HostEvent::ApprovalChangedEvent(_)
            | host::HostEvent::ClarificationChangedEvent(_)
            | host::HostEvent::DeferredChangedEvent(_)
            | host::HostEvent::OutputAvailableEvent(_)
            | host::HostEvent::RunChangedEvent(_)
            | host::HostEvent::TranscriptChangedEvent(_)
    ) {
        return Err(ProjectionError::UnauthorizedEventClass);
    }
    let value = serde_json::to_value(params).map_err(|_| ProjectionError::InvalidGeneratedValue)?;
    let object = value
        .as_object()
        .ok_or(ProjectionError::InvalidGeneratedValue)?;
    Ok(SafeHostEvent {
        delivery: project_field(object, "delivery")?,
    })
}
