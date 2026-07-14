//! Bounded local session search over canonical records and validated display mirrors.

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use starweaver_core::RunId;
use starweaver_session::{
    InputPart, RunRecord, SessionFilter, SessionRecord, SessionSearchCapabilities,
    SessionSearchConsistency, SessionSearchCoverage, SessionSearchCoverageState,
    SessionSearchCursorBinding, SessionSearchCursorCodec, SessionSearchError,
    SessionSearchFilterKind, SessionSearchGranularity, SessionSearchHighlight, SessionSearchHit,
    SessionSearchLocation, SessionSearchPage, SessionSearchProvider, SessionSearchQuery,
    SessionSearchQueryMode, SessionSearchScope, SessionSearchSnippet, SessionSearchSort,
    SessionSearchSource, SessionSearchSummary, SessionSearchVisibility, SessionSearchWarning,
    SessionSearchWarningKind, SessionStore,
};
use starweaver_stream::{
    DisplayMessage, DisplayMessageKind, DisplayVisibility, ReplayCursor, ReplayScope,
    ReplaySnapshot,
};

/// Resource bounds for local metadata and display-file discovery.
#[derive(Clone, Debug)]
pub struct LocalSessionSearchLimits {
    /// Maximum UTF-8 query bytes.
    pub max_query_bytes: usize,
    /// Maximum accepted page size.
    pub max_page_size: u32,
    /// Maximum candidate sessions loaded from canonical storage.
    pub max_candidate_sessions: usize,
    /// Maximum candidate runs loaded across sessions.
    pub max_candidate_runs: usize,
    /// Maximum display files read.
    pub max_display_files: usize,
    /// Maximum bytes read from one display file.
    pub max_file_bytes: u64,
    /// Maximum aggregate display bytes read.
    pub max_total_display_bytes: u64,
    /// Maximum projected display candidates retained before ranking.
    pub max_display_hits: usize,
    /// Maximum wall time for one compatibility-mirror scan.
    pub max_scan_duration: Duration,
    /// Maximum returned snippet bytes.
    pub max_snippet_bytes: usize,
}

impl Default for LocalSessionSearchLimits {
    fn default() -> Self {
        Self {
            max_query_bytes: 4 * 1024,
            max_page_size: 100,
            max_candidate_sessions: 2_000,
            max_candidate_runs: 10_000,
            max_display_files: 1_000,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_display_bytes: 64 * 1024 * 1024,
            max_display_hits: 10_000,
            max_scan_duration: Duration::from_secs(2),
            max_snippet_bytes: 320,
        }
    }
}

/// Read-through local provider. SQLite-backed stores are canonical; display files are best effort.
#[derive(Clone)]
pub struct LocalSessionSearchProvider {
    store: Arc<dyn SessionStore>,
    scope_fingerprint: String,
    display_root: Option<PathBuf>,
    limits: LocalSessionSearchLimits,
    provider_generation: String,
    cursor_codec: SessionSearchCursorCodec,
}

