use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{RpcError, INVALID_PARAMS};

/// Environment access mode requested by a host RPC environment attachment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAttachmentAccessMode {
    /// Read-only access.
    ReadOnly,
    /// Read and write access.
    #[default]
    ReadWrite,
}

/// Scope for a host-control environment attachment lease.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAttachmentScopeKind {
    /// Lease is scoped to the current host connection.
    #[default]
    Connection,
    /// Lease can be reused by runs for one session.
    Session,
}

/// Host-control scope for an environment attachment lease.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAttachmentScope {
    /// Scope kind.
    #[serde(default)]
    pub kind: EnvironmentAttachmentScopeKind,
    /// Session id for session-scoped leases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Readiness policy requested by an attachment operation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentReadinessPolicy {
    /// Operation fails when readiness cannot be proven.
    #[default]
    Required,
    /// Operation succeeds with degraded status when readiness fails.
    BestEffort,
    /// Operation skips probing.
    Skip,
}

/// Readiness request for attachment operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReadinessRequest {
    /// Probe policy.
    #[serde(default)]
    pub policy: EnvironmentReadinessPolicy,
    /// Optional readiness timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Attachment readiness phase.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentReadinessPhase {
    /// Readiness was proven.
    #[default]
    Ready,
    /// Readiness failed but the operation continued under best-effort policy.
    Degraded,
    /// Attachment is unreachable or not usable.
    Unavailable,
    /// Readiness was not probed.
    Skipped,
}

/// Tool capability summary for an environment attachment.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReadinessCapabilities {
    /// File capabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// One-shot command capabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Background process capabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub process: Vec<String>,
}

/// Readiness summary for an environment attachment.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReadiness {
    /// Transport readiness.
    #[serde(default)]
    pub transport: EnvironmentReadinessPhase,
    /// Concrete environment readiness.
    #[serde(default)]
    pub environment: EnvironmentReadinessPhase,
    /// Capability summary.
    #[serde(default)]
    pub capabilities: EnvironmentReadinessCapabilities,
    /// Optional safe diagnostic message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Host-control attachment lease status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAttachmentStatus {
    /// Probe succeeded and the provider can be used.
    #[default]
    Ready,
    /// Lease exists but readiness is only best-effort.
    Degraded,
    /// Probe failed or transport is unreachable.
    Unavailable,
    /// Lease was released.
    Detached,
}

/// Host-control reference to an environment attached to a run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAttachmentRef {
    /// Stable attachment id within the run.
    pub id: String,
    /// Attachment implementation kind, such as `local` or `envd`.
    #[serde(default = "default_environment_attachment_kind")]
    pub kind: String,
    /// Requested access mode.
    #[serde(default)]
    pub mode: EnvironmentAttachmentAccessMode,
    /// Whether this attachment is the default SDK environment mount.
    #[serde(default, rename = "default")]
    pub is_default: bool,
    /// Existing host-control lease id, when this ref points at a pre-attached environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_lease_id: Option<String>,
    /// Endpoint reference or URL for remote implementations.
    #[serde(default, alias = "endpoint", skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    /// Concrete environment id within the implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Host metadata for the attachment.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

impl EnvironmentAttachmentRef {
    /// Return the environment id requested by this ref, if any.
    #[must_use]
    pub fn requested_environment_id(&self) -> Option<&str> {
        self.environment_id.as_deref()
    }

    /// Return the endpoint ref requested by this ref, if any.
    #[must_use]
    pub fn requested_endpoint_ref(&self) -> Option<&str> {
        self.endpoint_ref.as_deref()
    }

    /// Return the existing host-control lease id, if any.
    #[must_use]
    pub fn requested_attachment_lease_id(&self) -> Option<&str> {
        self.attachment_lease_id.as_deref()
    }
}

