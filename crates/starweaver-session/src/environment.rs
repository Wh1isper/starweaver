//! Product-neutral durable environment attachment and mount contracts.
//!
//! These records deliberately contain only public environment identities and reviewed display
//! projections. Provider endpoints, credentials, and private resource source references belong to
//! the product-side resolver and must never be placed in these values.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    DurableHostEventClass, DurableHostEventScope, MutationReceipt, PendingHostEventPublication,
    SessionStoreError, SessionStoreResult,
};

/// Maximum page size accepted by durable attachment queries, matching the host schema.
pub const MAX_ENVIRONMENT_PAGE_SIZE: u32 = 200;
/// Maximum active mounts retained for one run, matching the host result bound.
pub const MAX_ENVIRONMENT_MOUNTS_PER_RUN: u32 = 128;
/// Stable operation identifier for attachment creation.
pub const ENVIRONMENT_ATTACH_OPERATION: &str = "environment.attach";
/// Stable operation identifier for attachment detachment.
pub const ENVIRONMENT_DETACH_OPERATION: &str = "environment.detach";
/// Stable operation identifier for mounting an allowlisted resource.
pub const ENVIRONMENT_MOUNT_OPERATION: &str = "environment.mount";
/// Stable operation identifier for unmounting a resource.
pub const ENVIRONMENT_UNMOUNT_OPERATION: &str = "environment.unmount";

/// Product-neutral attachment scope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurableEnvironmentScope {
    /// Scope follows one transport-owned connection and is never transferable.
    Connection {
        /// Opaque host-generated connection identity.
        connection_id: String,
    },
    /// Scope follows one durable session.
    Session {
        /// Session identity.
        session_id: String,
    },
    /// Scope follows one durable run.
    Run {
        /// Session identity.
        session_id: String,
        /// Run identity.
        run_id: String,
    },
}

impl DurableEnvironmentScope {
    /// Validate scope identities.
    ///
    /// # Errors
    ///
    /// Returns an error when a scoped session or run identity is empty.
    pub fn validate(&self) -> SessionStoreResult<()> {
        match self {
            Self::Connection { connection_id } => {
                require_non_empty("environment scope connection", connection_id)
            }
            Self::Session { session_id } => {
                require_non_empty("environment scope session", session_id)
            }
            Self::Run { session_id, run_id } => {
                require_non_empty("environment scope session", session_id)?;
                require_non_empty("environment scope run", run_id)
            }
        }
    }

    /// Return whether this scope permits use by the specified run.
    #[must_use]
    pub fn permits_run(&self, connection_id: Option<&str>, session_id: &str, run_id: &str) -> bool {
        match self {
            Self::Connection {
                connection_id: owner,
            } => connection_id == Some(owner.as_str()),
            Self::Session { session_id: owner } => owner == session_id,
            Self::Run {
                session_id: owner_session,
                run_id: owner_run,
            } => owner_session == session_id && owner_run == run_id,
        }
    }
}

/// Durable attachment lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableEnvironmentStatus {
    /// Provider is being prepared.
    Attaching,
    /// Provider is ready for use.
    Ready,
    /// Provider is available with reduced capability.
    Degraded,
    /// Attachment is terminally detached.
    Detached,
}

/// Safe durable projection of an environment attachment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableEnvironmentAttachment {
    /// Trusted authority binding owning the record and its idempotency namespace.
    pub authority_binding: String,
    /// Stable attachment identity.
    pub attachment_id: String,
    /// Public, configured environment catalog identity.
    pub environment_id: String,
    /// Optional reviewed display label; never a provider endpoint or source reference.
    pub display_name: Option<String>,
    /// Attachment scope.
    pub scope: DurableEnvironmentScope,
    /// Lifecycle state.
    pub status: DurableEnvironmentStatus,
    /// Monotonic record revision, beginning at one.
    pub revision: u64,
    /// Last committed mutation time.
    pub updated_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for DurableEnvironmentAttachment {
    const SCHEMA: &'static str = "starweaver.session.environment_attachment";
}

