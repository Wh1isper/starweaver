//! Owner-private durable file helpers shared by RPC-owned registries.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use serde::Serialize;
use uuid::Uuid;

use crate::{RpcHostError, RpcHostResult};

pub(crate) fn open_private_lock(path: &Path) -> RpcHostResult<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    validate_private_regular_file(&file, path)?;
    Ok(file)
}

pub(crate) fn atomic_write_json(
    path: &Path,
    value: &impl Serialize,
    description: &str,
) -> RpcHostResult<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| RpcHostError::Storage(format!("encode {description}: {error}")))?;
    atomic_write(path, &bytes, description)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], description: &str) -> RpcHostResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| RpcHostError::Storage(format!("{description} has no parent directory")))?;
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(parent, path);
    let write_result = (|| -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        atomic_replace(&temporary, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result.map_err(Into::into)
}

fn temporary_path(parent: &Path, destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rpc-state");
    parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()))
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    atomicwrites::replace_atomic(source, destination)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    OpenOptions::new().read(true).open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

fn validate_private_regular_file(file: &fs::File, path: &Path) -> RpcHostResult<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(RpcHostError::Storage(format!(
            "private RPC state is not a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        if metadata.mode() & 0o077 != 0 {
            return Err(RpcHostError::Storage(format!(
                "private RPC state has unsafe permissions: {}",
                path.display()
            )));
        }
    }
    Ok(())
}
