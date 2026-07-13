//! HTTP bearer authentication and method authorization.

use std::{
    collections::BTreeSet,
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{RpcHostError, RpcHostResult};

const DEFAULT_TOKEN_FILE: &str = "http-token";
const MIN_TOKEN_BYTES: usize = 32;

/// Authorization scopes understood by the HTTP RPC transport.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcHttpScope {
    /// Read-only discovery, diagnostics, session, run, replay, and HITL queries.
    Read,
    /// Create sessions, start/control runs, and mutate run environment attachments.
    Run,
    /// Decide approvals and complete deferred tools.
    Approval,
    /// Administrative state/configuration mutations.
    Admin,
    /// Stop the RPC server.
    Shutdown,
}

impl RpcHttpScope {
    /// Stable configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Run => "run",
            Self::Approval => "approval",
            Self::Admin => "admin",
            Self::Shutdown => "shutdown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "read" => Some(Self::Read),
            "run" => Some(Self::Run),
            "approval" => Some(Self::Approval),
            "admin" => Some(Self::Admin),
            "shutdown" => Some(Self::Shutdown),
            _ => None,
        }
    }
}

/// Secure bearer-token source and browser/host allowlists for HTTP RPC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcHttpAuthConfig {
    /// Environment variable holding the bearer token.
    pub token_env: String,
    /// Optional token file. Relative paths are resolved from the RPC config directory.
    pub token_file: Option<PathBuf>,
    /// Scopes granted to this credential.
    pub scopes: BTreeSet<RpcHttpScope>,
    /// Browser origins permitted to send requests. Empty rejects every `Origin` header.
    pub allowed_origins: BTreeSet<String>,
    /// Additional exact HTTP Host values, without a scheme.
    pub allowed_hosts: BTreeSet<String>,
}

impl Default for RpcHttpAuthConfig {
    fn default() -> Self {
        Self {
            token_env: "STARWEAVER_RPC_TOKEN".to_string(),
            token_file: None,
            scopes: all_scopes(),
            allowed_origins: BTreeSet::new(),
            allowed_hosts: BTreeSet::new(),
        }
    }
}

/// Loaded HTTP credential. Its debug form never reveals the token.
#[derive(Clone, Eq, PartialEq)]
pub struct RpcHttpCredential {
    token: String,
    scopes: BTreeSet<RpcHttpScope>,
}

impl std::fmt::Debug for RpcHttpCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RpcHttpCredential")
            .field("token", &"[REDACTED]")
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl RpcHttpAuthConfig {
    /// Load an environment/file credential, or atomically generate a mode-0600 token file.
    pub(super) fn load_credential(&self, state_dir: &Path) -> RpcHostResult<RpcHttpCredential> {
        let token = match env::var(&self.token_env) {
            Ok(token) => {
                validate_token(&token, &format!("environment variable {}", self.token_env))?
            }
            Err(env::VarError::NotPresent) => {
                let path = self
                    .token_file
                    .clone()
                    .unwrap_or_else(|| state_dir.join(DEFAULT_TOKEN_FILE));
                load_or_generate_token(&path)?
            }
            Err(error) => {
                return Err(RpcHostError::Invalid(format!(
                    "HTTP bearer token environment variable {} is not valid unicode: {error}",
                    self.token_env
                )));
            }
        };
        Ok(RpcHttpCredential {
            token,
            scopes: self.scopes.clone(),
        })
    }
}

impl RpcHttpCredential {
    pub(super) fn authorizes(&self, supplied: &str, scope: RpcHttpScope) -> bool {
        constant_time_eq(self.token.as_bytes(), supplied.as_bytes()) && self.scopes.contains(&scope)
    }

    pub(super) fn authenticates(&self, supplied: &str) -> bool {
        constant_time_eq(self.token.as_bytes(), supplied.as_bytes())
    }
}

/// Return the scope required by one implemented method.
///
/// Unknown methods return `None` so transports fail closed before dispatch.
#[must_use]
pub fn required_scope(method: &str) -> Option<RpcHttpScope> {
    let scope = match method {
        "shutdown" => RpcHttpScope::Shutdown,
        "run.start"
        | "run.prompt"
        | "run.cancel"
        | "run.steer"
        | "session.create"
        | "environment.attach"
        | "environment.detach"
        | "environment.active_mount"
        | "environment.active_unmount" => RpcHttpScope::Run,
        "approval.decide" | "deferred.complete" | "deferred.fail" => RpcHttpScope::Approval,
        "model.select" | "session.current.set" | "session.delete" => RpcHttpScope::Admin,
        "initialize"
        | "diagnostics.get"
        | "profile.list"
        | "model.list"
        | "profile.get"
        | "model.current"
        | "config.get"
        | "session.list"
        | "session.get"
        | "session.current.get"
        | "run.status"
        | "run.await"
        | "run.attach"
        | "session.output"
        | "stream.replay"
        | "session.replay"
        | "approval.list"
        | "approval.show"
        | "deferred.list"
        | "deferred.show"
        | "environment.list"
        | "environment.health"
        | "environment.active_list"
        | "stream.subscribe"
        | "stream.unsubscribe" => RpcHttpScope::Read,
        _ => return None,
    };
    Some(scope)
}