impl std::fmt::Debug for LocalSessionSearchProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalSessionSearchProvider")
            .field("display_enabled", &self.display_root.is_some())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl LocalSessionSearchProvider {
    /// Build a metadata/input/output provider for one host-owned local scope.
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, scope: &SessionSearchScope) -> Self {
        let scope_fingerprint = scope.fingerprint();
        let generation = format!("local-v1-{}", &scope_fingerprint[..16]);
        let cursor_codec = SessionSearchCursorCodec::new(format!(
            "starweaver-local-session-search:{scope_fingerprint}"
        ));
        Self {
            store,
            scope_fingerprint,
            display_root: None,
            limits: LocalSessionSearchLimits::default(),
            provider_generation: generation,
            cursor_codec,
        }
    }

    /// Enable best-effort scanning of compatibility display mirrors below this root.
    #[must_use]
    pub fn with_display_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.display_root = Some(root.into());
        self
    }

    /// Override local resource bounds.
    #[must_use]
    pub fn with_limits(mut self, limits: LocalSessionSearchLimits) -> Self {
        self.limits = limits;
        self
    }

    fn validate_query(&self, query: &mut SessionSearchQuery) -> Result<(), SessionSearchError> {
        if query.limit == 0 || query.limit > self.limits.max_page_size {
            return Err(SessionSearchError::InvalidQuery(format!(
                "limit must be between 1 and {}",
                self.limits.max_page_size
            )));
        }
        if query.mode != SessionSearchQueryMode::Literal {
            return Err(SessionSearchError::Unsupported(format!(
                "query mode {:?}",
                query.mode
            )));
        }
        if let Some(text) = query.text.as_mut() {
            *text = text.trim().to_string();
            if text.is_empty() {
                query.text = None;
            } else if text.len() > self.limits.max_query_bytes {
                return Err(SessionSearchError::InvalidQuery(
                    "query text exceeds local byte limit".to_string(),
                ));
            }
        }
        let capabilities = self.capabilities();
        if query.sources.is_empty() {
            query.sources.clone_from(&capabilities.sources);
        }
        if let Some(source) = query
            .sources
            .iter()
            .find(|source| !capabilities.sources.contains(source))
        {
            return Err(SessionSearchError::Unsupported(format!(
                "source {source:?}"
            )));
        }
        if !capabilities.granularities.contains(&query.granularity) {
            return Err(SessionSearchError::Unsupported(format!(
                "granularity {:?}",
                query.granularity
            )));
        }
        if !capabilities.sorts.contains(&query.sort) {
            return Err(SessionSearchError::Unsupported(format!(
                "sort {:?}",
                query.sort
            )));
        }
        if query
            .filter
            .display_visibilities
            .iter()
            .any(|visibility| *visibility != SessionSearchVisibility::Public)
        {
            return Err(SessionSearchError::Unsupported(
                "local default projection searches public display text only".to_string(),
            ));
        }
        validate_range(query.filter.created.as_ref())?;
        validate_range(query.filter.updated.as_ref())?;
        Ok(())
    }

    async fn canonical_candidates(
        &self,
        query: &SessionSearchQuery,
    ) -> Result<Vec<(SessionRecord, Vec<RunRecord>)>, SessionSearchError> {
        let sessions = self
            .store
            .list_sessions(SessionFilter {
                profile: query.filter.profile.clone(),
                workspace: query.filter.workspace.clone(),
                limit: Some(self.limits.max_candidate_sessions.saturating_add(1)),
                ..SessionFilter::default()
            })
            .await
            .map_err(store_error)?;
        if sessions.len() > self.limits.max_candidate_sessions {
            return Err(SessionSearchError::Unavailable(
                "local candidate session limit exceeded; narrow the filters".to_string(),
            ));
        }
        let mut candidates = Vec::new();
        let mut run_count = 0usize;
        for session in sessions {
            if !session_matches(&session, query) {
                continue;
            }
            let mut runs = self
                .store
                .list_runs(&session.session_id)
                .await
                .map_err(store_error)?;
            run_count = run_count.saturating_add(runs.len());
            if run_count > self.limits.max_candidate_runs {
                return Err(SessionSearchError::Unavailable(
                    "local candidate run limit exceeded; narrow the filters".to_string(),
                ));
            }
            if !query.filter.run_statuses.is_empty() {
                runs.retain(|run| query.filter.run_statuses.contains(&run.status));
                if runs.is_empty() {
                    continue;
                }
            }
            candidates.push((session, runs));
        }
        Ok(candidates)
    }

    fn metadata_browse_hits(
        candidates: &[(SessionRecord, Vec<RunRecord>)],
    ) -> Vec<SessionSearchHit> {
        candidates
            .iter()
            .map(|(session, runs)| {
                let run = runs.last();
                build_hit(
                    session,
                    run,
                    SessionSearchSource::SessionMetadata,
                    format!("session:{}", session.session_id.as_str()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    session.updated_at,
                )
            })
            .collect()
    }

    fn record_hits(
        &self,
        candidates: &[(SessionRecord, Vec<RunRecord>)],
        query: &SessionSearchQuery,
        needle: &str,
    ) -> Vec<SessionSearchHit> {
        let mut hits = Vec::new();
        for (session, runs) in candidates {
            if query
                .sources
                .contains(&SessionSearchSource::SessionMetadata)
                && let Some(title) = session.title.as_deref()
                && let Some((score, snippet)) =
                    literal_match(title, needle, self.limits.max_snippet_bytes)
            {
                hits.push(build_hit(
                    session,
                    None,
                    SessionSearchSource::SessionMetadata,
                    format!("session:{}:title", session.session_id.as_str()),
                    Some(snippet),
                    Some(score),
                    None,
                    None,
                    None,
                    session.updated_at,
                ));
            }
            for run in runs {
                if query.sources.contains(&SessionSearchSource::RunInput) {
                    for (index, input) in run.input.iter().enumerate() {
                        let InputPart::Text { text, .. } = input else {
                            continue;
                        };
                        if let Some((score, snippet)) =
                            literal_match(text, needle, self.limits.max_snippet_bytes)
                        {
                            hits.push(build_hit(
                                session,
                                Some(run),
                                SessionSearchSource::RunInput,
                                format!(
                                    "session:{}:run:{}:input:{index}",
                                    session.session_id.as_str(),
                                    run.run_id.as_str()
                                ),
                                Some(snippet),
                                Some(score),
                                None,
                                None,
                                None,
                                run.created_at,
                            ));
                        }
                    }
                }
                if query
                    .sources
                    .contains(&SessionSearchSource::RunOutputPreview)
                    && let Some(preview) = run.output_preview.as_deref()
                    && let Some((score, snippet)) =
                        literal_match(preview, needle, self.limits.max_snippet_bytes)
                {
                    hits.push(build_hit(
                        session,
                        Some(run),
                        SessionSearchSource::RunOutputPreview,
                        format!(
                            "session:{}:run:{}:output",
                            session.session_id.as_str(),
                            run.run_id.as_str()
                        ),
                        Some(snippet),
                        Some(score),
                        None,
                        None,
                        None,
                        run.updated_at,
                    ));
                }
            }
        }
        hits
    }

    fn display_hits(
        &self,
        candidates: &[(SessionRecord, Vec<RunRecord>)],
        needle: &str,
        coverage: &mut SessionSearchCoverage,
    ) -> Vec<SessionSearchHit> {
        let Some(root) = self.display_root.as_deref() else {
            mark_display_unavailable(coverage, "display mirror root is not configured");
            return Vec::new();
        };
        let Ok(canonical_root) = root.canonicalize() else {
            mark_display_unavailable(coverage, "display mirror root is unavailable");
            return Vec::new();
        };
        let mut hits = Vec::new();
        let started = Instant::now();
        let mut files = 0usize;
        let mut total_bytes = 0u64;
        let mut found_file = false;
        let mut warned_missing = false;
        for (session, runs) in candidates {
            for run in runs {
                if files >= self.limits.max_display_files
                    || total_bytes >= self.limits.max_total_display_bytes
                    || hits.len() >= self.limits.max_display_hits
                    || started.elapsed() >= self.limits.max_scan_duration
                {
                    coverage.state = SessionSearchCoverageState::Partial;
                    coverage.warnings.push(SessionSearchWarning {
                        kind: SessionSearchWarningKind::LimitReached,
                        message: "local display scan limit reached".to_string(),
                    });
                    coverage
                        .unavailable_sources
                        .insert(SessionSearchSource::DisplayMessage);
                    return hits;
                }
                let Some(path) = validated_display_path(&canonical_root, session, run) else {
                    coverage.state = SessionSearchCoverageState::Partial;
                    coverage.warnings.push(SessionSearchWarning {
                        kind: SessionSearchWarningKind::UnavailableSource,
                        message: "a display mirror identity was not a safe path component"
                            .to_string(),
                    });
                    continue;
                };
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if !warned_missing {
                            coverage.state = SessionSearchCoverageState::Partial;
                            coverage.warnings.push(SessionSearchWarning {
                                kind: SessionSearchWarningKind::MissingSource,
                                message: "one or more compatibility display mirrors are missing"
                                    .to_string(),
                            });
                            warned_missing = true;
                        }
                        continue;
                    }
                    Err(_) => {
                        coverage.state = SessionSearchCoverageState::Partial;
                        coverage.warnings.push(SessionSearchWarning {
                            kind: SessionSearchWarningKind::UnavailableSource,
                            message: "a compatibility display mirror is unreadable".to_string(),
                        });
                        continue;
                    }
                };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.len() > self.limits.max_file_bytes
                    || total_bytes.saturating_add(metadata.len())
                        > self.limits.max_total_display_bytes
                {
                    coverage.state = SessionSearchCoverageState::Partial;
                    coverage.warnings.push(SessionSearchWarning {
                        kind: SessionSearchWarningKind::LimitReached,
                        message: "a display mirror was outside type or byte limits".to_string(),
                    });
                    continue;
                }
                let Ok(canonical_path) = path.canonicalize() else {
                    coverage.state = SessionSearchCoverageState::Partial;
                    continue;
                };
                if !canonical_path.starts_with(&canonical_root) {
                    coverage.state = SessionSearchCoverageState::Partial;
                    coverage.warnings.push(SessionSearchWarning {
                        kind: SessionSearchWarningKind::UnavailableSource,
                        message: "a display mirror escaped the configured root".to_string(),
                    });
                    continue;
                }
                files += 1;
                total_bytes = total_bytes.saturating_add(metadata.len());
                found_file = true;
                let Ok(bytes) = fs::read(&canonical_path) else {
                    coverage.state = SessionSearchCoverageState::Partial;
                    continue;
                };
                let Some(messages) = parse_display_messages(&bytes) else {
                    coverage.state = SessionSearchCoverageState::Partial;
                    coverage.warnings.push(SessionSearchWarning {
                        kind: SessionSearchWarningKind::MalformedSource,
                        message: "a compatibility display mirror could not be parsed".to_string(),
                    });
                    continue;
                };
                let mut identities = HashSet::new();
                for message in messages {
                    if hits.len() >= self.limits.max_display_hits
                        || started.elapsed() >= self.limits.max_scan_duration
                    {
                        coverage.state = SessionSearchCoverageState::Partial;
                        coverage
                            .searched_sources
                            .insert(SessionSearchSource::DisplayMessage);
                        coverage.warnings.push(SessionSearchWarning {
                            kind: SessionSearchWarningKind::LimitReached,
                            message: "local display scan result or time limit reached".to_string(),
                        });
                        return hits;
                    }
                    if message.session_id != session.session_id
                        || message.visibility != DisplayVisibility::Public
                        || !approved_display_kind(message.kind)
                    {
                        continue;
                    }
                    let Some(text) = message.preview.as_deref() else {
                        continue;
                    };
                    let document_id = format!(
                        "session:{}:archive-run:{}:display:{}:source-run:{}",
                        session.session_id.as_str(),
                        run.run_id.as_str(),
                        message.sequence,
                        message.run_id.as_str()
                    );
                    if !identities.insert(document_id.clone()) {
                        continue;
                    }
                    let Some((score, snippet)) =
                        literal_match(text, needle, self.limits.max_snippet_bytes)
                    else {
                        continue;
                    };
                    let cursor = ReplayCursor::display(
                        ReplayScope::run(run.run_id.as_str()),
                        message.sequence,
                    );
                    hits.push(build_hit(
                        session,
                        Some(run),
                        SessionSearchSource::DisplayMessage,
                        document_id,
                        Some(snippet),
                        Some(score),
                        Some(ReplayScope::run(run.run_id.as_str())),
                        Some(message),
                        Some(cursor),
                        run.updated_at,
                    ));
                }
            }
        }
        if found_file {
            coverage
                .searched_sources
                .insert(SessionSearchSource::DisplayMessage);
            coverage.state = SessionSearchCoverageState::Partial;
            coverage.warnings.push(SessionSearchWarning {
                kind: SessionSearchWarningKind::UnverifiedSource,
                message: "compatibility display mirrors are best-effort and may be stale"
                    .to_string(),
            });
        } else {
            coverage
                .unavailable_sources
                .insert(SessionSearchSource::DisplayMessage);
        }
        hits
    }

    fn corpus_generation(
        &self,
        candidates: &[(SessionRecord, Vec<RunRecord>)],
        query: &SessionSearchQuery,
    ) -> String {
        let mut digest = Sha256::new();
        digest.update(self.provider_generation.as_bytes());
        for (session, runs) in candidates {
            digest.update(session.session_id.as_str().as_bytes());
            digest.update(session.updated_at.to_rfc3339().as_bytes());
            digest.update(session.title.as_deref().unwrap_or_default().as_bytes());
            digest.update(session.profile.as_deref().unwrap_or_default().as_bytes());
            digest.update(session.workspace.as_deref().unwrap_or_default().as_bytes());
            digest.update(format!("{:?}", session.status).as_bytes());
            for run in runs {
                digest.update(run.run_id.as_str().as_bytes());
                digest.update(run.updated_at.to_rfc3339().as_bytes());
                digest.update(run.status.as_str().as_bytes());
                digest.update(run.output_preview.as_deref().unwrap_or_default().as_bytes());
                if query.sources.contains(&SessionSearchSource::RunInput) {
                    for input in &run.input {
                        if let InputPart::Text { text, .. } = input {
                            digest.update(text.as_bytes());
                        }
                    }
                }
                if query.sources.contains(&SessionSearchSource::DisplayMessage)
                    && let Some(root) = self.display_root.as_deref()
                    && let Ok(root) = root.canonicalize()
                    && let Some(path) = validated_display_path(&root, session, run)
                {
                    match fs::symlink_metadata(path) {
                        Ok(metadata) => {
                            digest.update(metadata.len().to_le_bytes());
                            if let Ok(modified) = metadata.modified()
                                && let Ok(elapsed) =
                                    modified.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            {
                                digest.update(elapsed.as_nanos().to_le_bytes());
                            }
                            digest.update([u8::from(metadata.file_type().is_symlink())]);
                        }
                        Err(_) => digest.update(b"missing-display-mirror"),
                    }
                }
            }
        }
        format!("local-v1-{:x}", digest.finalize())
    }

    fn cursor_binding(
        &self,
        query: &SessionSearchQuery,
        scope: &SessionSearchScope,
        generation: &str,
    ) -> Result<Option<SessionSearchCursorBinding>, SessionSearchError> {
        let Some(cursor) = query.cursor.as_deref() else {
            return Ok(None);
        };
        let binding = self.cursor_codec.decode(cursor)?;
        let expected_query = query.fingerprint()?;
        if binding.version != 1
            || binding.provider != "local"
            || binding.query_fingerprint != expected_query
            || binding.scope_fingerprint != scope.fingerprint()
            || binding.generation != generation
        {
            return Err(SessionSearchError::InvalidCursor(
                "cursor does not belong to this query, scope, provider, or generation".to_string(),
            ));
        }
        Ok(Some(binding))
    }
}

