//! Product-neutral durable host-event evidence and replay queries.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use starweaver_core::{RunId, SessionId};

use crate::{RunRecord, RunStatus, SessionStoreError, SessionStoreResult};

/// Maximum number of durable host events returned by one storage query.
pub const MAX_HOST_EVENT_PAGE_SIZE: usize = 500;

/// Largest backend position shared by memory and `SQLite` implementations.
pub const MAX_HOST_EVENT_POSITION: u64 = 9_223_372_036_854_775_807;

/// Stable product-neutral event classes persisted by the session store.
///
/// Transport-owned subscription closure is intentionally absent: it contains connection-local
/// cursor and delivery-sequence material and is not durable host evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableHostEventClass {
    /// A durable session projection changed.
    SessionChanged,
    /// A durable run projection changed.
    RunChanged,
    /// Durable run output became available.
    OutputAvailable,
    /// An approval projection changed.
    ApprovalChanged,
    /// A deferred-tool projection changed.
    DeferredChanged,
    /// A clarification projection changed.
    ClarificationChanged,
    /// An environment attachment projection changed.
    EnvironmentChanged,
    /// A durable operator-facing diagnostic was recorded.
    Diagnostic,
}

impl DurableHostEventClass {
    /// Every durable host-event class in canonical order.
    pub const ALL: [Self; 8] = [
        Self::SessionChanged,
        Self::RunChanged,
        Self::OutputAvailable,
        Self::ApprovalChanged,
        Self::DeferredChanged,
        Self::ClarificationChanged,
        Self::EnvironmentChanged,
        Self::Diagnostic,
    ];

    /// Return the stable storage identifier for this event class.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionChanged => "session_changed",
            Self::RunChanged => "run_changed",
            Self::OutputAvailable => "output_available",
            Self::ApprovalChanged => "approval_changed",
            Self::DeferredChanged => "deferred_changed",
            Self::ClarificationChanged => "clarification_changed",
            Self::EnvironmentChanged => "environment_changed",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// Resource scope owned by one durable host event.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurableHostEventScope {
    /// Host-global evidence.
    Global,
    /// Evidence belonging to one session.
    Session {
        /// Durable session identity.
        session_id: SessionId,
    },
    /// Evidence belonging to one run in one session.
    Run {
        /// Durable session identity.
        session_id: SessionId,
        /// Durable run identity.
        run_id: RunId,
    },
}

impl DurableHostEventScope {
    /// Build a session scope.
    #[must_use]
    pub const fn session(session_id: SessionId) -> Self {
        Self::Session { session_id }
    }

    /// Build a run scope.
    #[must_use]
    pub const fn run(session_id: SessionId, run_id: RunId) -> Self {
        Self::Run { session_id, run_id }
    }

    /// Return the stable scope-kind identifier.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Session { .. } => "session",
            Self::Run { .. } => "run",
        }
    }

    /// Return the session identity carried by this scope, when present.
    #[must_use]
    pub const fn session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Global => None,
            Self::Session { session_id } | Self::Run { session_id, .. } => Some(session_id),
        }
    }

    /// Return the run identity carried by this scope, when present.
    #[must_use]
    pub const fn run_id(&self) -> Option<&RunId> {
        match self {
            Self::Run { run_id, .. } => Some(run_id),
            Self::Global | Self::Session { .. } => None,
        }
    }

    /// Return whether a record scope is visible inside this requested scope.
    ///
    /// A global view contains all resources, a session view contains that session and its runs,
    /// and a run view contains only that exact run.
    #[must_use]
    pub fn contains(&self, record_scope: &Self) -> bool {
        match (self, record_scope) {
            (Self::Global, _) => true,
            (
                Self::Session {
                    session_id: expected,
                },
                Self::Session { session_id } | Self::Run { session_id, .. },
            ) => expected == session_id,
            (
                Self::Run {
                    session_id: expected_session,
                    run_id: expected_run,
                },
                Self::Run { session_id, run_id },
            ) => expected_session == session_id && expected_run == run_id,
            (Self::Session { .. } | Self::Run { .. }, Self::Global | Self::Session { .. }) => false,
        }
    }

    fn identity_components(&self) -> Vec<&str> {
        match self {
            Self::Global => vec!["global"],
            Self::Session { session_id } => vec!["session", session_id.as_str()],
            Self::Run { session_id, run_id } => {
                vec!["run", session_id.as_str(), run_id.as_str()]
            }
        }
    }
}

