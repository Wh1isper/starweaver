//! Local envd service backed by an in-process environment provider.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use serde_json::json;
use starweaver_core::Metadata;
use starweaver_envd_core::{
    DEFAULT_ENVIRONMENT_ID, EffectRecord, EnvdError, EnvdResult, EnvironmentCapabilities,
    EnvironmentCapability, EnvironmentDescriptor, EnvironmentStateSnapshot, EnvironmentStatus,
    MountBackendDescriptor, MountDescriptor, MountMode, MountStatus, MutationResult,
    OperationRecord,
};
use starweaver_environment::DynEnvironmentProvider;

use crate::convert::{env_error_to_envd, process_to_envd, resource_to_envd};

mod service;

/// Local envd service over an in-process environment provider.
///
/// This is the direct local implementation used by the first refactor phase.
/// The memory store here tracks envd state metadata and operation records; file
/// and shell behavior is delegated to the existing provider backend.
#[derive(Clone)]
pub struct LocalEnvd {
    environment_id: String,
    store_kind: String,
    provider: DynEnvironmentProvider,
    state_version: Arc<AtomicU64>,
    operations: Arc<Mutex<Vec<OperationRecord>>>,
    effects: Arc<Mutex<Vec<EffectRecord>>>,
}

impl LocalEnvd {
    /// Create a local envd service with the default CLI environment id.
    #[must_use]
    pub fn new(provider: DynEnvironmentProvider) -> Self {
        Self {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            store_kind: "ephemeral".to_string(),
            provider,
            state_version: Arc::new(AtomicU64::new(1)),
            operations: Arc::new(Mutex::new(Vec::new())),
            effects: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Set the environment id.
    #[must_use]
    pub fn with_environment_id(mut self, environment_id: impl Into<String>) -> Self {
        self.environment_id = environment_id.into();
        self
    }

    /// Set the state store kind.
    #[must_use]
    pub fn with_store_kind(mut self, store_kind: impl Into<String>) -> Self {
        self.store_kind = store_kind.into();
        self
    }

    /// Return the configured environment id.
    #[must_use]
    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }

    fn ensure_environment(&self, environment_id: &str) -> EnvdResult<()> {
        if environment_id == self.environment_id {
            Ok(())
        } else {
            Err(EnvdError::not_found(format!(
                "unknown environment id: {environment_id}"
            )))
        }
    }

    fn descriptor(&self) -> EnvironmentDescriptor {
        let mut metadata = Metadata::default();
        metadata.insert("provider_id".to_string(), json!(self.provider.id()));
        EnvironmentDescriptor {
            environment_id: self.environment_id.clone(),
            kind: "local".to_string(),
            store: self.store_kind.clone(),
            status: EnvironmentStatus::Open,
            state_version: self.state_version.load(Ordering::SeqCst),
            policy_revision: 1,
            capabilities: EnvironmentCapabilities {
                features: self.capabilities(),
            },
            metadata,
        }
    }

    fn capabilities(&self) -> Vec<EnvironmentCapability> {
        let mut capabilities = vec![
            EnvironmentCapability::Files,
            EnvironmentCapability::Shell,
            EnvironmentCapability::Snapshots,
            EnvironmentCapability::ContextSummary,
        ];
        if self.provider.clone().process_shell_provider().is_some() {
            capabilities.push(EnvironmentCapability::Processes);
        }
        capabilities
    }

    fn default_mount(&self) -> MountDescriptor {
        let mut backend_metadata = Metadata::default();
        backend_metadata.insert("provider_id".to_string(), json!(self.provider.id()));
        MountDescriptor {
            mount_id: "workspace".to_string(),
            root: "/".to_string(),
            mode: MountMode::ReadWrite,
            backend: MountBackendDescriptor {
                kind: "provider".to_string(),
                metadata: backend_metadata,
            },
            generation: 1,
            status: MountStatus::Ready,
        }
    }

    fn record_operation(
        &self,
        kind: impl Into<String>,
        metadata: Metadata,
    ) -> EnvdResult<MutationResult> {
        let state_version = self.state_version.fetch_add(1, Ordering::SeqCst) + 1;
        let operation_id = format!("op_{state_version:08}");
        let operation = OperationRecord {
            operation_id: operation_id.clone(),
            kind: kind.into(),
            state_version,
            metadata: metadata.clone(),
        };
        let effect = EffectRecord {
            effect_id: format!("fx_{state_version:08}"),
            operation_id: operation_id.clone(),
            kind: "provider_effect".to_string(),
            metadata,
        };
        self.operations
            .lock()
            .map_err(|error| EnvdError::provider(error.to_string()))?
            .push(operation);
        self.effects
            .lock()
            .map_err(|error| EnvdError::provider(error.to_string()))?
            .push(effect);
        Ok(MutationResult {
            state_version,
            operation_id,
        })
    }

    fn operation_metadata(path_key: &str, path: impl Into<String>) -> Metadata {
        Metadata::from_iter([(path_key.to_string(), json!(path.into()))])
    }

    fn process_provider(&self) -> EnvdResult<starweaver_environment::DynProcessShellProvider> {
        self.provider
            .clone()
            .process_shell_provider()
            .ok_or_else(|| EnvdError::unsupported("environment does not support processes"))
    }

    async fn snapshot(&self) -> EnvdResult<EnvironmentStateSnapshot> {
        let provider_state = self
            .provider
            .export_state()
            .await
            .map_err(env_error_to_envd)?;
        let resources = provider_state
            .resources
            .iter()
            .cloned()
            .map(resource_to_envd)
            .collect();
        let processes = provider_state
            .processes
            .iter()
            .cloned()
            .map(process_to_envd)
            .collect();
        let operations = self
            .operations
            .lock()
            .map_err(|error| EnvdError::provider(error.to_string()))?
            .clone();
        let effects = self
            .effects
            .lock()
            .map_err(|error| EnvdError::provider(error.to_string()))?
            .clone();
        let mut metadata = Metadata::default();
        metadata.insert(
            "provider_state".to_string(),
            serde_json::to_value(provider_state)
                .map_err(|error| EnvdError::provider(error.to_string()))?,
        );
        Ok(EnvironmentStateSnapshot {
            descriptor: self.descriptor(),
            mounts: vec![self.default_mount()],
            resources,
            processes,
            operations,
            effects,
            metadata,
        })
    }
}
