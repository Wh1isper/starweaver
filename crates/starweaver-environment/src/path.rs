//! Provider path normalization and ripgrep-style path pattern matching.

use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;

use crate::{EnvironmentError, EnvironmentResult};

pub const DEFAULT_TMP_DIR: &str = ".starweaver/tmp";
pub const LOCAL_TMP_DIR_PREFIX: &str = "starweaver-";

pub fn join_logical_path(root: &str, child: &str) -> String {
    if root.is_empty() || root == "." {
        child.to_string()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), child)
    }
}

pub fn parent_path(path: &str) -> Option<String> {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string())
}

pub fn logical_ancestors(path: &str) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut current = normalize_str_path(path);
    while let Some(parent) = parent_path(&current) {
        if parent.is_empty() {
            break;
        }
        ancestors.push(parent.clone());
        current = parent;
    }
    ancestors
}

pub fn replace_logical_prefix(path: &str, src: &str, dst: &str) -> String {
    if path == src {
        return dst.to_string();
    }
    path.strip_prefix(src)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .map_or_else(|| path.to_string(), |suffix| join_logical_path(dst, suffix))
}

pub struct PathGlob {
    matcher: GlobMatcher,
    recursive_prefix_matcher: Option<GlobMatcher>,
    pattern: String,
    anchored: bool,
}

impl PathGlob {
    pub(crate) fn new(pattern: &str) -> EnvironmentResult<Self> {
        let mut normalized = pattern.replace('\\', "/");
        if normalized.is_empty() {
            normalized = "**/*".to_string();
        }
        if let Some(stripped) = normalized.strip_prefix("./") {
            normalized = stripped.to_string();
        }
        let anchored = normalized.starts_with('/');
        let glob_pattern = if anchored {
            let stripped = normalized.trim_start_matches('/');
            if stripped.is_empty() {
                "*"
            } else {
                stripped
            }
        } else {
            normalized.as_str()
        };
        let matcher = compile_glob(glob_pattern)?;
        let recursive_prefix_matcher = glob_pattern
            .strip_prefix("**/")
            .map(compile_glob)
            .transpose()?;
        Ok(Self {
            matcher,
            recursive_prefix_matcher,
            pattern: glob_pattern.to_string(),
            anchored,
        })
    }

    pub(crate) fn is_match(&self, path: &str) -> bool {
        let normalized = normalize_str_path(path);
        if self.anchored && !self.pattern.contains('/') && normalized.contains('/') {
            return false;
        }
        if self.pattern == "**" || self.pattern == "**/*" {
            return true;
        }
        if self.matcher.is_match(&normalized) {
            return true;
        }
        if let Some(matcher) = &self.recursive_prefix_matcher {
            if matcher.is_match(&normalized) {
                return true;
            }
        }
        if !self.anchored && !self.pattern.contains('/') {
            if let Some(name) = normalized.rsplit('/').next() {
                return self.matcher.is_match(name);
            }
        }
        false
    }
}

fn compile_glob(pattern: &str) -> EnvironmentResult<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))
        .map(|glob| glob.compile_matcher())
}

/// Normalize a provider path for glob-style matching.
#[must_use]
pub fn normalize_match_path(path: &str) -> String {
    normalize_str_path(path)
}

/// Return default path candidates for provider-scoped policy matching.
#[must_use]
pub fn path_match_candidates(path: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique_candidate(&mut candidates, path.replace('\\', "/"));
    push_unique_candidate(&mut candidates, normalize_match_path(path));
    candidates
}

/// Match one path candidate against a relaxed-view pattern.
///
/// Patterns prefixed with `re:` are interpreted as regular expressions;
/// all other patterns use the same ripgrep-style glob semantics as provider
/// glob searches.
///
/// # Errors
///
/// Returns an error when the glob or regular expression pattern is invalid.
pub fn matches_path_pattern(path: &str, pattern: &str) -> EnvironmentResult<bool> {
    let pattern = pattern.trim();
    if let Some(regex) = pattern.strip_prefix("re:") {
        let matcher = RegexMatcher::new(regex)
            .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?;
        return matcher
            .is_match(path.as_bytes())
            .map_err(|error| EnvironmentError::Provider(error.to_string()));
    }
    PathGlob::new(pattern).map(|path_glob| path_glob.is_match(path))
}

pub fn push_unique_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

pub fn normalize_path(path: &Path) -> String {
    normalize_str_path(&path.to_string_lossy())
}