/// Stable deterministic identity for one logical transition's event publication.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventPublicationKey(String);

impl EventPublicationKey {
    /// Derive an unambiguous publication key from a transition identity and event ordinal.
    ///
    /// The transition identity must itself be stable across retries, for example a durable
    /// mutation receipt, admission identity, or sealed run-evidence identity.
    ///
    /// # Errors
    ///
    /// Returns an error when `transition_identity` is empty.
    pub fn derive(
        transition_identity: &str,
        ordinal: u32,
        scope: &DurableHostEventScope,
        event_class: DurableHostEventClass,
    ) -> SessionStoreResult<Self> {
        if transition_identity.is_empty() {
            return Err(SessionStoreError::Failed(
                "host event transition identity cannot be empty".to_string(),
            ));
        }
        let ordinal = ordinal.to_string();
        let mut components = vec![transition_identity, event_class.as_str(), ordinal.as_str()];
        components.extend(scope.identity_components());
        Ok(Self(format!(
            "host-event-publication-sha256:{:x}",
            framed_digest(components)
        )))
    }

    /// Return the publication-key string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One view-independent host event waiting for durable-log materialization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingHostEventPublication {
    /// Deterministic logical-publication identity.
    pub publication_key: EventPublicationKey,
    /// Deterministic public event identity retained across every view and retry.
    pub event_id: String,
    /// Resource scope owned by this event.
    pub scope: DurableHostEventScope,
    /// Stable event class used for storage-level eligibility filtering.
    pub event_class: DurableHostEventClass,
    /// Product-neutral object projection. It contains event fields but no wire cursor, caller,
    /// authority, feature-set, or view material.
    pub projection: Value,
    /// Time at which the authoritative transition occurred.
    pub occurred_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for PendingHostEventPublication {
    const SCHEMA: &'static str = "starweaver.session.pending_host_event_publication";
}

impl PendingHostEventPublication {
    /// Build deterministic publication evidence for one logical transition.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty transition identity or a non-object projection.
    pub fn new(
        transition_identity: &str,
        ordinal: u32,
        scope: DurableHostEventScope,
        event_class: DurableHostEventClass,
        projection: Value,
        occurred_at: DateTime<Utc>,
    ) -> SessionStoreResult<Self> {
        if !projection.is_object() {
            return Err(SessionStoreError::Failed(
                "durable host event projection must be a JSON object".to_string(),
            ));
        }
        let publication_key =
            EventPublicationKey::derive(transition_identity, ordinal, &scope, event_class)?;
        let event_id = derived_event_id(&publication_key);
        Ok(Self {
            publication_key,
            event_id,
            scope,
            event_class,
            projection,
            occurred_at,
        })
    }

    /// Validate persisted publication evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when identity or projection invariants are invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        if self.publication_key.as_str().is_empty() || self.event_id.is_empty() {
            return Err(SessionStoreError::Failed(
                "durable host event identity cannot be empty".to_string(),
            ));
        }
        if self.event_id != derived_event_id(&self.publication_key) {
            return Err(SessionStoreError::Conflict(format!(
                "host event identity does not match publication {}",
                self.publication_key.as_str()
            )));
        }
        if !self.projection.is_object() {
            return Err(SessionStoreError::Failed(
                "durable host event projection must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }
}

/// Product-neutral run summary used by durable `run_changed` projections.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunChangedSummary {
    /// Run creation time.
    pub created_at: DateTime<Utc>,
    /// Stable diagnostic category when the run terminated with a diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_ref: Option<String>,
    /// User-visible output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Decimal string revision used without a transport-specific integer wrapper.
    pub revision: String,
    /// Durable run identity.
    pub run_id: RunId,
    /// Durable session identity.
    pub session_id: SessionId,
    /// Durable run status.
    pub status: RunStatus,
    /// Last authoritative update time.
    pub updated_at: DateTime<Utc>,
}

impl From<&RunRecord> for RunChangedSummary {
    fn from(run: &RunRecord) -> Self {
        Self {
            created_at: run.created_at,
            diagnostic_ref: run.terminal_error.as_ref().map(|error| error.code.clone()),
            output_preview: run.output_preview.clone(),
            revision: run.revision.to_string(),
            run_id: run.run_id.clone(),
            session_id: run.session_id.clone(),
            status: run.status,
            updated_at: run.updated_at,
        }
    }
}

/// Product-neutral durable `run_changed` event projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunChangedProjection {
    /// Stable event discriminator.
    pub kind: String,
    /// Complete authoritative run summary.
    pub run: RunChangedSummary,
}

/// Product-neutral durable `output_available` event projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputAvailableProjection {
    /// Stable event discriminator.
    pub kind: String,
    /// Stable output identity derived from the durable run revision.
    pub output_ref: String,
    /// User-visible output preview.
    pub preview: String,
    /// Durable run identity.
    pub run_id: RunId,
    /// Durable session identity.
    pub session_id: SessionId,
}

