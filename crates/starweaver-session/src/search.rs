//! Product-neutral session discovery contracts.

use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use starweaver_core::{AgentId, RunId, SessionId};
use starweaver_stream::{ReplayCursor, ReplayScope};
use thiserror::Error;

use crate::{RunStatus, SessionStatus};

/// Read-only provider for session discovery projections.
#[async_trait]
pub trait SessionSearchProvider: Send + Sync {
    /// Advertise behavior supported by this provider instance.
    fn capabilities(&self) -> SessionSearchCapabilities;

    /// Search within a host-constructed authorization scope.
    async fn search(
        &self,
        scope: &SessionSearchScope,
        query: SessionSearchQuery,
    ) -> Result<SessionSearchPage, SessionSearchError>;
}

/// Host-constructed authorization namespace. It is deliberately separate from user input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSearchScope {
    namespace: String,
    policy_fingerprint: String,
}

impl SessionSearchScope {
    /// Construct a scope from a host-owned namespace and policy revision.
    #[must_use]
    pub fn new(namespace: impl Into<String>, policy_fingerprint: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            policy_fingerprint: policy_fingerprint.into(),
        }
    }

    /// Construct the conventional single-user local-store scope.
    #[must_use]
    pub fn local(namespace: impl Into<String>) -> Self {
        Self::new(namespace, "local-user-visible-v1")
    }

    /// Return a non-reversible digest suitable for cursor and mutation bindings.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        digest_parts(&[&self.namespace, &self.policy_fingerprint])
    }
}

/// Text interpretation mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchQueryMode {
    /// Case-insensitive literal lexical matching.
    #[default]
    Literal,
    /// Tokenizer-defined phrase matching.
    Phrase,
    /// Prefix matching.
    Prefix,
    /// Semantic similarity matching.
    Semantic,
}

/// Searchable projection family.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchSource {
    /// Session title and approved typed metadata.
    SessionMetadata,
    /// Canonical textual input parts.
    RunInput,
    /// Bounded persisted run output preview.
    RunOutputPreview,
    /// Approved user-visible display text.
    DisplayMessage,
}

impl SessionSearchSource {
    /// Return all baseline source families.
    #[must_use]
    pub fn baseline() -> BTreeSet<Self> {
        BTreeSet::from([
            Self::SessionMetadata,
            Self::RunInput,
            Self::RunOutputPreview,
            Self::DisplayMessage,
        ])
    }
}

/// Result grouping level.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchGranularity {
    /// Return one best hit per session.
    #[default]
    Session,
    /// Return one best hit per session/run pair.
    Run,
    /// Return every stable projected occurrence.
    Occurrence,
}

/// Stable ordering requested by the caller.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchSort {
    /// Provider selects relevance for text and updated time for browsing.
    #[default]
    Auto,
    /// Relevance descending with stable updated/identity tie breakers.
    Relevance,
    /// Session update time descending, then session id descending.
    UpdatedDesc,
}

/// Display visibility classes that a host may permit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchVisibility {
    /// Ordinary user-visible display text.
    Public,
    /// Diagnostic text, only when a host explicitly permits it.
    Diagnostic,
    /// Internal text, only when a host explicitly permits it.
    Internal,
}

/// Inclusive lower / exclusive upper timestamp range.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchTimeRange {
    /// Inclusive lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    /// Exclusive upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
}

/// Typed filters for search and metadata browsing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchFilter {
    /// Allowed session statuses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_statuses: Vec<SessionStatus>,
    /// Allowed run statuses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run_statuses: Vec<RunStatus>,
    /// Exact profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Exact workspace display value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Session creation range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<SessionSearchTimeRange>,
    /// Session update range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<SessionSearchTimeRange>,
    /// Explicit session id allowlist.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub session_ids: BTreeSet<SessionId>,
    /// Host-approved display visibility classes.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub display_visibilities: BTreeSet<SessionSearchVisibility>,
}

