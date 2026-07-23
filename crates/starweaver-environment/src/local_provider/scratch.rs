//! Local provider scratch path and shell environment helpers.

use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    EnvironmentResult, display_local_path, join_logical_path, map_io_error,
    normalize_scratch_filename,
};

use super::LocalEnvironmentProvider;

impl LocalEnvironmentProvider {
    pub(super) fn scratch_file_relative_path(&self, filename: &str) -> EnvironmentResult<String> {
        let filename = normalize_scratch_filename(filename)?;
        Ok(self.scratch_namespace.as_deref().map_or_else(
            || filename.clone(),
            |namespace| join_logical_path(namespace, &filename),
        ))
    }

    pub(crate) fn shell_scratch_dir_path(&self) -> EnvironmentResult<PathBuf> {
        let relative_path = self.scratch_namespace.as_deref().unwrap_or_default();
        let path = self.resolve_scratch_relative_path(relative_path, true)?;
        std::fs::create_dir_all(&path).map_err(|error| map_io_error(&path, &error))?;
        self.resolve_scratch_relative_path(relative_path, true)
    }

    pub(super) fn shell_environment(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> EnvironmentResult<BTreeMap<String, String>> {
        let mut environment = environment.clone();
        let scratch_dir = display_local_path(&self.shell_scratch_dir_path()?);
        for key in ["TMPDIR", "TMP", "TEMP"] {
            environment
                .entry(key.to_string())
                .or_insert_with(|| scratch_dir.clone());
        }
        Ok(environment)
    }
}