/// Build a durable `run_changed` publication from authoritative storage-domain state.
///
/// # Errors
///
/// Returns an error when projection serialization or publication identity validation fails.
pub fn run_changed_publication(
    transition_identity: &str,
    ordinal: u32,
    run: &RunRecord,
) -> SessionStoreResult<PendingHostEventPublication> {
    let projection = serde_json::to_value(RunChangedProjection {
        kind: "run_changed".to_string(),
        run: RunChangedSummary::from(run),
    })
    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
    PendingHostEventPublication::new(
        transition_identity,
        ordinal,
        DurableHostEventScope::run(run.session_id.clone(), run.run_id.clone()),
        DurableHostEventClass::RunChanged,
        projection,
        run.updated_at,
    )
}

/// Build a durable `output_available` publication when a run has a user-visible preview.
///
/// # Errors
///
/// Returns an error when projection serialization or publication identity validation fails.
pub fn output_available_publication(
    transition_identity: &str,
    ordinal: u32,
    run: &RunRecord,
) -> SessionStoreResult<Option<PendingHostEventPublication>> {
    let Some(preview) = run.output_preview.clone() else {
        return Ok(None);
    };
    let projection = serde_json::to_value(OutputAvailableProjection {
        kind: "output_available".to_string(),
        output_ref: format!(
            "run-output:{}:{}:{}",
            run.session_id.as_str(),
            run.run_id.as_str(),
            run.revision
        ),
        preview,
        run_id: run.run_id.clone(),
        session_id: run.session_id.clone(),
    })
    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
    PendingHostEventPublication::new(
        transition_identity,
        ordinal,
        DurableHostEventScope::run(run.session_id.clone(), run.run_id.clone()),
        DurableHostEventClass::OutputAvailable,
        projection,
        run.updated_at,
    )
    .map(Some)
}

/// Add authoritative run/output events unless the caller already supplied that class and scope.
///
/// This lets transport boundaries supply richer compatible projections while ensuring a bare
/// run-evidence commit never leaves its authoritative state without atomic host publication.
///
/// # Errors
///
/// Returns an error when an automatically projected publication cannot be built.
pub fn append_authoritative_run_publications<'a>(
    publications: &mut Vec<PendingHostEventPublication>,
    transition_identity: &str,
    runs: impl IntoIterator<Item = &'a RunRecord>,
) -> SessionStoreResult<()> {
    for (index, run) in runs.into_iter().enumerate() {
        let scope = DurableHostEventScope::run(run.session_id.clone(), run.run_id.clone());
        let ordinal = u32::try_from(index).map_err(|error| {
            SessionStoreError::Failed(format!("too many authoritative run publications: {error}"))
        })?;
        if !publications.iter().any(|publication| {
            publication.scope == scope
                && publication.event_class == DurableHostEventClass::RunChanged
        }) {
            publications.push(run_changed_publication(transition_identity, ordinal, run)?);
        }
        if run.output_preview.is_some()
            && !publications.iter().any(|publication| {
                publication.scope == scope
                    && publication.event_class == DurableHostEventClass::OutputAvailable
            })
            && let Some(publication) =
                output_available_publication(transition_identity, ordinal, run)?
        {
            publications.push(publication);
        }
        if run.status.is_terminal() && run.output_preview.is_some() {
            let run_changed_index = publications.iter().position(|publication| {
                publication.scope == scope
                    && publication.event_class == DurableHostEventClass::RunChanged
            });
            let output_index = publications.iter().position(|publication| {
                publication.scope == scope
                    && publication.event_class == DurableHostEventClass::OutputAvailable
            });
            if let (Some(run_changed_index), Some(output_index)) = (run_changed_index, output_index)
                && output_index > run_changed_index
            {
                let output = publications.remove(output_index);
                publications.insert(run_changed_index, output);
            }
        }
    }
    Ok(())
}

