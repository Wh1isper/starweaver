//! Generated closed wire types.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::{fmt, str::FromStr};

fn deserialize_json_object<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("expected JSON object"))
    }
}

fn validate_string(value: &str, minimum: usize, maximum: usize, kind: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length < minimum || length > maximum {
        return Err(format!("{kind} length must be in {minimum}..={maximum}"));
    }
    if minimum > 0 && value.trim().is_empty() {
        return Err(format!("{kind} must not be blank"));
    }
    match kind {
        "SchemaDigest"
            if value.len() != 71
                || !value.starts_with("sha256:")
                || !value[7..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            Err("SchemaDigest must be canonical sha256 hex".to_string())
        }
        "HostEventCursor"
            if !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') =>
        {
            Err("HostEventCursor must be unpadded base64url".to_string())
        }
        "Timestamp" if !value.ends_with('Z') => Err("Timestamp must be RFC 3339 UTC".to_string()),
        _ => Ok(()),
    }
}

/// Discriminator `already_exists`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AlreadyExistsDataKind {
    #[serde(rename = "already_exists")]
    Value,
}

/// Generated closed object `AlreadyExistsData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlreadyExistsData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: AlreadyExistsDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Discriminator `approval_changed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ApprovalChangedEventKind {
    #[serde(rename = "approval_changed")]
    Value,
}

/// Generated closed object `ApprovalChangedEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalChangedEvent {
    /// Wire field `approval`.
    pub approval: ApprovalSummary,
    /// Wire field `kind`.
    pub kind: ApprovalChangedEventKind,
}

/// Generated closed object `ApprovalDecideParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecideParams {
    /// Wire field `approvalId`.
    pub approval_id: ApprovalId,
    /// Wire field `decision`.
    pub decision: String,
    /// Wire field `expectedRevision`.
    pub expected_revision: DecimalU64,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `reason`.
    pub reason: Option<String>,
}

/// Generated closed object `ApprovalDecideResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecideResult {
    /// Wire field `approval`.
    pub approval: ApprovalSummary,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Validated generated string `ApprovalId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ApprovalId(String);
impl ApprovalId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "ApprovalId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for ApprovalId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `ApprovalListResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalListResult {
    /// Wire field `approvals`.
    pub approvals: Vec<ApprovalSummary>,
    /// Wire field `page`.
    pub page: PageInfo,
}

/// Generated closed object `ApprovalShowParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalShowParams {
    /// Wire field `approvalId`.
    pub approval_id: ApprovalId,
}

/// Generated closed object `ApprovalShowResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalShowResult {
    /// Wire field `approval`.
    pub approval: ApprovalSummary,
}

/// Generated string enum `ApprovalStatus`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ApprovalStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "cancelled")]
    Cancelled,
}

/// Generated closed object `ApprovalSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalSummary {
    /// Wire field `approvalId`.
    pub approval_id: ApprovalId,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    /// Wire field `status`.
    pub status: ApprovalStatus,
    /// Wire field `title`.
    pub title: String,
    /// Wire field `updatedAt`.
    pub updated_at: Timestamp,
}

/// Validated generated string `AttachmentId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AttachmentId(String);
impl AttachmentId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "AttachmentId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for AttachmentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for AttachmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed discriminated union `AttachmentScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AttachmentScope {
    /// `ConnectionAttachmentScope`.
    ConnectionAttachmentScope(ConnectionAttachmentScope),
    /// `SessionAttachmentScope`.
    SessionAttachmentScope(SessionAttachmentScope),
    /// `RunAttachmentScope`.
    RunAttachmentScope(RunAttachmentScope),
}

/// Discriminator `authorization_denied`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AuthorizationDeniedDataKind {
    #[serde(rename = "authorization_denied")]
    Value,
}

/// Generated closed object `AuthorizationDeniedData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationDeniedData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: AuthorizationDeniedDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `CatalogListParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogListParams {}

/// Generated closed object `CatalogListResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogListResult {
    /// Wire field `profiles`.
    pub profiles: Vec<ProfileSummary>,
    /// Wire field `selection`.
    pub selection: ModelSelection,
}

/// Generated closed object `ClarificationAnswer`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `freeText`.
    pub free_text: Option<String>,
    /// Wire field `question`.
    pub question: String,
    /// Wire field `selectedOptions`.
    pub selected_options: Vec<String>,
}

/// Discriminator `clarification_changed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClarificationChangedEventKind {
    #[serde(rename = "clarification_changed")]
    Value,
}

/// Generated closed object `ClarificationChangedEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationChangedEvent {
    /// Wire field `clarification`.
    pub clarification: ClarificationSummary,
    /// Wire field `kind`.
    pub kind: ClarificationChangedEventKind,
}

/// Validated generated string `ClarificationId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ClarificationId(String);
impl ClarificationId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "ClarificationId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for ClarificationId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for ClarificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `ClarificationQuestion`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationQuestion {
    /// Wire field `header`.
    pub header: String,
    /// Wire field `multiSelect`.
    pub multi_select: bool,
    /// Wire field `options`.
    pub options: Vec<ClarificationQuestionOption>,
    /// Wire field `question`.
    pub question: String,
}

/// Generated closed object `ClarificationQuestionOption`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationQuestionOption {
    /// Wire field `description`.
    pub description: String,
    /// Wire field `label`.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `preview`.
    pub preview: Option<String>,
}