/// A bounded search request. `text` is never a regular expression or shell expression.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchQuery {
    /// Optional literal text. `None` performs metadata browsing.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "query")]
    pub text: Option<String>,
    /// Text interpretation mode.
    #[serde(default)]
    pub mode: SessionSearchQueryMode,
    /// Typed filters.
    #[serde(default, alias = "filters")]
    pub filter: SessionSearchFilter,
    /// Projection families. Empty means provider defaults.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sources: BTreeSet<SessionSearchSource>,
    /// Result grouping level.
    #[serde(default)]
    pub granularity: SessionSearchGranularity,
    /// Stable ordering.
    #[serde(default)]
    pub sort: SessionSearchSort,
    /// Maximum hits requested.
    pub limit: u32,
    /// Opaque provider cursor.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "after")]
    pub cursor: Option<String>,
}

impl Default for SessionSearchQuery {
    fn default() -> Self {
        Self {
            text: None,
            mode: SessionSearchQueryMode::Literal,
            filter: SessionSearchFilter::default(),
            sources: SessionSearchSource::baseline(),
            granularity: SessionSearchGranularity::Session,
            sort: SessionSearchSort::Auto,
            limit: 20,
            cursor: None,
        }
    }
}

impl SessionSearchQuery {
    /// Return a stable fingerprint that intentionally excludes the pagination cursor.
    ///
    /// # Errors
    /// Returns an invalid-query error if the typed query cannot be serialized.
    pub fn fingerprint(&self) -> Result<String, SessionSearchError> {
        let mut normalized = self.clone();
        normalized.cursor = None;
        if let Some(text) = normalized.text.as_mut() {
            *text = text.trim().to_lowercase();
        }
        let bytes = serde_json::to_vec(&normalized)
            .map_err(|error| SessionSearchError::InvalidQuery(error.to_string()))?;
        Ok(hex_digest(&bytes))
    }
}

/// Filter vocabulary advertised by a provider.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchFilterKind {
    /// Session status.
    SessionStatus,
    /// Run status.
    RunStatus,
    /// Profile.
    Profile,
    /// Workspace.
    Workspace,
    /// Created timestamp range.
    CreatedTime,
    /// Updated timestamp range.
    UpdatedTime,
    /// Explicit session ids.
    SessionIds,
    /// Display visibility policy.
    DisplayVisibility,
}

/// Search consistency behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchConsistency {
    /// Reads canonical state directly.
    ReadThrough,
    /// A materialized index is updated in the canonical transaction.
    TransactionalIndex,
    /// An asynchronous index may lag canonical state.
    EventualIndex,
}

/// Provider capability advertisement.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchCapabilities {
    /// Stable provider kind, not an endpoint or topology description.
    pub provider: String,
    /// Supported text modes.
    pub query_modes: BTreeSet<SessionSearchQueryMode>,
    /// Searchable projection families.
    pub sources: BTreeSet<SessionSearchSource>,
    /// Supported typed filters.
    pub filters: BTreeSet<SessionSearchFilterKind>,
    /// Supported grouping levels.
    pub granularities: BTreeSet<SessionSearchGranularity>,
    /// Supported orderings.
    pub sorts: BTreeSet<SessionSearchSort>,
    /// Whether occurrence provenance is returned.
    pub occurrence_locations: bool,
    /// Whether snippets are returned.
    pub snippets: bool,
    /// Whether relevance scores are returned.
    pub scores: bool,
    /// Whether freshness watermarks are returned.
    pub freshness_watermarks: bool,
    /// Largest accepted page size.
    pub max_page_size: u32,
    /// Consistency behavior.
    pub consistency: SessionSearchConsistency,
}

/// Completeness state for one page.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchCoverageState {
    /// Every requested source in the provider corpus was searched.
    Complete,
    /// Search succeeded against an index that may lag canonical writes.
    EventuallyConsistent,
    /// One or more requested sources were unavailable or bounded out.
    Partial,
    /// A declared fallback produced the page after primary failure.
    Degraded,
}

/// Safe warning category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchWarningKind {
    /// A compatibility display mirror was absent.
    MissingSource,
    /// A source could not be parsed.
    MalformedSource,
    /// A configured resource bound was reached.
    LimitReached,
    /// A source was unreadable or outside policy.
    UnavailableSource,
    /// A best-effort source has no authoritative freshness proof.
    UnverifiedSource,
    /// Results came from an explicit fallback.
    Fallback,
}

/// Safe coverage warning. Messages must not contain backend paths or indexed content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchWarning {
    /// Warning category.
    pub kind: SessionSearchWarningKind,
    /// Safe bounded explanation.
    pub message: String,
}

