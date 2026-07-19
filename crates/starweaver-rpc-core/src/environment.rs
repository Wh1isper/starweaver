use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{INVALID_PARAMS, RpcError};

/// Reserved agent-facing mount id for the host's configured local environment.
pub const LOCAL_ENVIRONMENT_ATTACHMENT_ID: &str = "local";

/// Attachment kind for the host's configured local environment.
pub const LOCAL_ENVIRONMENT_ATTACHMENT_KIND: &str = "local";

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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttachmentRef {
    /// Stable attachment id within the run.
    pub id: String,
    /// Attachment implementation kind, such as `local` or `envd`.
    #[serde(default = "default_environment_attachment_kind")]
    pub kind: String,
    /// Requested access mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<EnvironmentAttachmentAccessMode>,
    /// Whether this attachment is the default SDK environment mount.
    #[serde(default, rename = "default")]
    pub is_default: bool,
    /// Whether this attachment is the default shell/process mount.
    #[serde(default, rename = "defaultForShell")]
    pub is_default_for_shell: bool,
    /// Existing host-control lease id, when this ref points at a pre-attached environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_lease_id: Option<String>,
    /// Endpoint reference or URL for remote implementations.
    #[serde(default, alias = "endpoint", skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    /// Concrete environment id within the implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Bearer token for authenticated transports. This is request-only and must
    /// not be echoed in host results or persisted run metadata.
    #[serde(default, alias = "token", skip_serializing)]
    pub auth_token: Option<String>,
    /// Host metadata for the attachment.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

impl EnvironmentAttachmentRef {
    /// Return the effective access mode, defaulting omitted refs to read-write.
    #[must_use]
    pub fn resolved_mode(&self) -> EnvironmentAttachmentAccessMode {
        self.mode.unwrap_or_default()
    }

    /// Return the access mode explicitly requested by this ref, if any.
    #[must_use]
    pub const fn requested_mode(&self) -> Option<EnvironmentAttachmentAccessMode> {
        self.mode
    }

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

    /// Return the bearer token requested by this ref, if any.
    #[must_use]
    pub fn requested_auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    /// Return the existing host-control lease id, if any.
    #[must_use]
    pub fn requested_attachment_lease_id(&self) -> Option<&str> {
        self.attachment_lease_id.as_deref()
    }
}

/// Host-control attachment lease returned by `environment.attach`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Whether this attachment prefers to be the default shell/process mount.
    #[serde(default, rename = "defaultForShell")]
    pub is_default_for_shell: bool,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// Params for `environment.active_mount`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentActiveMountParams {
    /// Active run id.
    pub run_id: String,
    /// Attachment source or lease ref to mount.
    pub attachment: EnvironmentAttachmentRef,
    /// Replace an existing mount with the same id.
    #[serde(default)]
    pub replace: bool,
    /// Whether to inject model-visible context after the lifecycle event.
    #[serde(default = "default_true")]
    pub inject_context: bool,
    /// Optional optimistic binding version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_binding_version: Option<u64>,
    /// Idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Params for `environment.active_unmount`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentActiveUnmountParams {
    /// Active run id.
    pub run_id: String,
    /// Mount id to remove.
    pub mount_id: String,
    /// Required when removing the current default mount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_default_mount_id: Option<String>,
    /// Required when removing the current default shell mount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_default_shell_mount_id: Option<String>,
    /// Whether to inject model-visible context after the lifecycle event.
    #[serde(default = "default_true")]
    pub inject_context: bool,
    /// Optional optimistic binding version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_binding_version: Option<u64>,
    /// Idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Params for `environment.active_list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentActiveListParams {
    /// Active run id.
    pub run_id: String,
}

