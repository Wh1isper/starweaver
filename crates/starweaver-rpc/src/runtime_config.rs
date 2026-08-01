//! Persistent reloadable RPC runtime configuration snapshots.
//!
//! Bootstrap configuration remains immutable in `RpcConfig`. This manager owns only the safe
//! runtime declaration, immutable generations, active/desired pointers, and receipt-backed
//! publication used by future admissions.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHostError, RpcHostResult, config::RpcRuntimeMaterialization,
    private_fs::atomic_write_json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use starweaver_rpc_core::generated as host;
use starweaver_session::RuntimeConfigSnapshotRef;

const STATE_VERSION: u32 = 2;
const CONFIG_DIR_NAME: &str = "runtime-config";
const SNAPSHOT_DIR_NAME: &str = "snapshots";
const STATE_FILE_NAME: &str = "state.json";

#[derive(Clone)]
pub(crate) struct RuntimeConfigManager {
    inner: Arc<RuntimeConfigManagerInner>,
}

struct RuntimeConfigManagerInner {
    root: PathBuf,
    base_config: RpcConfig,
    state: Mutex<RuntimeConfigState>,
}

struct RuntimeConfigState {
    persisted: PersistedRuntimeConfigState,
    snapshots: BTreeMap<u64, Arc<RuntimeConfigSnapshot>>,
}

/// One immutable, fully materialized runtime generation pinned by an admitted run.
#[derive(Clone)]
pub(crate) struct RuntimeConfigSnapshot {
    pub(crate) revision: RuntimeConfigRevision,
    pub(crate) document: host::ConfigDocument,
    materialization: RpcRuntimeMaterialization,
    #[cfg(test)]
    pub(crate) config: Arc<RpcConfig>,
    pub(crate) catalog: Arc<RpcAgentCatalog>,
}

