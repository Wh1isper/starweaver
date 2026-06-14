//! Local filesystem helper functions for the local environment provider.

use std::{io, path::Path};

use crate::{EnvironmentError, EnvironmentResult, LOCAL_TMP_DIR_PREFIX};

pub fn map_io_error(path: &Path, error: &io::Error) -> EnvironmentError {
    match error.kind() {
        io::ErrorKind::NotFound => EnvironmentError::NotFound(path.display().to_string()),
        io::ErrorKind::PermissionDenied => {
            EnvironmentError::AccessDenied(path.display().to_string())
        }
        _ => EnvironmentError::Provider(error.to_string()),
    }
}

pub fn create_local_tmp_dir(base_dir: Option<&Path>) -> Option<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(LOCAL_TMP_DIR_PREFIX);
    match base_dir {
        Some(base_dir) => {
            if std::fs::create_dir_all(base_dir).is_err() {
                return None;
            }
            builder.tempdir_in(base_dir).ok()
        }
        None => builder.tempdir().ok(),
    }
}

pub fn prepare_local_destination(path: &Path, overwrite: bool) -> EnvironmentResult<()> {
    if path.exists() {
        if !overwrite {
            return Err(EnvironmentError::InvalidRequest(format!(
                "destination already exists: {}",
                path.display()
            )));
        }
        let metadata = std::fs::metadata(path).map_err(|error| map_io_error(path, &error))?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(path).map_err(|error| map_io_error(path, &error))?;
        } else {
            std::fs::remove_file(path).map_err(|error| map_io_error(path, &error))?;
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| map_io_error(parent, &error))?;
    }
    Ok(())
}

pub fn copy_local_dir(src: &Path, dst: &Path) -> EnvironmentResult<()> {
    std::fs::create_dir_all(dst).map_err(|error| map_io_error(dst, &error))?;
    for entry in std::fs::read_dir(src).map_err(|error| map_io_error(src, &error))? {
        let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let source_path = entry.path();
        let destination_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        if file_type.is_dir() {
            copy_local_dir(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &destination_path)
                .map(|_| ())
                .map_err(|error| map_io_error(&source_path, &error))?;
        }
    }
    Ok(())
}
