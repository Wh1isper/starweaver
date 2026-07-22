use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Reserved agent-facing mount id for the host's configured local environment.
pub const LOCAL_ENVIRONMENT_ATTACHMENT_ID: &str = "local";

/// Attachment kind for the host's configured local environment.
pub const LOCAL_ENVIRONMENT_ATTACHMENT_KIND: &str = "local";

/// Provider-private access mode used while materializing a durable host attachment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAttachmentAccessMode {
    /// Read-only access.
    ReadOnly,
    /// Read and write access.
    #[default]
    ReadWrite,
}

/// Provider-private environment reference used by the RPC runtime.
///
/// The generated `starweaver.host` types are the only wire contract. This type may contain
/// credentials while a configured durable attachment is being materialized and must only be
/// persisted or returned through an explicit safe projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttachmentRef {
    /// Stable mount id within the run.
    pub id: String,
    /// Provider kind, currently `local` or `envd`.
    #[serde(default = "default_environment_attachment_kind")]
    pub kind: String,
    /// Requested access mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<EnvironmentAttachmentAccessMode>,
    /// Whether this is the default SDK mount.
    #[serde(default, rename = "default")]
    pub is_default: bool,
    /// Whether this is the default shell/process mount.
    #[serde(default, rename = "defaultForShell")]
    pub is_default_for_shell: bool,
    /// Provider endpoint reference. This value can contain process arguments and is private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    /// Concrete environment id within the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Provider bearer token. This value is request-only and is never serialized.
    #[serde(default, skip_serializing)]
    pub auth_token: Option<String>,
    /// Provider-private metadata. No keys are safe for host projection by default.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

impl EnvironmentAttachmentRef {
    /// Return the effective access mode, defaulting omitted refs to read-write.
    #[must_use]
    pub fn resolved_mode(&self) -> EnvironmentAttachmentAccessMode {
        self.mode.unwrap_or_default()
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
}

/// Canonicalize validated provider-private refs for semantic comparison and execution.
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

/// Return whether an internal environment mount id is a valid ASCII slug.
#[must_use]
pub fn is_valid_environment_attachment_id(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "." | ".." | "environment")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn default_environment_attachment_kind() -> String {
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string()
}