pub fn parse_scope_list(value: &str) -> RpcHostResult<BTreeSet<RpcHttpScope>> {
    let mut scopes = BTreeSet::new();
    for name in value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let Some(scope) = RpcHttpScope::parse(name) else {
            return Err(RpcHostError::Invalid(format!(
                "unknown HTTP RPC scope '{name}'; expected read, run, approval, admin, or shutdown"
            )));
        };
        scopes.insert(scope);
    }
    if scopes.is_empty() {
        return Err(RpcHostError::Invalid(
            "HTTP RPC credential must grant at least one scope".to_string(),
        ));
    }
    Ok(scopes)
}

fn all_scopes() -> BTreeSet<RpcHttpScope> {
    BTreeSet::from([
        RpcHttpScope::Read,
        RpcHttpScope::Run,
        RpcHttpScope::Approval,
        RpcHttpScope::Admin,
        RpcHttpScope::Shutdown,
    ])
}

fn validate_token(token: &str, source: &str) -> RpcHostResult<String> {
    let token = token.trim().to_string();
    if token.len() < MIN_TOKEN_BYTES || token.chars().any(char::is_whitespace) {
        return Err(RpcHostError::Invalid(format!(
            "HTTP bearer token from {source} must contain at least {MIN_TOKEN_BYTES} non-whitespace bytes"
        )));
    }
    Ok(token)
}

fn load_or_generate_token(path: &Path) -> RpcHostResult<String> {
    match fs::read_to_string(path) {
        Ok(token) => {
            validate_token_file_permissions(path)?;
            validate_token(&token, &format!("token file {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => generate_token_file(path),
        Err(error) => Err(RpcHostError::Io(error)),
    }
}

fn generate_token_file(path: &Path) -> RpcHostResult<String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(token.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            eprintln!(
                "generated RPC HTTP bearer token at {} (mode 0600); the token is not printed",
                path.display()
            );
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_or_generate_token(path)
        }
        Err(error) => Err(RpcHostError::Io(error)),
    }
}

#[cfg(unix)]
fn validate_token_file_permissions(path: &Path) -> RpcHostResult<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(RpcHostError::Invalid(format!(
            "HTTP bearer token path {} must be a regular file",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(RpcHostError::Invalid(format!(
            "HTTP bearer token file {} must not be accessible by group or other users (expected mode 0600)",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_token_file_permissions(path: &Path) -> RpcHostResult<()> {
    if !fs::metadata(path)?.is_file() {
        return Err(RpcHostError::Invalid(format!(
            "HTTP bearer token path {} must be a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn constant_time_eq(expected: &[u8], supplied: &[u8]) -> bool {
    let max_len = expected.len().max(supplied.len());
    let mut difference = expected.len() ^ supplied.len();
    for index in 0..max_len {
        let left = expected.get(index).copied().unwrap_or(0);
        let right = supplied.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn generated_token_is_long_and_private() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcHttpAuthConfig {
            token_env: format!("STARWEAVER_TEST_TOKEN_{}", Uuid::new_v4().simple()),
            ..RpcHttpAuthConfig::default()
        };
        let credential = config.load_credential(temp.path()).unwrap();
        assert!(credential.authenticates(&credential.token));
        assert!(temp.path().join(DEFAULT_TOKEN_FILE).is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(temp.path().join(DEFAULT_TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn configured_token_file_rejects_group_or_other_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let token_file = temp.path().join("configured-token");
        fs::write(
            &token_file,
            "configured-token-0123456789abcdef-0123456789abcdef\n",
        )
        .unwrap();
        fs::set_permissions(&token_file, fs::Permissions::from_mode(0o644)).unwrap();
        let config = RpcHttpAuthConfig {
            token_env: format!("STARWEAVER_TEST_TOKEN_{}", Uuid::new_v4().simple()),
            token_file: Some(token_file),
            ..RpcHttpAuthConfig::default()
        };
        let error = config.load_credential(temp.path()).unwrap_err();
        assert!(error.to_string().contains("mode 0600"), "{error}");
    }

    #[test]
    fn scope_mapping_separates_sensitive_method_groups_and_rejects_unknown_methods() {
        assert_eq!(required_scope("session.list"), Some(RpcHttpScope::Read));
        assert_eq!(required_scope("run.start"), Some(RpcHttpScope::Run));
        assert_eq!(
            required_scope("approval.decide"),
            Some(RpcHttpScope::Approval)
        );
        assert_eq!(required_scope("session.delete"), Some(RpcHttpScope::Admin));
        assert_eq!(required_scope("shutdown"), Some(RpcHttpScope::Shutdown));
        assert_eq!(required_scope("future.mutating_method"), None);
    }

    #[test]
    fn token_comparison_rejects_prefixes_and_different_lengths() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"same-prefix"));
        assert!(!constant_time_eq(b"same", b"diff"));
    }
}