/// Generated closed object `ClarificationResolveParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationResolveParams {
    /// Wire field `answers`.
    pub answers: Vec<ClarificationAnswer>,
    /// Wire field `clarificationId`.
    pub clarification_id: ClarificationId,
    /// Wire field `expectedRevision`.
    pub expected_revision: DecimalU64,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `response`.
    pub response: Option<String>,
}

/// Generated closed object `ClarificationResolveResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationResolveResult {
    /// Wire field `clarification`.
    pub clarification: ClarificationSummary,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Generated string enum `ClarificationStatus`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClarificationStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "resolved")]
    Resolved,
    #[serde(rename = "expired")]
    Expired,
}

/// Generated closed object `ClarificationSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationSummary {
    /// Wire field `clarificationId`.
    pub clarification_id: ClarificationId,
    /// Wire field `questions`.
    pub questions: Vec<ClarificationQuestion>,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    /// Wire field `status`.
    pub status: ClarificationStatus,
    /// Wire field `updatedAt`.
    pub updated_at: Timestamp,
}

/// Generated closed object `ClientInfo`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientInfo {
    /// Wire field `name`.
    pub name: String,
    /// Wire field `version`.
    pub version: String,
}

/// Discriminator `configuration_failed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ConfigurationFailedDataKind {
    #[serde(rename = "configuration_failed")]
    Value,
}

/// Generated closed object `ConfigurationFailedData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationFailedData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: ConfigurationFailedDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Discriminator `connection`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ConnectionAttachmentScopeKind {
    #[serde(rename = "connection")]
    Value,
}

/// Generated closed object `ConnectionAttachmentScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionAttachmentScope {
    /// Wire field `kind`.
    pub kind: ConnectionAttachmentScopeKind,
}

/// Generated string enum `ContinuationMode`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContinuationMode {
    #[serde(rename = "preserve")]
    Preserve,
    #[serde(rename = "compatible")]
    Compatible,
    #[serde(rename = "switch")]
    Switch,
}

/// Discriminator `cursor_invalid`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CursorInvalidDataKind {
    #[serde(rename = "cursor_invalid")]
    Value,
}

/// Generated closed object `CursorInvalidData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorInvalidData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: CursorInvalidDataKind,
    /// Wire field `reason`.
    pub reason: CursorInvalidReason,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated string enum `CursorInvalidReason`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CursorInvalidReason {
    #[serde(rename = "malformed")]
    Malformed,
    #[serde(rename = "integrity_failed")]
    IntegrityFailed,
    #[serde(rename = "scope_mismatch")]
    ScopeMismatch,
    #[serde(rename = "view_mismatch")]
    ViewMismatch,
    #[serde(rename = "storage_mismatch")]
    StorageMismatch,
    #[serde(rename = "retention_gap")]
    RetentionGap,
}

/// Canonical decimal-string unsigned 64-bit value.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DecimalU64(u64);
impl DecimalU64 {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
    pub fn checked_increment(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}
impl fmt::Display for DecimalU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl FromStr for DecimalU64 {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("non-canonical decimal u64".to_string());
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|error| error.to_string())
    }
}
impl Serialize for DecimalU64 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Discriminator `deferred_changed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DeferredChangedEventKind {
    #[serde(rename = "deferred_changed")]
    Value,
}

/// Generated closed object `DeferredChangedEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredChangedEvent {
    /// Wire field `deferred`.
    pub deferred: DeferredSummary,
    /// Wire field `kind`.
    pub kind: DeferredChangedEventKind,
}

/// Generated closed object `DeferredCompleteParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredCompleteParams {
    /// Wire field `deferredId`.
    pub deferred_id: DeferredId,
    /// Wire field `expectedRevision`.
    pub expected_revision: DecimalU64,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `resultText`.
    pub result_text: String,
}

/// Generated closed object `DeferredCompleteResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredCompleteResult {
    /// Wire field `deferred`.
    pub deferred: DeferredSummary,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Generated closed object `DeferredFailParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredFailParams {
    /// Wire field `deferredId`.
    pub deferred_id: DeferredId,
    /// Wire field `error`.
    pub error: String,
    /// Wire field `expectedRevision`.
    pub expected_revision: DecimalU64,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
}

/// Generated closed object `DeferredFailResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredFailResult {
    /// Wire field `deferred`.
    pub deferred: DeferredSummary,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Validated generated string `DeferredId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct DeferredId(String);
impl DeferredId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "DeferredId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for DeferredId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for DeferredId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `DeferredListResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredListResult {
    /// Wire field `deferred`.
    pub deferred: Vec<DeferredSummary>,
    /// Wire field `page`.
    pub page: PageInfo,
}

/// Generated closed object `DeferredShowParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredShowParams {
    /// Wire field `deferredId`.
    pub deferred_id: DeferredId,
}

/// Generated closed object `DeferredShowResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredShowResult {
    /// Wire field `deferred`.
    pub deferred: DeferredSummary,
}

/// Generated string enum `DeferredStatus`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DeferredStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "cancelled")]
    Cancelled,
}

/// Generated closed object `DeferredSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredSummary {
    /// Wire field `deferredId`.
    pub deferred_id: DeferredId,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    /// Wire field `status`.
    pub status: DeferredStatus,
    /// Wire field `toolName`.
    pub tool_name: String,
    /// Wire field `updatedAt`.
    pub updated_at: Timestamp,
}

/// Generated closed object `DeferredToolDefinition`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredToolDefinition {
    /// Wire field `description`.
    pub description: String,
    #[serde(deserialize_with = "deserialize_json_object")]
    /// Wire field `inputSchema`.
    pub input_schema: JsonObject,
    /// Wire field `inputSchemaDigest`.
    pub input_schema_digest: SchemaDigest,
    /// Wire field `instructions`.
    pub instructions: Vec<String>,
    /// Wire field `name`.
    pub name: String,
}

