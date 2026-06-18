//! Local filesystem and shell environment provider.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use starweaver_core::Metadata;

use crate::{
    copy_local_dir, create_local_tmp_dir, display_local_path, local_grep_file_match_limit,
    local_search_walk_builder, local_shell_metadata, map_io_error, normalize_local_config_path,
    normalize_match_path, normalize_path, normalize_tmp_namespace, path_match_candidates,
    prepare_local_destination, push_unique_candidate, push_unique_path,
    render_environment_context_xml, render_local_file_tree_listing, run_local_shell_command,
    DynProcessShellProvider, EnvironmentError, EnvironmentPolicy, EnvironmentProvider,
    EnvironmentResult, EnvironmentState, FileGlobMatch, FileGlobOptions, FileGrepMatch,
    FileGrepOptions, FilePolicy, FileStat, FileTreeBlock, LocalGrepSink, PathGlob, ShellCommand,
    ShellOutput, ShellPolicy, ShellReviewEnvironmentContext, DEFAULT_FILE_TREE_MAX_DEPTH,
};
use async_trait::async_trait;
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection, SearcherBuilder};

/// Local provider skeleton with policy-aware filesystem access.
#[derive(Clone, Debug)]
pub struct LocalEnvironmentProvider {
    id: String,
    root: PathBuf,
    allowed_paths: Vec<PathBuf>,
    tmp_dir: Option<Arc<tempfile::TempDir>>,
    tmp_namespace: Option<String>,
    policy: EnvironmentPolicy,
    processes: Arc<Mutex<BTreeMap<String, LocalShellProcess>>>,
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
            tmp_dir,
            tmp_namespace: None,
            root,
            policy: EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            },
            processes: Arc::new(Mutex::new(BTreeMap::new())),
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
        let path = self.resolve_provider_path(path, false)?;
        std::fs::read_to_string(&path)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        let path = self.resolve_provider_path(path, false)?;
        let bytes =
            std::fs::read(&path).map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        if offset >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = length.map_or(bytes.len(), |length| {
            offset.saturating_add(length).min(bytes.len())
        });
        Ok(bytes[offset..end].to_vec())
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        let path = self.resolve_provider_path(path, true)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        }
        std::fs::write(&path, content).map_err(|error| map_io_error(&path, &error))
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let path = self.resolve_provider_path(path, true)?;
        if self.is_exact_allowed_root(&path) && path.exists() {
            return Ok(());
        }
        let result = if parents {
            std::fs::create_dir_all(&path)
        } else {
            std::fs::create_dir(&path)
        };
        result.map_err(|error| map_io_error(&path, &error))
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let path = self.resolve_provider_path(path, true)?;
        if self.is_exact_allowed_root(&path) {
            return Err(EnvironmentError::InvalidRequest(
                "refusing to delete an allowed environment root".to_string(),
            ));
        }
        let metadata = std::fs::metadata(&path).map_err(|error| map_io_error(&path, &error))?;
        if metadata.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_dir(&path)
            }
        } else {
            std::fs::remove_file(&path)
        }
        .map_err(|error| map_io_error(&path, &error))
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = self.resolve_provider_path(src, true)?;
        let dst = self.resolve_provider_path(dst, true)?;
        if self.is_exact_allowed_root(&src) {
            return Err(EnvironmentError::InvalidRequest(
                "refusing to move an allowed environment root".to_string(),
            ));
        }
        if self.is_exact_allowed_root(&dst) {
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
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = self.resolve_provider_path(src, false)?;
        let dst = self.resolve_provider_path(dst, true)?;
        if self.is_exact_allowed_root(&src) {
            return Err(EnvironmentError::InvalidRequest(
                "refusing to copy an allowed environment root".to_string(),
            ));
        }
        if self.is_exact_allowed_root(&dst) {
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
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let normalized = self.tmp_file_relative_path(filename)?;
        let tmp_dir = self.tmp_dir_path().ok_or_else(|| {
            EnvironmentError::Provider("local temporary directory is unavailable".to_string())
        })?;
        let path = tmp_dir.join(&normalized);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        }
        std::fs::write(&path, content).map_err(|error| map_io_error(&path, &error))?;
        Ok(display_local_path(&normalize_local_config_path(path)))
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        let path = self.resolve_provider_path(path, false)?;
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
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        let path = self.resolve_provider_path(path, false)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|error| map_io_error(&path, &error))? {
            let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        entries.sort();
        Ok(entries)
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        let path_glob = PathGlob::new(pattern)?;
        let search_root = self.resolve_provider_path(path, false)?;
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
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            let logical = self.logical_provider_path(entry.path())?;
            if !self.policy.files.permits(&logical, false) {
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
    }

    async fn grep(
        &self,
        path: &str,
        pattern: &str,
        options: FileGrepOptions,
    ) -> EnvironmentResult<Vec<FileGrepMatch>> {
        let matcher = RegexMatcher::new_line_matcher(pattern)
            .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?;
        let include = options
            .include
            .clone()
            .unwrap_or_else(|| "**/*".to_string());
        let path_glob = PathGlob::new(&include)?;
        let search_root = self.resolve_provider_path(path, false)?;
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
            let logical = self.logical_provider_path(entry.path())?;
            if !self.policy.files.permits(&logical, false) {
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
            let mut sink = LocalGrepSink::new(&logical, &mut grep_matches, options.max_results);
            let _ = searcher.search_path(&matcher, entry.path(), &mut sink);
        }
        Ok(grep_matches)
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        let cwd = match command.cwd.as_deref() {
            Some(cwd) => self.resolve_provider_path(cwd, false)?,
            None => self.root.clone(),
        };
        if !cwd.is_dir() {
            return Err(EnvironmentError::InvalidRequest(format!(
                "shell cwd is not a directory: {}",
                cwd.display()
            )));
        }
        let environment = self.shell_environment(&command.environment)?;
        run_local_shell_command(
            &command.command,
            &cwd,
            &environment,
            command.timeout_seconds,
        )
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        let mut file_trees = Vec::new();
        for allowed_path in context_file_tree_roots(&self.allowed_paths) {
            let visible_root = self.logical_root_for_allowed_path(allowed_path);
            let tree = render_local_file_tree_listing(
                allowed_path,
                &visible_root,
                &self.policy,
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
            self.id(),
            &display_local_path(&self.root),
            self.tmp_dir_path().map(display_local_path),
            &file_trees,
            self.policy.shell.allow_execute,
            Some(local_shell_metadata()),
        )))
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
    if components.is_empty() || components.len() >= DEFAULT_FILE_TREE_MAX_DEPTH {
        return false;
    }
    let gitignore = root_gitignore(root);
    let mut logical = String::new();
    for component in components {
        let name = component.as_os_str().to_string_lossy();
        if name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules" | ".git" | ".venv" | "__pycache__"
            )
        {
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