impl DurableEnvironmentAttachment {
    /// Validate persisted attachment invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when an identity, display label, scope, or revision is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("environment authority binding", &self.authority_binding)?;
        require_non_empty("environment attachment id", &self.attachment_id)?;
        require_non_empty("public environment id", &self.environment_id)?;
        if self.display_name.as_ref().is_some_and(String::is_empty) {
            return Err(SessionStoreError::Failed(
                "environment display name cannot be empty".to_string(),
            ));
        }
        self.scope.validate()?;
        require_revision(self.revision, "environment attachment")
    }
}

/// Durable mount lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableEnvironmentMountStatus {
    /// Resource is actively mounted.
    Mounted,
    /// Resource is terminally unmounted.
    Unmounted,
}

/// Safe durable projection of one mounted resource.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableEnvironmentMount {
    /// Trusted authority binding owning the mount.
    pub authority_binding: String,
    /// Stable mount identity.
    pub mount_id: String,
    /// Owning attachment identity.
    pub attachment_id: String,
    /// Owning session identity.
    pub session_id: String,
    /// Owning run identity.
    pub run_id: String,
    /// Reviewed label resolved from a product-private resource allowlist.
    pub resource_label: String,
    /// Mount lifecycle state.
    pub status: DurableEnvironmentMountStatus,
    /// Monotonic record revision, beginning at one.
    pub revision: u64,
    /// Last committed mutation time.
    pub updated_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for DurableEnvironmentMount {
    const SCHEMA: &'static str = "starweaver.session.environment_mount";
}

impl DurableEnvironmentMount {
    /// Validate persisted mount invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when an identity, safe resource label, or revision is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("environment authority binding", &self.authority_binding)?;
        require_non_empty("environment mount id", &self.mount_id)?;
        require_non_empty("environment attachment id", &self.attachment_id)?;
        require_non_empty("environment mount session", &self.session_id)?;
        require_non_empty("environment mount run", &self.run_id)?;
        require_non_empty("environment resource label", &self.resource_label)?;
        require_revision(self.revision, "environment mount")
    }
}

/// Stable identity and visibility scope for an environment event projected at commit time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentHostEventContext {
    /// Stable identity of the logical environment transition across retries.
    pub transition_identity: String,
    /// Durable visibility scope for the resulting event.
    pub scope: DurableHostEventScope,
}

impl EnvironmentHostEventContext {
    /// Validate stable event identity and scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the transition or a scoped durable identity is empty.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty(
            "environment host event transition identity",
            &self.transition_identity,
        )?;
        match &self.scope {
            DurableHostEventScope::Global => Ok(()),
            DurableHostEventScope::Session { session_id } => {
                require_non_empty("environment host event session", session_id.as_str())
            }
            DurableHostEventScope::Run { session_id, run_id } => {
                require_non_empty("environment host event session", session_id.as_str())?;
                require_non_empty("environment host event run", run_id.as_str())
            }
        }
    }

    /// Project one transaction-final attachment into its exact durable host event.
    ///
    /// # Errors
    ///
    /// Returns an error when the context, attachment, or projected publication is invalid.
    pub fn publication(
        &self,
        attachment: &DurableEnvironmentAttachment,
    ) -> SessionStoreResult<PendingHostEventPublication> {
        self.validate()?;
        attachment.validate()?;
        let attachment_scope = match &attachment.scope {
            DurableEnvironmentScope::Connection { .. } => {
                serde_json::json!({"kind": "connection"})
            }
            DurableEnvironmentScope::Session { session_id } => {
                serde_json::json!({"kind": "session", "sessionId": session_id})
            }
            DurableEnvironmentScope::Run { session_id, run_id } => serde_json::json!({
                "kind": "run",
                "sessionId": session_id,
                "runId": run_id,
            }),
        };
        let status = match attachment.status {
            DurableEnvironmentStatus::Attaching => "attaching",
            DurableEnvironmentStatus::Ready => "ready",
            DurableEnvironmentStatus::Degraded => "degraded",
            DurableEnvironmentStatus::Detached => "detached",
        };
        let mut projected_attachment = serde_json::Map::from_iter([
            (
                "attachmentId".to_string(),
                serde_json::Value::String(attachment.attachment_id.clone()),
            ),
            (
                "environmentId".to_string(),
                serde_json::Value::String(attachment.environment_id.clone()),
            ),
            (
                "revision".to_string(),
                serde_json::Value::String(attachment.revision.to_string()),
            ),
            ("scope".to_string(), attachment_scope),
            (
                "status".to_string(),
                serde_json::Value::String(status.to_string()),
            ),
        ]);
        if let Some(display_name) = &attachment.display_name {
            projected_attachment.insert(
                "displayName".to_string(),
                serde_json::Value::String(display_name.clone()),
            );
        }
        PendingHostEventPublication::new(
            &self.transition_identity,
            0,
            self.scope.clone(),
            DurableHostEventClass::EnvironmentChanged,
            serde_json::json!({
                "kind": "environment_changed",
                "attachment": serde_json::Value::Object(projected_attachment),
            }),
            attachment.updated_at,
        )
    }
}

