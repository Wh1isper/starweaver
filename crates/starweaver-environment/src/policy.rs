//! Filesystem and shell access policies for environment providers.

use serde::{Deserialize, Serialize};

/// Filesystem policy for provider-backed tools.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilePolicy {
    /// Whether read operations are allowed.
    pub allow_read: bool,
    /// Whether write operations are allowed.
    pub allow_write: bool,
    /// Allowed logical path prefixes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_prefixes: Vec<String>,
}

impl FilePolicy {
    /// Policy allowing read-only access to all provider-visible files.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            allow_read: true,
            allow_write: false,
            allowed_prefixes: Vec::new(),
        }
    }

    /// Policy allowing read/write access to all provider-visible files.
    #[must_use]
    pub const fn read_write() -> Self {
        Self {
            allow_read: true,
            allow_write: true,
            allowed_prefixes: Vec::new(),
        }
    }

    pub(crate) fn permits(&self, path: &str, write: bool) -> bool {
        if write && !self.allow_write {
            return false;
        }
        if !write && !self.allow_read {
            return false;
        }
        self.allowed_prefixes.is_empty()
            || self
                .allowed_prefixes
                .iter()
                .any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
    }
}

/// Shell execution policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellPolicy {
    /// Whether shell or direct-program execution is allowed.
    pub allow_execute: bool,
    /// Allowed direct-program executable names or paths.
    ///
    /// Empty means shell scripts and direct programs are both accepted by the
    /// provider. A non-empty list disables arbitrary shell-script execution and
    /// only permits exact executable matches through the structured program APIs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_programs: Vec<String>,
}

impl ShellPolicy {
    /// Policy allowing all provider-visible commands.
    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            allow_execute: true,
            allowed_programs: Vec::new(),
        }
    }

    pub(crate) const fn permits_shell(&self) -> bool {
        self.allow_execute && self.allowed_programs.is_empty()
    }

    pub(crate) fn permits_program(&self, program: &str) -> bool {
        self.allow_execute
            && (self.allowed_programs.is_empty()
                || self
                    .allowed_programs
                    .iter()
                    .any(|allowed| allowed == program))
    }

    pub(crate) const fn permits_program_environment_overrides(&self) -> bool {
        self.allowed_programs.is_empty()
    }
}

/// Environment provider policy bundle.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentPolicy {
    /// Filesystem policy.
    pub files: FilePolicy,
    /// Shell policy.
    pub shell: ShellPolicy,
}
