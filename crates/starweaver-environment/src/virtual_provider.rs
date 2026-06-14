//! Deterministic in-memory environment provider for tests and previews.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use crate::{
    normalize_tmp_namespace, EnvironmentPolicy, FilePolicy, ShellOutput, ShellPolicy,
    ShellProcessSnapshot,
};

mod impls;
mod process;
mod store;

/// Deterministic in-memory environment provider for tests and previews.
#[derive(Clone, Debug)]
pub struct VirtualEnvironmentProvider {
    id: String,
    policy: EnvironmentPolicy,
    tmp_namespace: Option<String>,
    files: Arc<Mutex<BTreeMap<String, String>>>,
    binary_files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    directories: Arc<Mutex<BTreeSet<String>>>,
    shell_outputs: Arc<Mutex<BTreeMap<String, ShellOutput>>>,
    processes: Arc<Mutex<BTreeMap<String, ShellProcessSnapshot>>>,
}

impl VirtualEnvironmentProvider {
    /// Create a virtual provider.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            policy: EnvironmentPolicy {
                files: FilePolicy::read_write(),
                shell: ShellPolicy::allow_all(),
            },
            tmp_namespace: None,
            files: Arc::new(Mutex::new(BTreeMap::new())),
            binary_files: Arc::new(Mutex::new(BTreeMap::new())),
            directories: Arc::new(Mutex::new(BTreeSet::new())),
            shell_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            processes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Set provider policy.
    #[must_use]
    pub fn with_policy(mut self, policy: EnvironmentPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set a provider-scoped temporary file namespace.
    ///
    /// Namespaces isolate tool-generated large output files under a stable
    /// subdirectory of the provider temporary root.
    #[must_use]
    pub fn with_tmp_namespace(mut self, namespace: impl AsRef<str>) -> Self {
        self.tmp_namespace = normalize_tmp_namespace(namespace.as_ref()).ok();
        self
    }

    /// Add a virtual UTF-8 text file.
    #[must_use]
    pub fn with_file(self, path: impl Into<String>, content: impl Into<String>) -> Self {
        let path = path.into();
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.clone(), content.into());
        }
        if let Ok(mut binary_files) = self.binary_files.lock() {
            binary_files.remove(&path);
        }
        self
    }

    /// Add a virtual binary file.
    #[must_use]
    pub fn with_bytes(self, path: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        let path = path.into();
        if let Ok(mut binary_files) = self.binary_files.lock() {
            binary_files.insert(path.clone(), content.into());
        }
        if let Ok(mut files) = self.files.lock() {
            files.remove(&path);
        }
        self
    }

    /// Add deterministic shell output.
    #[must_use]
    pub fn with_shell_output(self, command: impl Into<String>, output: ShellOutput) -> Self {
        if let Ok(mut shell_outputs) = self.shell_outputs.lock() {
            shell_outputs.insert(command.into(), output);
        }
        self
    }

    /// Add a deterministic background process snapshot.
    #[must_use]
    pub fn with_process(self, snapshot: ShellProcessSnapshot) -> Self {
        if let Ok(mut processes) = self.processes.lock() {
            processes.insert(snapshot.process_id.clone(), snapshot);
        }
        self
    }
}
