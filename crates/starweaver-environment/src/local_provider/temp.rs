//! Local provider temporary path and shell environment helpers.

use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    EnvironmentResult, display_local_path, join_logical_path, map_io_error, normalize_tmp_filename,
};

use super::LocalEnvironmentProvider;

impl LocalEnvironmentProvider {
    pub(super) fn tmp_file_relative_path(&self, filename: &str) -> EnvironmentResult<String> {
        let filename = normalize_tmp_filename(filename)?;
        Ok(self.tmp_namespace.as_deref().map_or_else(
            || filename.clone(),
            |namespace| join_logical_path(namespace, &filename),
        ))
    }

    pub(super) fn shell_tmp_dir_path(&self) -> EnvironmentResult<Option<PathBuf>> {
        let Some(tmp_dir) = self.tmp_dir_path() else {
            return Ok(None);
        };
        let path = self.tmp_namespace.as_deref().map_or_else(
            || tmp_dir.to_path_buf(),
            |namespace| tmp_dir.join(namespace),
        );
        std::fs::create_dir_all(&path).map_err(|error| map_io_error(&path, &error))?;
        Ok(Some(path))
    }

    pub(super) fn shell_environment(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> EnvironmentResult<BTreeMap<String, String>> {
        let mut environment = environment.clone();
        if let Some(tmp_dir) = self.shell_tmp_dir_path()? {
            let tmp_dir = display_local_path(&tmp_dir);
            for key in ["TMPDIR", "TMP", "TEMP"] {
                environment
                    .entry(key.to_string())
                    .or_insert_with(|| tmp_dir.clone());
            }
        }
        Ok(environment)
    }
}
