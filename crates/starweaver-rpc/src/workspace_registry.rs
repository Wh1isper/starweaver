//! RPC-owned persistent workspace authority registry.
//!
//! Durable session provenance is deliberately separate from these live grants. Only entries in
//! this owner-private registry can materialize a local workspace environment.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    RpcHostError, RpcHostResult,
    private_fs::{atomic_write_json, open_private_lock},
};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use starweaver_session::WorkspaceProvenanceRef;

const REGISTRY_VERSION: u32 = 3;
const REGISTRY_FILE_NAME: &str = "workspace-registry.json";
const REGISTRY_LOCK_FILE_NAME: &str = "workspace-registry.lock";

/// One safe registry projection plus the private canonical root used inside RPC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceGrant {
    pub(crate) workspace_id: String,
    pub(crate) display_label: Option<String>,
    pub(crate) provenance_digest: String,
    pub(crate) revision: u64,
    pub(crate) state: WorkspaceGrantState,
    pub(crate) canonical_root: PathBuf,
}

/// Current live authority state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceGrantState {
    Active,
    Revoked,
}

/// Result of a persistent workspace mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceMutation {
    pub(crate) grant: WorkspaceGrant,
    pub(crate) replayed: bool,
}

/// A live usage fence. Workspace removal fails while any run or session mutation holds one.
pub(crate) struct WorkspaceGrantLease {
    registry: WorkspaceRegistry,
    grant: WorkspaceGrant,
}

impl WorkspaceGrantLease {
    pub(crate) fn grant(&self) -> &WorkspaceGrant {
        &self.grant
    }

    pub(crate) fn canonical_root(&self) -> &Path {
        &self.grant.canonical_root
    }
}

