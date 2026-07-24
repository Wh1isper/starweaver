//! Durable session and run records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use starweaver_context::ResumableState;
use starweaver_core::{
    CheckpointId, ConversationId, Metadata, RunId, RunLifecycle, SessionId, TaskId, TraceContext,
    VersionedRecordError,
};
use starweaver_stream::{ReplayCursor, ReplayCursorFamily, ReplayScope};

use crate::{input::InputPart, management::SessionDeletionFence};

/// Current durable session status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session can accept work.
    #[default]
    Active,
    /// Session is archived.
    Archived,
    /// Session reached a failed terminal state.
    Failed,
    /// Session is tombstoned. Retained evidence is not model-visible.
    Deleted,
}

/// Durable run status composed from admission state and the shared runtime lifecycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DurableRunStatus(Option<RunLifecycle>);

/// Backward-compatible public name for the durable run status.
pub type RunStatus = DurableRunStatus;

#[allow(non_upper_case_globals)]
impl DurableRunStatus {
    /// Run is accepted and awaiting runtime admission.
    pub const Queued: Self = Self(None);
    /// Runtime initialization is in progress.
    pub const Starting: Self = Self(Some(RunLifecycle::Starting));
    /// Runtime is actively executing.
    pub const Running: Self = Self(Some(RunLifecycle::Running));
    /// Runtime is waiting for external work.
    pub const Waiting: Self = Self(Some(RunLifecycle::Waiting));
    /// Runtime completed successfully.
    pub const Completed: Self = Self(Some(RunLifecycle::Completed));
    /// Runtime failed.
    pub const Failed: Self = Self(Some(RunLifecycle::Failed));
    /// Runtime was cancelled or interrupted.
    pub const Cancelled: Self = Self(Some(RunLifecycle::Cancelled));

    /// Return the admitted runtime lifecycle, or `None` while queued.
    #[must_use]
    pub const fn lifecycle(self) -> Option<RunLifecycle> {
        self.0
    }

    /// Return the stable flat wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self.0 {
            None => "queued",
            Some(lifecycle) => lifecycle.as_str(),
        }
    }

    /// Return whether the run owns the active session slot.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self.0,
            None | Some(RunLifecycle::Starting | RunLifecycle::Running | RunLifecycle::Waiting)
        )
    }

    /// Return whether the run reached a terminal lifecycle.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        match self.0 {
            Some(lifecycle) => lifecycle.is_terminal(),
            None => false,
        }
    }
}

impl From<RunLifecycle> for DurableRunStatus {
    fn from(value: RunLifecycle) -> Self {
        Self(Some(value))
    }
}

impl TryFrom<DurableRunStatus> for RunLifecycle {
    type Error = QueuedRunStatus;

    fn try_from(value: DurableRunStatus) -> Result<Self, Self::Error> {
        value.0.ok_or(QueuedRunStatus)
    }
}

impl Serialize for DurableRunStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DurableRunStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "queued" => Ok(Self::Queued),
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(D::Error::custom(format!("unknown run status: {other}"))),
        }
    }
}

/// Product-neutral diagnostic retained with a terminal durable run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunTerminalError {
    /// Stable `snake_case` category chosen by the producing boundary.
    pub code: String,
    /// Sanitized diagnostic suitable for durable retrieval.
    pub message: String,
}

impl RunTerminalError {
    /// Build a terminal diagnostic from a stable code and safe message.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Atomic terminal status, output, and diagnostic projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunTerminalProjection {
    /// Terminal durable status.
    pub status: RunStatus,
    /// User-visible final output preview, not an error transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Safe terminal diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunTerminalError>,
}

impl RunTerminalProjection {
    /// Build and validate a terminal projection.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-terminal status, a failed status without a diagnostic,
    /// a completed status with a diagnostic, or an empty diagnostic field.
    pub fn try_new(
        status: RunStatus,
        output_preview: Option<String>,
        error: Option<RunTerminalError>,
    ) -> Result<Self, RunTerminalProjectionError> {
        let projection = Self {
            status,
            output_preview,
            error,
        };
        projection.validate()?;
        Ok(projection)
    }

