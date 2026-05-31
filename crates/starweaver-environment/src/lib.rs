//! Environment provider abstractions for filesystem, shell, and resource access.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use thiserror::Error;

/// Shared environment provider reference.
pub type DynEnvironmentProvider = Arc<dyn EnvironmentProvider>;

/// Environment operation failure.
#[derive(Debug, Error)]
pub enum EnvironmentError {
    /// Access was denied by policy.
    #[error("environment access denied: {0}")]
    AccessDenied(String),
    /// Requested resource was not found.
    #[error("environment resource not found: {0}")]
    NotFound(String),
    /// Input was invalid for this provider.
    #[error("invalid environment request: {0}")]
    InvalidRequest(String),
    /// Provider execution failed.
    #[error("environment provider failed: {0}")]
    Provider(String),
}

/// Result alias for environment provider operations.
pub type EnvironmentResult<T> = Result<T, EnvironmentError>;

/// Filesystem policy for provider-backed tools.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilePolicy {
    /// Whether read operations are allowed.
    pub allow_read: bool,
    /// Whether write operations are allowed.
    pub allow_write: bool,
    /// Allowed logical path prefixes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_prefixes: Vec<String>,
}

impl FilePolicy {
    /// Policy allowing read-only access to all provider-visible files.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            allow_read: true,
            allow_write: false,
            allowed_prefixes: Vec::new(),
        }
    }

    /// Policy allowing read/write access to all provider-visible files.
    #[must_use]
    pub const fn read_write() -> Self {
        Self {
            allow_read: true,
            allow_write: true,
            allowed_prefixes: Vec::new(),
        }
    }

    fn permits(&self, path: &str, write: bool) -> bool {
        if write && !self.allow_write {
            return false;
        }
        if !write && !self.allow_read {
            return false;
        }
        self.allowed_prefixes.is_empty()
            || self
                .allowed_prefixes
                .iter()
                .any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
    }
}

/// Shell execution policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellPolicy {
    /// Whether shell execution is allowed.
    pub allow_execute: bool,
    /// Allowed program names. Empty means any program accepted by the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_programs: Vec<String>,
}

impl ShellPolicy {
    /// Policy allowing all provider-visible commands.
    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            allow_execute: true,
            allowed_programs: Vec::new(),
        }
    }

    fn permits(&self, command: &str) -> bool {
        if !self.allow_execute {
            return false;
        }
        self.allowed_programs.is_empty()
            || command.split_whitespace().next().is_some_and(|program| {
                self.allowed_programs
                    .iter()
                    .any(|allowed| allowed == program)
            })
    }
}

/// Environment provider policy bundle.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentPolicy {
    /// Filesystem policy.
    pub files: FilePolicy,
    /// Shell policy.
    pub shell: ShellPolicy,
}

/// Glob options for provider-backed filesystem searches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobOptions {
    /// Include hidden dot paths.
    pub include_hidden: bool,
    /// Include provider-ignored files where the provider supports ignore files.
    pub include_ignored: bool,
    /// Maximum number of results to return. Zero means no explicit limit.
    pub max_results: usize,
}

impl Default for FileGlobOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            include_ignored: false,
            max_results: 500,
        }
    }
}

/// Grep options for provider-backed text searches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepOptions {
    /// Ripgrep-style glob include filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// Context lines before and after each match.
    pub context_lines: usize,
    /// Maximum total matches. Zero means no explicit limit.
    pub max_results: usize,
    /// Maximum matches per file. Zero means no explicit per-file limit.
    pub max_matches_per_file: usize,
    /// Maximum files to search. Zero means no explicit file limit.
    pub max_files: usize,
    /// Include hidden dot paths.
    pub include_hidden: bool,
    /// Include provider-ignored files where the provider supports ignore files.
    pub include_ignored: bool,
}

impl Default for FileGrepOptions {
    fn default() -> Self {
        Self {
            include: Some("**/*".to_string()),
            context_lines: 0,
            max_results: 100,
            max_matches_per_file: 20,
            max_files: 50,
            include_hidden: false,
            include_ignored: false,
        }
    }
}

