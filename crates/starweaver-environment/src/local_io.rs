//! Local filesystem helper functions for the local environment provider.

use std::{
    io::{self, Write},
    path::Path,
};

use crate::{EnvironmentError, EnvironmentResult, LOCAL_SCRATCH_DIR_PREFIX};

pub const LOCAL_WORKSPACE_TMP_DIR: &str = ".starweaver/tmp";

const LOCAL_WORKSPACE_TMP_GITIGNORE: &str = "*\n";

pub fn map_io_error(path: &Path, error: &io::Error) -> EnvironmentError {
    match error.kind() {
        io::ErrorKind::NotFound => EnvironmentError::NotFound(path.display().to_string()),
        io::ErrorKind::PermissionDenied => {
            EnvironmentError::AccessDenied(path.display().to_string())
        }
        _ => EnvironmentError::Provider(error.to_string()),
    }
}

pub fn create_local_scratch_dir(base_dir: Option<&Path>) -> EnvironmentResult<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(LOCAL_SCRATCH_DIR_PREFIX);
    match base_dir {
        Some(base_dir) => {
            std::fs::create_dir_all(base_dir).map_err(|error| map_io_error(base_dir, &error))?;
            builder
                .tempdir_in(base_dir)
                .map_err(|error| map_io_error(base_dir, &error))
        }
        None => builder
            .tempdir()
            .map_err(|error| EnvironmentError::Provider(error.to_string())),
    }
}

pub fn create_local_scratch_dir_with_workspace_fallback(
    workspace_root: &Path,
) -> EnvironmentResult<tempfile::TempDir> {
    use_workspace_tmp_fallback(workspace_root, create_local_scratch_dir(None))
}

#[cfg(test)]
pub fn create_local_scratch_dir_with_workspace_fallback_from(
    workspace_root: &Path,
    preferred_base_dir: &Path,
) -> EnvironmentResult<tempfile::TempDir> {
    use_workspace_tmp_fallback(
        workspace_root,
        create_local_scratch_dir(Some(preferred_base_dir)),
    )
}

fn use_workspace_tmp_fallback(
    workspace_root: &Path,
    primary_result: EnvironmentResult<tempfile::TempDir>,
) -> EnvironmentResult<tempfile::TempDir> {
    match primary_result {
        Ok(scratch_dir) => Ok(scratch_dir),
        Err(primary_error) => create_local_workspace_tmp_dir(workspace_root).map_err(
            |fallback_error| {
                EnvironmentError::Provider(format!(
                    "failed to create local scratch in the OS temporary directory ({primary_error}); workspace fallback also failed ({fallback_error})"
                ))
            },
        ),
    }
}

pub fn create_local_workspace_tmp_dir(
    workspace_root: &Path,
) -> EnvironmentResult<tempfile::TempDir> {
    ensure_workspace_directory(workspace_root)?;
    let starweaver_dir = workspace_root.join(".starweaver");
    ensure_workspace_directory(&starweaver_dir)?;
    let tmp_dir = workspace_root.join(LOCAL_WORKSPACE_TMP_DIR);
    ensure_workspace_directory(&tmp_dir)?;
    initialize_workspace_tmp_gitignore(&tmp_dir)?;
    create_local_scratch_dir(Some(&tmp_dir))
}

fn ensure_workspace_directory(path: &Path) -> EnvironmentResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(EnvironmentError::AccessDenied(format!(
                "workspace tmp directory must not be a symlink: {}",
                path.display()
            )))
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(EnvironmentError::InvalidRequest(format!(
            "workspace tmp path is not a directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(map_io_error(path, &error)),
            }
            let metadata =
                std::fs::symlink_metadata(path).map_err(|error| map_io_error(path, &error))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(EnvironmentError::AccessDenied(format!(
                    "workspace tmp directory was replaced during initialization: {}",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(error) => Err(map_io_error(path, &error)),
    }
}

fn initialize_workspace_tmp_gitignore(tmp_dir: &Path) -> EnvironmentResult<()> {
    let gitignore = tmp_dir.join(".gitignore");
    match std::fs::symlink_metadata(&gitignore) {
        Ok(_) => return validate_workspace_tmp_gitignore(&gitignore),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(map_io_error(&gitignore, &error)),
    }

    let mut temporary = tempfile::Builder::new()
        .prefix(".gitignore-")
        .tempfile_in(tmp_dir)
        .map_err(|error| map_io_error(tmp_dir, &error))?;
    temporary
        .write_all(LOCAL_WORKSPACE_TMP_GITIGNORE.as_bytes())
        .map_err(|error| map_io_error(temporary.path(), &error))?;
    match temporary.persist_noclobber(&gitignore) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            validate_workspace_tmp_gitignore(&gitignore)
        }
        Err(error) => Err(map_io_error(&gitignore, &error.error)),
    }
}

fn validate_workspace_tmp_gitignore(gitignore: &Path) -> EnvironmentResult<()> {
    let metadata =
        std::fs::symlink_metadata(gitignore).map_err(|error| map_io_error(gitignore, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(EnvironmentError::InvalidRequest(format!(
            "workspace tmp .gitignore is not a regular file: {}",
            gitignore.display()
        )));
    }
    Ok(())
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