    /// Build a successful terminal projection.
    #[must_use]
    pub const fn completed(output_preview: Option<String>) -> Self {
        Self {
            status: RunStatus::Completed,
            output_preview,
            error: None,
        }
    }

    /// Build a failed terminal projection with no output preview.
    #[must_use]
    pub const fn failed(error: RunTerminalError) -> Self {
        Self {
            status: RunStatus::Failed,
            output_preview: None,
            error: Some(error),
        }
    }

    /// Build a cancelled terminal projection with an optional safe reason.
    #[must_use]
    pub const fn cancelled(error: Option<RunTerminalError>) -> Self {
        Self {
            status: RunStatus::Cancelled,
            output_preview: None,
            error,
        }
    }

    /// Validate the projection as a new terminal write.
    ///
    /// Historical records may lack a diagnostic. Stores validate this invariant only when
    /// terminalizing a non-terminal record, while preserving already-committed legacy evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when the projection violates terminal status or diagnostic invariants.
    pub fn validate(&self) -> Result<(), RunTerminalProjectionError> {
        if !self.status.is_terminal() {
            return Err(RunTerminalProjectionError::NonTerminalStatus(self.status));
        }
        if self.status == RunStatus::Failed && self.error.is_none() {
            return Err(RunTerminalProjectionError::MissingFailureDiagnostic);
        }
        if self.status == RunStatus::Completed && self.error.is_some() {
            return Err(RunTerminalProjectionError::UnexpectedSuccessDiagnostic);
        }
        if let Some(error) = self.error.as_ref() {
            if error.code.is_empty() {
                return Err(RunTerminalProjectionError::EmptyDiagnosticCode);
            }
            if error.message.is_empty() {
                return Err(RunTerminalProjectionError::EmptyDiagnosticMessage);
            }
        }
        Ok(())
    }

    /// Return whether this projection exactly matches a durable run record.
    #[must_use]
    pub fn matches(&self, run: &RunRecord) -> bool {
        (run.status, &run.output_preview, &run.terminal_error)
            == (self.status, &self.output_preview, &self.error)
    }

    /// Apply this complete projection to a run record.
    pub fn apply_to(&self, run: &mut RunRecord) {
        run.status = self.status;
        run.output_preview.clone_from(&self.output_preview);
        run.terminal_error.clone_from(&self.error);
    }
}

/// Invalid new terminal run projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTerminalProjectionError {
    /// The supplied status is not terminal.
    NonTerminalStatus(RunStatus),
    /// A failed run omitted its diagnostic.
    MissingFailureDiagnostic,
    /// A completed run carried an error diagnostic.
    UnexpectedSuccessDiagnostic,
    /// A non-terminal run carried a stale terminal diagnostic.
    UnexpectedNonTerminalDiagnostic(RunStatus),
    /// The stable diagnostic category is empty.
    EmptyDiagnosticCode,
    /// The diagnostic message is empty.
    EmptyDiagnosticMessage,
}

impl std::fmt::Display for RunTerminalProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonTerminalStatus(status) => {
                write!(formatter, "run status {} is not terminal", status.as_str())
            }
            Self::MissingFailureDiagnostic => {
                formatter.write_str("failed run requires a terminal diagnostic")
            }
            Self::UnexpectedSuccessDiagnostic => {
                formatter.write_str("completed run cannot carry a terminal diagnostic")
            }
            Self::UnexpectedNonTerminalDiagnostic(status) => write!(
                formatter,
                "non-terminal run status {} cannot carry a terminal diagnostic",
                status.as_str()
            ),
            Self::EmptyDiagnosticCode => formatter.write_str("terminal diagnostic code is empty"),
            Self::EmptyDiagnosticMessage => {
                formatter.write_str("terminal diagnostic message is empty")
            }
        }
    }
}

impl std::error::Error for RunTerminalProjectionError {}

/// A queued durable run has not entered an executable runtime lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuedRunStatus;

impl std::fmt::Display for QueuedRunStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("queued run has no runtime lifecycle")
    }
}

