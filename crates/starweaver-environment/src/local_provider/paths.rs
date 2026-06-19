//! Local provider path resolution helpers.

use std::path::{Path, PathBuf};

use crate::{
    display_local_path, is_absolute_request_path, is_tmp_path, normalize_absolute_request_path,
    normalize_local_config_path, normalize_path, normalize_requested_path, normalize_str_path,
    push_unique_path, EnvironmentError, EnvironmentResult, DEFAULT_TMP_DIR,
};

use super::LocalEnvironmentProvider;

impl LocalEnvironmentProvider {
    pub(super) fn resolve_provider_path(
        &self,
        path: &str,
        write: bool,
    ) -> EnvironmentResult<PathBuf> {
        let (visible_path, filesystem_path) = self.resolve_request_path_with_logical_path(path)?;
        if !self.path_is_managed_tmp(&filesystem_path)
            && !self.policy.files.permits(&visible_path, write)
        {
            return Err(EnvironmentError::AccessDenied(path.to_string()));
        }
        Ok(filesystem_path)
    }

    pub(super) fn resolve_request_path_with_logical_path(
        &self,
        path: &str,
    ) -> EnvironmentResult<(String, PathBuf)> {
        let requested = Path::new(path);
        if is_absolute_request_path(requested) {
            let filesystem_path = normalize_absolute_request_path(requested)?;
            if !self.is_under_allowed_roots(&filesystem_path) {
                return Err(EnvironmentError::AccessDenied(path.to_string()));
            }
            let visible_path = self.logical_provider_path(&filesystem_path)?;
            return Ok((visible_path, filesystem_path));
        }

        let mut logical_path = normalize_requested_path(path)?;
        if logical_path == "." {
            logical_path.clear();
        }
        if let Some(tmp_path) = self.managed_tmp_path(&logical_path)? {
            return Ok((logical_path, tmp_path));
        }
        Ok((logical_path.clone(), self.root.join(&logical_path)))
    }

    pub(super) fn is_under_allowed_roots(&self, path: &Path) -> bool {
        self.allowed_paths
            .iter()
            .any(|allowed_path| path == allowed_path || path.starts_with(allowed_path))
    }

    pub(super) fn is_exact_allowed_root(&self, path: &Path) -> bool {
        self.allowed_paths
            .iter()
            .any(|allowed_path| path == allowed_path)
    }

    pub(super) fn logical_provider_path(&self, path: &Path) -> EnvironmentResult<String> {
        let path = normalize_local_config_path(path.to_path_buf());
        if self.path_is_managed_tmp(&path) {
            return Ok(display_local_path(&path));
        }
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
        let mut allowed_paths = Vec::new();
        for path in paths {
            push_unique_path(&mut allowed_paths, normalize_local_config_path(path));
        }
        push_unique_path(&mut allowed_paths, self.root.clone());
        if let Some(tmp_dir) = self.tmp_dir_path() {
            push_unique_path(
                &mut allowed_paths,
                normalize_local_config_path(tmp_dir.to_path_buf()),
            );
        }
        self.allowed_paths = allowed_paths;
    }

    pub(super) fn managed_tmp_path(
        &self,
        logical_path: &str,
    ) -> EnvironmentResult<Option<PathBuf>> {
        if !is_tmp_path(logical_path) {
            return Ok(None);
        }
        let Some(tmp_dir) = self.tmp_dir_path() else {
            return Ok(None);
        };
        let normalized = normalize_str_path(logical_path);
        let relative = normalized
            .strip_prefix(DEFAULT_TMP_DIR)
            .and_then(|suffix| suffix.strip_prefix('/'))
            .unwrap_or_default();
        if relative.is_empty() {
            return Ok(Some(tmp_dir.to_path_buf()));
        }
        Ok(Some(tmp_dir.join(normalize_requested_path(relative)?)))
    }

    pub(crate) fn path_is_managed_tmp(&self, path: &Path) -> bool {
        let path = normalize_local_config_path(path.to_path_buf());
        self.tmp_dir_path().is_some_and(|tmp_dir| {
            let tmp_dir = normalize_local_config_path(tmp_dir.to_path_buf());
            path == tmp_dir || path.starts_with(tmp_dir)
        })
    }
}
