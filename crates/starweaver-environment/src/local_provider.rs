//! Local filesystem and shell environment provider.

use std::{
    collections::{BTreeMap, BinaryHeap},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use starweaver_core::Metadata;

use crate::{
    DEFAULT_FILE_TREE_MAX_DEPTH, DEFAULT_LOCAL_COMPLETED_PROCESS_RETENTION,
    DEFAULT_LOCAL_OUTPUT_BYTES, DEFAULT_LOCAL_PROCESS_CONCURRENCY, DynProcessShellProvider,
    EnvironmentError, EnvironmentPolicy, EnvironmentProvider, EnvironmentResult, EnvironmentState,
    FileGlobMatch, FileGlobOptions, FileGrepMatch, FileGrepOptions, FileListOptions,
    FileListResult, FilePolicy, FileStat, FileTreeBlock, LocalExecutionLimiter, LocalGrepSink,
    PathGlob, ProgramCommand, ShellCommand, ShellOutput, ShellPolicy,
    ShellReviewEnvironmentContext, copy_local_dir, create_local_tmp_dir, display_local_path,
    file_tree_directory_depth_increment, file_tree_directory_is_visible, list_ignore_match,
    local_grep_file_match_limit, local_search_walk_builder, local_shell_metadata, map_io_error,
    normalize_local_config_path, normalize_match_path, normalize_path, normalize_tmp_namespace,
    path_match_candidates, prepare_local_destination, push_unique_candidate, push_unique_path,
    render_environment_context_xml, render_local_file_tree_listing,
};
use async_trait::async_trait;
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection, SearcherBuilder};

/// Default maximum bytes returned by one local file read.
pub const DEFAULT_LOCAL_READ_BYTES: usize = 8 * 1024 * 1024;

/// Local provider with policy-aware filesystem access.
///
/// Physical-path containment rejects pre-existing symlink escapes, but this
/// provider is not an operating-system sandbox. Hosts that require resistance
/// to a concurrent untrusted filesystem writer must use a sandboxed provider
/// or keep allowed roots exclusively controlled for the duration of an
/// operation.
#[derive(Clone, Debug)]
pub struct LocalEnvironmentProvider {
    id: String,
    root: PathBuf,
    allowed_paths: Vec<PathBuf>,
    context_file_tree_roots: Option<Vec<PathBuf>>,
    tmp_dir: Option<Arc<tempfile::TempDir>>,
    tmp_namespace: Option<String>,
    policy: EnvironmentPolicy,
    processes: Arc<Mutex<BTreeMap<String, LocalShellProcess>>>,
    execution_limiter: Arc<LocalExecutionLimiter>,
    max_read_bytes: usize,
    max_output_bytes: usize,
    completed_process_retention: usize,
}

mod paths;
mod process;
mod temp;

pub use process::LocalShellProcess;