/// Discriminator `diagnostic`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DiagnosticEventKind {
    #[serde(rename = "diagnostic")]
    Value,
}

/// Generated closed object `DiagnosticEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticEvent {
    /// Wire field `code`.
    pub code: String,
    /// Wire field `kind`.
    pub kind: DiagnosticEventKind,
    /// Wire field `level`.
    pub level: String,
    /// Wire field `message`.
    pub message: String,
}

/// Generated closed object `DiagnosticsGetParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsGetParams {}

/// Generated closed object `DiagnosticsGetResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `pendingRecoveryItems`.
    pub pending_recovery_items: DecimalU64,
    /// Wire field `protocol`.
    pub protocol: ProtocolIdentity,
    /// Wire field `runtimeStatus`.
    pub runtime_status: String,
    /// Wire field `sdk`.
    pub sdk: String,
    /// Wire field `storageCurrent`.
    pub storage_current: bool,
    /// Wire field `version`.
    pub version: String,
}

/// Generated closed object `EmptyParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmptyParams {}

/// Generated closed object `EmptyResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmptyResult {}

/// Generated closed object `EnvironmentAttachParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttachParams {
    /// Wire field `environmentId`.
    pub environment_id: EnvironmentId,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `scope`.
    pub scope: AttachmentScope,
}

/// Generated closed object `EnvironmentAttachResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttachResult {
    /// Wire field `attachment`.
    pub attachment: EnvironmentAttachment,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Generated closed object `EnvironmentAttachment`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttachment {
    /// Wire field `attachmentId`.
    pub attachment_id: AttachmentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `displayName`.
    pub display_name: Option<String>,
    /// Wire field `environmentId`.
    pub environment_id: EnvironmentId,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `scope`.
    pub scope: AttachmentScope,
    /// Wire field `status`.
    pub status: EnvironmentStatus,
}

/// Discriminator `environment_changed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EnvironmentChangedEventKind {
    #[serde(rename = "environment_changed")]
    Value,
}

/// Generated closed object `EnvironmentChangedEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentChangedEvent {
    /// Wire field `attachment`.
    pub attachment: EnvironmentAttachment,
    /// Wire field `kind`.
    pub kind: EnvironmentChangedEventKind,
}

/// Generated closed object `EnvironmentDetachParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentDetachParams {
    /// Wire field `attachmentId`.
    pub attachment_id: AttachmentId,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
}

/// Generated closed object `EnvironmentDetachResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentDetachResult {
    /// Wire field `attachment`.
    pub attachment: EnvironmentAttachment,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Generated closed object `EnvironmentHealthParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentHealthParams {
    /// Wire field `attachmentId`.
    pub attachment_id: AttachmentId,
}

/// Generated closed object `EnvironmentHealthResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentHealthResult {
    /// Wire field `attachment`.
    pub attachment: EnvironmentAttachment,
    /// Wire field `checkedAt`.
    pub checked_at: Timestamp,
}

/// Validated generated string `EnvironmentId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EnvironmentId(String);
impl EnvironmentId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "EnvironmentId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for EnvironmentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `EnvironmentListParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `cursor`.
    pub cursor: Option<String>,
    /// Wire field `limit`.
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `scope`.
    pub scope: Option<AttachmentScope>,
}

/// Generated closed object `EnvironmentListResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentListResult {
    /// Wire field `attachments`.
    pub attachments: Vec<EnvironmentAttachment>,
    /// Wire field `page`.
    pub page: PageInfo,
}

/// Generated closed object `EnvironmentMountListParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMountListParams {
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `EnvironmentMountListResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMountListResult {
    /// Wire field `mounts`.
    pub mounts: Vec<EnvironmentMountSummary>,
}

/// Generated closed object `EnvironmentMountParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMountParams {
    /// Wire field `attachmentId`.
    pub attachment_id: AttachmentId,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `resourceRef`.
    pub resource_ref: String,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `EnvironmentMountResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMountResult {
    /// Wire field `mountId`.
    pub mount_id: String,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Generated closed object `EnvironmentMountSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMountSummary {
    /// Wire field `attachmentId`.
    pub attachment_id: AttachmentId,
    /// Wire field `mountId`.
    pub mount_id: String,
    /// Wire field `resourceLabel`.
    pub resource_label: String,
}

/// Generated string enum `EnvironmentStatus`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EnvironmentStatus {
    #[serde(rename = "attaching")]
    Attaching,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "degraded")]
    Degraded,
    #[serde(rename = "detached")]
    Detached,
}

/// Discriminator `environment_unavailable`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EnvironmentUnavailableDataKind {
    #[serde(rename = "environment_unavailable")]
    Value,
}

/// Generated closed object `EnvironmentUnavailableData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentUnavailableData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: EnvironmentUnavailableDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `EnvironmentUnmountParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentUnmountParams {
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `mountId`.
    pub mount_id: String,
}

/// Generated closed object `EnvironmentUnmountResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentUnmountResult {
    /// Wire field `mountId`.
    pub mount_id: String,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `removed`.
    pub removed: bool,
}

/// Generated closed object `EventDelivery`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventDelivery {
    /// Wire field `cursor`.
    pub cursor: HostEventCursor,
    /// Wire field `record`.
    pub record: EventRecord,
}