impl std::error::Error for QueuedRunStatus {}

/// Generic execution status for approval, deferred, checkpoint, and archive workflows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Item has been created.
    Pending,
    /// Item is currently processing.
    Running,
    /// Item is waiting on an external decision or worker.
    Waiting,
    /// Item completed successfully.
    Completed,
    /// Item failed.
    Failed,
    /// Item was cancelled.
    Cancelled,
}

/// Stable reference to an exported environment provider state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentStateRef {
    /// Environment provider name.
    pub provider: String,
    /// Stable provider state reference.
    pub reference: String,
    /// Provider state revision, hash, or generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Provider-specific metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Stable reference to a persisted checkpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointRef {
    /// Checkpoint id.
    pub checkpoint_id: CheckpointId,
    /// Run id.
    pub run_id: RunId,
    /// Checkpoint sequence within the run.
    pub sequence: usize,
    /// Runtime node name.
    pub node: String,
    /// Optional storage URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_ref: Option<String>,
    /// Stream cursor captured with this checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cursor: Option<usize>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Checkpoint metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Stable durable reference to a family-aware stream replay position.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StreamCursorRef {
    /// Canonical stream cursor. Family, scope, sequence, and backend position live here once.
    pub position: ReplayCursor,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Cursor metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl StreamCursorRef {
    /// Build a durable reference from a canonical cursor.
    #[must_use]
    pub fn new(position: ReplayCursor) -> Self {
        Self {
            position,
            created_at: Utc::now(),
            metadata: Metadata::default(),
        }
    }

    /// Return the cursor family.
    #[must_use]
    pub const fn family(&self) -> ReplayCursorFamily {
        self.position.family
    }

    /// Return the cursor scope.
    #[must_use]
    pub const fn scope(&self) -> &ReplayScope {
        &self.position.scope
    }

    /// Return the last observed sequence.
    #[must_use]
    pub const fn sequence(&self) -> usize {
        self.position.sequence
    }

    /// Return whether two references address the same stream family and scope.
    #[must_use]
    pub fn same_stream(&self, other: &Self) -> bool {
        self.family() == other.family() && self.scope() == other.scope()
    }

    /// Validate that this cursor belongs to the supplied run scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor addresses another run or a non-run scope.
    pub fn validate_for_run(&self, run_id: &RunId) -> Result<(), StreamCursorRefError> {
        let expected = ReplayScope::run(run_id.as_str());
        if self.scope() != &expected {
            return Err(StreamCursorRefError::WrongScope {
                expected: expected.as_str().to_string(),
                actual: self.scope().as_str().to_string(),
            });
        }
        Ok(())
    }

    /// Validate that replacing an existing same-stream cursor does not regress.
    ///
    /// # Errors
    ///
    /// Returns an error when the proposed sequence is behind the current sequence.
    pub fn validate_progression(&self, current: &Self) -> Result<(), StreamCursorRefError> {
        if self.same_stream(current) && self.sequence() < current.sequence() {
            return Err(StreamCursorRefError::SequenceRegression {
                family: self.family(),
                scope: self.scope().as_str().to_string(),
                current: current.sequence(),
                proposed: self.sequence(),
            });
        }
        Ok(())
    }
}

/// Invalid durable stream-cursor update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamCursorRefError {
    /// Cursor scope does not identify the run being updated.
    WrongScope {
        /// Expected run scope.
        expected: String,
        /// Supplied scope.
        actual: String,
    },
    /// Cursor sequence would move durable replay progress backwards.
    SequenceRegression {
        /// Cursor family.
        family: ReplayCursorFamily,
        /// Cursor scope.
        scope: String,
        /// Current sequence.
        current: usize,
        /// Proposed older sequence.
        proposed: usize,
    },
}

impl std::fmt::Display for StreamCursorRefError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongScope { expected, actual } => {
                write!(
                    formatter,
                    "expected cursor scope {expected}, received {actual}"
                )
            }
            Self::SequenceRegression {
                family,
                scope,
                current,
                proposed,
            } => write!(
                formatter,
                "{} cursor for {scope} regressed from {current} to {proposed}",
                family.as_str()
            ),
        }
    }
}