impl LocalEnvironmentProvider {
    /// Create a local provider rooted at a directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = normalize_local_config_path(root.into());
        let tmp_dir = create_local_tmp_dir(None).map(Arc::new);
        let mut allowed_paths = vec![root.clone()];
        if let Some(tmp_path) = tmp_dir.as_ref().map(|tmp_dir| tmp_dir.path()) {
            push_unique_path(
                &mut allowed_paths,
                normalize_local_config_path(tmp_path.to_path_buf()),
            );
        }
        Self {
            id: "local".to_string(),
            allowed_paths,
            context_file_tree_roots: None,
            tmp_dir,
            tmp_namespace: None,
            root,
            policy: EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            },
            processes: Arc::new(Mutex::new(BTreeMap::new())),
            execution_limiter: Arc::new(LocalExecutionLimiter::new(
                DEFAULT_LOCAL_PROCESS_CONCURRENCY,
            )),
            max_read_bytes: DEFAULT_LOCAL_READ_BYTES,
            max_output_bytes: DEFAULT_LOCAL_OUTPUT_BYTES,
            completed_process_retention: DEFAULT_LOCAL_COMPLETED_PROCESS_RETENTION,
        }
    }

    /// Restore a local provider from a trusted environment state snapshot.
    ///
    /// The caller must supply the policy because persisted local paths are host
    /// capabilities, not portable authority. This method should only be used when
    /// the snapshot was produced by the same trusted host.
    ///
    /// # Errors
    ///
    /// Returns an error when the state does not contain a valid local root or
    /// allowed path list.
    pub fn from_trusted_state(
        state: &EnvironmentState,
        policy: EnvironmentPolicy,
    ) -> EnvironmentResult<Self> {
        let root_value =
            state.metadata.get("root").cloned().ok_or_else(|| {
                EnvironmentError::InvalidRequest("missing local root".to_string())
            })?;
        let root = serde_json::from_value::<PathBuf>(root_value)
            .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?;
        let allowed_paths = state
            .metadata
            .get("allowed_paths")
            .cloned()
            .map(serde_json::from_value::<Vec<PathBuf>>)
            .transpose()
            .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?
            .unwrap_or_default();
        Ok(Self::new(root)
            .with_id(state.provider_id.clone())
            .with_policy(policy)
            .with_allowed_paths(allowed_paths))
    }

    /// Set provider id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set policy.
    #[must_use]
    pub fn with_policy(mut self, policy: EnvironmentPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set absolute filesystem roots that this provider may access.
    ///
    /// The provider root remains the default directory for relative paths and is
    /// always included even when omitted from `paths`.
    #[must_use]
    pub fn with_allowed_paths<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let allowed_paths = paths
            .into_iter()
            .map(|path| normalize_local_config_path(path.into()))
            .collect::<Vec<_>>();
        self.rebuild_allowed_paths_with_managed_roots(allowed_paths);
        self
    }

    /// Set filesystem roots rendered in generated environment context file trees.
    ///
    /// This can be narrower than [`Self::allowed_paths`] when auxiliary roots
    /// should remain available to tools without consuming prompt budget as
    /// workspace file-tree context.
    #[must_use]
    pub fn with_context_file_tree_roots<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.context_file_tree_roots = Some(
            paths
                .into_iter()
                .map(|path| normalize_local_config_path(path.into()))
                .collect(),
        );
        self
    }

    /// Create provider-managed temporary files under a specific base directory.
    ///
    /// The created session directory is added to the allowed path set and is
    /// cleaned up when the provider and its clones are dropped. If creation
    /// fails, the existing temporary directory configuration is left unchanged.
    #[must_use]
    pub fn with_tmp_base_dir(mut self, base_dir: impl Into<PathBuf>) -> Self {
        let old_tmp_dir = self
            .tmp_dir_path()
            .map(|path| normalize_local_config_path(path.to_path_buf()));
        let base_dir = normalize_local_config_path(base_dir.into());
        if let Some(tmp_dir) = create_local_tmp_dir(Some(&base_dir)) {
            self.tmp_dir = Some(Arc::new(tmp_dir));
            let allowed_paths = self
                .allowed_paths
                .iter()
                .filter(|path| old_tmp_dir.as_ref() != Some(path))
                .cloned()
                .collect::<Vec<_>>();
            self.rebuild_allowed_paths_with_managed_roots(allowed_paths);
        }
        self
    }

    /// Set a provider-scoped temporary file namespace.
    ///
    /// Namespaces isolate tool-generated large output files under a stable
    /// subdirectory of the provider temporary root.
    #[must_use]
    pub fn with_tmp_namespace(mut self, namespace: impl AsRef<str>) -> Self {
        self.tmp_namespace = normalize_tmp_namespace(namespace.as_ref()).ok();
        self
    }

    /// Set the shared foreground/background local process concurrency limit.
    ///
    /// Values below one are clamped to one. The setting should be applied before
    /// cloning the provider.
    #[must_use]
    pub fn with_max_concurrent_processes(mut self, maximum: usize) -> Self {
        self.execution_limiter = Arc::new(LocalExecutionLimiter::new(maximum));
        self
    }

    /// Set the maximum bytes returned by one file read.
    ///
    /// Explicit larger ranges and unbounded reads whose result exceeds this
    /// limit fail rather than allocating an attacker-controlled buffer.
    #[must_use]
    pub const fn with_max_read_bytes(mut self, maximum: usize) -> Self {
        self.max_read_bytes = maximum;
        self
    }

    /// Set the maximum bytes retained independently for stdout and stderr.
    ///
    /// Readers continue draining after reaching this bound so a chatty child
    /// cannot block on a full pipe. Zero captures no output while preserving
    /// total byte and truncation metadata.
    #[must_use]
    pub const fn with_max_output_bytes(mut self, maximum: usize) -> Self {
        self.max_output_bytes = maximum;
        self
    }

    /// Set how many completed background process snapshots remain queryable.
    #[must_use]
    pub const fn with_completed_process_retention(mut self, retention: usize) -> Self {
        self.completed_process_retention = retention;
        self
    }

    /// Return configured local filesystem roots.
    #[must_use]
    pub fn allowed_paths(&self) -> &[PathBuf] {
        &self.allowed_paths
    }

    /// Return this provider's managed temporary directory when available.
    #[must_use]
    pub fn tmp_dir_path(&self) -> Option<&Path> {
        self.tmp_dir.as_ref().map(|tmp_dir| tmp_dir.path())
    }
}