/// Validated generated string `EventId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventId(String);
impl EventId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "EventId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated string enum `EventProfile`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EventProfile {
    #[serde(rename = "conversation.v1")]
    ConversationV1,
    #[serde(rename = "operations.v1")]
    OperationsV1,
    #[serde(rename = "desktop.conversation.v1")]
    DesktopConversationV1,
}

/// Generated closed object `EventRecord`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventRecord {
    /// Wire field `event`.
    pub event: HostEvent,
    /// Wire field `eventId`.
    pub event_id: EventId,
    /// Wire field `occurredAt`.
    pub occurred_at: Timestamp,
    /// Wire field `scope`.
    pub scope: ResourceScope,
}

/// Generated closed object `EventViewRequest`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventViewRequest {
    /// Wire field `optionalFeatures`.
    pub optional_features: Vec<String>,
    /// Wire field `profile`.
    pub profile: EventProfile,
    /// Wire field `scope`.
    pub scope: ResourceScope,
}

/// Generated closed object `EventsReplayParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsReplayParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `cursor`.
    pub cursor: Option<HostEventCursor>,
    /// Wire field `limit`.
    pub limit: u32,
    /// Wire field `view`.
    pub view: EventViewRequest,
}

/// Generated closed object `EventsReplayResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsReplayResult {
    /// Wire field `deliveries`.
    pub deliveries: Vec<EventDelivery>,
    /// Wire field `hasMore`.
    pub has_more: bool,
    /// Wire field `nextCursor`.
    pub next_cursor: HostEventCursor,
}

/// Generated closed object `EventsSubscribeParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsSubscribeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `cursor`.
    pub cursor: Option<HostEventCursor>,
    /// Wire field `view`.
    pub view: EventViewRequest,
}

/// Discriminator `1`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EventsSubscribeResultNextDeliverySequence {
    #[serde(rename = "1")]
    Value,
}

/// Generated closed object `EventsSubscribeResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsSubscribeResult {
    /// Wire field `acceptedCursor`.
    pub accepted_cursor: HostEventCursor,
    /// Wire field `fenceCursor`.
    pub fence_cursor: HostEventCursor,
    /// Wire field `nextDeliverySequence`.
    pub next_delivery_sequence: EventsSubscribeResultNextDeliverySequence,
    /// Wire field `subscriptionId`.
    pub subscription_id: SubscriptionId,
}

/// Generated closed object `EventsUnsubscribeParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsUnsubscribeParams {
    /// Wire field `subscriptionId`.
    pub subscription_id: SubscriptionId,
}

/// Generated closed object `EventsUnsubscribeResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventsUnsubscribeResult {
    /// Wire field `closed`.
    pub closed: bool,
    /// Wire field `subscriptionId`.
    pub subscription_id: SubscriptionId,
}

/// Validated generated string `FeatureId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FeatureId(String);
impl FeatureId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "FeatureId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for FeatureId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Discriminator `global`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GlobalResourceScopeKind {
    #[serde(rename = "global")]
    Value,
}

/// Generated closed object `GlobalResourceScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalResourceScope {
    /// Wire field `kind`.
    pub kind: GlobalResourceScopeKind,
}

/// Generated closed discriminated union `HostEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum HostEvent {
    /// `SessionChangedEvent`.
    SessionChangedEvent(SessionChangedEvent),
    /// `RunChangedEvent`.
    RunChangedEvent(RunChangedEvent),
    /// `OutputAvailableEvent`.
    OutputAvailableEvent(OutputAvailableEvent),
    /// `ApprovalChangedEvent`.
    ApprovalChangedEvent(ApprovalChangedEvent),
    /// `DeferredChangedEvent`.
    DeferredChangedEvent(DeferredChangedEvent),
    /// `ClarificationChangedEvent`.
    ClarificationChangedEvent(ClarificationChangedEvent),
    /// `EnvironmentChangedEvent`.
    EnvironmentChangedEvent(EnvironmentChangedEvent),
    /// `DiagnosticEvent`.
    DiagnosticEvent(DiagnosticEvent),
}

/// Validated generated string `HostEventCursor`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HostEventCursor(String);
impl HostEventCursor {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 1024, "HostEventCursor")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for HostEventCursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for HostEventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `HostEventNotificationParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostEventNotificationParams {
    /// Wire field `delivery`.
    pub delivery: EventDelivery,
    /// Wire field `deliverySequence`.
    pub delivery_sequence: DecimalU64,
    /// Wire field `subscriptionId`.
    pub subscription_id: SubscriptionId,
}

/// Discriminator `idempotency_conflict`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum IdempotencyConflictDataKind {
    #[serde(rename = "idempotency_conflict")]
    Value,
}

/// Generated closed object `IdempotencyConflictData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdempotencyConflictData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: IdempotencyConflictDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Validated generated string `IdempotencyKey`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);
impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "IdempotencyKey")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `InitializeParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeParams {
    /// Wire field `clientInfo`.
    pub client_info: ClientInfo,
    /// Wire field `protocol`.
    pub protocol: ProtocolIdentity,
    /// Wire field `requiredFeatures`.
    pub required_features: Vec<FeatureId>,
    /// Wire field `supportedFeatures`.
    pub supported_features: Vec<FeatureId>,
}

/// Generated closed object `InitializeResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeResult {
    /// Wire field `launch`.
    pub launch: LaunchCompatibility,
    /// Wire field `negotiatedFeatures`.
    pub negotiated_features: Vec<FeatureId>,
    /// Wire field `protocol`.
    pub protocol: ProtocolIdentity,
    /// Wire field `runtimeBuild`.
    pub runtime_build: RuntimeBuildIdentity,
    /// Wire field `runtimeStatus`.
    pub runtime_status: String,
    /// Wire field `serverInfo`.
    pub server_info: ServerInfo,
    /// Wire field `startupReconciliation`.
    pub startup_reconciliation: StartupReconciliation,
    /// Wire field `storage`.
    pub storage: StorageCompatibility,
    /// Wire field `supportedFeatures`.
    pub supported_features: Vec<FeatureId>,
    /// Wire field `workspace`.
    pub workspace: WorkspaceCompatibility,
}