impl std::error::Error for StreamCursorRefError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentStreamCursorRefWire {
    position: ReplayCursor,
    created_at: DateTime<Utc>,
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyStreamCursorRefWire {
    family: String,
    scope: String,
    sequence: usize,
    #[serde(default)]
    cursor: Option<String>,
    created_at: DateTime<Utc>,
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StreamCursorRefWire {
    Current(CurrentStreamCursorRefWire),
    Legacy(LegacyStreamCursorRefWire),
}

impl<'de> Deserialize<'de> for StreamCursorRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match StreamCursorRefWire::deserialize(deserializer)? {
            StreamCursorRefWire::Current(current) => Ok(Self {
                position: current.position,
                created_at: current.created_at,
                metadata: current.metadata,
            }),
            StreamCursorRefWire::Legacy(legacy) => {
                let LegacyStreamCursorRefWire {
                    family,
                    scope,
                    sequence,
                    cursor,
                    created_at,
                    metadata,
                } = legacy;
                let family = match family.as_str() {
                    "raw_runtime" => ReplayCursorFamily::RawRuntime,
                    "display" => ReplayCursorFamily::Display,
                    "replay_event" => ReplayCursorFamily::ReplayEvent,
                    other => {
                        return Err(D::Error::custom(format!(
                            "unknown stream cursor family: {other}"
                        )));
                    }
                };
                let mut position =
                    ReplayCursor::for_family(family, ReplayScope::from_string(scope), sequence);
                position.backend_cursor = cursor;
                Ok(Self {
                    position,
                    created_at,
                    metadata,
                })
            }
        }
    }
}

/// Durable workspace provenance that does not confer live filesystem authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProvenanceRef {
    /// Stable execution-domain-local workspace identity.
    pub workspace_id: String,
    /// Safe user-facing label. This may preserve a legacy path as historical evidence only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_label: Option<String>,
    /// Digest of the canonical provenance declaration used when this reference was recorded.
    pub provenance_digest: String,
}

impl WorkspaceProvenanceRef {
    /// Build a host-approved workspace provenance reference.
    #[must_use]
    pub fn new(
        workspace_id: impl Into<String>,
        display_label: Option<String>,
        provenance_digest: impl Into<String>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            display_label,
            provenance_digest: provenance_digest.into(),
        }
    }

    /// Derive stable durable provenance for a canonical root in one execution domain.
    ///
    /// This records identity only. It never grants filesystem authority; a host must still match
    /// it against an independently registered live workspace grant.
    #[must_use]
    pub fn for_execution_domain_root(
        execution_domain_id: &str,
        canonical_root: &str,
        display_label: Option<String>,
    ) -> Self {
        let identity = workspace_identity_digest(
            b"starweaver.workspace.id.v1\0",
            execution_domain_id,
            canonical_root,
        );
        let provenance = workspace_identity_digest(
            b"starweaver.workspace.provenance.v1\0",
            execution_domain_id,
            canonical_root,
        );
        Self {
            workspace_id: format!("workspace_{}", &identity[..32]),
            display_label,
            provenance_digest: format!("sha256:{provenance}"),
        }
    }

    /// Migrate one legacy workspace string into unbound historical provenance.
    ///
    /// The derived `legacy:` identity is deliberately outside the host workspace-id namespace and
    /// must never be resolved as a live grant. It only keeps migration and filtering deterministic.
    #[must_use]
    pub fn from_legacy_display(display: impl Into<String>) -> Self {
        let display = display.into();
        let mut digest = Sha256::new();
        digest.update(b"starweaver.session.workspace_provenance.legacy.v1\0");
        digest.update(display.as_bytes());
        let digest = format!("sha256:{:x}", digest.finalize());
        Self {
            workspace_id: format!("legacy:{digest}"),
            display_label: Some(display),
            provenance_digest: digest,
        }
    }

    /// Return whether this reference was synthesized from legacy display-only evidence.
    #[must_use]
    pub fn is_legacy_unbound(&self) -> bool {
        self.workspace_id.starts_with("legacy:")
    }

    /// Return the safe value used by legacy list/filter projections.
    #[must_use]
    pub fn display_value(&self) -> &str {
        self.display_label.as_deref().unwrap_or(&self.workspace_id)
    }

    /// Return whether a legacy workspace filter matches the stable id or display projection.
    #[must_use]
    pub fn matches_filter(&self, workspace: &str) -> bool {
        self.workspace_id == workspace || self.display_label.as_deref() == Some(workspace)
    }

    /// Validate durable provenance identity without conferring live authority.
    ///
    /// # Errors
    ///
    /// Returns an error when required identity evidence is empty or a legacy identity is
    /// internally inconsistent.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.workspace_id.trim().is_empty() || self.provenance_digest.trim().is_empty() {
            return Err("workspace provenance identity is empty");
        }
        if self.is_legacy_unbound()
            && (self.workspace_id != format!("legacy:{}", self.provenance_digest)
                || self.display_label.as_deref().is_none_or(str::is_empty))
        {
            return Err("legacy workspace provenance is inconsistent");
        }
        Ok(())
    }
}