/// One materialized durable host-event record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableHostEventRecord {
    /// Storage-domain monotonic position. This is never exposed directly on the public wire.
    pub position: u64,
    /// Deterministic logical-publication identity.
    pub publication_key: EventPublicationKey,
    /// Stable public event identity.
    pub event_id: String,
    /// Resource scope owned by this event.
    pub scope: DurableHostEventScope,
    /// Stable event class.
    pub event_class: DurableHostEventClass,
    /// Product-neutral object projection.
    pub projection: Value,
    /// Time at which the authoritative transition occurred.
    pub occurred_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for DurableHostEventRecord {
    const SCHEMA: &'static str = "starweaver.session.durable_host_event";
}

impl DurableHostEventRecord {
    /// Materialize one pending publication at a storage-assigned position.
    #[must_use]
    pub fn from_pending(position: u64, pending: PendingHostEventPublication) -> Self {
        Self {
            position,
            publication_key: pending.publication_key,
            event_id: pending.event_id,
            scope: pending.scope,
            event_class: pending.event_class,
            projection: pending.projection,
            occurred_at: pending.occurred_at,
        }
    }

    /// Return the pending form used for exact-retry comparison.
    #[must_use]
    pub fn pending_projection(&self) -> PendingHostEventPublication {
        PendingHostEventPublication {
            publication_key: self.publication_key.clone(),
            event_id: self.event_id.clone(),
            scope: self.scope.clone(),
            event_class: self.event_class,
            projection: self.projection.clone(),
            occurred_at: self.occurred_at,
        }
    }
}

/// Bounded storage query over one resource scope and an admitted set of event classes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableHostEventQuery {
    /// Exact requested scope. Containment is applied by [`DurableHostEventScope::contains`].
    pub scope: DurableHostEventScope,
    /// Eligible event classes. Empty sets are rejected.
    pub event_classes: BTreeSet<DurableHostEventClass>,
    /// Return eligible records strictly after this backend position.
    pub after_position: Option<u64>,
    /// Maximum records returned, from 1 through [`MAX_HOST_EVENT_PAGE_SIZE`].
    pub limit: usize,
}

impl DurableHostEventQuery {
    /// Build and validate a replay query.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty class set or invalid page size.
    pub fn new(
        scope: DurableHostEventScope,
        event_classes: impl IntoIterator<Item = DurableHostEventClass>,
        after_position: Option<u64>,
        limit: usize,
    ) -> SessionStoreResult<Self> {
        let event_classes = event_classes.into_iter().collect::<BTreeSet<_>>();
        if event_classes.is_empty() {
            return Err(SessionStoreError::Failed(
                "durable host event query requires at least one event class".to_string(),
            ));
        }
        if !(1..=MAX_HOST_EVENT_PAGE_SIZE).contains(&limit) {
            return Err(SessionStoreError::Failed(format!(
                "durable host event query limit must be between 1 and {MAX_HOST_EVENT_PAGE_SIZE}"
            )));
        }
        if after_position.is_some_and(|position| position > MAX_HOST_EVENT_POSITION) {
            return Err(SessionStoreError::Failed(format!(
                "durable host event position exceeds {MAX_HOST_EVENT_POSITION}"
            )));
        }
        Ok(Self {
            scope,
            event_classes,
            after_position,
            limit,
        })
    }
}

/// One bounded, eligibility-filtered durable host-event page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableHostEventPage {
    /// Eligible records in durable order.
    pub records: Vec<DurableHostEventRecord>,
    /// Last eligible backend position, or the requested start position when the page is empty.
    pub next_position: Option<u64>,
    /// Whether another eligible record exists after `next_position`.
    pub has_more: bool,
}

fn derived_event_id(publication_key: &EventPublicationKey) -> String {
    format!(
        "event-sha256:{:x}",
        framed_digest([publication_key.as_str()])
    )
}

