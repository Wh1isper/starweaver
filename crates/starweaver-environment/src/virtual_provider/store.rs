//! Virtual provider in-memory store helpers.

use std::collections::BTreeSet;

use crate::{
    DEFAULT_SCRATCH_DIR, EnvironmentError, EnvironmentResult, is_scratch_path, join_logical_path,
    logical_ancestors, normalize_scratch_filename,
};

use super::VirtualEnvironmentProvider;

impl VirtualEnvironmentProvider {
    pub(super) fn check_file(&self, path: &str, write: bool) -> EnvironmentResult<()> {
        if is_scratch_path(path) || self.policy.files.permits(path, write) {
            Ok(())
        } else {
            Err(EnvironmentError::AccessDenied(path.to_string()))
        }
    }

    pub(super) fn all_file_keys(&self) -> EnvironmentResult<Vec<String>> {
        let mut keys = BTreeSet::new();
        keys.extend(
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .keys()
                .cloned(),
        );
        keys.extend(
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .keys()
                .cloned(),
        );
        Ok(keys.into_iter().collect())
    }

    pub(super) fn all_dir_keys(&self) -> EnvironmentResult<Vec<String>> {
        Ok(self
            .directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .iter()
            .cloned()
            .collect())
    }

    pub(super) fn insert_directory_ancestors(&self, path: &str) -> EnvironmentResult<()> {
        {
            let mut directories = self
                .directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            for ancestor in logical_ancestors(path) {
                directories.insert(ancestor);
            }
        }
        Ok(())
    }

    pub(super) fn scratch_file_path(&self, filename: &str) -> EnvironmentResult<String> {
        let filename = normalize_scratch_filename(filename)?;
        let relative = self.scratch_namespace.as_deref().map_or_else(
            || filename.clone(),
            |namespace| join_logical_path(namespace, &filename),
        );
        Ok(join_logical_path(DEFAULT_SCRATCH_DIR, &relative))
    }

    pub(super) fn path_exists_unchecked(&self, path: &str) -> EnvironmentResult<bool> {
        if self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains_key(path)
            || self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .contains_key(path)
            || self
                .directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .contains(path)
        {
            return Ok(true);
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        Ok(self
            .all_file_keys()?
            .iter()
            .any(|entry| entry.starts_with(&prefix))
            || self
                .all_dir_keys()?
                .iter()
                .any(|entry| entry.starts_with(&prefix)))
    }

    pub(super) fn ensure_virtual_destination(
        &self,
        src: &str,
        dst: &str,
        overwrite: bool,
    ) -> EnvironmentResult<()> {
        if src == dst {
            return Err(EnvironmentError::InvalidRequest(
                "source and destination must differ".to_string(),
            ));
        }
        if self.path_exists_unchecked(dst)? && !overwrite {
            return Err(EnvironmentError::InvalidRequest(format!(
                "destination already exists: {dst}"
            )));
        }
        Ok(())
    }
}
