//! Atomic replacement for small CLI-owned state files.

use std::{
    fs::File,
    io::{self, Write as _},
    path::Path,
};

/// Write bytes to a temporary sibling and atomically replace the destination.
///
/// `tempfile::NamedTempFile::persist` uses the platform replacement primitive, including replacing
/// an existing destination on Windows where `std::fs::rename` does not provide that behavior.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}