/// Generated closed discriminated union `InputPart`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum InputPart {
    /// `TextInputPart`.
    TextInputPart(TextInputPart),
    /// `ResourceInputPart`.
    ResourceInputPart(ResourceInputPart),
}

/// Generated closed object `InteractionListParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InteractionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `cursor`.
    pub cursor: Option<String>,
    /// Wire field `limit`.
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `runId`.
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `sessionId`.
    pub session_id: Option<SessionId>,
}

/// Discriminator `internal_error`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InternalErrorDataKind {
    #[serde(rename = "internal_error")]
    Value,
}

/// Generated closed object `InternalErrorData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InternalErrorData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: InternalErrorDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Discriminator `invalid_params`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InvalidParamsDataKind {
    #[serde(rename = "invalid_params")]
    Value,
}

/// Generated closed object `InvalidParamsData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidParamsData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: InvalidParamsDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Discriminator `invalid_request`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InvalidRequestDataKind {
    #[serde(rename = "invalid_request")]
    Value,
}

/// Generated closed object `InvalidRequestData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidRequestData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: InvalidRequestDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Explicit arbitrary JSON object `JsonObject`.
pub type JsonObject = Value;

/// Generated closed object `LaunchCapabilityCaps`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchCapabilityCaps {
    /// Wire field `clarifyingQuestions`.
    pub clarifying_questions: bool,
    /// Wire field `hitl`.
    pub hitl: bool,
    /// Wire field `nativeLocalShell`.
    pub native_local_shell: bool,
}

/// Generated closed object `LaunchCompatibility`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchCompatibility {
    /// Wire field `acceptedMaximumVersion`.
    pub accepted_maximum_version: u32,
    /// Wire field `acceptedMinimumVersion`.
    pub accepted_minimum_version: u32,
    /// Wire field `configurationGeneration`.
    pub configuration_generation: DecimalU64,
    /// Wire field `effectiveSchema`.
    pub effective_schema: LaunchSchemaIdentity,
    /// Wire field `envelopeDigest`.
    pub envelope_digest: SchemaDigest,
    /// Wire field `mode`.
    pub mode: String,
}

/// Generated closed object `LaunchDatabase`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchDatabase {
    /// Wire field `identity`.
    pub identity: String,
    /// Wire field `path`.
    pub path: String,
}

/// Discriminator `workspace_execution`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LaunchEnvelopeMode {
    #[serde(rename = "workspace_execution")]
    Value,
}

/// Generated closed object `LaunchEnvelope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchEnvelope {
    /// Wire field `capabilityCaps`.
    pub capability_caps: LaunchCapabilityCaps,
    /// Wire field `configurationGeneration`.
    pub configuration_generation: DecimalU64,
    /// Wire field `database`.
    pub database: LaunchDatabase,
    /// Wire field `defaultProfile`.
    pub default_profile: String,
    /// Wire field `executionDomainId`.
    pub execution_domain_id: String,
    /// Wire field `mode`.
    pub mode: LaunchEnvelopeMode,
    /// Wire field `profiles`.
    pub profiles: Vec<LaunchProfile>,
    /// Wire field `providers`.
    pub providers: Vec<LaunchProvider>,
    /// Wire field `schema`.
    pub schema: LaunchSchemaIdentity,
    /// Wire field `stateDirectory`.
    pub state_directory: String,
    /// Wire field `workspace`.
    pub workspace: LaunchWorkspace,
}

/// Generated closed object `LaunchProfile`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchProfile {
    /// Wire field `instructions`.
    pub instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `modelConfig`.
    pub model_config: Option<String>,
    /// Wire field `modelId`.
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `modelSettings`.
    pub model_settings: Option<String>,
    /// Wire field `name`.
    pub name: String,
    /// Wire field `toolsets`.
    pub toolsets: Vec<String>,
}

/// Generated closed object `LaunchProvider`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchProvider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `baseUrl`.
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `credentialEnv`.
    pub credential_env: Option<String>,
    /// Wire field `enabled`.
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `endpointPath`.
    pub endpoint_path: Option<String>,
    /// Wire field `name`.
    pub name: String,
}

/// Discriminator `starweaver.rpc.launch`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LaunchSchemaIdentityName {
    #[serde(rename = "starweaver.rpc.launch")]
    Value,
}

/// Generated closed object `LaunchSchemaIdentity`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchSchemaIdentity {
    /// Wire field `name`.
    pub name: LaunchSchemaIdentityName,
    /// Wire field `version`.
    pub version: u32,
}

/// Generated closed object `LaunchWorkspace`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchWorkspace {
    /// Wire field `identity`.
    pub identity: String,
    /// Wire field `root`.
    pub root: String,
}

/// Discriminator `method_not_found`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MethodNotFoundDataKind {
    #[serde(rename = "method_not_found")]
    Value,
}

/// Generated closed object `MethodNotFoundData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodNotFoundData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: MethodNotFoundDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `ModelSelectParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelectParams {
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `profile`.
    pub profile: String,
}