impl Drop for WorkspaceGrantLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.registry.shared.state.lock()
            && let Some(count) = state.active_leases.get_mut(&self.grant.workspace_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.active_leases.remove(&self.grant.workspace_id);
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorkspaceRegistry {
    shared: Arc<WorkspaceRegistryShared>,
}

struct WorkspaceRegistryShared {
    state_dir: PathBuf,
    execution_domain_id: String,
    state: Mutex<WorkspaceRegistryState>,
}

struct WorkspaceRegistryState {
    persisted: PersistedWorkspaceRegistry,
    active_leases: HashMap<String, usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedWorkspaceRegistry {
    version: u32,
    execution_domain_id: String,
    entries: BTreeMap<String, PersistedWorkspaceEntry>,
    receipts: BTreeMap<String, PersistedWorkspaceReceipt>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedWorkspaceEntry {
    canonical_root: PathBuf,
    file_identity: WorkspaceFileIdentity,
    display_label: Option<String>,
    provenance_digest: String,
    revision: u64,
    state: WorkspaceGrantState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFileIdentity {
    platform: String,
    primary: u64,
    secondary: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedWorkspaceReceipt {
    operation: String,
    fingerprint: String,
    result: PersistedWorkspaceMutationResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedWorkspaceMutationResult {
    workspace_id: String,
    display_label: Option<String>,
    provenance_digest: String,
    revision: u64,
    state: WorkspaceGrantState,
}

impl WorkspaceRegistry {
    pub(crate) fn load_or_create(
        state_dir: &Path,
        execution_domain_id: &str,
    ) -> RpcHostResult<Self> {
        fs::create_dir_all(state_dir)?;
        let lock = open_private_lock(&state_dir.join(REGISTRY_LOCK_FILE_NAME))?;
        lock.lock_exclusive()?;
        let loaded = load_registry(&state_dir.join(REGISTRY_FILE_NAME), execution_domain_id);
        let unlock = fs2::FileExt::unlock(&lock);
        let persisted = loaded?;
        unlock?;
        Ok(Self {
            shared: Arc::new(WorkspaceRegistryShared {
                state_dir: state_dir.to_path_buf(),
                execution_domain_id: execution_domain_id.to_string(),
                state: Mutex::new(WorkspaceRegistryState {
                    persisted,
                    active_leases: HashMap::new(),
                }),
            }),
        })
    }

    /// Register an existing directory. Canonical duplicate roots retain one opaque identity.
    pub(crate) fn register(
        &self,
        root: &Path,
        display_label: Option<String>,
        idempotency_key: &str,
        fingerprint: &str,
    ) -> RpcHostResult<WorkspaceMutation> {
        {
            let state = self.lock_state()?;
            if let Some(receipt) = state.persisted.receipts.get(idempotency_key) {
                if receipt.operation != "workspace.register" || receipt.fingerprint != fingerprint {
                    return Err(RpcHostError::IdempotencyConflict(
                        "workspace idempotency key was reused for a different command".to_string(),
                    ));
                }
                let entry = state
                    .persisted
                    .entries
                    .get(&receipt.result.workspace_id)
                    .ok_or_else(|| {
                        RpcHostError::Storage(
                            "workspace mutation receipt references a missing entry".to_string(),
                        )
                    })?;
                let mutation = WorkspaceMutation {
                    grant: receipt_grant(&receipt.result, entry),
                    replayed: true,
                };
                drop(state);
                return Ok(mutation);
            }
        }
        let (canonical_root, file_identity) = canonical_directory(root)?;
        let root_text = canonical_root.to_str().ok_or_else(|| {
            RpcHostError::Invalid("workspace root must have a UTF-8 canonical identity".to_string())
        })?;
        let workspace_id = workspace_identity(&self.shared.execution_domain_id, root_text);
        let provenance_digest = provenance_digest(&self.shared.execution_domain_id, root_text);
        let mut state = self.lock_state()?;
        if let Some(receipt) = state.persisted.receipts.get(idempotency_key) {
            if receipt.operation != "workspace.register" || receipt.fingerprint != fingerprint {
                return Err(RpcHostError::IdempotencyConflict(
                    "workspace idempotency key was reused for a different command".to_string(),
                ));
            }
            let entry = state
                .persisted
                .entries
                .get(&receipt.result.workspace_id)
                .ok_or_else(|| {
                    RpcHostError::Storage(
                        "workspace mutation receipt references a missing entry".to_string(),
                    )
                })?;
            let mutation = WorkspaceMutation {
                grant: receipt_grant(&receipt.result, entry),
                replayed: true,
            };
            drop(state);
            return Ok(mutation);
        }

        let mut persisted = state.persisted.clone();
        let entry = persisted
            .entries
            .entry(workspace_id.clone())
            .or_insert_with(|| PersistedWorkspaceEntry {
                canonical_root: canonical_root.clone(),
                file_identity: file_identity.clone(),
                display_label: display_label.clone(),
                provenance_digest: provenance_digest.clone(),
                revision: 1,
                state: WorkspaceGrantState::Active,
            });
        if entry.canonical_root != canonical_root
            || entry.file_identity != file_identity
            || entry.provenance_digest != provenance_digest
        {
            return Err(RpcHostError::Storage(
                "workspace identity collision, replacement, or corrupt registry entry".to_string(),
            ));
        }
        if entry.state == WorkspaceGrantState::Revoked {
            entry.state = WorkspaceGrantState::Active;
            entry.revision = entry
                .revision
                .checked_add(1)
                .ok_or_else(|| RpcHostError::Storage("workspace revision overflow".to_string()))?;
            entry.display_label = display_label;
        }
        let mutation = WorkspaceMutation {
            grant: grant(&workspace_id, entry),
            replayed: false,
        };
        record_receipt(
            &mut persisted,
            idempotency_key,
            "workspace.register",
            fingerprint,
            &mutation.grant,
        );
        self.persist_locked(&persisted)?;
        state.persisted = persisted;
        drop(state);
        Ok(mutation)
    }

    pub(crate) fn remove(
        &self,
        workspace_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        fingerprint: &str,
    ) -> RpcHostResult<WorkspaceMutation> {
        let mut state = self.lock_state()?;
        if let Some(receipt) = state.persisted.receipts.get(idempotency_key) {
            if receipt.operation != "workspace.remove" || receipt.fingerprint != fingerprint {
                return Err(RpcHostError::IdempotencyConflict(
                    "workspace idempotency key was reused for a different command".to_string(),
                ));
            }
            let entry = state
                .persisted
                .entries
                .get(&receipt.result.workspace_id)
                .ok_or_else(|| {
                    RpcHostError::Storage(
                        "workspace mutation receipt references a missing entry".to_string(),
                    )
                })?;
            let mutation = WorkspaceMutation {
                grant: receipt_grant(&receipt.result, entry),
                replayed: true,
            };
            drop(state);
            return Ok(mutation);
        }
        if state.active_leases.get(workspace_id).copied().unwrap_or(0) != 0 {
            return Err(RpcHostError::RunConflict(
                "workspace has active or admitting runs and must be drained before removal"
                    .to_string(),
            ));
        }
        let mut persisted = state.persisted.clone();
        let entry = persisted
            .entries
            .get_mut(workspace_id)
            .ok_or_else(|| RpcHostError::NotFound("workspace grant".to_string()))?;
        if entry.revision != expected_revision {
            return Err(RpcHostError::RunConflict(
                "workspace revision does not match expectedRevision".to_string(),
            ));
        }
        if entry.state == WorkspaceGrantState::Active {
            entry.state = WorkspaceGrantState::Revoked;
            entry.revision = entry
                .revision
                .checked_add(1)
                .ok_or_else(|| RpcHostError::Storage("workspace revision overflow".to_string()))?;
        }
        let mutation = WorkspaceMutation {
            grant: grant(workspace_id, entry),
            replayed: false,
        };
        record_receipt(
            &mut persisted,
            idempotency_key,
            "workspace.remove",
            fingerprint,
            &mutation.grant,
        );
        self.persist_locked(&persisted)?;
        state.persisted = persisted;
        drop(state);
        Ok(mutation)
    }

    pub(crate) fn list_after(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> RpcHostResult<(Vec<WorkspaceGrant>, bool)> {
        let state = self.lock_state()?;
        let mut entries = state
            .persisted
            .entries
            .iter()
            .filter(|(workspace_id, _)| after.is_none_or(|after| workspace_id.as_str() > after))
            .map(|(workspace_id, entry)| grant(workspace_id, entry))
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        drop(state);
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        Ok((entries, has_more))
    }

    #[cfg(test)]
    pub(crate) fn lease_first_active_for_tests(&self) -> RpcHostResult<WorkspaceGrantLease> {
        let workspace_id = {
            let state = self.lock_state()?;
            state
                .persisted
                .entries
                .iter()
                .find(|(_, entry)| entry.state == WorkspaceGrantState::Active)
                .map(|(workspace_id, _)| workspace_id.clone())
                .ok_or_else(|| RpcHostError::NotFound("active workspace grant".to_string()))?
        };
        self.lease_active(&workspace_id)
    }

    pub(crate) fn lease_active(&self, workspace_id: &str) -> RpcHostResult<WorkspaceGrantLease> {
        if workspace_id.starts_with("legacy:") {
            return Err(RpcHostError::NotFound("active workspace grant".to_string()));
        }
        let mut state = self.lock_state()?;
        let entry = state
            .persisted
            .entries
            .get(workspace_id)
            .ok_or_else(|| RpcHostError::NotFound("active workspace grant".to_string()))?;
        if entry.state != WorkspaceGrantState::Active {
            return Err(RpcHostError::NotFound("active workspace grant".to_string()));
        }
        validate_live_entry(&self.shared.execution_domain_id, workspace_id, entry)?;
        let grant = grant(workspace_id, entry);
        *state
            .active_leases
            .entry(workspace_id.to_string())
            .or_default() += 1;
        drop(state);
        Ok(WorkspaceGrantLease {
            registry: self.clone(),
            grant,
        })
    }

    fn lock_state(&self) -> RpcHostResult<MutexGuard<'_, WorkspaceRegistryState>> {
        self.shared
            .state
            .lock()
            .map_err(|_| RpcHostError::Runtime("workspace registry lock poisoned".to_string()))
    }

    fn persist_locked(&self, registry: &PersistedWorkspaceRegistry) -> RpcHostResult<()> {
        let lock = open_private_lock(&self.shared.state_dir.join(REGISTRY_LOCK_FILE_NAME))?;
        lock.lock_exclusive()?;
        let result = atomic_write_json(
            &self.shared.state_dir.join(REGISTRY_FILE_NAME),
            registry,
            "workspace registry",
        );
        let unlock = fs2::FileExt::unlock(&lock);
        result?;
        unlock?;
        Ok(())
    }
}

fn load_registry(
    path: &Path,
    execution_domain_id: &str,
) -> RpcHostResult<PersistedWorkspaceRegistry> {
    match fs::read(path) {
        Ok(bytes) => {
            let registry: PersistedWorkspaceRegistry =
                serde_json::from_slice(&bytes).map_err(|error| {
                    RpcHostError::Storage(format!("invalid workspace registry: {error}"))
                })?;
            if registry.version != REGISTRY_VERSION {
                return Err(RpcHostError::Storage(
                    "unsupported workspace registry version".to_string(),
                ));
            }
            if registry.execution_domain_id != execution_domain_id {
                return Err(RpcHostError::Storage(
                    "workspace registry execution-domain binding mismatch".to_string(),
                ));
            }
            for (workspace_id, entry) in &registry.entries {
                validate_entry_identity(execution_domain_id, workspace_id, entry)?;
                if entry.state == WorkspaceGrantState::Active {
                    validate_live_entry(execution_domain_id, workspace_id, entry)?;
                }
            }
            for receipt in registry.receipts.values() {
                let expected_result_state = match receipt.operation.as_str() {
                    "workspace.register" => WorkspaceGrantState::Active,
                    "workspace.remove" => WorkspaceGrantState::Revoked,
                    _ => {
                        return Err(RpcHostError::Storage(
                            "workspace registry contains an invalid mutation receipt".to_string(),
                        ));
                    }
                };
                if receipt.fingerprint.is_empty() {
                    return Err(RpcHostError::Storage(
                        "workspace registry contains an invalid mutation receipt".to_string(),
                    ));
                }
                let entry = registry
                    .entries
                    .get(&receipt.result.workspace_id)
                    .ok_or_else(|| {
                        RpcHostError::Storage(
                            "workspace mutation receipt references a missing entry".to_string(),
                        )
                    })?;
                if receipt.result.revision == 0
                    || receipt.result.revision > entry.revision
                    || receipt.result.provenance_digest != entry.provenance_digest
                    || receipt.result.state != expected_result_state
                {
                    return Err(RpcHostError::Storage(
                        "workspace registry contains a corrupt mutation receipt".to_string(),
                    ));
                }
            }
            Ok(registry)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(PersistedWorkspaceRegistry {
            version: REGISTRY_VERSION,
            execution_domain_id: execution_domain_id.to_string(),
            entries: BTreeMap::new(),
            receipts: BTreeMap::new(),
        }),
        Err(error) => Err(error.into()),
    }
}

fn canonical_directory(root: &Path) -> RpcHostResult<(PathBuf, WorkspaceFileIdentity)> {
    let supplied = fs::symlink_metadata(root).map_err(|_| {
        RpcHostError::Invalid("workspace root must be an existing accessible directory".to_string())
    })?;
    if supplied.file_type().is_symlink() {
        return Err(RpcHostError::Invalid(
            "workspace root must not be a symbolic link".to_string(),
        ));
    }
    let canonical = fs::canonicalize(root).map_err(|_| {
        RpcHostError::Invalid("workspace root must be an existing accessible directory".to_string())
    })?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        RpcHostError::Invalid("workspace root must be an existing accessible directory".to_string())
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RpcHostError::Invalid(
            "workspace root must be a non-symlink directory".to_string(),
        ));
    }
    let identity = workspace_file_identity(&canonical, &metadata)?;
    Ok((canonical, identity))
}

fn validate_entry_identity(
    execution_domain_id: &str,
    workspace_id: &str,
    entry: &PersistedWorkspaceEntry,
) -> RpcHostResult<()> {
    let root = entry.canonical_root.to_str().ok_or_else(|| {
        RpcHostError::Storage("workspace registry contains a non-UTF-8 root".to_string())
    })?;
    if workspace_identity(execution_domain_id, root) != workspace_id
        || provenance_digest(execution_domain_id, root) != entry.provenance_digest
    {
        return Err(RpcHostError::Storage(
            "workspace registry entry failed identity validation".to_string(),
        ));
    }
    Ok(())
}

fn validate_live_entry(
    execution_domain_id: &str,
    workspace_id: &str,
    entry: &PersistedWorkspaceEntry,
) -> RpcHostResult<()> {
    validate_entry_identity(execution_domain_id, workspace_id, entry)?;
    let (canonical, identity) = canonical_directory(&entry.canonical_root).map_err(|_| {
        RpcHostError::Storage("active workspace root is unavailable or unsafe".to_string())
    })?;
    if canonical != entry.canonical_root || identity != entry.file_identity {
        return Err(RpcHostError::Storage(
            "active workspace root was replaced or rebound".to_string(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn workspace_file_identity(
    _path: &Path,
    metadata: &fs::Metadata,
) -> RpcHostResult<WorkspaceFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    Ok(WorkspaceFileIdentity {
        platform: "unix-dev-inode-v1".to_string(),
        primary: metadata.dev(),
        secondary: metadata.ino(),
    })
}

#[cfg(windows)]
fn workspace_file_identity(
    path: &Path,
    _metadata: &fs::Metadata,
) -> RpcHostResult<WorkspaceFileIdentity> {
    let file_id::FileId::LowRes {
        volume_serial_number,
        file_index,
    } = file_id::get_low_res_file_id(path)?
    else {
        return Err(RpcHostError::Invalid(
            "workspace file identity has an unexpected platform representation".to_string(),
        ));
    };
    Ok(WorkspaceFileIdentity {
        platform: "windows-volume-file-v1".to_string(),
        primary: u64::from(volume_serial_number),
        secondary: file_index,
    })
}

#[cfg(not(any(unix, windows)))]
fn workspace_file_identity(
    _path: &Path,
    _metadata: &fs::Metadata,
) -> RpcHostResult<WorkspaceFileIdentity> {
    Err(RpcHostError::Invalid(
        "workspace file identity is unsupported on this platform".to_string(),
    ))
}

fn workspace_identity(execution_domain_id: &str, canonical_root: &str) -> String {
    WorkspaceProvenanceRef::for_execution_domain_root(execution_domain_id, canonical_root, None)
        .workspace_id
}

fn provenance_digest(execution_domain_id: &str, canonical_root: &str) -> String {
    WorkspaceProvenanceRef::for_execution_domain_root(execution_domain_id, canonical_root, None)
        .provenance_digest
}

fn grant(workspace_id: &str, entry: &PersistedWorkspaceEntry) -> WorkspaceGrant {
    WorkspaceGrant {
        workspace_id: workspace_id.to_string(),
        display_label: entry.display_label.clone(),
        provenance_digest: entry.provenance_digest.clone(),
        revision: entry.revision,
        state: entry.state,
        canonical_root: entry.canonical_root.clone(),
    }
}

fn receipt_grant(
    result: &PersistedWorkspaceMutationResult,
    entry: &PersistedWorkspaceEntry,
) -> WorkspaceGrant {
    WorkspaceGrant {
        workspace_id: result.workspace_id.clone(),
        display_label: result.display_label.clone(),
        provenance_digest: result.provenance_digest.clone(),
        revision: result.revision,
        state: result.state,
        canonical_root: entry.canonical_root.clone(),
    }
}

fn record_receipt(
    registry: &mut PersistedWorkspaceRegistry,
    key: &str,
    operation: &str,
    fingerprint: &str,
    grant: &WorkspaceGrant,
) {
    registry.receipts.insert(
        key.to_string(),
        PersistedWorkspaceReceipt {
            operation: operation.to_string(),
            fingerprint: fingerprint.to_string(),
            result: PersistedWorkspaceMutationResult {
                workspace_id: grant.workspace_id.clone(),
                display_label: grant.display_label.clone(),
                provenance_digest: grant.provenance_digest.clone(),
                revision: grant.revision,
                state: grant.state,
            },
        },
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn registry_persists_duplicate_identity_and_rejects_removal_while_leased() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let registry = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        let first = registry
            .register(&root, Some("One".to_string()), "register-1", "sha256:first")
            .unwrap();
        let duplicate = registry
            .register(
                &root,
                Some("Other".to_string()),
                "register-2",
                "sha256:second",
            )
            .unwrap();
        assert_eq!(first.grant.workspace_id, duplicate.grant.workspace_id);
        assert_eq!(duplicate.grant.display_label.as_deref(), Some("One"));

        let lease = registry.lease_active(&first.grant.workspace_id).unwrap();
        assert!(matches!(
            registry.remove(
                &first.grant.workspace_id,
                first.grant.revision,
                "remove-1",
                "sha256:remove"
            ),
            Err(RpcHostError::RunConflict(_))
        ));
        drop(lease);
        let removed = registry
            .remove(
                &first.grant.workspace_id,
                first.grant.revision,
                "remove-1",
                "sha256:remove",
            )
            .unwrap();
        assert_eq!(removed.grant.state, WorkspaceGrantState::Revoked);

        let reopened = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        assert!(matches!(
            reopened.lease_active(&first.grant.workspace_id),
            Err(RpcHostError::NotFound(_))
        ));
    }

    #[test]
    fn idempotent_replay_returns_the_original_mutation_projection_after_later_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let registry = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        let registered = registry
            .register(
                &root,
                Some("Original".to_string()),
                "register-original",
                "sha256:register-original",
            )
            .unwrap();
        let removed = registry
            .remove(
                &registered.grant.workspace_id,
                registered.grant.revision,
                "remove-original",
                "sha256:remove-original",
            )
            .unwrap();

        let replayed_register = registry
            .register(
                &root,
                Some("Original".to_string()),
                "register-original",
                "sha256:register-original",
            )
            .unwrap();
        assert!(replayed_register.replayed);
        assert_eq!(replayed_register.grant, registered.grant);

        let reactivated = registry
            .register(
                &root,
                Some("Reactivated".to_string()),
                "register-reactivated",
                "sha256:register-reactivated",
            )
            .unwrap();
        assert!(reactivated.grant.revision > removed.grant.revision);
        let replayed_remove = registry
            .remove(
                &registered.grant.workspace_id,
                registered.grant.revision,
                "remove-original",
                "sha256:remove-original",
            )
            .unwrap();
        assert!(replayed_remove.replayed);
        assert_eq!(replayed_remove.grant, removed.grant);

        drop(registry);
        let reopened = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        let replayed_after_restart = reopened
            .remove(
                &registered.grant.workspace_id,
                registered.grant.revision,
                "remove-original",
                "sha256:remove-original",
            )
            .unwrap();
        assert_eq!(replayed_after_restart.grant, removed.grant);

        fs::remove_dir(&root).unwrap();
        let replayed_without_root = reopened
            .register(
                &root,
                Some("Original".to_string()),
                "register-original",
                "sha256:register-original",
            )
            .unwrap();
        assert_eq!(replayed_without_root.grant, registered.grant);
        assert!(matches!(
            reopened.register(
                &root,
                Some("Original".to_string()),
                "register-original",
                "sha256:conflict"
            ),
            Err(RpcHostError::IdempotencyConflict(_))
        ));
    }

    #[test]
    fn registry_load_rejects_receipt_states_impossible_for_the_recorded_operation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let registry = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        let registered = registry
            .register(&root, None, "register-receipt", "sha256:register-receipt")
            .unwrap();
        registry
            .remove(
                &registered.grant.workspace_id,
                registered.grant.revision,
                "remove-receipt",
                "sha256:remove-receipt",
            )
            .unwrap();
        drop(registry);

        let path = temp.path().join(REGISTRY_FILE_NAME);
        let original: PersistedWorkspaceRegistry =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for (receipt_key, impossible_state) in [
            ("register-receipt", WorkspaceGrantState::Revoked),
            ("remove-receipt", WorkspaceGrantState::Active),
        ] {
            let mut corrupted = original.clone();
            corrupted
                .receipts
                .get_mut(receipt_key)
                .unwrap()
                .result
                .state = impossible_state;
            fs::write(&path, serde_json::to_vec(&corrupted).unwrap()).unwrap();
            assert!(matches!(
                WorkspaceRegistry::load_or_create(temp.path(), "domain-a"),
                Err(RpcHostError::Storage(_))
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn active_lease_rejects_root_replacement_and_symlink_rebinding() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let replacement = temp.path().join("replacement");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&replacement).unwrap();
        let registry = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        let registered = registry
            .register(&root, None, "register", "sha256:register")
            .unwrap();

        fs::rename(&root, temp.path().join("original")).unwrap();
        fs::rename(&replacement, &root).unwrap();
        assert!(matches!(
            registry.lease_active(&registered.grant.workspace_id),
            Err(RpcHostError::Storage(_))
        ));

        fs::remove_dir(&root).unwrap();
        symlink(temp.path().join("original"), &root).unwrap();
        assert!(matches!(
            registry.lease_active(&registered.grant.workspace_id),
            Err(RpcHostError::Storage(_))
        ));
    }

    #[test]
    fn idempotency_conflict_and_execution_domain_binding_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let registry = WorkspaceRegistry::load_or_create(temp.path(), "domain-a").unwrap();
        registry
            .register(&root, None, "same-key", "sha256:first")
            .unwrap();
        assert!(matches!(
            registry.register(&root, None, "same-key", "sha256:different"),
            Err(RpcHostError::IdempotencyConflict(_))
        ));
        drop(registry);
        assert!(matches!(
            WorkspaceRegistry::load_or_create(temp.path(), "domain-b"),
            Err(RpcHostError::Storage(_))
        ));
    }
}