fn framed_digest<'a>(components: impl IntoIterator<Item = &'a str>) -> impl std::fmt::LowerHex {
    let mut digest = Sha256::new();
    for component in components {
        digest.update(component.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(component.as_bytes());
        digest.update(b";");
    }
    digest.finalize()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use starweaver_core::{ConversationId, RunId, SessionId};

    use crate::{RunRecord, RunStatus};

    use super::{
        DurableHostEventClass, DurableHostEventQuery, DurableHostEventScope,
        PendingHostEventPublication, append_authoritative_run_publications,
        output_available_publication, run_changed_publication,
    };

    #[test]
    fn publication_identity_is_stable_and_framed() {
        let occurred_at = Utc::now();
        let first = PendingHostEventPublication::new(
            "a:b",
            0,
            DurableHostEventScope::session(SessionId::from_string("c")),
            DurableHostEventClass::SessionChanged,
            json!({"revision": "1"}),
            occurred_at,
        )
        .expect("first publication");
        let retry = PendingHostEventPublication::new(
            "a:b",
            0,
            DurableHostEventScope::session(SessionId::from_string("c")),
            DurableHostEventClass::SessionChanged,
            json!({"revision": "1"}),
            occurred_at,
        )
        .expect("retry publication");
        let ambiguous_without_framing = PendingHostEventPublication::new(
            "a",
            0,
            DurableHostEventScope::session(SessionId::from_string("b:c")),
            DurableHostEventClass::SessionChanged,
            json!({"revision": "1"}),
            occurred_at,
        )
        .expect("second publication");

        assert_eq!(first, retry);
        assert_ne!(
            first.publication_key,
            ambiguous_without_framing.publication_key
        );
        assert_ne!(first.event_id, ambiguous_without_framing.event_id);
    }

    #[test]
    fn authoritative_run_projections_are_product_neutral_wire_shapes() {
        let mut run = RunRecord::new(
            SessionId::from_string("session-event"),
            RunId::from_string("run-event"),
            ConversationId::from_string("conversation-event"),
        );
        run.status = RunStatus::Completed;
        run.output_preview = Some("ready".to_string());
        run.revision = 7;

        let changed =
            run_changed_publication("run-transition", 0, &run).expect("run changed publication");
        assert_eq!(changed.projection["kind"], json!("run_changed"));
        assert_eq!(changed.projection["run"]["revision"], json!("7"));
        assert_eq!(changed.projection["run"]["runId"], json!("run-event"));
        assert_eq!(changed.projection["run"]["status"], json!("completed"));

        let output = output_available_publication("run-transition", 0, &run)
            .expect("output publication")
            .expect("preview produces output event");
        assert_eq!(output.projection["kind"], json!("output_available"));
        assert_eq!(output.projection["preview"], json!("ready"));
        assert_eq!(output.projection["runId"], json!("run-event"));
        assert_eq!(output.projection["sessionId"], json!("session-event"));
    }

    #[test]
    fn terminal_output_is_published_before_the_terminal_run_event() {
        let mut run = RunRecord::new(
            SessionId::from_string("session-terminal"),
            RunId::from_string("run-terminal"),
            ConversationId::from_string("conversation-terminal"),
        );
        run.status = RunStatus::Completed;
        run.output_preview = Some("complete".to_string());

        let preseeded =
            run_changed_publication("terminal-transition", 0, &run).expect("preseeded run event");
        for mut publications in [Vec::new(), vec![preseeded]] {
            append_authoritative_run_publications(&mut publications, "terminal-transition", [&run])
                .expect("authoritative terminal publications");
            assert_eq!(
                publications
                    .iter()
                    .map(|publication| publication.event_class)
                    .collect::<Vec<_>>(),
                vec![
                    DurableHostEventClass::OutputAvailable,
                    DurableHostEventClass::RunChanged,
                ]
            );
        }
    }

    #[test]
    fn scope_containment_is_hierarchical() {
        let session_id = SessionId::from_string("session-1");
        let run = DurableHostEventScope::run(session_id.clone(), RunId::from_string("run-1"));
        assert!(DurableHostEventScope::Global.contains(&run));
        assert!(DurableHostEventScope::session(session_id).contains(&run));
        assert!(run.contains(&run));
        assert!(!DurableHostEventScope::session(SessionId::from_string("other")).contains(&run));
    }

    #[test]
    fn query_rejects_empty_classes_and_unbounded_limits() {
        assert!(DurableHostEventQuery::new(DurableHostEventScope::Global, [], None, 10).is_err());
        assert!(
            DurableHostEventQuery::new(
                DurableHostEventScope::Global,
                [DurableHostEventClass::Diagnostic],
                None,
                501,
            )
            .is_err()
        );
        assert!(
            DurableHostEventQuery::new(
                DurableHostEventScope::Global,
                [DurableHostEventClass::Diagnostic],
                Some(super::MAX_HOST_EVENT_POSITION + 1),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn publication_validation_rejects_forged_event_identity() {
        let mut publication = PendingHostEventPublication::new(
            "transition",
            0,
            DurableHostEventScope::Global,
            DurableHostEventClass::Diagnostic,
            json!({"code": "test"}),
            Utc::now(),
        )
        .expect("publication");
        publication.event_id = "caller-selected".to_string();
        assert!(publication.validate().is_err());
    }
}
