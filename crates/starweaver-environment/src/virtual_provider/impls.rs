//! Virtual environment provider trait implementation.

use std::{
    collections::{BTreeSet, BinaryHeap},
    sync::Arc,
};

use async_trait::async_trait;
use starweaver_core::Metadata;

use crate::{
    include_path, list_ignore_match, normalize_requested_path, parent_path, path_contains,
    render_environment_context_xml, render_virtual_file_tree_listing, replace_logical_prefix,
    strip_path_prefix, DynProcessShellProvider, EnvironmentError, EnvironmentProvider,
    EnvironmentResult, EnvironmentState, FileGlobMatch, FileGlobOptions, FileListOptions,
    FileListResult, FileStat, FileTreeBlock, PathGlob, ShellCommand, ShellOutput,
    ShellReviewEnvironmentContext, DEFAULT_FILE_TREE_MAX_DEPTH,
};

use super::VirtualEnvironmentProvider;

#[async_trait]
impl EnvironmentProvider for VirtualEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        Some(self)
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        ShellReviewEnvironmentContext {
            default_cwd: Some(".".to_string()),
            allowed_paths: vec![".".to_string()],
            shell_platform: Some("virtual".to_string()),
            shell_executable: None,
        }
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        self.check_file(path, false)?;
        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        if let Some(content) = text_content {
            return Ok(content);
        }
        let bytes = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned()
            .ok_or_else(|| EnvironmentError::NotFound(path.to_string()))?;
        String::from_utf8(bytes).map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        self.check_file(path, false)?;
        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        let bytes = if let Some(content) = text_content {
            content.into_bytes()
        } else {
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .get(path)
                .cloned()
                .ok_or_else(|| EnvironmentError::NotFound(path.to_string()))?
        };
        if offset >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = length.map_or(bytes.len(), |length| {
            offset.saturating_add(length).min(bytes.len())
        });
        Ok(bytes[offset..end].to_vec())
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        self.check_file(path, true)?;
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(path.to_string(), content.to_string());
        self.binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .remove(path);
        Ok(())
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let normalized = normalize_requested_path(path)?;
        self.check_file(&normalized, true)?;
        if normalized.is_empty() || normalized == "." {
            return Ok(());
        }
        if self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains_key(&normalized)
            || self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .contains_key(&normalized)
        {
            return Err(EnvironmentError::InvalidRequest(format!(
                "path already exists as a file: {normalized}"
            )));
        }
        if parents {
            self.insert_directory_ancestors(&normalized)?;
        } else if let Some(parent) = parent_path(&normalized) {
            if !parent.is_empty() && !self.path_exists_unchecked(&parent)? {
                return Err(EnvironmentError::NotFound(parent));
            }
        }
        self.directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(normalized);
        Ok(())
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let normalized = normalize_requested_path(path)?;
        self.check_file(&normalized, true)?;
        let removed_file = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .remove(&normalized)
            .is_some()
            || self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .remove(&normalized)
                .is_some();
        if removed_file {
            return Ok(());
        }
        let prefix = format!("{}/", normalized.trim_end_matches('/'));
        let file_children = self
            .all_file_keys()?
            .into_iter()
            .filter(|entry| entry.starts_with(&prefix))
            .collect::<Vec<_>>();
        let dir_children = self
            .all_dir_keys()?
            .into_iter()
            .filter(|entry| entry.starts_with(&prefix))
            .collect::<Vec<_>>();
        let explicit_dir = self
            .directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains(&normalized);
        if !explicit_dir && file_children.is_empty() && dir_children.is_empty() {
            return Err(EnvironmentError::NotFound(path.to_string()));
        }
        if !recursive && (!file_children.is_empty() || !dir_children.is_empty()) {
            return Err(EnvironmentError::InvalidRequest(format!(
                "directory is not empty: {normalized}"
            )));
        }
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .retain(|entry, _| entry != &normalized && !entry.starts_with(&prefix));
        self.binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .retain(|entry, _| entry != &normalized && !entry.starts_with(&prefix));
        self.directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .retain(|entry| entry != &normalized && !entry.starts_with(&prefix));
        Ok(())
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = normalize_requested_path(src)?;
        let dst = normalize_requested_path(dst)?;
        self.check_file(&src, true)?;
        self.check_file(&dst, true)?;
        self.ensure_virtual_destination(&src, &dst, overwrite)?;
        self.copy_path(&src, &dst, overwrite).await?;
        self.delete_path(&src, true).await
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = normalize_requested_path(src)?;
        let dst = normalize_requested_path(dst)?;
        self.check_file(&src, false)?;
        self.check_file(&dst, true)?;
        self.ensure_virtual_destination(&src, &dst, overwrite)?;
        if overwrite && self.path_exists_unchecked(&dst)? {
            self.delete_path(&dst, true).await?;
        }

        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(&src)
            .cloned();
        if let Some(content) = text_content {
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(dst.clone(), content);
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .remove(&dst);
            return Ok(());
        }
        let binary_content = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(&src)
            .cloned();
        if let Some(content) = binary_content {
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(dst.clone(), content);
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .remove(&dst);
            return Ok(());
        }

        let prefix = format!("{}/", src.trim_end_matches('/'));
        let text_entries = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect::<Vec<_>>();
        let binary_entries = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect::<Vec<_>>();
        let dir_entries = self
            .all_dir_keys()?
            .into_iter()
            .filter(|path| path == &src || path.starts_with(&prefix))
            .collect::<Vec<_>>();
        if text_entries.is_empty() && binary_entries.is_empty() && dir_entries.is_empty() {
            return Err(EnvironmentError::NotFound(src));
        }

        for dir in dir_entries {
            let target = replace_logical_prefix(&dir, &src, &dst);
            self.directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(target);
        }
        for (path, content) in text_entries {
            let target = replace_logical_prefix(&path, &src, &dst);
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(target.clone(), content);
        }
        for (path, content) in binary_entries {
            let target = replace_logical_prefix(&path, &src, &dst);
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(target.clone(), content);
        }
        Ok(())
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let normalized = self.tmp_file_path(filename)?;
        self.binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(normalized.clone(), content.to_vec());
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .remove(&normalized);
        Ok(normalized)
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        self.check_file(path, false)?;
        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        if let Some(content) = text_content {
            return Ok(FileStat {
                size: content.len() as u64,
                is_file: true,
                is_dir: false,
                modified_unix_seconds: None,
            });
        }
        let binary_content = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        if let Some(content) = binary_content {
            return Ok(FileStat {
                size: content.len() as u64,
                is_file: true,
                is_dir: false,
                modified_unix_seconds: None,
            });
        }
        if self
            .directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains(path)
        {
            return Ok(FileStat {
                size: 0,
                is_file: false,
                is_dir: true,
                modified_unix_seconds: None,
            });
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        if self
            .all_file_keys()?
            .iter()
            .any(|entry| entry.starts_with(&prefix))
            || self
                .all_dir_keys()?
                .iter()
                .any(|entry| entry.starts_with(&prefix))
        {
            return Ok(FileStat {
                size: 0,
                is_file: false,
                is_dir: true,
                modified_unix_seconds: None,
            });
        }
        Err(EnvironmentError::NotFound(path.to_string()))
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        let normalized = normalize_requested_path(path)?;
        self.check_file(&normalized, false)?;
        let prefix = if normalized.is_empty() || normalized == "." {
            String::new()
        } else {
            format!("{}/", normalized.trim_end_matches('/'))
        };
        let mut entries = BTreeSet::new();
        {
            let files = self
                .files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            entries.extend(
                files
                    .keys()
                    .filter(|entry| entry.starts_with(&prefix))
                    .cloned(),
            );
        }
        {
            let binary_files = self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            entries.extend(
                binary_files
                    .keys()
                    .filter(|entry| entry.starts_with(&prefix))
                    .cloned(),
            );
        }
        {
            let directories = self
                .directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            entries.extend(
                directories
                    .iter()
                    .filter(|entry| entry.starts_with(&prefix))
                    .cloned(),
            );
        }
        Ok(entries.into_iter().collect())
    }

    async fn list_with_options(
        &self,
        path: &str,
        options: FileListOptions,
    ) -> EnvironmentResult<FileListResult> {
        let normalized = normalize_requested_path(path)?;
        self.check_file(&normalized, false)?;
        let prefix = if normalized.is_empty() || normalized == "." {
            String::new()
        } else {
            format!("{}/", normalized.trim_end_matches('/'))
        };
        if options.max_entries == 0 {
            let mut entries = BTreeSet::new();
            {
                let files = self
                    .files
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                entries.extend(files.keys().filter_map(|entry| {
                    list_entry_matches(&prefix, &options.ignore_patterns, entry)
                }));
            }
            {
                let binary_files = self
                    .binary_files
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                entries.extend(binary_files.keys().filter_map(|entry| {
                    list_entry_matches(&prefix, &options.ignore_patterns, entry)
                }));
            }
            {
                let directories = self
                    .directories
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                entries.extend(directories.iter().filter_map(|entry| {
                    list_entry_matches(&prefix, &options.ignore_patterns, entry)
                }));
            }
            let total_entries = entries.len();
            return Ok(FileListResult {
                entries: entries.into_iter().collect(),
                truncated: false,
                total_entries,
            });
        }

        let mut entries = BinaryHeap::new();
        let mut total_entries = 0usize;
        {
            let files = self
                .files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            collect_limited_list_entries(
                files.keys(),
                &prefix,
                &options.ignore_patterns,
                options.max_entries,
                &mut entries,
                &mut total_entries,
            );
        }
        {
            let binary_files = self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            collect_limited_list_entries(
                binary_files.keys(),
                &prefix,
                &options.ignore_patterns,
                options.max_entries,
                &mut entries,
                &mut total_entries,
            );
        }
        {
            let directories = self
                .directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            collect_limited_list_entries(
                directories.iter(),
                &prefix,
                &options.ignore_patterns,
                options.max_entries,
                &mut entries,
                &mut total_entries,
            );
        }
        let entries = entries.into_sorted_vec();
        Ok(FileListResult {
            truncated: total_entries > entries.len(),
            total_entries,
            entries,
        })
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
        for entry in self.all_file_keys()? {
            if path_contains(prefix, &entry)
                && include_path(&entry, options.include_hidden)
                && path_glob.is_match(strip_path_prefix(prefix, &entry))
            {
                glob_matches.push(FileGlobMatch { path: entry });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        self.shell_outputs
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(&command.command)
            .cloned()
            .ok_or(EnvironmentError::NotFound(command.command))
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        let mut files = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .clone();
        for path in self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .keys()
        {
            files.entry(path.clone()).or_default();
        }
        let tree = render_virtual_file_tree_listing(&files, ".", DEFAULT_FILE_TREE_MAX_DEPTH);
        Ok(Some(render_environment_context_xml(
            self.id(),
            ".",
            None,
            &[FileTreeBlock {
                path: ".".to_string(),
                listing_text: tree,
            }],
            self.policy.shell.allow_execute,
            None,
        )))
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        Ok(EnvironmentState {
            provider_id: self.id.clone(),
            files: self
                .files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .clone(),
            resources: self
                .resources
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .clone(),
            processes: self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .values()
                .cloned()
                .collect(),
            metadata: Metadata::from_iter([(
                crate::ENVIRONMENT_PROVIDER_KIND_KEY.to_string(),
                serde_json::json!("virtual"),
            )]),
        })
    }
}

fn list_entry_matches(prefix: &str, ignore_patterns: &[String], entry: &str) -> Option<String> {
    if entry.starts_with(prefix) && !list_ignore_match(ignore_patterns, entry) {
        Some(entry.to_string())
    } else {
        None
    }
}

fn collect_limited_list_entries<'a, I>(
    candidates: I,
    prefix: &str,
    ignore_patterns: &[String],
    max_entries: usize,
    entries: &mut BinaryHeap<String>,
    total_entries: &mut usize,
) where
    I: IntoIterator<Item = &'a String>,
{
    for entry in candidates {
        let Some(entry) = list_entry_matches(prefix, ignore_patterns, entry) else {
            continue;
        };
        *total_entries = total_entries.saturating_add(1);
        if entries.len() < max_entries {
            entries.push(entry);
        } else if entries.peek().is_some_and(|largest| entry < *largest) {
            entries.pop();
            entries.push(entry);
        }
    }
}
