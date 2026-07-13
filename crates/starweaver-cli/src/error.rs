//! CLI error types.

use std::{io, path::PathBuf};

/// CLI result alias.
pub type CliResult<T> = Result<T, CliError>;

/// CLI failure.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Command-line parser failed.
    #[error("{0}")]
    Usage(String),
    /// Command-line parser requested display output.
    #[error("{0}")]
    Display(String),
    /// Configuration loading failed.
    #[error("configuration error: {0}")]
    Config(String),
    /// Local storage failed.
    #[error("storage error: {0}")]
    Storage(String),
    /// Requested record was missing.
    #[error("not found: {0}")]
    NotFound(String),
    /// Filesystem operation failed.
    #[error("filesystem error at {}: {source}", path.display())]
    Io {
        /// Path involved in the failure.
        path: PathBuf,
        /// Source IO error.
        #[source]
        source: io::Error,
    },
    /// Runtime execution failed.
    #[error("run failed: {0}")]
    Run(String),
    /// Serialization failed.
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}

impl From<toml::de::Error> for CliError {
    fn from(error: toml::de::Error) -> Self {
        Self::Config(error.to_string())
    }
}

impl From<toml::ser::Error> for CliError {
    fn from(error: toml::ser::Error) -> Self {
        Self::Config(error.to_string())
    }
}

/// Map an IO error with a path.
pub fn io_error(path: impl Into<PathBuf>, source: io::Error) -> CliError {
    CliError::Io {
        path: path.into(),
        source,
    }
}