/// Generated closed object `ModelSelectResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelectResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `selection`.
    pub selection: ModelSelection,
}

/// Generated closed object `ModelSelection`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    /// Wire field `modelId`.
    pub model_id: String,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `selectedProfile`.
    pub selected_profile: String,
}

/// Generated closed object `ModelSelectionGetParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelectionGetParams {}

/// Generated closed object `ModelSelectionGetResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelectionGetResult {
    /// Wire field `selection`.
    pub selection: ModelSelection,
}

/// Generated closed object `MutationReceipt`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationReceipt {
    /// Wire field `fingerprint`.
    pub fingerprint: SchemaDigest,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `operation`.
    pub operation: String,
    /// Wire field `receiptId`.
    pub receipt_id: ReceiptId,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    /// Wire field `replayed`.
    pub replayed: bool,
    /// Wire field `state`.
    pub state: String,
    /// Wire field `targetRef`.
    pub target_ref: String,
}

/// Discriminator `not_found`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum NotFoundDataKind {
    #[serde(rename = "not_found")]
    Value,
}

/// Generated closed object `NotFoundData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotFoundData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: NotFoundDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Discriminator `not_initialized`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum NotInitializedDataKind {
    #[serde(rename = "not_initialized")]
    Value,
}

/// Generated closed object `NotInitializedData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotInitializedData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: NotInitializedDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Discriminator `output_available`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OutputAvailableEventKind {
    #[serde(rename = "output_available")]
    Value,
}

/// Generated closed object `OutputAvailableEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputAvailableEvent {
    /// Wire field `kind`.
    pub kind: OutputAvailableEventKind,
    /// Wire field `outputRef`.
    pub output_ref: String,
    /// Wire field `preview`.
    pub preview: String,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `PageInfo`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageInfo {
    /// Wire field `hasMore`.
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `nextCursor`.
    pub next_cursor: Option<String>,
}

/// Discriminator `parse_error`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ParseErrorDataKind {
    #[serde(rename = "parse_error")]
    Value,
}

/// Generated closed object `ParseErrorData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParseErrorData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: ParseErrorDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `ProfileDetail`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileDetail {
    /// Wire field `instructions`.
    pub instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `label`.
    pub label: Option<String>,
    /// Wire field `mcpServers`.
    pub mcp_servers: Vec<String>,
    /// Wire field `modelId`.
    pub model_id: String,
    /// Wire field `name`.
    pub name: String,
    /// Wire field `subagents`.
    pub subagents: Vec<String>,
    /// Wire field `toolsets`.
    pub toolsets: Vec<String>,
}

/// Generated closed object `ProfileGetParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileGetParams {
    /// Wire field `name`.
    pub name: String,
}

/// Generated closed object `ProfileGetResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileGetResult {
    /// Wire field `profile`.
    pub profile: ProfileDetail,
}

/// Generated closed object `ProfileSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `label`.
    pub label: Option<String>,
    /// Wire field `modelId`.
    pub model_id: String,
    /// Wire field `name`.
    pub name: String,
    /// Wire field `source`.
    pub source: String,
}

/// Discriminator `starweaver.host`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProtocolIdentityName {
    #[serde(rename = "starweaver.host")]
    Value,
}

/// Generated closed object `ProtocolIdentity`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolIdentity {
    /// Wire field `major`.
    pub major: u32,
    /// Wire field `name`.
    pub name: ProtocolIdentityName,
    /// Wire field `revision`.
    pub revision: String,
    /// Wire field `schemaDigest`.
    pub schema_digest: SchemaDigest,
}

/// Validated generated string `ReceiptId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ReceiptId(String);
impl ReceiptId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "ReceiptId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for ReceiptId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for ReceiptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Validated generated string `RequestId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestId(String);
impl RequestId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "RequestId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Discriminator `resource`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ResourceInputPartKind {
    #[serde(rename = "resource")]
    Value,
}

/// Generated closed object `ResourceInputPart`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceInputPart {
    /// Wire field `kind`.
    pub kind: ResourceInputPartKind,
    /// Wire field `mediaType`.
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `name`.
    pub name: Option<String>,
    /// Wire field `uri`.
    pub uri: String,
}

/// Generated closed discriminated union `ResourceScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResourceScope {
    /// `GlobalResourceScope`.
    GlobalResourceScope(GlobalResourceScope),
    /// `SessionResourceScope`.
    SessionResourceScope(SessionResourceScope),
    /// `RunResourceScope`.
    RunResourceScope(RunResourceScope),
}

/// Discriminator `run`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunAttachmentScopeKind {
    #[serde(rename = "run")]
    Value,
}

/// Generated closed object `RunAttachmentScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunAttachmentScope {
    /// Wire field `kind`.
    pub kind: RunAttachmentScopeKind,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Discriminator `run_changed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunChangedEventKind {
    #[serde(rename = "run_changed")]
    Value,
}

/// Generated closed object `RunChangedEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunChangedEvent {
    /// Wire field `kind`.
    pub kind: RunChangedEventKind,
    /// Wire field `run`.
    pub run: RunSummary,
}

/// Discriminator `run_conflict`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunConflictDataKind {
    #[serde(rename = "run_conflict")]
    Value,
}