/// Glob result entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobMatch {
    /// Provider-scoped path.
    pub path: String,
}

/// Grep result entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepMatch {
    /// Provider-scoped path.
    pub path: String,
    /// One-based line number.
    pub line_number: usize,
    /// Matching line without a trailing newline.
    pub matching_line: String,
    /// Context block containing the matching line.
    pub context: String,
    /// One-based line number where the context block starts.
    pub context_start_line: usize,
}

/// Stable resource reference returned by environment providers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceRef {
    /// Provider-specific resource identifier.
    pub id: String,
    /// Resource URI.
    pub uri: String,
    /// Resource metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Shell execution output.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellOutput {
    /// Process exit status code when available.
    pub status: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Execution metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Serializable environment state snapshot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentState {
    /// Provider identifier.
    pub provider_id: String,
    /// Logical files for virtual or resumable providers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
    /// Resource references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceRef>,
    /// Provider metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl EnvironmentState {
    /// Convert the snapshot into JSON for `AgentContext` state domains.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}

/// Provider boundary used by SDK tools and service runtimes.
#[async_trait]
pub trait EnvironmentProvider: Send + Sync {
    /// Provider identifier.
    fn id(&self) -> &str;

    /// Read a UTF-8 text file.
    async fn read_text(&self, path: &str) -> EnvironmentResult<String>;

    /// Write a UTF-8 text file.
    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()>;

    /// List logical entries under a path.
    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>>;

    /// Match provider-scoped paths with ripgrep-style glob semantics.
    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        let path_glob = PathGlob::new(pattern)?;
        let mut glob_matches = Vec::new();
        for entry in self.list(path).await? {
            if include_path(&entry, options.include_hidden) && path_glob.is_match(&entry) {
                glob_matches.push(FileGlobMatch { path: entry });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

    /// Search provider-scoped text files with ripgrep regex semantics.
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
        let files = self
            .glob(
                path,
                &include,
                FileGlobOptions {
                    include_hidden: options.include_hidden,
                    include_ignored: options.include_ignored,
                    max_results: 0,
                },
            )
            .await?;
        let mut matches = Vec::new();
        let mut searched_files = 0;
        for file in files {
            if options.max_files > 0 && searched_files >= options.max_files {
                break;
            }
            let Ok(content) = self.read_text(&file.path).await else {
                continue;
            };
            searched_files += 1;
            search_text(
                &file.path,
                &content,
                &matcher,
                options.context_lines,
                options.max_matches_per_file,
                options.max_results,
                &mut matches,
            )?;
            if options.max_results > 0 && matches.len() >= options.max_results {
                break;
            }
        }
        Ok(matches)
    }

    /// Execute a command.
    async fn run_shell(&self, command: &str) -> EnvironmentResult<ShellOutput>;

    /// Export provider state for resume.
    async fn export_state(&self) -> EnvironmentResult<EnvironmentState>;
}

/// Deterministic in-memory environment provider for tests and previews.
#[derive(Clone, Debug)]
pub struct VirtualEnvironmentProvider {
    id: String,
    policy: EnvironmentPolicy,
    files: Arc<Mutex<BTreeMap<String, String>>>,
    shell_outputs: Arc<Mutex<BTreeMap<String, ShellOutput>>>,
}

impl VirtualEnvironmentProvider {
    /// Create a virtual provider.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            policy: EnvironmentPolicy {
                files: FilePolicy::read_write(),
                shell: ShellPolicy::allow_all(),
            },
            files: Arc::new(Mutex::new(BTreeMap::new())),
            shell_outputs: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Set provider policy.
    #[must_use]
    pub fn with_policy(mut self, policy: EnvironmentPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Add a virtual file.
    #[must_use]
    pub fn with_file(self, path: impl Into<String>, content: impl Into<String>) -> Self {
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.into(), content.into());
        }
        self
    }

    /// Add deterministic shell output.
    #[must_use]
    pub fn with_shell_output(self, command: impl Into<String>, output: ShellOutput) -> Self {
        if let Ok(mut shell_outputs) = self.shell_outputs.lock() {
            shell_outputs.insert(command.into(), output);
        }
        self
    }

    fn check_file(&self, path: &str, write: bool) -> EnvironmentResult<()> {
        if self.policy.files.permits(path, write) {
            Ok(())
        } else {
            Err(EnvironmentError::AccessDenied(path.to_string()))
        }
    }
}

