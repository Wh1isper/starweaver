//! Local provider path resolution helpers.

use std::{
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use crate::{
    EnvironmentError, EnvironmentResult, absolute_request_path, display_local_path,
    is_absolute_request_path, map_io_error, normalize_absolute_request_path,
    normalize_local_config_path, normalize_path, normalize_requested_path, push_unique_path,
};

use super::LocalEnvironmentProvider;

impl LocalEnvironmentProvider {
    pub(super) fn resolve_provider_path(
        &self,
        path: &str,
        write: bool,
    ) -> EnvironmentResult<PathBuf> {
        let (_, filesystem_path) = self.resolve_authorized_request_path(path, write)?;
        let allow_trusted_allowed_root_ancestors =
            !self.path_targets_managed_scratch(&filesystem_path);
        self.resolve_physical_path(
            &filesystem_path,
            write,
            path,
            allow_trusted_allowed_root_ancestors,
        )
    }

    pub(super) fn resolve_provider_search_root(
        &self,
        path: &str,
    ) -> EnvironmentResult<Option<PathBuf>> {
        let (_, filesystem_path) = self.resolve_authorized_request_path(path, false)?;
        match std::fs::symlink_metadata(&filesystem_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(None),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(map_io_error(&filesystem_path, &error)),
        }
        let allow_trusted_allowed_root_ancestors =
            !self.path_targets_managed_scratch(&filesystem_path);
        self.resolve_physical_path(
            &filesystem_path,
            false,
            path,
            allow_trusted_allowed_root_ancestors,
        )
        .map(Some)
    }

    /// Resolve a path entry for operations that mutate the entry itself instead
    /// of following it to mutate its target (for example unlink or rename).
    pub(super) fn resolve_provider_entry_path(&self, path: &str) -> EnvironmentResult<PathBuf> {
        let (_, filesystem_path) = self.resolve_authorized_request_path(path, true)?;
        self.resolve_entry_path(&filesystem_path, path)
    }

    fn resolve_authorized_request_path(
        &self,
        path: &str,
        write: bool,
    ) -> EnvironmentResult<(String, PathBuf)> {
        let (visible_path, filesystem_path) = self.resolve_request_path_with_logical_path(path)?;
        if !self.path_targets_managed_scratch(&filesystem_path)
            && !self.policy.files.permits(&visible_path, write)
        {
            return Err(EnvironmentError::AccessDenied(path.to_string()));
        }
        Ok((visible_path, filesystem_path))
    }

    pub(super) fn resolve_scratch_relative_path(
        &self,
        relative_path: &str,
        write: bool,
    ) -> EnvironmentResult<PathBuf> {
        let scratch_dir = self.scratch_dir_path();
        // `tempfile` can return a path beneath a platform-owned symlink (such
        // as macOS `/var`). Normalize only the managed directory's parent so
        // the managed directory itself remains visible to the containment
        // check if it has been replaced with a symlink.
        let parent = scratch_dir.parent().ok_or_else(|| {
            EnvironmentError::InvalidRequest("local scratch directory has no parent".to_string())
        })?;
        let file_name = scratch_dir.file_name().ok_or_else(|| {
            EnvironmentError::InvalidRequest("local scratch directory has no name".to_string())
        })?;
        let normalized_relative_path = normalize_requested_path(relative_path)?;
        let scratch_root = normalize_local_config_path(parent.to_path_buf()).join(file_name);
        let path = scratch_root.join(normalized_relative_path);
        let requested_path = display_local_path(&path);
        let resolved_path = self.resolve_physical_path(&path, write, &requested_path, false)?;
        if !resolved_path.starts_with(&scratch_root) {
            return Err(EnvironmentError::AccessDenied(requested_path));
        }
        Ok(resolved_path)
    }

    fn resolve_physical_path(
        &self,
        path: &Path,
        write: bool,
        requested_path: &str,
        allow_trusted_allowed_root_ancestors: bool,
    ) -> EnvironmentResult<PathBuf> {
        if write {
            self.reject_existing_symlink_components(
                path,
                requested_path,
                allow_trusted_allowed_root_ancestors,
            )?;
        }
        let resolved_path = normalize_local_config_path(path.to_path_buf());
        if !self.is_under_allowed_roots(&resolved_path) {
            return Err(EnvironmentError::AccessDenied(requested_path.to_string()));
        }
        Ok(resolved_path)
    }

    fn resolve_entry_path(&self, path: &Path, requested_path: &str) -> EnvironmentResult<PathBuf> {
        let parent = path.parent().ok_or_else(|| {
            EnvironmentError::InvalidRequest(format!("path has no parent: {requested_path}"))
        })?;
        let file_name = path.file_name().ok_or_else(|| {
            EnvironmentError::InvalidRequest(format!("path has no file name: {requested_path}"))
        })?;
        self.reject_existing_symlink_components(
            parent,
            requested_path,
            !self.path_targets_managed_scratch(path),
        )?;
        let resolved_parent = normalize_local_config_path(parent.to_path_buf());
        if !self.is_under_allowed_roots(&resolved_parent) {
            return Err(EnvironmentError::AccessDenied(requested_path.to_string()));
        }
        Ok(resolved_parent.join(file_name))
    }

    fn reject_existing_symlink_components(
        &self,
        path: &Path,
        requested_path: &str,
        allow_trusted_allowed_root_ancestors: bool,
    ) -> EnvironmentResult<()> {
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component.as_os_str());
            // On Windows, a prefix alone (for example `C:` or `\\?\C:`) is
            // not a complete path and cannot be queried for metadata.
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let resolved_component = normalize_local_config_path(current.clone());
                    let is_trusted_allowed_root_ancestor = allow_trusted_allowed_root_ancestors
                        && self.effective_allowed_paths.iter().any(|allowed_root| {
                            allowed_root != &resolved_component
                                && allowed_root.starts_with(&resolved_component)
                        });
                    if !is_trusted_allowed_root_ancestor {
                        return Err(EnvironmentError::AccessDenied(requested_path.to_string()));
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(map_io_error(&current, &error)),
            }
        }
        Ok(())
    }

    pub(super) fn resolve_request_path_with_logical_path(
        &self,
        path: &str,
    ) -> EnvironmentResult<(String, PathBuf)> {
        let requested = Path::new(path);
        if is_absolute_request_path(requested) {
            let filesystem_path = absolute_request_path(requested)?;
            let resolved_path = normalize_absolute_request_path(requested)?;
            if !self.is_under_allowed_roots(&resolved_path) {
                return Err(EnvironmentError::AccessDenied(path.to_string()));
            }
            let visible_path = self.logical_provider_path(&resolved_path)?;
            return Ok((visible_path, filesystem_path));
        }

        let mut logical_path = normalize_requested_path(path)?;
        if logical_path == "." {
            logical_path.clear();
        }
        Ok((logical_path.clone(), self.root.join(&logical_path)))
    }

    pub(super) fn is_under_allowed_roots(&self, path: &Path) -> bool {
        self.effective_allowed_paths
            .iter()
            .any(|allowed_path| path == allowed_path || path.starts_with(allowed_path))
    }

    pub(super) fn is_exact_allowed_root(&self, path: &Path) -> bool {
        self.effective_allowed_paths
            .iter()
            .any(|allowed_path| path == allowed_path)
    }

    pub(super) fn logical_provider_path(&self, path: &Path) -> EnvironmentResult<String> {
        let path = normalize_local_config_path(path.to_path_buf());
        if let Ok(relative) = path.strip_prefix(&self.root) {
            return Ok(normalize_path(relative));
        }
        if self.is_under_allowed_roots(&path) {
            return Ok(display_local_path(&path));
        }
        Err(EnvironmentError::AccessDenied(path.display().to_string()))
    }

    pub(super) fn logical_root_for_allowed_path(&self, path: &Path) -> String {
        let path = normalize_local_config_path(path.to_path_buf());
        path.strip_prefix(&self.root)
            .map_or_else(|_| display_local_path(&path), normalize_path)
    }

    pub(super) fn resolve_shell_cwd(&self, cwd: Option<&str>) -> EnvironmentResult<PathBuf> {
        let cwd = match cwd {
            Some(cwd) => self.resolve_provider_path(cwd, false)?,
            None => self.root.clone(),
        };
        if !cwd.is_dir() {
            return Err(EnvironmentError::InvalidRequest(format!(
                "shell cwd is not a directory: {}",
                cwd.display()
            )));
        }
        Ok(cwd)
    }

    pub(super) fn rebuild_allowed_paths_with_managed_roots(&mut self, paths: Vec<PathBuf>) {
        let mut configured_allowed_paths = Vec::new();
        for path in paths {
            push_unique_path(
                &mut configured_allowed_paths,
                normalize_local_config_path(path),
            );
        }
        push_unique_path(&mut configured_allowed_paths, self.root.clone());
        self.configured_allowed_paths = configured_allowed_paths;
        self.rebuild_effective_allowed_paths();
    }

    pub(super) fn rebuild_effective_allowed_paths(&mut self) {
        let scratch_dir = normalize_local_config_path(self.scratch_dir_path().to_path_buf());
        self.effective_allowed_paths = self.configured_allowed_paths.clone();
        push_unique_path(&mut self.effective_allowed_paths, scratch_dir);
    }

    fn path_targets_managed_scratch(&self, path: &Path) -> bool {
        let scratch_dir = self.scratch_dir_path();
        path == scratch_dir || path.starts_with(scratch_dir)
    }

    #[cfg(test)]
    pub(crate) fn path_is_managed_scratch(&self, path: &Path) -> bool {
        let path = normalize_local_config_path(path.to_path_buf());
        let scratch_dir = normalize_local_config_path(self.scratch_dir_path().to_path_buf());
        path == scratch_dir || path.starts_with(scratch_dir)
    }
}