#[async_trait]
impl SessionSearchProvider for LocalSessionSearchProvider {
    fn capabilities(&self) -> SessionSearchCapabilities {
        let mut sources = BTreeSet::from([
            SessionSearchSource::SessionMetadata,
            SessionSearchSource::RunInput,
            SessionSearchSource::RunOutputPreview,
        ]);
        if self.display_root.is_some() {
            sources.insert(SessionSearchSource::DisplayMessage);
        }
        SessionSearchCapabilities {
            provider: "local".to_string(),
            query_modes: BTreeSet::from([SessionSearchQueryMode::Literal]),
            sources,
            filters: BTreeSet::from([
                SessionSearchFilterKind::SessionStatus,
                SessionSearchFilterKind::RunStatus,
                SessionSearchFilterKind::Profile,
                SessionSearchFilterKind::Workspace,
                SessionSearchFilterKind::CreatedTime,
                SessionSearchFilterKind::UpdatedTime,
                SessionSearchFilterKind::SessionIds,
                SessionSearchFilterKind::DisplayVisibility,
            ]),
            granularities: BTreeSet::from([
                SessionSearchGranularity::Session,
                SessionSearchGranularity::Run,
                SessionSearchGranularity::Occurrence,
            ]),
            sorts: BTreeSet::from([
                SessionSearchSort::Auto,
                SessionSearchSort::Relevance,
                SessionSearchSort::UpdatedDesc,
            ]),
            occurrence_locations: true,
            snippets: true,
            scores: true,
            freshness_watermarks: false,
            max_page_size: self.limits.max_page_size,
            consistency: SessionSearchConsistency::ReadThrough,
        }
    }

