//! Compile-time identity for an installed CLI distribution.

/// CLI distribution version derived from a release tag, or the package version in development.
pub(crate) const VERSION: &str = env!("STARWEAVER_BUILD_VERSION");
/// Rust SDK package version used to compile this distribution.
pub(crate) const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Source Git revision recorded by release automation.
pub(crate) const REVISION: &str = env!("STARWEAVER_BUILD_REVISION");
/// Rust target triple for the built distribution.
pub(crate) const TARGET: &str = env!("STARWEAVER_BUILD_TARGET");