#[async_trait]
impl EnvironmentProvider for LocalEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        Some(self)
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        ShellReviewEnvironmentContext {
            default_cwd: Some(display_local_path(&self.root)),
            allowed_paths: self
                .allowed_paths
                .iter()
                .map(|path| display_local_path(path))
                .collect(),
            shell_platform: Some(std::env::consts::OS.to_string()),
            shell_executable: std::env::var("SHELL").ok(),
        }
    }

    fn path_match_candidates(&self, path: &str) -> Vec<String> {
        let mut candidates = path_match_candidates(path);
        if let Ok((visible_path, filesystem_path)) =
            self.resolve_request_path_with_logical_path(path)
        {
            push_unique_candidate(&mut candidates, visible_path.clone());
            push_unique_candidate(&mut candidates, normalize_match_path(&visible_path));
            push_unique_candidate(&mut candidates, display_local_path(&filesystem_path));
        }
        candidates
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_path(path, false)?;
            let file = File::open(&path).map_err(|error| map_io_error(&path, &error))?;
            let probe = provider.max_read_bytes.saturating_add(1);
            let limit = u64::try_from(probe).map_err(|_| {
                EnvironmentError::InvalidRequest(
                    "file read length exceeds platform limits".to_string(),
                )
            })?;
            let mut bytes = Vec::with_capacity(provider.max_read_bytes.min(64 * 1024));
            file.take(limit)
                .read_to_end(&mut bytes)
                .map_err(|error| map_io_error(&path, &error))?;
            if bytes.len() > provider.max_read_bytes {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "file read exceeds configured {} byte limit; request a bounded byte range",
                    provider.max_read_bytes
                )));
            }
            String::from_utf8(bytes).map_err(|error| EnvironmentError::Provider(error.to_string()))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_path(path, false)?;
            let mut file = File::open(&path).map_err(|error| map_io_error(&path, &error))?;
            let offset = u64::try_from(offset).map_err(|_| {
                EnvironmentError::InvalidRequest(
                    "file read offset exceeds platform limits".to_string(),
                )
            })?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|error| map_io_error(&path, &error))?;
            if length.is_some_and(|length| length > provider.max_read_bytes) {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "requested file read exceeds configured {} byte limit",
                    provider.max_read_bytes
                )));
            }
            let requested = length.unwrap_or(provider.max_read_bytes);
            let probe = if length.is_some() {
                requested
            } else {
                requested.saturating_add(1)
            };
            let limit = u64::try_from(probe).map_err(|_| {
                EnvironmentError::InvalidRequest(
                    "file read length exceeds platform limits".to_string(),
                )
            })?;
            let mut bytes = Vec::with_capacity(requested.min(64 * 1024));
            file.take(limit)
                .read_to_end(&mut bytes)
                .map_err(|error| map_io_error(&path, &error))?;
            if length.is_none() && bytes.len() > provider.max_read_bytes {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "file read exceeds configured {} byte limit; request a bounded range",
                    provider.max_read_bytes
                )));
            }
            Ok(bytes)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        let provider = self.clone();
        let path = path.to_string();
        let content = content.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let content = content.as_str();
            let path = provider.resolve_provider_path(path, true)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            }
            std::fs::write(&path, content).map_err(|error| map_io_error(&path, &error))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_path(path, true)?;
            if provider.is_exact_allowed_root(&path) && path.exists() {
                return Ok(());
            }
            let result = if parents {
                std::fs::create_dir_all(&path)
            } else {
                std::fs::create_dir(&path)
            };
            result.map_err(|error| map_io_error(&path, &error))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_entry_path(path)?;
            if provider.is_exact_allowed_root(&path) {
                return Err(EnvironmentError::InvalidRequest(
                    "refusing to delete an allowed environment root".to_string(),
                ));
            }
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|error| map_io_error(&path, &error))?;
            if metadata.file_type().is_symlink() {
                std::fs::remove_file(&path)
            } else if metadata.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_dir(&path)
                }
            } else {
                std::fs::remove_file(&path)
            }
            .map_err(|error| map_io_error(&path, &error))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let provider = self.clone();
        let src = src.to_string();
        let dst = dst.to_string();
        crate::blocking::run(move || {
            let src = src.as_str();
            let dst = dst.as_str();
            let src = provider.resolve_provider_entry_path(src)?;
            let dst = provider.resolve_provider_path(dst, true)?;
            if provider.is_exact_allowed_root(&src) {
                return Err(EnvironmentError::InvalidRequest(
                    "refusing to move an allowed environment root".to_string(),
                ));
            }
            if provider.is_exact_allowed_root(&dst) {
                return Err(EnvironmentError::InvalidRequest(
                    "destination must not be an allowed environment root".to_string(),
                ));
            }
            if src == dst {
                return Err(EnvironmentError::InvalidRequest(
                    "source and destination must differ".to_string(),
                ));
            }
            prepare_local_destination(&dst, overwrite)?;
            std::fs::rename(&src, &dst).map_err(|error| map_io_error(&src, &error))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let provider = self.clone();
        let src = src.to_string();
        let dst = dst.to_string();
        crate::blocking::run(move || {
            let src = src.as_str();
            let dst = dst.as_str();
            let src = provider.resolve_provider_path(src, false)?;
            let dst = provider.resolve_provider_path(dst, true)?;
            if provider.is_exact_allowed_root(&src) {
                return Err(EnvironmentError::InvalidRequest(
                    "refusing to copy an allowed environment root".to_string(),
                ));
            }
            if provider.is_exact_allowed_root(&dst) {
                return Err(EnvironmentError::InvalidRequest(
                    "destination must not be an allowed environment root".to_string(),
                ));
            }
            if src == dst {
                return Err(EnvironmentError::InvalidRequest(
                    "source and destination must differ".to_string(),
                ));
            }
            let metadata = std::fs::metadata(&src).map_err(|error| map_io_error(&src, &error))?;
            prepare_local_destination(&dst, overwrite)?;
            if metadata.is_dir() {
                copy_local_dir(&src, &dst)
            } else {
                std::fs::copy(&src, &dst)
                    .map(|_| ())
                    .map_err(|error| map_io_error(&src, &error))
            }
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let provider = self.clone();
        let filename = filename.to_string();
        let content = content.to_vec();
        crate::blocking::run(move || {
            let filename = filename.as_str();
            let content = content.as_slice();
            let normalized = provider.tmp_file_relative_path(filename)?;
            let path = provider.resolve_tmp_relative_path(&normalized, true)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| map_io_error(parent, &error))?;
            }
            let path = provider.resolve_tmp_relative_path(&normalized, true)?;
            std::fs::write(&path, content).map_err(|error| map_io_error(&path, &error))?;
            Ok(display_local_path(&path))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_path(path, false)?;
            let metadata = std::fs::metadata(&path).map_err(|error| map_io_error(&path, &error))?;
            let modified_unix_seconds = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            Ok(FileStat {
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                is_file: metadata.is_file(),
                is_dir: metadata.is_dir(),
                modified_unix_seconds,
            })
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_path(path, false)?;
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&path).map_err(|error| map_io_error(&path, &error))? {
                let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                entries.push(entry.file_name().to_string_lossy().to_string());
            }
            entries.sort_unstable();
            Ok(entries)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn list_with_options(
        &self,
        path: &str,
        options: FileListOptions,
    ) -> EnvironmentResult<FileListResult> {
        let provider = self.clone();
        let path = path.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let path = provider.resolve_provider_path(path, false)?;
            if options.max_entries == 0 {
                let mut entries = Vec::new();
                for entry in
                    std::fs::read_dir(&path).map_err(|error| map_io_error(&path, &error))?
                {
                    let entry =
                        entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !list_ignore_match(&options.ignore_patterns, &name) {
                        entries.push(name);
                    }
                }
                entries.sort_unstable();
                let total_entries = entries.len();
                return Ok(FileListResult {
                    entries,
                    truncated: false,
                    total_entries,
                });
            }

            let mut entries = BinaryHeap::new();
            let mut total_entries = 0usize;
            for entry in std::fs::read_dir(&path).map_err(|error| map_io_error(&path, &error))? {
                let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                let name = entry.file_name().to_string_lossy().to_string();
                if list_ignore_match(&options.ignore_patterns, &name) {
                    continue;
                }
                total_entries = total_entries.saturating_add(1);
                if entries.len() < options.max_entries {
                    entries.push(name);
                } else if entries.peek().is_some_and(|largest| name < *largest) {
                    entries.pop();
                    entries.push(name);
                }
            }
            let entries = entries.into_sorted_vec();
            Ok(FileListResult {
                truncated: total_entries > entries.len(),
                total_entries,
                entries,
            })
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        let provider = self.clone();
        let path = path.to_string();
        let pattern = pattern.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let pattern = pattern.as_str();
            let path_glob = PathGlob::new(pattern)?;
            let search_root = provider.resolve_provider_path(path, false)?;
            let builder = local_search_walk_builder(
                &search_root,
                options.include_hidden,
                options.include_ignored,
            );
            let mut glob_matches = Vec::new();
            for entry in builder.build() {
                let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if !entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file() || file_type.is_dir())
                {
                    continue;
                }
                if entry.path() == search_root {
                    continue;
                }
                let logical = provider.logical_provider_path(entry.path())?;
                if !provider.policy.files.permits(&logical, false) {
                    continue;
                }
                let candidate = entry
                    .path()
                    .strip_prefix(&search_root)
                    .map(normalize_path)
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if path_glob.is_match(&candidate) {
                    glob_matches.push(FileGlobMatch { path: logical });
                    if options.max_results > 0 && glob_matches.len() >= options.max_results {
                        break;
                    }
                }
            }
            Ok(glob_matches)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn grep(
        &self,
        path: &str,
        pattern: &str,
        options: FileGrepOptions,
    ) -> EnvironmentResult<Vec<FileGrepMatch>> {
        let provider = self.clone();
        let path = path.to_string();
        let pattern = pattern.to_string();
        crate::blocking::run(move || {
            let path = path.as_str();
            let pattern = pattern.as_str();
            let matcher = RegexMatcher::new_line_matcher(pattern)
                .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?;
            let include = options
                .include
                .clone()
                .unwrap_or_else(|| "**/*".to_string());
            let path_glob = PathGlob::new(&include)?;
            let search_root = provider.resolve_provider_path(path, false)?;
            let builder = local_search_walk_builder(
                &search_root,
                options.include_hidden,
                options.include_ignored,
            );
            let mut grep_matches = Vec::new();
            let mut searched_files = 0;
            for entry in builder.build() {
                if options.max_results > 0 && grep_matches.len() >= options.max_results {
                    break;
                }
                if options.max_files > 0 && searched_files >= options.max_files {
                    break;
                }
                let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if !entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
                {
                    continue;
                }
                let logical = provider.logical_provider_path(entry.path())?;
                if !provider.policy.files.permits(&logical, false) {
                    continue;
                }
                let candidate = entry
                    .path()
                    .strip_prefix(&search_root)
                    .map(normalize_path)
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if !path_glob.is_match(&candidate) {
                    continue;
                }

                searched_files += 1;
                let max_matches = local_grep_file_match_limit(&options, grep_matches.len());
                let mut searcher = SearcherBuilder::new()
                    .line_number(true)
                    .before_context(options.context_lines)
                    .after_context(options.context_lines)
                    .binary_detection(BinaryDetection::quit(b'\x00'))
                    .max_matches(max_matches)
                    .build();
                let mut sink = LocalGrepSink::new(
                    &logical,
                    &mut grep_matches,
                    options.context_lines,
                    options.max_results,
                );
                let _ = searcher.search_path(&matcher, entry.path(), &mut sink);
            }
            Ok(grep_matches)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        let provider = self.clone();
        crate::blocking::run_cancellable(move |cancelled| {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(EnvironmentError::Provider(
                    "shell command cancelled before start".to_string(),
                ));
            }
            if !provider.policy.shell.permits_shell() {
                return Err(EnvironmentError::AccessDenied(command.command));
            }
            let cwd = match command.cwd.as_deref() {
                Some(cwd) => provider.resolve_provider_path(cwd, false)?,
                None => provider.root.clone(),
            };
            if !cwd.is_dir() {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "shell cwd is not a directory: {}",
                    cwd.display()
                )));
            }
            let environment = provider.shell_environment(&command.environment)?;
            provider.reap_local_processes()?;
            let _execution_permit = provider.execution_limiter.try_acquire()?;
            crate::shell::run_local_shell_command(
                &command.command,
                &cwd,
                &environment,
                command.timeout_seconds,
                provider.max_output_bytes,
                &cancelled,
            )
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn run_program(&self, command: ProgramCommand) -> EnvironmentResult<ShellOutput> {
        let provider = self.clone();
        crate::blocking::run_cancellable(move |cancelled| {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(EnvironmentError::Provider(
                    "program cancelled before start".to_string(),
                ));
            }
            if !provider.policy.shell.permits_program(&command.program) {
                return Err(EnvironmentError::AccessDenied(command.display_command()));
            }
            if !command.environment.is_empty()
                && !provider
                    .policy
                    .shell
                    .permits_program_environment_overrides()
            {
                return Err(EnvironmentError::InvalidRequest(
                    "environment overrides are not allowed for allowlisted direct programs"
                        .to_string(),
                ));
            }
            let cwd = match command.cwd.as_deref() {
                Some(cwd) => provider.resolve_provider_path(cwd, false)?,
                None => provider.root.clone(),
            };
            if !cwd.is_dir() {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "program cwd is not a directory: {}",
                    cwd.display()
                )));
            }
            let environment = provider.shell_environment(&command.environment)?;
            provider.reap_local_processes()?;
            let _execution_permit = provider.execution_limiter.try_acquire()?;
            crate::shell::run_local_program_command(
                &command,
                &cwd,
                &environment,
                provider.max_output_bytes,
                &cancelled,
            )
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        let provider = self.clone();
        crate::blocking::run(move || {
            let mut file_trees = Vec::new();
            let file_tree_roots = provider
                .context_file_tree_roots
                .as_deref()
                .unwrap_or(&provider.allowed_paths);
            for allowed_path in context_file_tree_roots(file_tree_roots) {
                let visible_root = provider.logical_root_for_allowed_path(allowed_path);
                let tree = render_local_file_tree_listing(
                    allowed_path,
                    &visible_root,
                    &provider.policy,
                    DEFAULT_FILE_TREE_MAX_DEPTH,
                )?;
                if !tree.is_empty() && !tree.starts_with("Directory not found") {
                    file_trees.push(FileTreeBlock {
                        path: display_local_path(allowed_path),
                        listing_text: tree,
                    });
                }
            }
            Ok(Some(render_environment_context_xml(
                provider.id(),
                &display_local_path(&provider.root),
                provider.tmp_dir_path().map(display_local_path),
                &file_trees,
                provider.policy.shell.allow_execute,
                Some(local_shell_metadata()),
            )))
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        let mut metadata = Metadata::default();
        metadata.insert(
            crate::ENVIRONMENT_PROVIDER_KIND_KEY.to_string(),
            serde_json::json!("local"),
        );
        metadata.insert("root".to_string(), serde_json::json!(self.root));
        metadata.insert(
            "allowed_paths".to_string(),
            serde_json::json!(self.allowed_paths),
        );
        Ok(EnvironmentState {
            provider_id: self.id.clone(),
            files: BTreeMap::new(),
            resources: Vec::new(),
            processes: Vec::new(),
            metadata,
        })
    }
}

fn context_file_tree_roots(allowed_paths: &[PathBuf]) -> Vec<&PathBuf> {
    let mut roots = Vec::new();
    for path in allowed_paths {
        if allowed_paths
            .iter()
            .any(|root| path != root && path_is_visible_under_root(path, root))
        {
            continue;
        }
        roots.push(path);
    }
    roots
}

fn path_is_visible_under_root(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return false;
    }
    let gitignore = root_gitignore(root);
    let mut logical = String::new();
    let mut depth = 0;
    for component in components {
        let name = component.as_os_str().to_string_lossy();
        if !file_tree_directory_is_visible(&name) {
            return false;
        }
        depth += file_tree_directory_depth_increment(&name);
        if depth >= DEFAULT_FILE_TREE_MAX_DEPTH {
            return false;
        }
        if !logical.is_empty() {
            logical.push('/');
        }
        logical.push_str(&name);
        if gitignore
            .as_ref()
            .is_some_and(|matcher| matcher.matched(&logical, true).is_ignore())
        {
            return false;
        }
    }
    true
}

fn root_gitignore(root: &Path) -> Option<ignore::gitignore::Gitignore> {
    let content = std::fs::read_to_string(root.join(".gitignore")).ok()?;
    let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
    for line in content.lines() {
        builder.add_line(None, line).ok()?;
    }
    builder.build().ok()
}