    async fn search(
        &self,
        scope: &SessionSearchScope,
        mut query: SessionSearchQuery,
    ) -> Result<SessionSearchPage, SessionSearchError> {
        if scope.fingerprint() != self.scope_fingerprint {
            return Err(SessionSearchError::PermissionDenied);
        }
        self.validate_query(&mut query)?;
        let candidates = self.canonical_candidates(&query).await?;
        let generation = self.corpus_generation(&candidates, &query);
        let cursor_binding = self.cursor_binding(&query, scope, &generation)?;
        let offset = cursor_binding.as_ref().map_or(0, |binding| binding.offset);
        let mut coverage = SessionSearchCoverage {
            state: SessionSearchCoverageState::Complete,
            searched_sources: BTreeSet::new(),
            unavailable_sources: BTreeSet::new(),
            indexed_through: None,
            generation: Some(generation.clone()),
            warnings: Vec::new(),
        };
        let mut hits = if let Some(text) = query.text.as_deref() {
            let needle = text.to_lowercase();
            let mut hits = self.record_hits(&candidates, &query, &needle);
            coverage.searched_sources.extend(
                query
                    .sources
                    .iter()
                    .copied()
                    .filter(|source| *source != SessionSearchSource::DisplayMessage),
            );
            if query.sources.contains(&SessionSearchSource::DisplayMessage) {
                let scanner = self.clone();
                let scan_candidates = candidates.clone();
                let scan_needle = needle.clone();
                let mut scan_coverage = coverage.clone();
                let (display_hits, updated_coverage) = tokio::task::spawn_blocking(move || {
                    let display_hits =
                        scanner.display_hits(&scan_candidates, &scan_needle, &mut scan_coverage);
                    (display_hits, scan_coverage)
                })
                .await
                .map_err(|_| {
                    SessionSearchError::Failed("local display scan task failed".to_string())
                })?;
                coverage = updated_coverage;
                hits.extend(display_hits);
            }
            hits
        } else {
            coverage
                .searched_sources
                .insert(SessionSearchSource::SessionMetadata);
            Self::metadata_browse_hits(&candidates)
        };
        sort_hits(&mut hits, query.text.is_some(), query.sort);
        hits = deduplicate(hits, query.granularity);
        if offset > hits.len()
            || cursor_binding.as_ref().is_some_and(|binding| {
                offset == 0
                    || hits
                        .get(offset - 1)
                        .map(|hit| hit.location.document_id.as_str())
                        != binding.last_identity.as_deref()
            })
        {
            return Err(SessionSearchError::InvalidCursor(
                "cursor sort position is no longer valid".to_string(),
            ));
        }
        let page_limit = query.limit as usize;
        let end = offset.saturating_add(page_limit).min(hits.len());
        let page_hits = hits[offset..end].to_vec();
        let next_cursor = if end < hits.len() {
            let binding = SessionSearchCursorBinding {
                version: 1,
                provider: "local".to_string(),
                query_fingerprint: query.fingerprint()?,
                scope_fingerprint: scope.fingerprint(),
                generation: generation.clone(),
                offset: end,
                last_identity: page_hits.last().map(|hit| hit.location.document_id.clone()),
            };
            Some(self.cursor_codec.encode(&binding)?)
        } else {
            None
        };
        coverage.warnings.truncate(16);
        Ok(SessionSearchPage {
            hits: page_hits,
            next_cursor,
            coverage,
        })
    }
}