/// Shared authority, idempotency, timestamp, and event evidence for an environment mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentMutationContext {
    /// Trusted authority binding owning state and the idempotency namespace.
    pub authority_binding: String,
    /// Idempotency key scoped to `authority_binding` across environment operations.
    pub idempotency_key: String,
    /// Canonical fingerprint of the normalized, authorized command.
    pub command_fingerprint: String,
    /// Authoritative commit timestamp.
    pub occurred_at: DateTime<Utc>,
    /// Optional stable event context projected from transaction-final attachment state.
    pub host_event: Option<EnvironmentHostEventContext>,
}

impl EnvironmentMutationContext {
    /// Validate shared mutation evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when authority, idempotency, fingerprint, or event evidence is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("environment authority binding", &self.authority_binding)?;
        require_non_empty("environment idempotency key", &self.idempotency_key)?;
        require_non_empty("environment command fingerprint", &self.command_fingerprint)?;
        if let Some(host_event) = &self.host_event {
            host_event.validate()?;
        }
        Ok(())
    }
}

/// Atomic attachment creation command. All provider-private resolution has already occurred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachEnvironment {
    /// Shared mutation evidence.
    pub context: EnvironmentMutationContext,
    /// Host-generated stable attachment identity.
    pub attachment_id: String,
    /// Public configured environment identity.
    pub environment_id: String,
    /// Reviewed public display label.
    pub display_name: Option<String>,
    /// Authorized attachment scope.
    pub scope: DurableEnvironmentScope,
    /// Initial resolved provider status.
    pub status: DurableEnvironmentStatus,
}

impl AttachEnvironment {
    /// Validate command evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when shared mutation evidence or a command identity is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("environment attachment id", &self.attachment_id)?;
        require_non_empty("public environment id", &self.environment_id)?;
        if self.display_name.as_ref().is_some_and(String::is_empty) {
            return Err(SessionStoreError::Failed(
                "environment display name cannot be empty".to_string(),
            ));
        }
        if self.status == DurableEnvironmentStatus::Detached {
            return Err(SessionStoreError::Failed(
                "new environment attachment cannot be detached".to_string(),
            ));
        }
        self.scope.validate()
    }
}

/// Atomic attachment detachment command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetachEnvironment {
    /// Shared mutation evidence.
    pub context: EnvironmentMutationContext,
    /// Attachment identity.
    pub attachment_id: String,
}

impl DetachEnvironment {
    /// Validate command evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when shared mutation evidence or a command identity is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("environment attachment id", &self.attachment_id)
    }
}

/// Atomic allowlisted-resource mount command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountEnvironmentResource {
    /// Shared mutation evidence.
    pub context: EnvironmentMutationContext,
    /// Host-generated mount identity.
    pub mount_id: String,
    /// Ready attachment identity.
    pub attachment_id: String,
    /// Owning session.
    pub session_id: String,
    /// Owning run.
    pub run_id: String,
    /// Current trusted connection identity when consuming connection-scoped authority.
    pub connection_id: Option<String>,
    /// Reviewed safe label from the product-private resource resolver.
    pub resource_label: String,
}