fn workspace_identity_digest(
    domain: &[u8],
    execution_domain_id: &str,
    canonical_root: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(
        u64::try_from(execution_domain_id.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    digest.update(execution_domain_id.as_bytes());
    digest.update(
        u64::try_from(canonical_root.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    digest.update(canonical_root.as_bytes());
    format!("{:x}", digest.finalize())
}

/// Immutable runtime-configuration snapshot identity pinned at run admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigSnapshotRef {
    /// Monotonic runtime-config generation.
    pub generation: u64,
    /// Opaque revision etag identifying the retained immutable declaration.
    pub etag: String,
    /// Digest of the fully resolved non-secret run materialization.
    pub materialization_digest: String,
}

impl RuntimeConfigSnapshotRef {
    /// Build a runtime-config snapshot reference.
    #[must_use]
    pub fn new(
        generation: u64,
        etag: impl Into<String>,
        materialization_digest: impl Into<String>,
    ) -> Self {
        Self {
            generation,
            etag: etag.into(),
            materialization_digest: materialization_digest.into(),
        }
    }

    /// Validate an immutable runtime-config snapshot identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the generation or required identity evidence is empty.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.generation == 0 {
            return Err("runtime config generation must be positive");
        }
        if self.etag.trim().is_empty() || self.materialization_digest.trim().is_empty() {
            return Err("runtime config snapshot identity is empty");
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct SessionRecordV1 {
    #[serde(default)]
    workspace: Option<String>,
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
}

fn decode_session_record_v1(payload: Value) -> Result<SessionRecord, VersionedRecordError> {
    let legacy =
        serde_json::from_value::<SessionRecordV1>(payload).map_err(VersionedRecordError::Json)?;
    let mut current = serde_json::from_value::<SessionRecord>(Value::Object(legacy.fields))
        .map_err(VersionedRecordError::Json)?;
    current.workspace = legacy
        .workspace
        .map(WorkspaceProvenanceRef::from_legacy_display);
    Ok(current)
}

/// Durable session record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionRecord {
    /// Session id.
    pub session_id: SessionId,
    /// Host-derived store/tenant namespace. Legacy records default to `local`.
    #[serde(default = "default_session_namespace")]
    pub namespace_id: String,
    /// Host-derived owner/principal. Lineage and metadata never assign authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Monotonic optimistic concurrency revision.
    #[serde(default = "initial_session_revision")]
    pub revision: u64,
    /// Deletion/continuation fence.
    #[serde(default)]
    pub deletion_fence: SessionDeletionFence,
    /// User-facing title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace provenance. This evidence never recreates a host-local live grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceProvenanceRef>,
    /// Runtime profile or model profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Session status.
    #[serde(default)]
    pub status: SessionStatus,
    /// Last exported context state.
    #[serde(default)]
    pub state: ResumableState,
    /// Latest exported environment state reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_state: Option<EnvironmentStateRef>,
    /// Latest stream cursor refs by family.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Session trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Parent session id for forks or delegated flows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    /// Latest run in the session sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_run_id: Option<RunId>,
    /// Latest completed run usable as continuation source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_success_run_id: Option<RunId>,
    /// Currently queued, running, or waiting run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<RunId>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

fn default_session_namespace() -> String {
    crate::LOCAL_SESSION_NAMESPACE.to_string()
}

const fn initial_session_revision() -> u64 {
    1
}

const fn initial_run_revision() -> u64 {
    1
}

impl starweaver_core::VersionedRecord for SessionRecord {
    const SCHEMA: &'static str = "starweaver.session.session_record";
    const VERSION: u32 = 2;
    const ALLOW_BARE_V0: bool = true;

    fn decode_version(version: u32, payload: Value) -> Result<Self, VersionedRecordError> {
        match version {
            1 => decode_session_record_v1(payload),
            Self::VERSION => serde_json::from_value(payload).map_err(VersionedRecordError::Json),
            actual => Err(VersionedRecordError::UnsupportedVersion {
                schema: Self::SCHEMA,
                supported: Self::VERSION,
                actual,
            }),
        }
    }

    fn decode_bare_v0(payload: Value) -> Result<Self, VersionedRecordError> {
        decode_session_record_v1(payload)
    }
}

impl SessionRecord {
    /// Validate workspace provenance carried by this durable session.
    ///
    /// # Errors
    ///
    /// Returns an error when the optional provenance is malformed.
    pub fn validate_provenance(&self) -> Result<(), &'static str> {
        self.workspace
            .as_ref()
            .map_or(Ok(()), WorkspaceProvenanceRef::validate)
    }

    /// Build a session record with default state.
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            namespace_id: default_session_namespace(),
            owner_id: None,
            revision: initial_session_revision(),
            deletion_fence: SessionDeletionFence::Stable,
            title: None,
            workspace: None,
            profile: None,
            status: SessionStatus::Active,
            state: ResumableState::default(),
            environment_state: None,
            stream_cursors: Vec::new(),
            trace_context: TraceContext::default(),
            parent_session_id: None,
            head_run_id: None,
            head_success_run_id: None,
            active_run_id: None,
            created_at: now,
            updated_at: now,
            metadata: Metadata::default(),
        }
    }
}

#[derive(Deserialize)]
struct RunRecordV1 {
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
}

fn decode_run_record_v1(payload: Value) -> Result<RunRecord, VersionedRecordError> {
    let legacy =
        serde_json::from_value::<RunRecordV1>(payload).map_err(VersionedRecordError::Json)?;
    serde_json::from_value::<RunRecord>(Value::Object(legacy.fields))
        .map_err(VersionedRecordError::Json)
}

/// Durable run record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunRecord {
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Monotonic optimistic-concurrency revision.
    #[serde(default = "initial_run_revision")]
    pub revision: u64,
    /// Conversation id.
    pub conversation_id: ConversationId,
    /// User, API, or service input parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<InputPart>,
    /// Durable run status.
    #[serde(default)]
    pub status: RunStatus,
    /// Final output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Safe diagnostic for a failed or cancelled terminal run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_error: Option<RunTerminalError>,
    /// Final structured output preview or summary.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub structured_output: Value,
    /// Latest checkpoint reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<CheckpointRef>,
    /// Latest environment state reference for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_state: Option<EnvironmentStateRef>,
    /// Latest stream cursor refs by family.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Monotonic order inside the session.
    #[serde(default)]
    pub sequence_no: usize,
    /// Run snapshot used as continuation source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<RunId>,
    /// Parent run identifier when this run is delegated from another run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Parent-scoped delegated task identifier when this run executes a lightweight task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<TaskId>,
    /// Trigger source such as cli, service, schedule, or delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    /// Profile resolved for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Immutable runtime-config snapshot pinned when this run was admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_snapshot: Option<RuntimeConfigSnapshotRef>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for RunRecord {
    const SCHEMA: &'static str = "starweaver.session.run_record";
    const VERSION: u32 = 2;
    const ALLOW_BARE_V0: bool = true;

    fn decode_version(version: u32, payload: Value) -> Result<Self, VersionedRecordError> {
        match version {
            1 => decode_run_record_v1(payload),
            Self::VERSION => serde_json::from_value(payload).map_err(VersionedRecordError::Json),
            actual => Err(VersionedRecordError::UnsupportedVersion {
                schema: Self::SCHEMA,
                supported: Self::VERSION,
                actual,
            }),
        }
    }

    fn decode_bare_v0(payload: Value) -> Result<Self, VersionedRecordError> {
        decode_run_record_v1(payload)
    }
}