fn validate_range(
    range: Option<&starweaver_session::SessionSearchTimeRange>,
) -> Result<(), SessionSearchError> {
    if let Some(range) = range
        && let (Some(from), Some(until)) = (range.from, range.until)
        && from >= until
    {
        return Err(SessionSearchError::InvalidQuery(
            "time range lower bound must precede upper bound".to_string(),
        ));
    }
    Ok(())
}

fn session_matches(session: &SessionRecord, query: &SessionSearchQuery) -> bool {
    (query.filter.session_statuses.is_empty()
        || query.filter.session_statuses.contains(&session.status))
        && (query.filter.session_ids.is_empty()
            || query.filter.session_ids.contains(&session.session_id))
        && in_range(session.created_at, query.filter.created.as_ref())
        && in_range(session.updated_at, query.filter.updated.as_ref())
}

fn in_range(
    value: chrono::DateTime<chrono::Utc>,
    range: Option<&starweaver_session::SessionSearchTimeRange>,
) -> bool {
    range.is_none_or(|range| {
        range.from.is_none_or(|from| value >= from) && range.until.is_none_or(|until| value < until)
    })
}

#[allow(clippy::too_many_arguments)]
fn build_hit(
    session: &SessionRecord,
    run: Option<&RunRecord>,
    source: SessionSearchSource,
    document_id: String,
    snippet: Option<SessionSearchSnippet>,
    score: Option<f64>,
    archive_scope: Option<ReplayScope>,
    display: Option<DisplayMessage>,
    cursor: Option<ReplayCursor>,
    matched_at: chrono::DateTime<chrono::Utc>,
) -> SessionSearchHit {
    let source_run_id = display.as_ref().map(|message| message.run_id.clone());
    let source_agent_id = display
        .as_ref()
        .and_then(|message| message.agent_id.clone());
    let display_sequence = display.as_ref().map(|message| message.sequence);
    let matched_at = display
        .as_ref()
        .map_or(matched_at, |message| message.timestamp);
    let document_id = opaque_document_id(&document_id);
    SessionSearchHit {
        session: SessionSearchSummary {
            session_id: session.session_id.clone(),
            title: session
                .title
                .as_deref()
                .map(|title| bounded_plain(title, 256)),
            status: session.status,
            profile: session
                .profile
                .as_deref()
                .map(|profile| bounded_plain(profile, 128)),
            workspace: session
                .workspace
                .as_deref()
                .map(|workspace| bounded_plain(workspace, 512)),
            created_at: session.created_at,
            updated_at: session.updated_at,
            run_status: run.map(|run| run.status),
            run_preview: run.and_then(|run| {
                run.output_preview
                    .as_deref()
                    .map(|preview| bounded_plain(preview, 160))
            }),
        },
        run_id: run.map(|run| run.run_id.clone()),
        source,
        location: SessionSearchLocation {
            session_id: session.session_id.clone(),
            run_id: run.map(|run| run.run_id.clone()),
            source,
            archive_scope,
            source_agent_id,
            source_run_id,
            display_sequence,
            cursor,
            document_id,
        },
        snippet,
        score,
        matched_at: Some(matched_at),
    }
}