#[async_trait]
impl EnvironmentProvider for VirtualEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        self.check_file(path, false)?;
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned()
            .ok_or_else(|| EnvironmentError::NotFound(path.to_string()))
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        self.check_file(path, true)?;
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(path.to_string(), content.to_string());
        Ok(())
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        self.check_file(path, false)?;
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        Ok(self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .keys()
            .filter(|entry| entry.starts_with(&prefix))
            .cloned()
            .collect())
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        self.check_file(path, false)?;
        let prefix = path.trim_matches('/');
        let path_glob = PathGlob::new(pattern)?;
        let mut glob_matches = Vec::new();
        for entry in self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .keys()
        {
            if path_contains(prefix, entry)
                && include_path(entry, options.include_hidden)
                && path_glob.is_match(strip_path_prefix(prefix, entry))
            {
                glob_matches.push(FileGlobMatch {
                    path: entry.clone(),
                });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

    async fn run_shell(&self, command: &str) -> EnvironmentResult<ShellOutput> {
        if !self.policy.shell.permits(command) {
            return Err(EnvironmentError::AccessDenied(command.to_string()));
        }
        self.shell_outputs
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(command)
            .cloned()
            .ok_or_else(|| EnvironmentError::NotFound(command.to_string()))
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        Ok(EnvironmentState {
            provider_id: self.id.clone(),
            files: self
                .files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .clone(),
            resources: Vec::new(),
            metadata: Metadata::default(),
        })
    }
}

/// Local provider skeleton with policy-aware filesystem access.
#[derive(Clone, Debug)]
pub struct LocalEnvironmentProvider {
    id: String,
    root: PathBuf,
    policy: EnvironmentPolicy,
}

impl LocalEnvironmentProvider {
    /// Create a local provider rooted at a directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            id: "local".to_string(),
            root: root.into(),
            policy: EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            },
        }
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

    fn resolve(&self, path: &str, write: bool) -> EnvironmentResult<PathBuf> {
        let logical_path = normalize_requested_path(path)?;
        if !self.policy.files.permits(&logical_path, write) {
            return Err(EnvironmentError::AccessDenied(path.to_string()));
        }
        Ok(self.root.join(logical_path))
    }

    fn logical_path(&self, path: &Path) -> EnvironmentResult<String> {
        path.strip_prefix(&self.root)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
            .map(normalize_path)
    }
}