/// Host-control attachment lease returned by `environment.attach`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAttachmentLease {
    /// Host-control lease id.
    pub attachment_lease_id: String,
    /// Lease scope.
    pub scope: EnvironmentAttachmentScope,
    /// Agent-facing mount id.
    pub id: String,
    /// Attachment implementation kind.
    pub kind: String,
    /// Requested access mode.
    pub mode: EnvironmentAttachmentAccessMode,
    /// Whether this attachment prefers to be the default mount.
    #[serde(default, rename = "default")]
    pub is_default: bool,
    /// Agent-facing mount root.
    pub mount_root: String,
    /// Attachment status.
    pub status: EnvironmentAttachmentStatus,
    /// Readiness summary.
    pub readiness: EnvironmentReadiness,
    /// Endpoint reference, redacted when necessary by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    /// Concrete environment id within the implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Safe host metadata for the attachment.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Params for `environment.attach`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAttachParams {
    /// Lease scope.
    #[serde(default)]
    pub scope: EnvironmentAttachmentScope,
    /// Attachment source.
    pub attachment: EnvironmentAttachmentRef,
    /// Readiness request.
    #[serde(default)]
    pub readiness: EnvironmentReadinessRequest,
    /// Idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Params for `environment.detach`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDetachParams {
    /// Lease id to detach.
    pub attachment_lease_id: String,
    /// Reserved force flag.
    #[serde(default)]
    pub force: bool,
    /// Idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Params for `environment.list`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentListParams {
    /// Scope filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<EnvironmentAttachmentScope>,
    /// Status filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<EnvironmentAttachmentStatus>,
    /// Result limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Reserved pagination token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

/// Params for `environment.health`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHealthParams {
    /// Existing lease id to probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_lease_id: Option<String>,
    /// Inline attachment source to probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<EnvironmentAttachmentRef>,
    /// Optional readiness timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Parse environment attachment refs from host RPC params.
///
/// # Errors
///
/// Returns an RPC invalid-params error when the shape is invalid or ids are duplicated.
pub fn environment_attachment_refs(
    params: &Value,
) -> Result<Vec<EnvironmentAttachmentRef>, RpcError> {
    let Some(value) = params
        .get("environmentAttachments")
        .or_else(|| params.get("environments"))
        .or_else(|| params.get("environment"))
    else {
        return Ok(Vec::new());
    };
    let mut refs = if value.is_array() {
        serde_json::from_value::<Vec<EnvironmentAttachmentRef>>(value.clone())
    } else {
        serde_json::from_value::<EnvironmentAttachmentRef>(value.clone()).map(|value| vec![value])
    }
    .map_err(|error| RpcError::new(INVALID_PARAMS, format!("invalid environment refs: {error}")))?;
    if refs.len() == 1 {
        refs[0].is_default = true;
    }
    validate_environment_attachment_refs(&refs)?;
    Ok(refs)
}

/// Build a serializable environment attachment result.
#[must_use]
pub fn environment_attachment_result(refs: &[EnvironmentAttachmentRef]) -> Value {
    json!({
        "environmentAttachments": refs,
    })
}

/// Build a serializable environment attachment lease result.
#[must_use]
pub fn environment_attachment_lease_result(lease: &EnvironmentAttachmentLease) -> Value {
    json!({
        "attachment": lease,
    })
}

/// Build a serializable environment attachment list result.
#[must_use]
pub fn environment_attachment_list_result(
    leases: &[EnvironmentAttachmentLease],
    next_page_token: Option<&str>,
) -> Value {
    json!({
        "attachments": leases,
        "nextPageToken": next_page_token,
    })
}

/// Build a serializable environment health result.
#[must_use]
pub fn environment_health_result(
    status: EnvironmentAttachmentStatus,
    readiness: &EnvironmentReadiness,
) -> Value {
    json!({
        "status": status,
        "readiness": readiness,
    })
}

/// Return whether an environment attachment id is a valid agent-facing mount slug.
#[must_use]
pub fn is_valid_environment_attachment_id(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "." | ".." | "environment")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_environment_attachment_refs(refs: &[EnvironmentAttachmentRef]) -> Result<(), RpcError> {
    let mut ids = BTreeSet::new();
    let mut default_count = 0_usize;
    for attachment in refs {
        if !is_valid_environment_attachment_id(&attachment.id) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!(
                    "invalid environment attachment id: {}; expected an ASCII slug",
                    attachment.id
                ),
            ));
        }
        if !ids.insert(attachment.id.clone()) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("duplicate environment attachment id: {}", attachment.id),
            ));
        }
        if attachment.is_default {
            default_count += 1;
        }
    }
    if refs.len() > 1 && default_count != 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "multiple environment attachments require exactly one default attachment",
        ));
    }
    Ok(())
}

fn default_environment_attachment_kind() -> String {
    "local".to_string()
}