fn opaque_document_id(identity: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"starweaver.session-search.document.v1\0");
    digest.update(identity.as_bytes());
    format!("doc1.{:x}", digest.finalize())
}

fn literal_match(
    text: &str,
    needle: &str,
    max_bytes: usize,
) -> Option<(f64, SessionSearchSnippet)> {
    let lowered = text.to_lowercase();
    let start = lowered.find(needle)?;
    let occurrences = lowered.match_indices(needle).count().max(1);
    let original_start = floor_boundary(text, start.min(text.len()));
    let original_end = ceil_boundary(text, start.saturating_add(needle.len()).min(text.len()));
    let context = max_bytes.saturating_sub(original_end.saturating_sub(original_start)) / 2;
    let snippet_start = floor_boundary(text, original_start.saturating_sub(context));
    let snippet_end = ceil_boundary(text, original_end.saturating_add(context).min(text.len()));
    let snippet_text = bounded_plain(&text[snippet_start..snippet_end], max_bytes);
    let highlight_start = original_start
        .saturating_sub(snippet_start)
        .min(snippet_text.len());
    let highlight_end = original_end
        .saturating_sub(snippet_start)
        .min(snippet_text.len());
    let score = f64::from(u32::try_from(occurrences).unwrap_or(u32::MAX));
    Some((
        score,
        SessionSearchSnippet {
            text: snippet_text,
            highlights: (highlight_start < highlight_end)
                .then_some(SessionSearchHighlight {
                    start: highlight_start,
                    end: highlight_end,
                })
                .into_iter()
                .collect(),
        },
    ))
}

fn bounded_plain(value: &str, max_bytes: usize) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if value.len() <= max_bytes {
        return value;
    }
    let end = floor_boundary(&value, max_bytes);
    value[..end].to_string()
}

fn floor_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn sort_hits(hits: &mut [SessionSearchHit], has_text: bool, sort: SessionSearchSort) {
    let relevance = matches!(sort, SessionSearchSort::Relevance)
        || (matches!(sort, SessionSearchSort::Auto) && has_text);
    hits.sort_by(|left, right| {
        if relevance {
            right
                .score
                .unwrap_or_default()
                .total_cmp(&left.score.unwrap_or_default())
                .then_with(|| right.session.updated_at.cmp(&left.session.updated_at))
                .then_with(|| left.location.document_id.cmp(&right.location.document_id))
        } else {
            right
                .session
                .updated_at
                .cmp(&left.session.updated_at)
                .then_with(|| {
                    right
                        .session
                        .session_id
                        .as_str()
                        .cmp(left.session.session_id.as_str())
                })
                .then_with(|| left.location.document_id.cmp(&right.location.document_id))
        }
    });
}

fn deduplicate(
    hits: Vec<SessionSearchHit>,
    granularity: SessionSearchGranularity,
) -> Vec<SessionSearchHit> {
    if granularity == SessionSearchGranularity::Occurrence {
        let mut documents = HashSet::new();
        return hits
            .into_iter()
            .filter(|hit| documents.insert(hit.location.document_id.clone()))
            .collect();
    }
    let mut groups = HashSet::new();
    hits.into_iter()
        .filter(|hit| {
            let group = match granularity {
                SessionSearchGranularity::Session => hit.session.session_id.as_str().to_string(),
                SessionSearchGranularity::Run => format!(
                    "{}:{}",
                    hit.session.session_id.as_str(),
                    hit.run_id.as_ref().map_or("", RunId::as_str)
                ),
                SessionSearchGranularity::Occurrence => unreachable!(),
            };
            groups.insert(group)
        })
        .collect()
}