#[async_trait]
impl EnvironmentProvider for LocalEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        let path = self.resolve(path, false)?;
        std::fs::read_to_string(&path)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        let path = self.resolve(path, true)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        }
        std::fs::write(&path, content)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        let path = self.resolve(path, false)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
        {
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
        self.resolve(path, false)?;
        let path_glob = PathGlob::new(pattern)?;
        let search_root = self.resolve(path, false)?;
        let mut builder = ignore::WalkBuilder::new(search_root);
        builder.hidden(!options.include_hidden);
        builder.ignore(!options.include_ignored);
        builder.git_ignore(!options.include_ignored);
        builder.git_global(!options.include_ignored);
        builder.git_exclude(!options.include_ignored);
        builder.require_git(false);
        let prefix = path.trim_matches('/');
        let mut glob_matches = Vec::new();
        for entry in builder.build() {
            let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            let logical = self.logical_path(entry.path())?;
            if !self.policy.files.permits(&logical, false) {
                continue;
            }
            let candidate = strip_path_prefix(prefix, &logical);
            if path_glob.is_match(candidate) {
                glob_matches.push(FileGlobMatch { path: logical });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

    async fn run_shell(&self, command: &str) -> EnvironmentResult<ShellOutput> {
        if !self.policy.shell.permits(command) {
            return Err(EnvironmentError::AccessDenied(command.to_string()));
        }
        Err(EnvironmentError::Provider(
            "local shell execution is reserved for the shell tool implementation".to_string(),
        ))
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        let mut metadata = Metadata::default();
        metadata.insert("root".to_string(), serde_json::json!(self.root));
        Ok(EnvironmentState {
            provider_id: self.id.clone(),
            files: BTreeMap::new(),
            resources: Vec::new(),
            metadata,
        })
    }
}

struct PathGlob {
    matcher: GlobMatcher,
    recursive_prefix_matcher: Option<GlobMatcher>,
    pattern: String,
    anchored: bool,
}

impl PathGlob {
    fn new(pattern: &str) -> EnvironmentResult<Self> {
        let mut normalized = pattern.replace('\\', "/");
        if normalized.is_empty() {
            normalized = "**/*".to_string();
        }
        if let Some(stripped) = normalized.strip_prefix("./") {
            normalized = stripped.to_string();
        }
        let anchored = normalized.starts_with('/');
        let glob_pattern = if anchored {
            let stripped = normalized.trim_start_matches('/');
            if stripped.is_empty() {
                "*"
            } else {
                stripped
            }
        } else {
            normalized.as_str()
        };
        let matcher = compile_glob(glob_pattern)?;
        let recursive_prefix_matcher = glob_pattern
            .strip_prefix("**/")
            .map(compile_glob)
            .transpose()?;
        Ok(Self {
            matcher,
            recursive_prefix_matcher,
            pattern: glob_pattern.to_string(),
            anchored,
        })
    }

    fn is_match(&self, path: &str) -> bool {
        let normalized = normalize_str_path(path);
        if self.anchored && !self.pattern.contains('/') && normalized.contains('/') {
            return false;
        }
        if self.pattern == "**" || self.pattern == "**/*" {
            return true;
        }
        if self.matcher.is_match(&normalized) {
            return true;
        }
        if let Some(matcher) = &self.recursive_prefix_matcher {
            if matcher.is_match(&normalized) {
                return true;
            }
        }
        if !self.anchored && !self.pattern.contains('/') {
            if let Some(name) = normalized.rsplit('/').next() {
                return self.matcher.is_match(name);
            }
        }
        false
    }
}

fn compile_glob(pattern: &str) -> EnvironmentResult<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))
        .map(|glob| glob.compile_matcher())
}

fn search_text(
    path: &str,
    content: &str,
    regex_matcher: &RegexMatcher,
    context_lines: usize,
    max_matches_per_file: usize,
    max_results: usize,
    grep_matches: &mut Vec<FileGrepMatch>,
) -> EnvironmentResult<()> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut file_matches = 0;
    for (index, line) in lines.iter().enumerate() {
        if max_results > 0 && grep_matches.len() >= max_results {
            break;
        }
        if max_matches_per_file > 0 && file_matches >= max_matches_per_file {
            break;
        }
        if regex_matcher
            .is_match(line.as_bytes())
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
        {
            let start_index = index.saturating_sub(context_lines);
            let end_index = (index + context_lines + 1).min(lines.len());
            grep_matches.push(FileGrepMatch {
                path: path.to_string(),
                line_number: index + 1,
                matching_line: line.trim_end_matches('\n').to_string(),
                context: lines[start_index..end_index].concat(),
                context_start_line: start_index + 1,
            });
            file_matches += 1;
        }
    }
    Ok(())
}

fn normalize_path(path: &Path) -> String {
    normalize_str_path(&path.to_string_lossy())
}

fn normalize_requested_path(path: &str) -> EnvironmentResult<String> {
    let requested = Path::new(path);
    if requested.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(EnvironmentError::InvalidRequest(path.to_string()));
    }
    Ok(normalize_str_path(path))
}

fn normalize_str_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized.trim_start_matches('/').to_string()
}

fn include_path(path: &str, include_hidden: bool) -> bool {
    include_hidden
        || !normalize_str_path(path)
            .split('/')
            .any(|segment| segment.starts_with('.') && segment.len() > 1)
}