/// Generated closed object `RunConflictData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunConflictData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: RunConflictDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Validated generated string `RunId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RunId(String);
impl RunId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "RunId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `RunInterruptParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInterruptParams {
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `reason`.
    pub reason: Option<String>,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `RunInterruptResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInterruptResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `run`.
    pub run: RunSummary,
}

/// Discriminator `run`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunResourceScopeKind {
    #[serde(rename = "run")]
    Value,
}

/// Generated closed object `RunResourceScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunResourceScope {
    /// Wire field `kind`.
    pub kind: RunResourceScopeKind,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `RunResumeParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunResumeParams {
    /// Wire field `continuationMode`.
    pub continuation_mode: ContinuationMode,
    /// Wire field `environmentAttachments`.
    pub environment_attachments: Vec<AttachmentId>,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `profile`.
    pub profile: Option<String>,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `RunResumeResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunResumeResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `run`.
    pub run: RunSummary,
    /// Wire field `sourceRunId`.
    pub source_run_id: RunId,
}

/// Generated closed object `RunStartParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunStartParams {
    /// Wire field `continuationMode`.
    pub continuation_mode: ContinuationMode,
    /// Wire field `environmentAttachments`.
    pub environment_attachments: Vec<AttachmentId>,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `input`.
    pub input: Vec<InputPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `profile`.
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `restoreFromRunId`.
    pub restore_from_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `sessionId`.
    pub session_id: Option<SessionId>,
}

/// Generated closed object `RunStartResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunStartResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `run`.
    pub run: RunSummary,
}

/// Generated string enum `RunStatus`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunStatus {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

/// Generated closed object `RunStatusParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunStatusParams {
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `RunStatusResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunStatusResult {
    /// Wire field `run`.
    pub run: RunSummary,
}

/// Generated closed object `RunSteerParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSteerParams {
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    /// Wire field `text`.
    pub text: String,
}

/// Generated closed object `RunSteerResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSteerResult {
    /// Wire field `accepted`.
    pub accepted: bool,
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
}

/// Generated closed object `RunSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSummary {
    /// Wire field `createdAt`.
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `outputPreview`.
    pub output_preview: Option<String>,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `runId`.
    pub run_id: RunId,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    /// Wire field `status`.
    pub status: RunStatus,
    /// Wire field `updatedAt`.
    pub updated_at: Timestamp,
}

/// Generated closed object `RuntimeBuildIdentity`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBuildIdentity {
    /// Wire field `buildRevision`.
    pub build_revision: String,
    /// Wire field `target`.
    pub target: String,
    /// Wire field `version`.
    pub version: String,
}

/// Validated generated string `SchemaDigest`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SchemaDigest(String);
impl SchemaDigest {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 71, 71, "SchemaDigest")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for SchemaDigest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for SchemaDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `ServerInfo`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerInfo {
    /// Wire field `name`.
    pub name: String,
    /// Wire field `version`.
    pub version: String,
}

/// Discriminator `session`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionAttachmentScopeKind {
    #[serde(rename = "session")]
    Value,
}

/// Generated closed object `SessionAttachmentScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAttachmentScope {
    /// Wire field `kind`.
    pub kind: SessionAttachmentScopeKind,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Discriminator `session_changed`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionChangedEventKind {
    #[serde(rename = "session_changed")]
    Value,
}

/// Generated closed object `SessionChangedEvent`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionChangedEvent {
    /// Wire field `kind`.
    pub kind: SessionChangedEventKind,
    /// Wire field `session`.
    pub session: SessionSummary,
}

/// Generated closed object `SessionCreateParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCreateParams {
    /// Wire field `deferredTools`.
    pub deferred_tools: Vec<DeferredToolDefinition>,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `profile`.
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `title`.
    pub title: Option<String>,
}

/// Generated closed object `SessionCreateResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCreateResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `session`.
    pub session: SessionSummary,
}

/// Generated closed object `SessionDeleteParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDeleteParams {
    /// Wire field `expectedRevision`.
    pub expected_revision: DecimalU64,
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `SessionDeleteResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDeleteResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `session`.
    pub session: SessionSummary,
}

/// Generated closed object `SessionForkParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionForkParams {
    /// Wire field `idempotencyKey`.
    pub idempotency_key: IdempotencyKey,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `title`.
    pub title: Option<String>,
}

/// Generated closed object `SessionForkResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionForkResult {
    /// Wire field `receipt`.
    pub receipt: MutationReceipt,
    /// Wire field `session`.
    pub session: SessionSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `sourceRunId`.
    pub source_run_id: Option<RunId>,
    /// Wire field `sourceSessionId`.
    pub source_session_id: SessionId,
}

/// Generated closed object `SessionGetParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGetParams {
    /// Wire field `runLimit`.
    pub run_limit: u32,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `SessionGetResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGetResult {
    /// Wire field `runs`.
    pub runs: Vec<RunSummary>,
    /// Wire field `session`.
    pub session: SessionSummary,
}

/// Validated generated string `SessionId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);
impl SessionId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "SessionId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generated closed object `SessionListParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `cursor`.
    pub cursor: Option<String>,
    /// Wire field `limit`.
    pub limit: u32,
}

/// Generated closed object `SessionListResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionListResult {
    /// Wire field `page`.
    pub page: PageInfo,
    /// Wire field `sessions`.
    pub sessions: Vec<SessionSummary>,
}

/// Discriminator `session`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionResourceScopeKind {
    #[serde(rename = "session")]
    Value,
}

/// Generated closed object `SessionResourceScope`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionResourceScope {
    /// Wire field `kind`.
    pub kind: SessionResourceScopeKind,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
}

/// Generated closed object `SessionSearchHit`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchHit {
    /// Wire field `highlights`.
    pub highlights: Vec<String>,
    /// Wire field `scoreBasisPoints`.
    pub score_basis_points: u32,
    /// Wire field `session`.
    pub session: SessionSummary,
}