impl RuntimeConfigSnapshot {
    pub(crate) fn durable_ref(&self) -> RuntimeConfigSnapshotRef {
        RuntimeConfigSnapshotRef::new(
            self.revision.generation,
            self.revision.etag.clone(),
            self.revision.materialization_digest.clone(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfigRevision {
    pub(crate) generation: u64,
    pub(crate) etag: String,
    pub(crate) materialization_digest: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigValidation {
    pub(crate) fingerprint: String,
    pub(crate) valid: bool,
    pub(crate) restart_required: bool,
    pub(crate) changed_categories: Vec<host::ConfigCategory>,
    pub(crate) issues: Vec<RuntimeConfigIssue>,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigIssue {
    pub(crate) category: host::ConfigCategory,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) severity: String,
}

#[derive(Clone)]
pub(crate) struct RuntimeConfigMutation {
    pub(crate) status: RuntimeConfigStatus,
    pub(crate) validation: RuntimeConfigValidation,
    pub(crate) replayed: bool,
    pub(crate) target_generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigStatus {
    pub(crate) active: RuntimeConfigRevision,
    pub(crate) desired: RuntimeConfigRevision,
}

impl RuntimeConfigStatus {
    pub(crate) fn restart_required(&self) -> bool {
        self.active.generation != self.desired.generation
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedRuntimeConfigState {
    version: u32,
    active_generation: u64,
    desired_generation: u64,
    revisions: BTreeMap<u64, RuntimeConfigRevision>,
    receipts: BTreeMap<String, PersistedConfigReceipt>,
    activation: Option<PersistedActivationIntent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedConfigReceipt {
    operation: String,
    fingerprint: String,
    target_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedActivationIntent {
    activation_id: String,
    desired_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedSnapshot {
    version: u32,
    revision: RuntimeConfigRevision,
    materialization: RpcRuntimeMaterialization,
}

impl RuntimeConfigManager {
    pub(crate) fn load_or_create(
        state_dir: &Path,
        base_config: RpcConfig,
        initial_catalog: RpcAgentCatalog,
    ) -> RpcHostResult<Self> {
        let root = state_dir.join(CONFIG_DIR_NAME);
        fs::create_dir_all(root.join(SNAPSHOT_DIR_NAME))?;
        let state_path = root.join(STATE_FILE_NAME);
        let (persisted, snapshots) = match fs::read(&state_path) {
            Ok(bytes) => {
                let persisted: PersistedRuntimeConfigState = serde_json::from_slice(&bytes)
                    .map_err(|error| {
                        RpcHostError::Storage(format!("invalid runtime config state: {error}"))
                    })?;
                if persisted.version != STATE_VERSION {
                    return Err(RpcHostError::Storage(
                        "unsupported runtime config state version".to_string(),
                    ));
                }
                reconcile_orphan_snapshots(&root, &persisted.revisions)?;
                let mut snapshots = BTreeMap::new();
                for (generation, revision) in &persisted.revisions {
                    let snapshot = load_snapshot(&root, *generation)?;
                    if snapshot.revision != *revision {
                        return Err(RpcHostError::Storage(
                            "runtime config snapshot revision mismatch".to_string(),
                        ));
                    }
                    let effective = base_config
                        .with_runtime_materialization(&snapshot.materialization)
                        .map_err(|_| {
                            RpcHostError::Storage(
                                "persisted runtime config snapshot is not materializable"
                                    .to_string(),
                            )
                        })?;
                    let catalog = RpcAgentCatalog::new(effective.clone()).map_err(|_| {
                        RpcHostError::Storage(
                            "persisted runtime config snapshot is not materializable".to_string(),
                        )
                    })?;
                    let document = snapshot.materialization.public_document();
                    snapshots.insert(
                        *generation,
                        Arc::new(RuntimeConfigSnapshot {
                            revision: revision.clone(),
                            document,
                            materialization: snapshot.materialization,
                            #[cfg(test)]
                            config: Arc::new(effective),
                            catalog: Arc::new(catalog),
                        }),
                    );
                }
                if !snapshots.contains_key(&persisted.active_generation)
                    || !snapshots.contains_key(&persisted.desired_generation)
                {
                    return Err(RpcHostError::Storage(
                        "runtime config pointers reference missing snapshots".to_string(),
                    ));
                }
                (persisted, snapshots)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                reconcile_orphan_snapshots(&root, &BTreeMap::new())?;
                let materialization = base_config.runtime_materialization()?;
                let document = materialization.public_document();
                let generation = base_config.launch.configuration_generation.max(1);
                let revision = revision_for(generation, &materialization)?;
                let snapshot = Arc::new(RuntimeConfigSnapshot {
                    revision: revision.clone(),
                    document,
                    materialization: materialization.clone(),
                    #[cfg(test)]
                    config: Arc::new(base_config.clone()),
                    catalog: Arc::new(initial_catalog),
                });
                let persisted = PersistedRuntimeConfigState {
                    version: STATE_VERSION,
                    active_generation: generation,
                    desired_generation: generation,
                    revisions: BTreeMap::from([(generation, revision.clone())]),
                    receipts: BTreeMap::new(),
                    activation: None,
                };
                write_snapshot(&root, &revision, &materialization)?;
                atomic_write_json(&state_path, &persisted, "runtime config state")?;
                (persisted, BTreeMap::from([(generation, snapshot)]))
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            inner: Arc::new(RuntimeConfigManagerInner {
                root,
                base_config,
                state: Mutex::new(RuntimeConfigState {
                    persisted,
                    snapshots,
                }),
            }),
        })
    }

    pub(crate) fn active_snapshot(&self) -> RpcHostResult<Arc<RuntimeConfigSnapshot>> {
        let state = self.lock_state()?;
        state
            .snapshots
            .get(&state.persisted.active_generation)
            .cloned()
            .ok_or_else(|| RpcHostError::Storage("active runtime config is missing".to_string()))
    }

    pub(crate) fn snapshot_for_ref(
        &self,
        reference: &RuntimeConfigSnapshotRef,
    ) -> RpcHostResult<Arc<RuntimeConfigSnapshot>> {
        let state = self.lock_state()?;
        let snapshot = state
            .snapshots
            .get(&reference.generation)
            .cloned()
            .ok_or_else(|| RpcHostError::NotFound("runtime config snapshot".to_string()))?;
        drop(state);
        if snapshot.revision.etag != reference.etag
            || snapshot.revision.materialization_digest != reference.materialization_digest
        {
            return Err(RpcHostError::Storage(
                "runtime config snapshot reference failed integrity validation".to_string(),
            ));
        }
        Ok(snapshot)
    }

    pub(crate) fn get(&self) -> RpcHostResult<(host::ConfigDocument, RuntimeConfigStatus)> {
        let state = self.lock_state()?;
        let desired = state
            .snapshots
            .get(&state.persisted.desired_generation)
            .ok_or_else(|| RpcHostError::Storage("desired runtime config is missing".to_string()))?
            .document
            .clone();
        let status = status_locked(&state)?;
        drop(state);
        Ok((desired, status))
    }

    pub(crate) fn validate(
        &self,
        candidate: &host::ConfigDocument,
    ) -> RpcHostResult<RuntimeConfigValidation> {
        let state = self.lock_state()?;
        let active = state
            .snapshots
            .get(&state.persisted.active_generation)
            .cloned()
            .ok_or_else(|| RpcHostError::Storage("active runtime config is missing".to_string()))?;
        drop(state);
        let materialization = active.materialization.apply_public_document(candidate)?;
        Ok(validate_materialization(
            &self.inner.base_config,
            &active.document,
            &materialization,
        ))
    }

    pub(crate) fn update(
        &self,
        expected_active_etag: &str,
        candidate: host::ConfigDocument,
        candidate_fingerprint: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<RuntimeConfigMutation> {
        let active = self.active_snapshot()?;
        let materialization = active.materialization.apply_public_document(&candidate)?;
        let validation =
            validate_materialization(&self.inner.base_config, &active.document, &materialization);
        if validation.fingerprint != candidate_fingerprint {
            return Err(RpcHostError::Invalid(
                "candidateFingerprint does not match the canonical candidate".to_string(),
            ));
        }
        self.commit_materialization(
            "config.update",
            expected_active_etag,
            materialization,
            validation,
            idempotency_key,
            command_fingerprint,
        )
    }

    pub(crate) fn reload(
        &self,
        mode: host::ConfigReloadMode,
        expected_active_etag: &str,
        candidate_etag: Option<&str>,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<(RuntimeConfigMutation, String)> {
        let materialization = self
            .inner
            .base_config
            .runtime_materialization_from_source()?;
        let active = self.active_snapshot()?;
        let validation =
            validate_materialization(&self.inner.base_config, &active.document, &materialization);
        let etag = etag_for(&materialization)?;
        if let Some(expected) = candidate_etag
            && expected != etag
        {
            return Err(RpcHostError::RunConflict(
                "runtime config source changed after validation".to_string(),
            ));
        }
        if mode == host::ConfigReloadMode::DryRun {
            let status = self.get()?.1;
            return Ok((
                RuntimeConfigMutation {
                    target_generation: status.desired.generation,
                    status,
                    validation,
                    replayed: false,
                },
                etag,
            ));
        }
        let mutation = self.commit_materialization(
            "config.reload",
            expected_active_etag,
            materialization,
            validation,
            idempotency_key,
            command_fingerprint,
        )?;
        Ok((mutation, etag))
    }

    fn commit_materialization(
        &self,
        operation: &str,
        expected_active_etag: &str,
        materialization: RpcRuntimeMaterialization,
        validation: RuntimeConfigValidation,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<RuntimeConfigMutation> {
        let mut state = self.lock_state()?;
        if let Some(receipt) = replay_receipt(
            &state.persisted,
            idempotency_key,
            operation,
            command_fingerprint,
        )? {
            return Ok(RuntimeConfigMutation {
                status: status_locked(&state)?,
                validation,
                replayed: true,
                target_generation: receipt.target_generation,
            });
        }
        require_active_etag(&state, expected_active_etag)?;
        if !validation.valid {
            return Err(RpcHostError::ConfigurationFailed(
                "runtime configuration validation failed".to_string(),
            ));
        }
        let generation = next_generation(&state.persisted)?;
        let snapshot = materialize_snapshot(&self.inner.base_config, generation, materialization)?;
        write_snapshot(
            &self.inner.root,
            &snapshot.revision,
            &snapshot.materialization,
        )?;
        let mut persisted = state.persisted.clone();
        persisted
            .revisions
            .insert(generation, snapshot.revision.clone());
        persisted.active_generation = generation;
        persisted.desired_generation = generation;
        persisted.activation = None;
        persisted.receipts.insert(
            idempotency_key.to_string(),
            PersistedConfigReceipt {
                operation: operation.to_string(),
                fingerprint: command_fingerprint.to_string(),
                target_generation: generation,
            },
        );
        self.persist_locked(&persisted)?;
        state.persisted = persisted;
        state.snapshots.insert(generation, Arc::new(snapshot));
        let status = status_locked(&state)?;
        drop(state);
        Ok(RuntimeConfigMutation {
            status,
            validation,
            replayed: false,
            target_generation: generation,
        })
    }

    pub(crate) fn discard(
        &self,
        desired_etag: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<RuntimeConfigMutation> {
        let mut state = self.lock_state()?;
        if let Some(receipt) = replay_receipt(
            &state.persisted,
            idempotency_key,
            "config.discard",
            command_fingerprint,
        )? {
            let active = state
                .snapshots
                .get(&state.persisted.active_generation)
                .ok_or_else(|| {
                    RpcHostError::Storage("active runtime config is missing".to_string())
                })?;
            let mutation = RuntimeConfigMutation {
                status: status_locked(&state)?,
                validation: validate_materialization(
                    &self.inner.base_config,
                    &active.document,
                    &active.materialization,
                ),
                replayed: true,
                target_generation: receipt.target_generation,
            };
            drop(state);
            return Ok(mutation);
        }
        let desired = state
            .snapshots
            .get(&state.persisted.desired_generation)
            .ok_or_else(|| {
                RpcHostError::Storage("desired runtime config is missing".to_string())
            })?;
        if desired.revision.etag != desired_etag {
            return Err(RpcHostError::RunConflict(
                "desired runtime config etag does not match".to_string(),
            ));
        }
        let target_generation = state.persisted.active_generation;
        let mut persisted = state.persisted.clone();
        persisted.desired_generation = target_generation;
        persisted.activation = None;
        persisted.receipts.insert(
            idempotency_key.to_string(),
            PersistedConfigReceipt {
                operation: "config.discard".to_string(),
                fingerprint: command_fingerprint.to_string(),
                target_generation,
            },
        );
        self.persist_locked(&persisted)?;
        state.persisted = persisted;
        let active = state
            .snapshots
            .get(&target_generation)
            .cloned()
            .ok_or_else(|| RpcHostError::Storage("active runtime config is missing".to_string()))?;
        let mutation = RuntimeConfigMutation {
            status: status_locked(&state)?,
            validation: validate_materialization(
                &self.inner.base_config,
                &active.document,
                &active.materialization,
            ),
            replayed: false,
            target_generation,
        };
        drop(state);
        Ok(mutation)
    }

    pub(crate) fn activate(
        &self,
        desired_etag: &str,
        activation_id: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<RuntimeConfigMutation> {
        let mut state = self.lock_state()?;
        if let Some(receipt) = replay_receipt(
            &state.persisted,
            idempotency_key,
            "config.activate",
            command_fingerprint,
        )? {
            let desired = state
                .snapshots
                .get(&state.persisted.desired_generation)
                .ok_or_else(|| {
                    RpcHostError::Storage("desired runtime config is missing".to_string())
                })?;
            let mutation = RuntimeConfigMutation {
                status: status_locked(&state)?,
                validation: validate_materialization(
                    &self.inner.base_config,
                    &desired.document,
                    &desired.materialization,
                ),
                replayed: true,
                target_generation: receipt.target_generation,
            };
            drop(state);
            return Ok(mutation);
        }
        if state.persisted.active_generation == state.persisted.desired_generation {
            return Err(RpcHostError::RunConflict(
                "runtime config has no staged restart-required revision".to_string(),
            ));
        }
        let desired = state
            .snapshots
            .get(&state.persisted.desired_generation)
            .cloned()
            .ok_or_else(|| {
                RpcHostError::Storage("desired runtime config is missing".to_string())
            })?;
        if desired.revision.etag != desired_etag {
            return Err(RpcHostError::RunConflict(
                "desired runtime config etag does not match".to_string(),
            ));
        }
        let target_generation = state.persisted.desired_generation;
        let mut persisted = state.persisted.clone();
        persisted.activation = Some(PersistedActivationIntent {
            activation_id: activation_id.to_string(),
            desired_generation: target_generation,
        });
        persisted.receipts.insert(
            idempotency_key.to_string(),
            PersistedConfigReceipt {
                operation: "config.activate".to_string(),
                fingerprint: command_fingerprint.to_string(),
                target_generation,
            },
        );
        self.persist_locked(&persisted)?;
        state.persisted = persisted;
        let mutation = RuntimeConfigMutation {
            status: status_locked(&state)?,
            validation: validate_materialization(
                &self.inner.base_config,
                &desired.document,
                &desired.materialization,
            ),
            replayed: false,
            target_generation,
        };
        drop(state);
        Ok(mutation)
    }

    fn lock_state(&self) -> RpcHostResult<MutexGuard<'_, RuntimeConfigState>> {
        self.inner
            .state
            .lock()
            .map_err(|_| RpcHostError::Runtime("runtime config lock poisoned".to_string()))
    }

    fn persist_locked(&self, persisted: &PersistedRuntimeConfigState) -> RpcHostResult<()> {
        atomic_write_json(
            &self.inner.root.join(STATE_FILE_NAME),
            persisted,
            "runtime config state",
        )
    }
}

fn validate_materialization(
    base: &RpcConfig,
    active_document: &host::ConfigDocument,
    candidate: &RpcRuntimeMaterialization,
) -> RuntimeConfigValidation {
    let candidate_document = candidate.public_document();
    let fingerprint = fingerprint_for(candidate).unwrap_or_else(|_| format!("sha256:{:064x}", 0));
    let mut changed_categories = Vec::new();
    if active_document.default_profile != candidate_document.default_profile {
        changed_categories.push(host::ConfigCategory::DefaultProfile);
    }
    if active_document.profiles != candidate_document.profiles {
        changed_categories.push(host::ConfigCategory::Profiles);
    }
    if active_document.providers != candidate_document.providers {
        changed_categories.push(host::ConfigCategory::Providers);
    }
    let materializable = base
        .with_runtime_materialization(candidate)
        .and_then(RpcAgentCatalog::new)
        .is_ok();
    let issues = if materializable {
        Vec::new()
    } else {
        vec![RuntimeConfigIssue {
            category: host::ConfigCategory::Runtime,
            code: "runtime.materialization_invalid".to_string(),
            message: "runtime configuration is not materializable".to_string(),
            severity: "error".to_string(),
        }]
    };
    RuntimeConfigValidation {
        fingerprint,
        valid: materializable,
        restart_required: false,
        changed_categories,
        issues,
    }
}

fn materialize_snapshot(
    base: &RpcConfig,
    generation: u64,
    materialization: RpcRuntimeMaterialization,
) -> RpcHostResult<RuntimeConfigSnapshot> {
    let effective = base.with_runtime_materialization(&materialization)?;
    #[cfg(test)]
    let catalog = RpcAgentCatalog::new(effective.clone())?;
    #[cfg(not(test))]
    let catalog = RpcAgentCatalog::new(effective)?;
    let document = materialization.public_document();
    Ok(RuntimeConfigSnapshot {
        revision: revision_for(generation, &materialization)?,
        document,
        materialization,
        #[cfg(test)]
        config: Arc::new(effective),
        catalog: Arc::new(catalog),
    })
}

fn revision_for(
    generation: u64,
    materialization: &RpcRuntimeMaterialization,
) -> RpcHostResult<RuntimeConfigRevision> {
    Ok(RuntimeConfigRevision {
        generation,
        etag: etag_for(materialization)?,
        materialization_digest: fingerprint_for(materialization)?,
    })
}

fn fingerprint_for(materialization: &RpcRuntimeMaterialization) -> RpcHostResult<String> {
    materialization_digest(
        b"starweaver.rpc.runtime-materialization/v2\0",
        materialization,
    )
}

fn etag_for(materialization: &RpcRuntimeMaterialization) -> RpcHostResult<String> {
    Ok(format!(
        "config-{}",
        materialization_digest(b"starweaver.rpc.runtime-config-etag/v2\0", materialization,)?
    ))
}

fn materialization_digest(
    domain: &[u8],
    materialization: &RpcRuntimeMaterialization,
) -> RpcHostResult<String> {
    let bytes = serde_json::to_vec(materialization)
        .map_err(|error| RpcHostError::Runtime(format!("encode runtime config: {error}")))?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn next_generation(state: &PersistedRuntimeConfigState) -> RpcHostResult<u64> {
    state
        .revisions
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| RpcHostError::Storage("runtime config generation overflow".to_string()))
}

fn require_active_etag(state: &RuntimeConfigState, expected: &str) -> RpcHostResult<()> {
    let active = state
        .snapshots
        .get(&state.persisted.active_generation)
        .ok_or_else(|| RpcHostError::Storage("active runtime config is missing".to_string()))?;
    if active.revision.etag != expected {
        return Err(RpcHostError::RunConflict(
            "active runtime config etag does not match".to_string(),
        ));
    }
    Ok(())
}

fn status_locked(state: &RuntimeConfigState) -> RpcHostResult<RuntimeConfigStatus> {
    let active = state
        .persisted
        .revisions
        .get(&state.persisted.active_generation)
        .cloned()
        .ok_or_else(|| {
            RpcHostError::Storage("active runtime config revision is missing".to_string())
        })?;
    let desired = state
        .persisted
        .revisions
        .get(&state.persisted.desired_generation)
        .cloned()
        .ok_or_else(|| {
            RpcHostError::Storage("desired runtime config revision is missing".to_string())
        })?;
    Ok(RuntimeConfigStatus { active, desired })
}

fn replay_receipt<'a>(
    state: &'a PersistedRuntimeConfigState,
    key: &str,
    operation: &str,
    fingerprint: &str,
) -> RpcHostResult<Option<&'a PersistedConfigReceipt>> {
    let Some(receipt) = state.receipts.get(key) else {
        return Ok(None);
    };
    if receipt.operation != operation || receipt.fingerprint != fingerprint {
        return Err(RpcHostError::IdempotencyConflict(
            "runtime config idempotency key was reused for a different command".to_string(),
        ));
    }
    Ok(Some(receipt))
}

fn reconcile_orphan_snapshots(
    root: &Path,
    revisions: &BTreeMap<u64, RuntimeConfigRevision>,
) -> RpcHostResult<()> {
    let directory = root.join(SNAPSHOT_DIR_NAME);
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err(RpcHostError::Storage(
                "runtime snapshot directory contains a non-file entry".to_string(),
            ));
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return Err(RpcHostError::Storage(
                "runtime snapshot directory contains a non-UTF-8 entry".to_string(),
            ));
        };
        if name.starts_with('.')
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        {
            fs::remove_file(path)?;
            continue;
        }
        let Some(stem) = name.strip_suffix(".json") else {
            return Err(RpcHostError::Storage(
                "runtime snapshot directory contains an unknown entry".to_string(),
            ));
        };
        let generation = stem.parse::<u64>().map_err(|_| {
            RpcHostError::Storage(
                "runtime snapshot directory contains an invalid generation".to_string(),
            )
        })?;
        if !revisions.contains_key(&generation) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn snapshot_path(root: &Path, generation: u64) -> PathBuf {
    root.join(SNAPSHOT_DIR_NAME)
        .join(format!("{generation}.json"))
}

fn load_snapshot(root: &Path, generation: u64) -> RpcHostResult<PersistedSnapshot> {
    let bytes = fs::read(snapshot_path(root, generation))?;
    let snapshot: PersistedSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| RpcHostError::Storage(format!("invalid runtime snapshot: {error}")))?;
    if snapshot.version != STATE_VERSION || snapshot.revision.generation != generation {
        return Err(RpcHostError::Storage(
            "runtime config snapshot identity mismatch".to_string(),
        ));
    }
    if snapshot.revision != revision_for(generation, &snapshot.materialization)? {
        return Err(RpcHostError::Storage(
            "runtime config snapshot failed digest validation".to_string(),
        ));
    }
    Ok(snapshot)
}

fn write_snapshot(
    root: &Path,
    revision: &RuntimeConfigRevision,
    materialization: &RpcRuntimeMaterialization,
) -> RpcHostResult<()> {
    let path = snapshot_path(root, revision.generation);
    if path.exists() {
        let existing = load_snapshot(root, revision.generation)?;
        if existing.revision == *revision && existing.materialization == *materialization {
            return Ok(());
        }
        return Err(RpcHostError::Storage(
            "runtime config generation is immutable".to_string(),
        ));
    }
    atomic_write_json(
        &path,
        &PersistedSnapshot {
            version: STATE_VERSION,
            revision: revision.clone(),
            materialization: materialization.clone(),
        },
        "runtime config snapshot",
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn hot_update_is_persistent_idempotent_and_preserves_private_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        fs::create_dir_all(config.workspace_root_for_tests()).unwrap();
        config.providers.get_mut("openai").unwrap().api_key_env =
            Some("PRIVATE_KEY_ENV".to_string());
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let manager =
            RuntimeConfigManager::load_or_create(temp.path(), config.clone(), catalog).unwrap();
        let (mut candidate, status) = manager.get().unwrap();
        candidate.default_profile = "default".to_string();
        candidate.profiles[0]
            .instructions
            .push("updated".to_string());
        let validation = manager.validate(&candidate).unwrap();
        let mutation = manager
            .update(
                &status.active.etag,
                candidate.clone(),
                &validation.fingerprint,
                "key-one",
                "sha256:command",
            )
            .unwrap();
        assert!(!mutation.replayed);
        assert!(!mutation.status.restart_required());
        let replay = manager
            .update(
                &status.active.etag,
                candidate,
                &validation.fingerprint,
                "key-one",
                "sha256:command",
            )
            .unwrap();
        assert!(replay.replayed);
        let active = manager.active_snapshot().unwrap();
        assert_eq!(
            active.config.providers["openai"].api_key_env.as_deref(),
            Some("PRIVATE_KEY_ENV")
        );
        let active_ref = active.durable_ref();
        drop(active);
        drop(manager);

        config.providers.get_mut("openai").unwrap().api_key_env = Some("DIFFERENT_ENV".to_string());
        config
            .profiles
            .get_mut("default")
            .unwrap()
            .instructions
            .clear();
        let changed_catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let reopened =
            RuntimeConfigManager::load_or_create(temp.path(), config, changed_catalog).unwrap();
        let restored = reopened.snapshot_for_ref(&active_ref).unwrap();
        assert_eq!(
            restored.config.providers["openai"].api_key_env.as_deref(),
            Some("PRIVATE_KEY_ENV")
        );
        assert_eq!(
            restored.config.profiles["default"].instructions,
            vec!["updated"]
        );
    }

    #[test]
    fn mcp_prefix_snapshot_round_trips_across_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        fs::create_dir_all(config.workspace_root_for_tests()).unwrap();
        config.mcp_servers = starweaver_tools::McpConfigDocument::from_slice(
            br#"{
                "servers": {
                    "canonical-empty": {"command":"canonical-mcp","prefix":""},
                    "legacy-whitespace": {"command":"legacy-mcp","tool_prefix":"   "}
                }
            }"#,
        )
        .unwrap()
        .servers;
        let expected = config.mcp_servers.clone();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let manager =
            RuntimeConfigManager::load_or_create(temp.path(), config.clone(), catalog).unwrap();
        assert_eq!(
            manager
                .active_snapshot()
                .unwrap()
                .materialization
                .mcp_servers,
            expected
        );
        drop(manager);

        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let reopened = RuntimeConfigManager::load_or_create(temp.path(), config, catalog).unwrap();
        assert_eq!(
            reopened
                .active_snapshot()
                .unwrap()
                .materialization
                .mcp_servers,
            expected
        );
    }

    #[test]
    fn startup_removes_unpublished_orphan_snapshot_generation() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        fs::create_dir_all(config.workspace_root_for_tests()).unwrap();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let manager =
            RuntimeConfigManager::load_or_create(temp.path(), config.clone(), catalog).unwrap();
        drop(manager);
        let orphan = snapshot_path(&temp.path().join(CONFIG_DIR_NAME), 999);
        fs::write(&orphan, b"unpublished crash residue").unwrap();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        RuntimeConfigManager::load_or_create(temp.path(), config, catalog).unwrap();
        assert!(!orphan.exists());
    }
}