fn path_contains(prefix: &str, path: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn strip_path_prefix<'a>(prefix: &str, path: &'a str) -> &'a str {
    if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(prefix)
            .and_then(|value| value.strip_prefix('/'))
            .unwrap_or(path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn virtual_provider_reads_lists_shells_and_exports_state() {
        let output = ShellOutput {
            status: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
            metadata: Metadata::default(),
        };
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file("src/lib.rs", "content")
            .with_shell_output("echo ok", output.clone());

        assert_eq!(provider.read_text("src/lib.rs").await.unwrap(), "content");
        provider
            .write_text("src/main.rs", "fn main() {}")
            .await
            .unwrap();
        assert_eq!(
            provider.read_text("src/main.rs").await.unwrap(),
            "fn main() {}"
        );
        assert_eq!(
            provider.list("src").await.unwrap(),
            vec!["src/lib.rs", "src/main.rs"]
        );
        assert_eq!(provider.run_shell("echo ok").await.unwrap(), output);
        let state = provider.export_state().await.unwrap();
        assert_eq!(state.provider_id, "test");
        assert_eq!(state.files["src/main.rs"], "fn main() {}");
    }

    #[tokio::test]
    async fn virtual_provider_globs_and_greps_with_native_matchers() {
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file("src/lib.rs", "pub fn library() {}\n")
            .with_file("src/main.rs", "fn main() { library(); }\n")
            .with_file("README.md", "library docs\n");

        let glob_matches = provider
            .glob("", "*.rs", FileGlobOptions::default())
            .await
            .unwrap();
        assert_eq!(
            glob_matches
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/main.rs"]
        );

        let grep_matches = provider
            .grep(
                "",
                "library",
                FileGrepOptions {
                    include: Some("**/*.rs".to_string()),
                    context_lines: 0,
                    max_results: 10,
                    max_matches_per_file: 10,
                    max_files: 50,
                    include_hidden: false,
                    include_ignored: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 2);
        assert_eq!(grep_matches[0].path, "src/lib.rs");
        assert_eq!(grep_matches[0].line_number, 1);
    }

    #[test]
    fn path_glob_matches_ripgrep_style_patterns() {
        let bare = PathGlob::new("*.rs").unwrap();
        assert!(bare.is_match("lib.rs"));
        assert!(bare.is_match("src/lib.rs"));
        assert!(!bare.is_match("src/lib.py"));

        let recursive = PathGlob::new("**/*.rs").unwrap();
        assert!(recursive.is_match("lib.rs"));
        assert!(recursive.is_match("src/lib.rs"));

        let anchored_file = PathGlob::new("/*.rs").unwrap();
        assert!(anchored_file.is_match("lib.rs"));
        assert!(!anchored_file.is_match("src/lib.rs"));

        let scoped_dir = PathGlob::new("src/*.rs").unwrap();
        assert!(scoped_dir.is_match("src/lib.rs"));
        assert!(!scoped_dir.is_match("src/nested/mod.rs"));

        let anchored_dir = PathGlob::new("/src/*.rs").unwrap();
        assert!(anchored_dir.is_match("src/lib.rs"));
        assert!(!anchored_dir.is_match("src/nested/mod.rs"));
        assert!(!anchored_dir.is_match("nested/src/lib.rs"));
    }

    #[tokio::test]
    async fn virtual_provider_search_respects_root_hidden_limits_and_invalid_patterns() {
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file("src/lib.rs", "alpha\nbeta\nalpha again\n")
            .with_file("src/nested/mod.rs", "alpha nested\n")
            .with_file("tests/lib.rs", "alpha test\n")
            .with_file("src/.hidden.rs", "alpha hidden\n")
            .with_file("README.md", "alpha docs\n");

        let src_matches = provider
            .glob("src", "*.rs", FileGlobOptions::default())
            .await
            .unwrap();
        assert_eq!(
            src_matches
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/nested/mod.rs"]
        );

        let hidden_default = provider
            .glob("src", ".*.rs", FileGlobOptions::default())
            .await
            .unwrap();
        assert!(hidden_default.is_empty());

        let hidden_included = provider
            .glob(
                "src",
                ".*.rs",
                FileGlobOptions {
                    include_hidden: true,
                    ..FileGlobOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(hidden_included[0].path, "src/.hidden.rs");

        let limited = provider
            .glob(
                "",
                "*.rs",
                FileGlobOptions {
                    max_results: 1,
                    ..FileGlobOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(limited.len(), 1);

        let grep_matches = provider
            .grep(
                "src",
                "alpha",
                FileGrepOptions {
                    include: Some("**/*.rs".to_string()),
                    context_lines: 1,
                    max_results: 2,
                    max_matches_per_file: 1,
                    max_files: 50,
                    include_hidden: false,
                    include_ignored: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 2);
        assert_eq!(grep_matches[0].path, "src/lib.rs");
        assert_eq!(grep_matches[0].line_number, 1);
        assert_eq!(grep_matches[0].context_start_line, 1);
        assert!(grep_matches[0].context.contains("beta"));
        assert_eq!(grep_matches[1].path, "src/nested/mod.rs");

        assert!(matches!(
            provider.grep("", "(", FileGrepOptions::default()).await,
            Err(EnvironmentError::InvalidRequest(_))
        ));
        assert!(matches!(
            provider.glob("", "[", FileGlobOptions::default()).await,
            Err(EnvironmentError::InvalidRequest(_))
        ));
    }

    #[tokio::test]
    async fn local_provider_search_respects_gitignore_hidden_and_policy() {
        let root = unique_test_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "needle\n").unwrap();
        std::fs::write(root.join("src/ignored.log"), "needle ignored\n").unwrap();
        std::fs::write(root.join(".hidden.rs"), "needle hidden\n").unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

        let visible = provider
            .glob("", "**/*", FileGlobOptions::default())
            .await
            .unwrap();
        let visible_paths = visible
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert!(visible_paths.contains(&"src/lib.rs"));
        assert!(!visible_paths.contains(&"src/ignored.log"));
        assert!(!visible_paths.contains(&".hidden.rs"));

        let all_files = provider
            .glob(
                "",
                "**/*",
                FileGlobOptions {
                    include_hidden: true,
                    include_ignored: true,
                    max_results: 0,
                },
            )
            .await
            .unwrap();
        let all_paths = all_files
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert!(all_paths.contains(&"src/ignored.log"));
        assert!(all_paths.contains(&".hidden.rs"));

        let grep_matches = provider
            .grep(
                "",
                "needle",
                FileGrepOptions {
                    include: Some("**/*".to_string()),
                    include_hidden: true,
                    include_ignored: true,
                    max_results: 0,
                    max_matches_per_file: 0,
                    max_files: 0,
                    context_lines: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 3);

        let restricted = provider.with_policy(EnvironmentPolicy {
            files: FilePolicy {
                allow_read: true,
                allow_write: false,
                allowed_prefixes: vec!["src".to_string()],
            },
            shell: ShellPolicy::default(),
        });
        assert!(matches!(
            restricted
                .glob("README.md", "**/*", FileGlobOptions::default())
                .await,
            Err(EnvironmentError::AccessDenied(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn local_provider_rejects_absolute_and_parent_paths() {
        let root = unique_test_dir();
        std::fs::write(root.join("safe..name.txt"), "ok").unwrap();
        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

        assert_eq!(
            provider.read_text("safe..name.txt").await.unwrap(),
            "ok".to_string()
        );
        assert!(matches!(
            provider.read_text("/etc/passwd").await,
            Err(EnvironmentError::InvalidRequest(_))
        ));
        assert!(matches!(
            provider.read_text("../outside.txt").await,
            Err(EnvironmentError::InvalidRequest(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn policy_denies_disallowed_file_access() {
        let provider = VirtualEnvironmentProvider::new("test").with_policy(EnvironmentPolicy {
            files: FilePolicy::default(),
            shell: ShellPolicy::default(),
        });
        assert!(matches!(
            provider.read_text("secret").await,
            Err(EnvironmentError::AccessDenied(_))
        ));
    }

    fn unique_test_dir() -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("starweaver-env-test-{suffix}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