/// Generated string enum `SessionSearchMode`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionSearchMode {
    #[serde(rename = "literal")]
    Literal,
    #[serde(rename = "hybrid")]
    Hybrid,
}

/// Generated closed object `SessionSearchParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `cursor`.
    pub cursor: Option<String>,
    /// Wire field `limit`.
    pub limit: u32,
    /// Wire field `mode`.
    pub mode: SessionSearchMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `profile`.
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `query`.
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `status`.
    pub status: Option<SessionStatus>,
}

/// Generated closed object `SessionSearchResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchResult {
    /// Wire field `hits`.
    pub hits: Vec<SessionSearchHit>,
    /// Wire field `page`.
    pub page: PageInfo,
}

/// Discriminator `session_search_unavailable`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionSearchUnavailableDataKind {
    #[serde(rename = "session_search_unavailable")]
    Value,
}

/// Generated closed object `SessionSearchUnavailableData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchUnavailableData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: SessionSearchUnavailableDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated string enum `SessionStatus`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

/// Generated closed object `SessionSummary`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSummary {
    /// Wire field `createdAt`.
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `profile`.
    pub profile: Option<String>,
    /// Wire field `revision`.
    pub revision: DecimalU64,
    /// Wire field `sessionId`.
    pub session_id: SessionId,
    /// Wire field `status`.
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `title`.
    pub title: Option<String>,
    /// Wire field `updatedAt`.
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `workspaceLabel`.
    pub workspace_label: Option<String>,
}

/// Generated closed object `ShutdownParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShutdownParams {
    /// Wire field `deadlineMs`.
    pub deadline_ms: u32,
}

/// Generated closed object `ShutdownResult`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShutdownResult {
    /// Wire field `status`.
    pub status: String,
}

/// Discriminator `stale_fence`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StaleFenceDataKind {
    #[serde(rename = "stale_fence")]
    Value,
}

/// Generated closed object `StaleFenceData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaleFenceData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: StaleFenceDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `StartupReconciliation`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartupReconciliation {
    /// Wire field `changedRunState`.
    pub changed_run_state: bool,
    /// Wire field `repairedRuns`.
    pub repaired_runs: DecimalU64,
}

/// Generated closed object `StorageCompatibility`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageCompatibility {
    /// Wire field `currentGeneration`.
    pub current_generation: DecimalU64,
    /// Wire field `maintenanceBarrierGeneration`.
    pub maintenance_barrier_generation: DecimalU64,
    /// Wire field `maximumReadableGeneration`.
    pub maximum_readable_generation: DecimalU64,
    /// Wire field `maximumWritableGeneration`.
    pub maximum_writable_generation: DecimalU64,
    /// Wire field `minimumReadableGeneration`.
    pub minimum_readable_generation: DecimalU64,
    /// Wire field `minimumWritableGeneration`.
    pub minimum_writable_generation: DecimalU64,
}

/// Discriminator `storage_unavailable`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StorageUnavailableDataKind {
    #[serde(rename = "storage_unavailable")]
    Value,
}

/// Generated closed object `StorageUnavailableData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageUnavailableData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: StorageUnavailableDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `SubscriptionClosedNotificationParams`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionClosedNotificationParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `lastFlushedCursor`.
    pub last_flushed_cursor: Option<HostEventCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `lastFlushedDeliverySequence`.
    pub last_flushed_delivery_sequence: Option<DecimalU64>,
    /// Wire field `reason`.
    pub reason: SubscriptionClosedReason,
    /// Wire field `subscriptionId`.
    pub subscription_id: SubscriptionId,
}

/// Generated string enum `SubscriptionClosedReason`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SubscriptionClosedReason {
    #[serde(rename = "terminal")]
    Terminal,
    #[serde(rename = "unsubscribed")]
    Unsubscribed,
    #[serde(rename = "overflow")]
    Overflow,
    #[serde(rename = "authorization_changed")]
    AuthorizationChanged,
    #[serde(rename = "sequence_exhausted")]
    SequenceExhausted,
}

/// Validated generated string `SubscriptionId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SubscriptionId(String);
impl SubscriptionId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 128, "SubscriptionId")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for SubscriptionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Discriminator `text`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TextInputPartKind {
    #[serde(rename = "text")]
    Value,
}

/// Generated closed object `TextInputPart`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextInputPart {
    /// Wire field `kind`.
    pub kind: TextInputPartKind,
    /// Wire field `text`.
    pub text: String,
}

/// Validated generated string `Timestamp`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Timestamp(String);
impl Timestamp {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_string(&value, 1, 40, "Timestamp")?;
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}
impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Discriminator `unsupported_feature`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum UnsupportedFeatureDataKind {
    #[serde(rename = "unsupported_feature")]
    Value,
}

/// Generated closed object `UnsupportedFeatureData`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnsupportedFeatureData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `diagnosticRef`.
    pub diagnostic_ref: Option<String>,
    /// Wire field `kind`.
    pub kind: UnsupportedFeatureDataKind,
    /// Wire field `reconciliationRequired`.
    pub reconciliation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `resourceKind`.
    pub resource_kind: Option<String>,
    /// Wire field `retryable`.
    pub retryable: bool,
}

/// Generated closed object `WorkspaceCompatibility`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceCompatibility {
    /// Wire field `executionDomainId`.
    pub execution_domain_id: String,
    /// Wire field `workspaceIdentity`.
    pub workspace_identity: String,
}