pub fn normalize_local_config_path(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_or_else(|_| path.clone(), |current_dir| current_dir.join(&path))
    };
    absolute.canonicalize().unwrap_or(absolute)
}

pub fn normalize_absolute_request_path(path: &Path) -> EnvironmentResult<PathBuf> {
    let normalized_path = normalize_absolute_request_path_input(path);
    if !normalized_path.is_absolute()
        || normalized_path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(EnvironmentError::InvalidRequest(path.display().to_string()));
    }
    Ok(normalize_local_config_path(normalized_path))
}

pub fn is_absolute_request_path(path: &Path) -> bool {
    normalize_absolute_request_path_input(path).is_absolute()
}

#[cfg(windows)]
fn normalize_absolute_request_path_input(path: &Path) -> PathBuf {
    let path = path.to_string_lossy();
    if let Some(stripped) = strip_windows_verbatim_prefix(&path) {
        return PathBuf::from(stripped);
    }
    if let Some(tmp_path) = windows_msys_tmp_path(&path) {
        return tmp_path;
    }
    windows_msys_drive_path(&path).map_or_else(|| PathBuf::from(path.as_ref()), PathBuf::from)
}

#[cfg(not(windows))]
fn normalize_absolute_request_path_input(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(windows)]
pub fn display_local_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    strip_windows_verbatim_prefix(&path)
        .unwrap_or_else(|| path.into_owned())
        .replace('\\', "/")
}

#[cfg(not(windows))]
pub fn display_local_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: &str) -> Option<String> {
    let normalized = path.replace('/', "\\");
    if let Some(stripped) = normalized.strip_prefix(r"\\?\UNC\") {
        return Some(format!("\\\\{stripped}"));
    }
    normalized
        .strip_prefix(r"\\?\")
        .map(std::string::ToString::to_string)
}

#[cfg(windows)]
fn windows_msys_drive_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let stripped = normalized.strip_prefix('/')?;
    let (drive, rest) = stripped
        .split_once('/')
        .map_or((stripped, ""), |(drive, rest)| (drive, rest));
    if drive.len() != 1 || !drive.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    let drive = drive.to_ascii_uppercase();
    if rest.is_empty() {
        Some(format!("{drive}:\\"))
    } else {
        Some(format!("{drive}:\\{}", rest.replace('/', "\\")))
    }
}

#[cfg(windows)]
fn windows_msys_tmp_path(path: &str) -> Option<PathBuf> {
    let normalized = path.replace('\\', "/");
    let relative = normalized
        .strip_prefix("/tmp/")
        .or_else(|| normalized.strip_prefix("/var/tmp/"))?;
    Some(std::env::temp_dir().join(relative.replace('/', "\\")))
}

pub fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub fn normalize_requested_path(path: &str) -> EnvironmentResult<String> {
    let requested = Path::new(path);
    if requested.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(EnvironmentError::InvalidRequest(path.to_string()));
    }
    Ok(normalize_str_path(path))
}

pub fn is_tmp_path(path: &str) -> bool {
    let normalized = normalize_str_path(path);
    normalized == DEFAULT_TMP_DIR || normalized.starts_with(&format!("{DEFAULT_TMP_DIR}/"))
}

pub fn normalize_tmp_filename(filename: &str) -> EnvironmentResult<String> {
    let normalized = normalize_requested_path(filename)?;
    if normalized.is_empty() {
        return Err(EnvironmentError::InvalidRequest(
            "tmp filename must be non-empty".to_string(),
        ));
    }
    if is_tmp_path(&normalized) {
        return Err(EnvironmentError::InvalidRequest(
            "tmp filename must be relative to the provider tmp directory".to_string(),
        ));
    }
    Ok(normalized)
}

pub fn normalize_tmp_namespace(namespace: &str) -> EnvironmentResult<String> {
    let normalized = normalize_tmp_filename(namespace)?;
    if normalized.contains('/') {
        return Err(EnvironmentError::InvalidRequest(
            "tmp namespace must be a single path segment".to_string(),
        ));
    }
    Ok(normalized)
}

pub fn normalize_str_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized.trim_start_matches('/').to_string()
}

pub fn include_path(path: &str, include_hidden: bool) -> bool {
    include_hidden
        || !normalize_str_path(path)
            .split('/')
            .any(|segment| segment.starts_with('.') && segment.len() > 1)
}

pub fn path_contains(prefix: &str, path: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

pub fn strip_path_prefix<'a>(prefix: &str, path: &'a str) -> &'a str {
    if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(prefix)
            .and_then(|value| value.strip_prefix('/'))
            .unwrap_or(path)
    }
}