impl RunRecord {
    /// Validate runtime-config provenance carried by this durable run.
    ///
    /// # Errors
    ///
    /// Returns an error when the optional snapshot identity is malformed.
    pub fn validate_provenance(&self) -> Result<(), &'static str> {
        self.config_snapshot
            .as_ref()
            .map_or(Ok(()), RuntimeConfigSnapshotRef::validate)
    }

    /// Return the complete terminal projection when this record is terminal.
    ///
    /// Historical failed records may legitimately return a projection with no diagnostic.
    #[must_use]
    pub fn terminal_projection(&self) -> Option<RunTerminalProjection> {
        self.status.is_terminal().then(|| RunTerminalProjection {
            status: self.status,
            output_preview: self.output_preview.clone(),
            error: self.terminal_error.clone(),
        })
    }

    /// Validate a newly persisted run state.
    ///
    /// Historical records are accepted when read, but every new write must carry a complete
    /// terminal projection and every non-terminal write must omit stale terminal diagnostics.
    ///
    /// # Errors
    ///
    /// Returns an error when status, output, and terminal diagnostic are inconsistent.
    pub fn validate_new_write(&self) -> Result<(), RunTerminalProjectionError> {
        self.terminal_projection().map_or_else(
            || {
                if self.terminal_error.is_some() {
                    Err(RunTerminalProjectionError::UnexpectedNonTerminalDiagnostic(
                        self.status,
                    ))
                } else {
                    Ok(())
                }
            },
            |terminal| terminal.validate(),
        )
    }

    /// Normalize caller-provided state before a new admission is persisted.
    ///
    /// Admission always creates a queued execution. Any stale terminal projection from a reused
    /// record is discarded rather than becoming active-run state or client-visible output.
    pub fn normalize_for_admission(&mut self) {
        self.status = RunStatus::Queued;
        self.output_preview = None;
        self.terminal_error = None;
    }

    /// Apply the legacy low-level status update contract.
    ///
    /// The legacy API cannot distinguish safe diagnostics from arbitrary caller text, so failed
    /// and cancelled updates discard their preview and persist a fixed generic diagnostic.
    pub fn apply_legacy_status_update(
        &mut self,
        status: RunStatus,
        output_preview: Option<String>,
    ) {
        self.status = status;
        match status {
            RunStatus::Failed => {
                self.output_preview = None;
                self.terminal_error = Some(RunTerminalError::new(
                    "legacy_status_update_failed",
                    "run failed",
                ));
            }
            RunStatus::Cancelled => {
                self.output_preview = None;
                self.terminal_error = Some(RunTerminalError::new(
                    "legacy_status_update_cancelled",
                    "run cancelled",
                ));
            }
            _ => {
                self.output_preview = output_preview;
                self.terminal_error = None;
            }
        }
    }

    /// Build a run record for a session.
    #[must_use]
    pub fn new(session_id: SessionId, run_id: RunId, conversation_id: ConversationId) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            run_id,
            revision: initial_run_revision(),
            conversation_id,
            input: Vec::new(),
            status: RunStatus::Queued,
            output_preview: None,
            terminal_error: None,
            structured_output: Value::Null,
            latest_checkpoint: None,
            environment_state: None,
            stream_cursors: Vec::new(),
            trace_context: TraceContext::default(),
            sequence_no: 0,
            restore_from_run_id: None,
            parent_run_id: None,
            parent_task_id: None,
            trigger_type: None,
            profile: None,
            config_snapshot: None,
            created_at: now,
            updated_at: now,
            metadata: Metadata::default(),
        }
    }
}