/// Canonicalize validated attachment refs for semantic comparison and execution.
#[must_use]
pub fn normalize_environment_attachment_refs(
    refs: &[EnvironmentAttachmentRef],
) -> Vec<EnvironmentAttachmentRef> {
    let mut normalized = refs.to_vec();
    let default_id = if normalized.len() == 1 {
        normalized.first().map(|attachment| attachment.id.clone())
    } else {
        normalized
            .iter()
            .find(|attachment| attachment.is_default)
            .map(|attachment| attachment.id.clone())
    };
    let default_shell_id = normalized
        .iter()
        .find(|attachment| attachment.is_default_for_shell)
        .map(|attachment| attachment.id.clone())
        .or_else(|| {
            let default_id = default_id.as_deref()?;
            normalized
                .iter()
                .find(|attachment| {
                    attachment.id == default_id
                        && attachment.resolved_mode() == EnvironmentAttachmentAccessMode::ReadWrite
                })
                .map(|attachment| attachment.id.clone())
        });
    for attachment in &mut normalized {
        attachment.mode = Some(attachment.resolved_mode());
        attachment.is_default = default_id.as_deref() == Some(attachment.id.as_str());
        attachment.is_default_for_shell =
            default_shell_id.as_deref() == Some(attachment.id.as_str());
    }
    normalized
}

/// Parse environment attachment refs from host RPC params.
///
/// # Errors
///
/// Returns an RPC invalid-params error when the shape is invalid, aliases conflict, or ids are
/// duplicated.
pub fn environment_attachment_refs(
    params: &Value,
) -> Result<Vec<EnvironmentAttachmentRef>, RpcError> {
    let mut parsed = Vec::new();
    for key in ["environmentAttachments", "environments", "environment"] {
        let Some(value) = params.get(key) else {
            continue;
        };
        let mut refs = if value.is_array() {
            serde_json::from_value::<Vec<EnvironmentAttachmentRef>>(value.clone())
        } else {
            serde_json::from_value::<EnvironmentAttachmentRef>(value.clone())
                .map(|value| vec![value])
        }
        .map_err(|error| {
            RpcError::new(
                INVALID_PARAMS,
                format!("invalid environment refs in {key}: {error}"),
            )
        })?;
        if refs.len() == 1 {
            refs[0].is_default = true;
        }
        validate_environment_attachment_refs(&refs)?;
        let mut normalized = normalize_environment_attachment_refs(&refs);
        normalized.sort_by(|left, right| left.id.cmp(&right.id));
        parsed.push((key, refs, normalized));
    }
    let Some((first_key, first, first_normalized)) = parsed.first() else {
        return Ok(Vec::new());
    };
    if let Some((conflicting_key, _, _)) = parsed
        .iter()
        .skip(1)
        .find(|(_, _, normalized)| normalized != first_normalized)
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "environment attachment aliases {first_key} and {conflicting_key} must match when both are supplied"
            ),
        ));
    }
    Ok(first.clone())
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
    json!(EnvironmentAttachResult {
        attachment: lease.clone(),
    })
}

/// Build a serializable environment attachment list result.
#[must_use]
pub fn environment_attachment_list_result(
    leases: &[EnvironmentAttachmentLease],
    next_page_token: Option<&str>,
) -> Value {
    json!(EnvironmentListResult {
        attachments: leases.to_vec(),
        next_page_token: next_page_token.map(ToString::to_string),
    })
}

/// Build a serializable environment health result.
#[must_use]
pub fn environment_health_result(
    status: EnvironmentAttachmentStatus,
    readiness: &EnvironmentReadiness,
) -> Value {
    json!(EnvironmentHealthResult {
        status,
        readiness: readiness.clone(),
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
    let mut default_for_shell_count = 0_usize;
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
        if attachment.id == LOCAL_ENVIRONMENT_ATTACHMENT_ID
            && attachment.kind != LOCAL_ENVIRONMENT_ATTACHMENT_KIND
        {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "reserved environment attachment id local requires kind local",
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
        if attachment.is_default_for_shell {
            default_for_shell_count += 1;
        }
    }
    if refs.len() > 1 && default_count != 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "multiple environment attachments require exactly one default attachment",
        ));
    }
    if default_for_shell_count > 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "environment attachments allow at most one defaultForShell attachment",
        ));
    }
    Ok(())
}