impl MountEnvironmentResource {
    /// Validate command evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when shared mutation evidence or a command identity is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("environment mount id", &self.mount_id)?;
        require_non_empty("environment attachment id", &self.attachment_id)?;
        require_non_empty("environment mount session", &self.session_id)?;
        require_non_empty("environment mount run", &self.run_id)?;
        if self.connection_id.as_ref().is_some_and(String::is_empty) {
            return Err(SessionStoreError::Failed(
                "environment mount connection identity cannot be empty".to_string(),
            ));
        }
        require_non_empty("environment resource label", &self.resource_label)
    }
}

/// Atomic resource unmount command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnmountEnvironmentResource {
    /// Shared mutation evidence.
    pub context: EnvironmentMutationContext,
    /// Mount identity.
    pub mount_id: String,
}

impl UnmountEnvironmentResource {
    /// Validate command evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when shared mutation evidence or a command identity is invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("environment mount id", &self.mount_id)
    }
}

/// Durable result of an attachment mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentAttachmentMutationResult {
    /// Authoritative post-mutation attachment.
    pub attachment: DurableEnvironmentAttachment,
    /// Durable mutation receipt.
    pub receipt: MutationReceipt,
}

impl starweaver_core::VersionedRecord for EnvironmentAttachmentMutationResult {
    const SCHEMA: &'static str = "starweaver.session.environment_attachment_mutation_result";
}

impl EnvironmentAttachmentMutationResult {
    /// Validate receipt and state linkage.
    ///
    /// # Errors
    ///
    /// Returns an error when state, receipt, operation, or target evidence disagrees.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.attachment.validate()?;
        self.receipt.validate()?;
        if self.receipt.target_ref != self.attachment.attachment_id {
            return Err(SessionStoreError::Conflict(
                "environment receipt target does not match attachment".to_string(),
            ));
        }
        Ok(())
    }

    /// Return an exact-replay projection.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        Self {
            attachment: self.attachment.clone(),
            receipt: self.receipt.replayed_projection(),
        }
    }
}

/// Durable result of a mount mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentMountMutationResult {
    /// Authoritative post-mutation mount.
    pub mount: DurableEnvironmentMount,
    /// Durable mutation receipt.
    pub receipt: MutationReceipt,
}

impl starweaver_core::VersionedRecord for EnvironmentMountMutationResult {
    const SCHEMA: &'static str = "starweaver.session.environment_mount_mutation_result";
}

impl EnvironmentMountMutationResult {
    /// Validate receipt and state linkage.
    ///
    /// # Errors
    ///
    /// Returns an error when state, receipt, operation, or target evidence disagrees.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.mount.validate()?;
        self.receipt.validate()?;
        if self.receipt.target_ref != self.mount.mount_id {
            return Err(SessionStoreError::Conflict(
                "environment receipt target does not match mount".to_string(),
            ));
        }
        Ok(())
    }

    /// Return an exact-replay projection.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        Self {
            mount: self.mount.clone(),
            receipt: self.receipt.replayed_projection(),
        }
    }
}

/// Typed payload stored in the shared environment idempotency namespace.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result_kind", rename_all = "snake_case")]
pub enum EnvironmentMutationResult {
    /// Attachment attach/detach result.
    Attachment(EnvironmentAttachmentMutationResult),
    /// Resource mount/unmount result.
    Mount(EnvironmentMountMutationResult),
}

impl starweaver_core::VersionedRecord for EnvironmentMutationResult {
    const SCHEMA: &'static str = "starweaver.session.environment_mutation_result";
}

/// Stable keyset position for attachment queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentAttachmentPageKey {
    /// Last seen update time.
    pub updated_at: DateTime<Utc>,
    /// Last seen stable identity.
    pub attachment_id: String,
}

