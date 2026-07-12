//! Read-only access to host capability dependencies during execution.

use std::{collections::BTreeSet, sync::Arc};

use crate::DependencyStore;

/// Host-authorized dependency grants for one Strict tool.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolCapabilityGrant {
    /// Stable host capability names the tool may access.
    pub host_capabilities: BTreeSet<String>,
    /// Runtime-recognized mutable context capability names the tool may access.
    pub context_capabilities: BTreeSet<String>,
    /// Whether the tool may receive configured shell environment values.
    pub shell_environment: bool,
}

impl ToolCapabilityGrant {
    /// Create a deny-by-default grant.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Authorize named host capabilities.
    #[must_use]
    pub fn with_host_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.host_capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    /// Authorize mutable context capabilities.
    #[must_use]
    pub fn with_context_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.context_capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    /// Authorize or deny shell environment projection.
    #[must_use]
    pub const fn with_shell_environment(mut self, allowed: bool) -> Self {
        self.shell_environment = allowed;
        self
    }
}

/// Read-only host capability registry supplied to runtime hooks and tool calls.
///
/// This handle exposes explicitly attached typed capabilities without exposing
/// conversation, task, note, usage, lifecycle, or other `AgentContext` state.
#[derive(Clone, Default)]
pub struct HostCapabilities {
    dependencies: DependencyStore,
}

impl HostCapabilities {
    pub(crate) const fn new(dependencies: DependencyStore) -> Self {
        Self { dependencies }
    }

    /// Get a host capability by Rust type.
    #[must_use]
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get::<T>()
    }

    /// Get a named host capability.
    #[must_use]
    pub fn get_named<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get_named::<T>(name)
    }

    /// Return the stable dependency keys available through this handle.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.dependencies.keys()
    }

    /// Return whether no host capabilities are attached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty()
    }
}

impl std::fmt::Debug for HostCapabilities {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostCapabilities")
            .field("keys", &self.keys())
            .finish()
    }
}