/// Coverage and freshness for every returned page, including empty pages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchCoverage {
    /// Overall state.
    pub state: SessionSearchCoverageState,
    /// Sources actually evaluated.
    pub searched_sources: BTreeSet<SessionSearchSource>,
    /// Requested sources that could not be evaluated.
    pub unavailable_sources: BTreeSet<SessionSearchSource>,
    /// Eventual-index watermark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_through: Option<DateTime<Utc>>,
    /// Opaque provider generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    /// Safe warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<SessionSearchWarning>,
}

/// Minimal canonical session projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchSummary {
    /// Session id.
    pub session_id: SessionId,
    /// User-facing title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Session status.
    pub status: SessionStatus,
    /// Profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Safe workspace display value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Optional matching run status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_status: Option<RunStatus>,
    /// Optional bounded matching run preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_preview: Option<String>,
}

/// Highlight range using UTF-8 byte offsets into the returned snippet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHighlight {
    /// Inclusive byte offset.
    pub start: usize,
    /// Exclusive byte offset.
    pub end: usize,
}

/// Bounded plain-text snippet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchSnippet {
    /// Plain text only.
    pub text: String,
    /// Match ranges into `text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<SessionSearchHighlight>,
}

/// Match provenance. Archive scope and source run identity remain distinct.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchLocation {
    /// Owning session.
    pub session_id: SessionId,
    /// Canonical run owning the projected document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Projection family.
    pub source: SessionSearchSource,
    /// Archive scope for display evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_scope: Option<ReplayScope>,
    /// Source agent for a display occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent_id: Option<AgentId>,
    /// Source run carried by a display message; it can differ from the archive owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_run_id: Option<RunId>,
    /// Display sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_sequence: Option<usize>,
    /// Family-aware display cursor when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
    /// Opaque stable occurrence identity.
    pub document_id: String,
}

/// One discovery hit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHit {
    /// Minimal canonical session projection.
    pub session: SessionSearchSummary,
    /// Matching run, always interpreted together with the session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Matching projection family.
    pub source: SessionSearchSource,
    /// Stable match provenance.
    pub location: SessionSearchLocation,
    /// Optional bounded plain-text snippet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<SessionSearchSnippet>,
    /// Provider-local relevance score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Source timestamp when meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_at: Option<DateTime<Utc>>,
}

/// One page of discovery results.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchPage {
    /// Hits in stable provider order.
    pub hits: Vec<SessionSearchHit>,
    /// Opaque cursor for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Completeness and freshness.
    pub coverage: SessionSearchCoverage,
}

/// Required search error categories.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SessionSearchError {
    /// Malformed or incompatible query.
    #[error("invalid search query: {0}")]
    InvalidQuery(String),
    /// Expired or incorrectly bound cursor.
    #[error("invalid search cursor: {0}")]
    InvalidCursor(String),
    /// Provider does not advertise the requested behavior.
    #[error("unsupported search capability: {0}")]
    Unsupported(String),
    /// Installed provider cannot currently serve the request.
    #[error("session search unavailable: {0}")]
    Unavailable(String),
    /// Scope is not permitted.
    #[error("session search permission denied")]
    PermissionDenied,
    /// Safe bounded internal failure.
    #[error("session search failed: {0}")]
    Failed(String),
}

impl SessionSearchError {
    /// Return the stable wire category.
    #[must_use]
    pub const fn category(&self) -> &'static str {
        match self {
            Self::InvalidQuery(_) => "invalid_query",
            Self::InvalidCursor(_) => "invalid_cursor",
            Self::Unsupported(_) => "unsupported",
            Self::Unavailable(_) => "unavailable",
            Self::PermissionDenied => "permission_denied",
            Self::Failed(_) => "failed",
        }
    }
}

/// Cursor payload shared by conforming providers. Callers treat its encoded form as opaque.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchCursorBinding {
    /// Cursor format version.
    pub version: u32,
    /// Stable provider id.
    pub provider: String,
    /// Normalized query fingerprint.
    pub query_fingerprint: String,
    /// Authorization-scope fingerprint.
    pub scope_fingerprint: String,
    /// Provider corpus/index generation.
    pub generation: String,
    /// Next stable ordered offset.
    pub offset: usize,
    /// Last stable identity, for diagnostics and additional tie binding.
    pub last_identity: Option<String>,
}