fn default_environment_attachment_kind() -> String {
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string()
}

const fn default_true() -> bool {
    true
}

/// Typed result for `environment.attach`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentAttachResult {
    /// Created or reused attachment lease.
    pub attachment: EnvironmentAttachmentLease,
}

/// Typed result for `environment.detach`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentDetachResult {
    /// Detached lease id.
    pub attachment_lease_id: String,
    /// Whether this call changed the lease state.
    pub detached: bool,
}

/// Typed result for `environment.list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentListResult {
    /// Matching attachment leases.
    pub attachments: Vec<EnvironmentAttachmentLease>,
    /// Reserved next-page token.
    #[serde(default)]
    pub next_page_token: Option<String>,
}

/// Typed result for `environment.health`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentHealthResult {
    /// Effective attachment status.
    pub status: EnvironmentAttachmentStatus,
    /// Readiness probe details.
    pub readiness: EnvironmentReadiness,
}

/// Stable active-run mount projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMountSummary {
    /// Agent-facing mount id.
    pub id: String,
    /// Provider kind.
    pub kind: String,
    /// Agent-facing root.
    pub root: String,
    /// Effective access mode.
    pub mode: EnvironmentAttachmentAccessMode,
    /// Whether this is the default SDK mount.
    #[serde(rename = "default")]
    pub is_default: bool,
    /// Whether this is the default shell mount.
    #[serde(rename = "defaultForShell")]
    pub is_default_for_shell: bool,
    /// Mount lifecycle status.
    pub status: String,
    /// Concrete provider environment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Safe host metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Stable active-run environment binding projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentBindingSummary {
    /// Monotonic binding version.
    pub binding_version: u64,
    /// Default SDK mount id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mount_id: Option<String>,
    /// Default shell mount id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_shell_mount_id: Option<String>,
    /// Effective mounts.
    pub mounts: Vec<EnvironmentMountSummary>,
}

/// Typed result for `environment.active_mount`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentActiveMountResult {
    /// Active run id.
    pub run_id: String,
    /// Durable environment operation id.
    pub operation_id: String,
    /// Mounted id.
    pub mount_id: String,
    /// Whether an existing mount was replaced.
    pub replace: bool,
    /// Mounted projection.
    pub mount: EnvironmentMountSummary,
    /// Binding version before the mutation.
    pub previous_binding_version: u64,
    /// Binding version after the mutation.
    pub binding_version: u64,
    /// Effective binding.
    pub environment: EnvironmentBindingSummary,
    /// Durable lifecycle cursor.
    pub event_cursor: starweaver_stream::ReplayCursor,
    /// Whether model-visible context was requested.
    pub context_injection_requested: bool,
    /// Whether context steering was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_injected: Option<bool>,
}

/// Typed result for `environment.active_unmount`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentActiveUnmountResult {
    /// Active run id.
    pub run_id: String,
    /// Durable environment operation id.
    pub operation_id: String,
    /// Removed mount id.
    pub mount_id: String,
    /// Removed projection.
    pub removed_mount: EnvironmentMountSummary,
    /// Binding version before the mutation.
    pub previous_binding_version: u64,
    /// Binding version after the mutation.
    pub binding_version: u64,
    /// Effective binding.
    pub environment: EnvironmentBindingSummary,
    /// Durable lifecycle cursor.
    pub event_cursor: starweaver_stream::ReplayCursor,
    /// Whether model-visible context was requested.
    pub context_injection_requested: bool,
    /// Whether context steering was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_injected: Option<bool>,
}

/// Typed result for `environment.active_list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentActiveListResult {
    /// Active run id.
    pub run_id: String,
    /// Effective binding.
    pub environment: EnvironmentBindingSummary,
}