fn validated_display_path(
    canonical_root: &Path,
    session: &SessionRecord,
    run: &RunRecord,
) -> Option<PathBuf> {
    if !safe_component(session.session_id.as_str()) || !safe_component(run.run_id.as_str()) {
        return None;
    }
    let path = canonical_root
        .join("sessions")
        .join(session.session_id.as_str())
        .join("runs")
        .join(run.run_id.as_str())
        .join("display.compact.json");
    if path
        .strip_prefix(canonical_root)
        .ok()?
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(path)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn parse_display_messages(bytes: &[u8]) -> Option<Vec<DisplayMessage>> {
    serde_json::from_slice::<ReplaySnapshot>(bytes)
        .map(|snapshot| snapshot.display_messages)
        .or_else(|_| serde_json::from_slice::<Vec<DisplayMessage>>(bytes))
        .ok()
}

fn approved_display_kind(kind: DisplayMessageKind) -> bool {
    matches!(
        kind,
        DisplayMessageKind::AssistantTextStart
            | DisplayMessageKind::AssistantTextDelta
            | DisplayMessageKind::AssistantTextEnd
            | DisplayMessageKind::RunCompleted
            | DisplayMessageKind::RunFailed
            | DisplayMessageKind::RunCancelled
    )
}

fn mark_display_unavailable(coverage: &mut SessionSearchCoverage, message: &str) {
    coverage.state = SessionSearchCoverageState::Partial;
    coverage
        .unavailable_sources
        .insert(SessionSearchSource::DisplayMessage);
    coverage.warnings.push(SessionSearchWarning {
        kind: SessionSearchWarningKind::UnavailableSource,
        message: message.to_string(),
    });
}

fn store_error(error: starweaver_session::SessionStoreError) -> SessionSearchError {
    SessionSearchError::Failed(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use starweaver_core::{ConversationId, Metadata, RunId, SessionId};
    use starweaver_session::{
        InputPart, RunRecord, SessionRecord, SessionSearchCoverageState, SessionSearchFilter,
        SessionSearchGranularity, SessionSearchProvider, SessionSearchQuery, SessionSearchScope,
        SessionSearchSource, SessionStore,
    };
    use starweaver_stream::{
        DisplayMessage, DisplayMessageKind, DisplayVisibility, ReplaySnapshot,
    };

    use super::*;
    use crate::SqliteSessionStore;

    async fn fixture(
        root: &Path,
    ) -> (
        Arc<SqliteSessionStore>,
        SessionSearchScope,
        LocalSessionSearchProvider,
        SessionId,
        RunId,
    ) {
        fs::create_dir_all(root).expect("create display root");
        let store = Arc::new(SqliteSessionStore::in_memory().expect("sqlite store"));
        let scope = SessionSearchScope::local("test-store");
        let provider = LocalSessionSearchProvider::new(store.clone(), &scope)
            .with_display_root(root.to_path_buf());
        let session_id = SessionId::from_string("session_search_one");
        let run_id = RunId::from_string("run_search_one");
        let mut session = SessionRecord::new(session_id.clone());
        session.title = Some("OAuth investigation".to_string());
        session.profile = Some("coding".to_string());
        session.workspace = Some("/workspace/project".to_string());
        session.metadata = Metadata::from_iter([(
            "secret".to_string(),
            json!("metadata-credential-never-indexed"),
        )]);
        store.save_session(session).await.expect("save session");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.input = vec![
            InputPart::text("literal [refresh]* and --hidden-option"),
            InputPart::ResourceRef {
                uri: "https://example.invalid/file?token=resource-secret".to_string(),
                media_type: "text/plain".to_string(),
                resource_type: "document".to_string(),
                resource_metadata: Metadata::default(),
                metadata: Metadata::default(),
            },
            InputPart::InlineBinary {
                data: b"inline-secret".to_vec(),
                media_type: "application/octet-stream".to_string(),
                metadata: Metadata::default(),
            },
        ];
        run.output_preview = Some("token refresh succeeded".to_string());
        run.structured_output = json!({"credential": "structured-secret"});
        store.append_run(run).await.expect("append run");
        (store, scope, provider, session_id, run_id)
    }

    #[tokio::test]
    async fn local_search_is_literal_and_excludes_opaque_projection_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_store, scope, provider, _, _) = fixture(temp.path()).await;
        for literal in ["[refresh]*", "--hidden-option"] {
            let page = provider
                .search(
                    &scope,
                    SessionSearchQuery {
                        text: Some(literal.to_string()),
                        sources: BTreeSet::from([SessionSearchSource::RunInput]),
                        granularity: SessionSearchGranularity::Occurrence,
                        ..SessionSearchQuery::default()
                    },
                )
                .await
                .expect("literal search");
            assert_eq!(page.hits.len(), 1, "literal query {literal}");
            assert!(page.hits[0].location.document_id.starts_with("doc1."));
            assert!(
                !page.hits[0]
                    .location
                    .document_id
                    .contains("session_search_one")
            );
        }
        for excluded in [
            "metadata-credential-never-indexed",
            "resource-secret",
            "inline-secret",
            "structured-secret",
        ] {
            let page = provider
                .search(
                    &scope,
                    SessionSearchQuery {
                        text: Some(excluded.to_string()),
                        sources: BTreeSet::from([SessionSearchSource::RunInput]),
                        ..SessionSearchQuery::default()
                    },
                )
                .await
                .expect("excluded projection search");
            assert!(page.hits.is_empty(), "must not index {excluded}");
        }
    }

    #[tokio::test]
    async fn cursor_is_bound_to_query_scope_provider_and_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (store, scope, provider, _, _) = fixture(temp.path()).await;
        let second_id = SessionId::from_string("session_search_two");
        let mut second = SessionRecord::new(second_id.clone());
        second.title = Some("OAuth second".to_string());
        store.save_session(second).await.expect("save second");
        let first = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("oauth".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                    limit: 1,
                    ..SessionSearchQuery::default()
                },
            )
            .await
            .expect("first page");
        let cursor = first.next_cursor.expect("next cursor");
        let next = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("oauth".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                    limit: 1,
                    cursor: Some(cursor.clone()),
                    ..SessionSearchQuery::default()
                },
            )
            .await
            .expect("next page");
        assert_eq!(next.hits.len(), 1);
        let mut changed = store
            .load_session(&second_id)
            .await
            .expect("load second session");
        changed.title = Some("OAuth changed generation".to_string());
        changed.updated_at = chrono::Utc::now();
        store.save_session(changed).await.expect("change corpus");
        let wrong_generation = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("oauth".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                    limit: 1,
                    cursor: Some(cursor.clone()),
                    ..SessionSearchQuery::default()
                },
            )
            .await;
        assert!(matches!(
            wrong_generation,
            Err(SessionSearchError::InvalidCursor(_))
        ));
        let wrong_query = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("different".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                    limit: 1,
                    cursor: Some(cursor.clone()),
                    ..SessionSearchQuery::default()
                },
            )
            .await;
        assert!(matches!(
            wrong_query,
            Err(SessionSearchError::InvalidCursor(_))
        ));
        let base_query = SessionSearchQuery {
            text: Some("oauth".to_string()),
            sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
            limit: 1,
            ..SessionSearchQuery::default()
        };
        let candidates = provider
            .canonical_candidates(&base_query)
            .await
            .expect("candidates");
        let correct_generation = provider.corpus_generation(&candidates, &base_query);
        for (bound_provider, generation) in [
            ("other", correct_generation.as_str()),
            ("local", "other-generation"),
        ] {
            let bound_cursor = provider
                .cursor_codec
                .encode(&SessionSearchCursorBinding {
                    version: 1,
                    provider: bound_provider.to_string(),
                    query_fingerprint: base_query.fingerprint().expect("fingerprint"),
                    scope_fingerprint: scope.fingerprint(),
                    generation: generation.to_string(),
                    offset: 1,
                    last_identity: None,
                })
                .expect("bound cursor");
            let result = provider
                .search(
                    &scope,
                    SessionSearchQuery {
                        cursor: Some(bound_cursor),
                        ..base_query.clone()
                    },
                )
                .await;
            assert!(matches!(result, Err(SessionSearchError::InvalidCursor(_))));
        }
        let wrong_scope = provider
            .search(
                &SessionSearchScope::local("other-store"),
                SessionSearchQuery {
                    text: Some("oauth".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::SessionMetadata]),
                    limit: 1,
                    cursor: Some(cursor),
                    ..SessionSearchQuery::default()
                },
            )
            .await;
        assert_eq!(wrong_scope, Err(SessionSearchError::PermissionDenied));
    }

    #[tokio::test]
    async fn display_search_parses_projection_and_reports_missing_mirrors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_store, scope, provider, session_id, run_id) = fixture(temp.path()).await;
        let run_dir = temp
            .path()
            .join("sessions")
            .join(session_id.as_str())
            .join("runs")
            .join(run_id.as_str());
        fs::create_dir_all(&run_dir).expect("create run dir");
        let public = DisplayMessage::new(
            1,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_preview("public searchable assistant summary");
        let mut internal = DisplayMessage::new(
            2,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_preview("internal-secret");
        internal.visibility = DisplayVisibility::Internal;
        let tool = DisplayMessage::new(
            3,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolResult,
        )
        .with_preview("tool-payload-secret");
        let snapshot = ReplaySnapshot {
            scope: Some(ReplayScope::run(run_id.as_str())),
            revision: 1,
            cursor: Some(ReplayCursor::display(ReplayScope::run(run_id.as_str()), 3)),
            display_messages: vec![public.clone(), public, internal, tool],
            metadata: Metadata::default(),
        };
        fs::write(
            run_dir.join("display.compact.json"),
            serde_json::to_vec(&snapshot).expect("serialize snapshot"),
        )
        .expect("write snapshot");
        let page = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("searchable".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::DisplayMessage]),
                    granularity: SessionSearchGranularity::Occurrence,
                    ..SessionSearchQuery::default()
                },
            )
            .await
            .expect("display search");
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.coverage.state, SessionSearchCoverageState::Partial);
        assert_eq!(
            page.hits[0].location.archive_scope,
            Some(ReplayScope::run(run_id.as_str()))
        );
        assert_eq!(page.hits[0].location.source_run_id.as_ref(), Some(&run_id));
        for secret in ["internal-secret", "tool-payload-secret"] {
            let page = provider
                .search(
                    &scope,
                    SessionSearchQuery {
                        text: Some(secret.to_string()),
                        sources: BTreeSet::from([SessionSearchSource::DisplayMessage]),
                        ..SessionSearchQuery::default()
                    },
                )
                .await
                .expect("redaction search");
            assert!(page.hits.is_empty());
        }
        fs::remove_file(run_dir.join("display.compact.json")).expect("remove mirror");
        let missing = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("searchable".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::DisplayMessage]),
                    ..SessionSearchQuery::default()
                },
            )
            .await
            .expect("missing mirror remains successful");
        assert!(missing.hits.is_empty());
        assert_eq!(missing.coverage.state, SessionSearchCoverageState::Partial);
    }

    #[tokio::test]
    async fn metadata_browse_remains_complete_without_display_mirrors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_store, scope, provider, _, _) = fixture(temp.path()).await;
        let page = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: None,
                    filter: SessionSearchFilter {
                        profile: Some("coding".to_string()),
                        ..SessionSearchFilter::default()
                    },
                    ..SessionSearchQuery::default()
                },
            )
            .await
            .expect("metadata browse");
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.coverage.state, SessionSearchCoverageState::Complete);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn display_symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let (_store, scope, provider, session_id, run_id) = fixture(temp.path()).await;
        let run_dir = temp
            .path()
            .join("sessions")
            .join(session_id.as_str())
            .join("runs")
            .join(run_id.as_str());
        fs::create_dir_all(&run_dir).expect("create run dir");
        let outside_file = outside.path().join("secret.json");
        fs::write(&outside_file, b"[]").expect("write outside");
        symlink(&outside_file, run_dir.join("display.compact.json")).expect("symlink mirror");
        let page = provider
            .search(
                &scope,
                SessionSearchQuery {
                    text: Some("secret".to_string()),
                    sources: BTreeSet::from([SessionSearchSource::DisplayMessage]),
                    ..SessionSearchQuery::default()
                },
            )
            .await
            .expect("safe partial result");
        assert!(page.hits.is_empty());
        assert_eq!(page.coverage.state, SessionSearchCoverageState::Partial);
    }
}