/// Bounded authority-scoped attachment query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentAttachmentQuery {
    /// Trusted authority binding.
    pub authority_binding: String,
    /// Optional exact scope filter.
    pub scope: Option<DurableEnvironmentScope>,
    /// Trusted viewer connection identity used to hide foreign connection scopes.
    pub connection_id: Option<String>,
    /// Requested page size, from one through [`MAX_ENVIRONMENT_PAGE_SIZE`].
    pub limit: u32,
    /// Exclusive descending keyset cursor.
    pub after: Option<EnvironmentAttachmentPageKey>,
}

impl EnvironmentAttachmentQuery {
    /// Validate query bounds and identities.
    ///
    /// # Errors
    ///
    /// Returns an error when authority, scope, cursor, target, or page bounds are invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("environment authority binding", &self.authority_binding)?;
        require_page_limit(self.limit)?;
        if let Some(scope) = &self.scope {
            scope.validate()?;
            if let DurableEnvironmentScope::Connection { connection_id } = scope
                && self.connection_id.as_deref() != Some(connection_id)
            {
                return Err(SessionStoreError::NotFound(
                    "environment connection scope".to_string(),
                ));
            }
        }
        if self.connection_id.as_ref().is_some_and(String::is_empty) {
            return Err(SessionStoreError::Failed(
                "environment viewer connection identity cannot be empty".to_string(),
            ));
        }
        if let Some(after) = &self.after {
            require_non_empty("environment attachment page key", &after.attachment_id)?;
        }
        Ok(())
    }
}

/// Stable bounded attachment page.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentAttachmentPage {
    /// Items ordered by `(updated_at DESC, attachment_id DESC)`.
    pub items: Vec<DurableEnvironmentAttachment>,
    /// Exclusive key for the next page.
    pub next: Option<EnvironmentAttachmentPageKeyProjection>,
}

/// Serializable form of an attachment page key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentAttachmentPageKeyProjection {
    /// Last seen update time.
    pub updated_at: DateTime<Utc>,
    /// Last seen stable identity.
    pub attachment_id: String,
}

impl From<EnvironmentAttachmentPageKeyProjection> for EnvironmentAttachmentPageKey {
    fn from(value: EnvironmentAttachmentPageKeyProjection) -> Self {
        Self {
            updated_at: value.updated_at,
            attachment_id: value.attachment_id,
        }
    }
}

/// Bounded stable active-mount query for one run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentMountQuery {
    /// Trusted authority binding.
    pub authority_binding: String,
    /// Owning session.
    pub session_id: String,
    /// Owning run.
    pub run_id: String,
    /// Requested maximum, from one through [`MAX_ENVIRONMENT_PAGE_SIZE`].
    pub limit: u32,
}

impl EnvironmentMountQuery {
    /// Validate query bounds and identities.
    ///
    /// # Errors
    ///
    /// Returns an error when authority, scope, cursor, target, or page bounds are invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("environment authority binding", &self.authority_binding)?;
        require_non_empty("environment mount session", &self.session_id)?;
        require_non_empty("environment mount run", &self.run_id)?;
        require_page_limit(self.limit)
    }
}

fn require_page_limit(limit: u32) -> SessionStoreResult<()> {
    if !(1..=MAX_ENVIRONMENT_PAGE_SIZE).contains(&limit) {
        return Err(SessionStoreError::Failed(format!(
            "environment page limit must be between 1 and {MAX_ENVIRONMENT_PAGE_SIZE}"
        )));
    }
    Ok(())
}

fn require_revision(revision: u64, label: &str) -> SessionStoreResult<()> {
    if revision == 0 {
        return Err(SessionStoreError::Failed(format!(
            "{label} revision must be greater than zero"
        )));
    }
    Ok(())
}

fn require_non_empty(label: &str, value: &str) -> SessionStoreResult<()> {
    if value.is_empty() {
        return Err(SessionStoreError::Failed(format!(
            "{label} cannot be empty"
        )));
    }
    Ok(())
}