/// Keyed cursor encoder/validator. Encoded cursors contain no paths or backend row ids.
#[derive(Clone, Debug)]
pub struct SessionSearchCursorCodec {
    key: Vec<u8>,
}

impl SessionSearchCursorCodec {
    /// Build a codec from host/provider secret material.
    #[must_use]
    pub fn new(key: impl AsRef<[u8]>) -> Self {
        Self {
            key: key.as_ref().to_vec(),
        }
    }

    /// Encode and authenticate a cursor binding.
    ///
    /// # Errors
    /// Returns a failed error if serialization unexpectedly fails.
    pub fn encode(
        &self,
        binding: &SessionSearchCursorBinding,
    ) -> Result<String, SessionSearchError> {
        let payload = serde_json::to_vec(binding)
            .map_err(|error| SessionSearchError::Failed(error.to_string()))?;
        let signature = self.signature(&payload);
        Ok(format!("ssc1.{}.{}", hex_encode(&payload), signature))
    }

    /// Decode and authenticate an opaque cursor.
    ///
    /// # Errors
    /// Returns `invalid_cursor` for malformed or unauthenticated input.
    pub fn decode(&self, cursor: &str) -> Result<SessionSearchCursorBinding, SessionSearchError> {
        let mut parts = cursor.split('.');
        if parts.next() != Some("ssc1") {
            return Err(SessionSearchError::InvalidCursor(
                "unknown cursor format".to_string(),
            ));
        }
        let payload = parts
            .next()
            .and_then(hex_decode)
            .ok_or_else(|| SessionSearchError::InvalidCursor("malformed cursor".to_string()))?;
        let signature = parts
            .next()
            .filter(|_| parts.next().is_none())
            .ok_or_else(|| SessionSearchError::InvalidCursor("malformed cursor".to_string()))?;
        if self.signature(&payload) != signature {
            return Err(SessionSearchError::InvalidCursor(
                "cursor authentication failed".to_string(),
            ));
        }
        serde_json::from_slice(&payload)
            .map_err(|_| SessionSearchError::InvalidCursor("malformed cursor payload".to_string()))
    }

    fn signature(&self, payload: &[u8]) -> String {
        let mut digest = Sha256::new();
        digest.update(b"starweaver.session-search.cursor.v1\0");
        digest.update(&self.key);
        digest.update([0]);
        digest.update(payload);
        hex_encode(&digest.finalize())
    }
}

/// Redacted document accepted by optional index writers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchDocument {
    /// Stable opaque document id.
    pub document_id: String,
    /// Owning session.
    pub session_id: SessionId,
    /// Optional owning run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Projection family.
    pub source: SessionSearchSource,
    /// Already-redacted text.
    pub text: String,
    /// Source content digest.
    pub content_digest: String,
    /// Source update time.
    pub updated_at: DateTime<Utc>,
}

/// Idempotent index operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchMutationOperation {
    /// Upsert a session projection.
    UpsertSession,
    /// Upsert a run projection.
    UpsertRun,
    /// Upsert a display occurrence or compact batch.
    UpsertDisplay,
    /// Delete a run projection.
    DeleteRun,
    /// Tombstone an entire session.
    TombstoneSession,
    /// Declare a new generation/rebuild boundary.
    ResetGeneration,
}

/// Versioned, idempotent search-index mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchMutation {
    /// Globally stable event id within the source namespace.
    pub event_id: String,
    /// Monotonic source revision. Older revisions must never replace newer ones.
    pub source_revision: u64,
    /// Projection/redaction schema revision.
    pub projection_version: String,
    /// Operation.
    pub operation: SessionSearchMutationOperation,
    /// Host authorization-scope fingerprint, never a caller-selected tenant id.
    pub scope_fingerprint: String,
    /// Redacted document for upserts; omitted for tombstones/resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<SessionSearchDocument>,
    /// Session targeted by delete/reset operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Run targeted by a run delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
}

/// Durable writer acknowledgement/watermark.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchCheckpoint {
    /// Accepted generation.
    pub generation: String,
    /// Last durably accepted event id.
    pub event_id: String,
    /// Indexed-through watermark when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_through: Option<DateTime<Utc>>,
}

/// Optional mutation target used by local or external indexes.
#[async_trait]
pub trait SessionSearchIndexWriter: Send + Sync {
    /// Apply mutations idempotently and durably.
    async fn apply(
        &self,
        mutations: &[SessionSearchMutation],
    ) -> Result<SessionSearchCheckpoint, SessionSearchIndexError>;
}

/// Index-publication errors.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SessionSearchIndexError {
    /// Mutation is invalid or conflicts with a stable event identity.
    #[error("invalid session search mutation: {0}")]
    InvalidMutation(String),
    /// Index is temporarily unavailable and the durable outbox should retry.
    #[error("session search index unavailable: {0}")]
    Unavailable(String),
    /// Safe bounded failure.
    #[error("session search index failed: {0}")]
    Failed(String),
}

fn digest_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    hex_encode(&digest.finalize())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex_encode(&digest.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            u8::try_from((high << 4) | low).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{collections::BTreeMap, sync::Mutex};

    use super::*;

    #[test]
    fn cursor_codec_authenticates_all_normative_bindings() {
        let codec = SessionSearchCursorCodec::new("test-secret");
        let binding = SessionSearchCursorBinding {
            version: 1,
            provider: "fake".to_string(),
            query_fingerprint: "query-a".to_string(),
            scope_fingerprint: "scope-a".to_string(),
            generation: "generation-a".to_string(),
            offset: 20,
            last_identity: Some("document-a".to_string()),
        };
        let encoded = codec.encode(&binding).expect("encode cursor");
        assert!(!encoded.contains("document-a"));
        assert_eq!(codec.decode(&encoded).expect("decode cursor"), binding);

        let mut tampered = encoded.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'a' { b'b' } else { b'a' };
        assert!(matches!(
            codec.decode(std::str::from_utf8(&tampered).expect("utf8")),
            Err(SessionSearchError::InvalidCursor(_))
        ));
    }

    #[test]
    fn query_fingerprint_normalizes_text_and_excludes_cursor() {
        let mut first = SessionSearchQuery {
            text: Some("  OAuth Refresh ".to_string()),
            cursor: Some("cursor-a".to_string()),
            ..SessionSearchQuery::default()
        };
        let second = SessionSearchQuery {
            text: Some("oauth refresh".to_string()),
            cursor: Some("cursor-b".to_string()),
            ..SessionSearchQuery::default()
        };
        assert_eq!(
            first.fingerprint().expect("fingerprint"),
            second.fingerprint().expect("fingerprint")
        );
        first.filter.profile = Some("coding".to_string());
        assert_ne!(
            first.fingerprint().expect("fingerprint"),
            second.fingerprint().expect("fingerprint")
        );
    }

    struct ConformanceProvider {
        response: Result<SessionSearchPage, SessionSearchError>,
    }

    #[async_trait]
    impl SessionSearchProvider for ConformanceProvider {
        fn capabilities(&self) -> SessionSearchCapabilities {
            SessionSearchCapabilities {
                provider: "fake".to_string(),
                query_modes: BTreeSet::from([SessionSearchQueryMode::Literal]),
                sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                filters: BTreeSet::new(),
                granularities: BTreeSet::from([SessionSearchGranularity::Session]),
                sorts: BTreeSet::from([SessionSearchSort::Auto]),
                occurrence_locations: false,
                snippets: false,
                scores: false,
                freshness_watermarks: false,
                max_page_size: 20,
                consistency: SessionSearchConsistency::ReadThrough,
            }
        }

        async fn search(
            &self,
            _scope: &SessionSearchScope,
            _query: SessionSearchQuery,
        ) -> Result<SessionSearchPage, SessionSearchError> {
            self.response.clone()
        }
    }

    fn empty_page(state: SessionSearchCoverageState) -> SessionSearchPage {
        SessionSearchPage {
            hits: Vec::new(),
            next_cursor: None,
            coverage: SessionSearchCoverage {
                state,
                searched_sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                unavailable_sources: BTreeSet::new(),
                indexed_through: None,
                generation: Some("fake-generation".to_string()),
                warnings: Vec::new(),
            },
        }
    }

    #[tokio::test]
    async fn fake_provider_distinguishes_empty_coverage_and_service_errors() {
        let scope = SessionSearchScope::local("fake");
        for state in [
            SessionSearchCoverageState::Complete,
            SessionSearchCoverageState::Partial,
            SessionSearchCoverageState::Degraded,
            SessionSearchCoverageState::EventuallyConsistent,
        ] {
            let provider = ConformanceProvider {
                response: Ok(empty_page(state)),
            };
            let page = provider
                .search(&scope, SessionSearchQuery::default())
                .await
                .expect("fake page");
            assert!(page.hits.is_empty());
            assert_eq!(page.coverage.state, state);
        }
        for error in [
            SessionSearchError::Unsupported("mode".to_string()),
            SessionSearchError::Unavailable("index".to_string()),
        ] {
            let provider = ConformanceProvider {
                response: Err(error.clone()),
            };
            assert_eq!(
                provider.search(&scope, SessionSearchQuery::default()).await,
                Err(error)
            );
        }
    }

    #[derive(Default)]
    struct ConformanceWriter {
        revisions: Mutex<BTreeMap<String, (u64, Option<SessionSearchDocument>)>>,
    }

    #[async_trait]
    impl SessionSearchIndexWriter for ConformanceWriter {
        async fn apply(
            &self,
            mutations: &[SessionSearchMutation],
        ) -> Result<SessionSearchCheckpoint, SessionSearchIndexError> {
            let mut revisions = self.revisions.lock().expect("lock revisions");
            for mutation in mutations {
                let key = mutation
                    .session_id
                    .as_ref()
                    .map_or_else(
                        || {
                            mutation
                                .document
                                .as_ref()
                                .map_or("generation", |document| document.session_id.as_str())
                        },
                        SessionId::as_str,
                    )
                    .to_string();
                let current = revisions.get(&key).map_or(0, |(revision, _)| *revision);
                if mutation.source_revision < current {
                    continue;
                }
                let document = match mutation.operation {
                    SessionSearchMutationOperation::TombstoneSession
                    | SessionSearchMutationOperation::DeleteRun => None,
                    _ => mutation.document.clone(),
                };
                revisions.insert(key, (mutation.source_revision, document));
            }
            Ok(SessionSearchCheckpoint {
                generation: "fake-generation".to_string(),
                event_id: mutations
                    .last()
                    .map_or("none", |mutation| mutation.event_id.as_str())
                    .to_string(),
                indexed_through: Some(Utc::now()),
            })
        }
    }

    #[tokio::test]
    async fn writer_conformance_prevents_delayed_upsert_resurrection() {
        let writer = ConformanceWriter::default();
        let session_id = SessionId::from_string("session_writer");
        let document = SessionSearchDocument {
            document_id: "doc".to_string(),
            session_id: session_id.clone(),
            run_id: None,
            source: SessionSearchSource::SessionMetadata,
            text: "safe projection".to_string(),
            content_digest: "digest".to_string(),
            updated_at: Utc::now(),
        };
        let upsert = SessionSearchMutation {
            event_id: "upsert-1".to_string(),
            source_revision: 1,
            projection_version: "v1".to_string(),
            operation: SessionSearchMutationOperation::UpsertSession,
            scope_fingerprint: "scope".to_string(),
            document: Some(document.clone()),
            session_id: Some(session_id.clone()),
            run_id: None,
        };
        let tombstone = SessionSearchMutation {
            event_id: "delete-2".to_string(),
            source_revision: 2,
            operation: SessionSearchMutationOperation::TombstoneSession,
            document: None,
            ..upsert.clone()
        };
        writer.apply(&[tombstone]).await.expect("tombstone");
        writer.apply(&[upsert]).await.expect("delayed upsert");
        let revisions = writer.revisions.lock().expect("lock revisions");
        assert_eq!(revisions[session_id.as_str()], (2, None));
    }

    #[test]
    fn scope_fingerprint_hides_namespace_and_policy() {
        let scope = SessionSearchScope::new("tenant-secret", "policy-secret");
        let fingerprint = scope.fingerprint();
        assert!(!fingerprint.contains("tenant-secret"));
        assert!(!fingerprint.contains("policy-secret"));
        assert_ne!(
            fingerprint,
            SessionSearchScope::new("other", "policy-secret").fingerprint()
        );
    }
}
