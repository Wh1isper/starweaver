use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{json, Value};
use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::{
    EnvdService, EnvironmentRequest, InitializeEnvdRequest, OpenEnvironmentRequest,
    DEFAULT_ENVIRONMENT_ID,
};
use starweaver_rpc_core::{
    environment_attachment_lease_result, environment_attachment_list_result,
    environment_health_result, is_valid_environment_attachment_id, EnvironmentAttachParams,
    EnvironmentAttachmentAccessMode, EnvironmentAttachmentLease, EnvironmentAttachmentRef,
    EnvironmentAttachmentScope, EnvironmentAttachmentScopeKind, EnvironmentAttachmentStatus,
    EnvironmentDetachParams, EnvironmentHealthParams, EnvironmentListParams, EnvironmentReadiness,
    EnvironmentReadinessCapabilities, EnvironmentReadinessPhase, EnvironmentReadinessPolicy,
    EnvironmentReadinessRequest, RpcError, ALREADY_EXISTS, ENVIRONMENT_UNAVAILABLE, INVALID_PARAMS,
    SERVER_ERROR, UNSUPPORTED_FEATURE,
};
use tokio::time::timeout;
use uuid::Uuid;

use crate::CliConfig;

const DEFAULT_READINESS_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone)]
pub(super) struct EnvironmentAttachmentManager {
    leases: Arc<Mutex<BTreeMap<String, LeaseRecord>>>,
    attach_idempotency: Arc<Mutex<BTreeMap<String, String>>>,
}

#[derive(Clone)]
struct LeaseRecord {
    lease: EnvironmentAttachmentLease,
    source: EnvironmentAttachmentRef,
}

impl EnvironmentAttachmentManager {
    pub(super) fn new(_config: CliConfig) -> Self {
        Self {
            leases: Arc::new(Mutex::new(BTreeMap::new())),
            attach_idempotency: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(super) async fn attach(&self, params: &Value) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentAttachParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid attach params: {error}"))
            })?;
        validate_scope(&params.scope)?;
        let source = validate_attachment_source(params.attachment, false)?;
        if let Some(key) = params.idempotency_key.as_deref() {
            if let Some(lease) = self.idempotent_attach_result(&params.scope, key)? {
                return Ok(environment_attachment_lease_result(&lease));
            }
        }
        if let Some(existing) = self.find_active_by_scope_and_id(&params.scope, &source.id)? {
            if same_source(&existing.source, &source) {
                if let Some(key) = params.idempotency_key.as_deref() {
                    self.store_attach_idempotency(
                        &params.scope,
                        key,
                        &existing.lease.attachment_lease_id,
                    )?;
                }
                return Ok(environment_attachment_lease_result(&existing.lease));
            }
            return Err(RpcError::new(
                ALREADY_EXISTS,
                format!(
                    "environment attachment already exists in scope: {}",
                    source.id
                ),
            ));
        }

        let (status, readiness) = self.probe_for_policy(&source, &params.readiness).await?;
        let lease = lease_from_source(
            new_lease_id(&source.id),
            params.scope,
            &source,
            status,
            readiness,
        );
        self.leases.lock().map_err(lock_error)?.insert(
            lease.attachment_lease_id.clone(),
            LeaseRecord {
                lease: lease.clone(),
                source,
            },
        );
        if let Some(key) = params.idempotency_key.as_deref() {
            self.store_attach_idempotency(&lease.scope, key, &lease.attachment_lease_id)?;
        }
        Ok(environment_attachment_lease_result(&lease))
    }

    pub(super) fn detach(&self, params: &Value) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentDetachParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid detach params: {error}"))
            })?;
        let detached = self.mark_detached(&params.attachment_lease_id)?;
        Ok(json!({
            "attachmentLeaseId": params.attachment_lease_id,
            "detached": detached,
        }))
    }

    pub(super) fn list(&self, params: &Value) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentListParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid list params: {error}"))
            })?;
        if let Some(scope) = params.scope.as_ref() {
            validate_scope(scope)?;
        }
        let limit = params.limit.unwrap_or(50);
        let leases = self
            .leases
            .lock()
            .map_err(lock_error)?
            .values()
            .filter(|record| {
                params
                    .scope
                    .as_ref()
                    .map_or(true, |scope| scope_matches(scope, &record.lease.scope))
            })
            .filter(|record| {
                params
                    .status
                    .map_or(true, |status| status == record.lease.status)
            })
            .take(limit)
            .map(|record| record.lease.clone())
            .collect::<Vec<_>>();
        Ok(environment_attachment_list_result(&leases, None))
    }

    pub(super) async fn health(&self, params: &Value) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentHealthParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid health params: {error}"))
            })?;
        let source =
            match (params.attachment_lease_id.as_deref(), params.attachment) {
                (Some(_), Some(_)) => return Err(RpcError::new(
                    INVALID_PARAMS,
                    "environment.health accepts either attachmentLeaseId or attachment, not both",
                )),
                (Some(lease_id), None) => self.lease_source(lease_id)?.ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("unknown environment attachment lease: {lease_id}"),
                    )
                })?,
                (None, Some(attachment)) => validate_attachment_source(attachment, false)?,
                (None, None) => {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "environment.health requires attachmentLeaseId or attachment",
                    ))
                }
            };
        let request = EnvironmentReadinessRequest {
            policy: EnvironmentReadinessPolicy::BestEffort,
            timeout_ms: params.timeout_ms,
        };
        let (status, readiness) = self.probe_diagnostic(&source, &request).await;
        Ok(environment_health_result(status, &readiness))
    }

    pub(super) async fn materialize_run_attachments(
        &self,
        refs: Vec<EnvironmentAttachmentRef>,
    ) -> Result<Vec<EnvironmentAttachmentRef>, RpcError> {
        let mut materialized = Vec::with_capacity(refs.len());
        for attachment in refs {
            if let Some(lease_id) = attachment
                .requested_attachment_lease_id()
                .map(ToString::to_string)
            {
                let record = self.lease_record(&lease_id)?.ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("unknown environment attachment lease: {lease_id}"),
                    )
                })?;
                if record.lease.status == EnvironmentAttachmentStatus::Detached {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        format!("environment attachment lease is detached: {lease_id}"),
                    ));
                }
                let mut source = record.source;
                source.id = attachment.id;
                source.is_default = attachment.is_default;
                source.attachment_lease_id = Some(lease_id);
                let request = EnvironmentReadinessRequest {
                    policy: EnvironmentReadinessPolicy::Required,
                    timeout_ms: Some(DEFAULT_READINESS_TIMEOUT_MS),
                };
                self.probe_for_policy(&source, &request).await?;
                materialized.push(source);
            } else {
                let source = validate_attachment_source(attachment, false)?;
                let request = EnvironmentReadinessRequest {
                    policy: EnvironmentReadinessPolicy::Required,
                    timeout_ms: Some(DEFAULT_READINESS_TIMEOUT_MS),
                };
                self.probe_for_policy(&source, &request).await?;
                materialized.push(source);
            }
        }
        Ok(materialized)
    }

    fn idempotent_attach_result(
        &self,
        scope: &EnvironmentAttachmentScope,
        key: &str,
    ) -> Result<Option<EnvironmentAttachmentLease>, RpcError> {
        let Some(lease_id) = self
            .attach_idempotency
            .lock()
            .map_err(lock_error)?
            .get(&idempotency_key(scope, key))
            .cloned()
        else {
            return Ok(None);
        };
        Ok(self.lease_record(&lease_id)?.map(|record| record.lease))
    }

    fn store_attach_idempotency(
        &self,
        scope: &EnvironmentAttachmentScope,
        key: &str,
        lease_id: &str,
    ) -> Result<(), RpcError> {
        self.attach_idempotency
            .lock()
            .map_err(lock_error)?
            .insert(idempotency_key(scope, key), lease_id.to_string());
        Ok(())
    }

    fn find_active_by_scope_and_id(
        &self,
        scope: &EnvironmentAttachmentScope,
        id: &str,
    ) -> Result<Option<LeaseRecord>, RpcError> {
        Ok(self
            .leases
            .lock()
            .map_err(lock_error)?
            .values()
            .find(|record| {
                record.lease.status != EnvironmentAttachmentStatus::Detached
                    && record.lease.id == id
                    && scope_matches(scope, &record.lease.scope)
            })
            .cloned())
    }

    fn lease_record(&self, lease_id: &str) -> Result<Option<LeaseRecord>, RpcError> {
        Ok(self
            .leases
            .lock()
            .map_err(lock_error)?
            .get(lease_id)
            .cloned())
    }

    fn lease_source(&self, lease_id: &str) -> Result<Option<EnvironmentAttachmentRef>, RpcError> {
        Ok(self.lease_record(lease_id)?.map(|record| record.source))
    }

    fn mark_detached(&self, lease_id: &str) -> Result<bool, RpcError> {
        let mut leases = self.leases.lock().map_err(lock_error)?;
        let Some(record) = leases.get_mut(lease_id) else {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown environment attachment lease: {lease_id}"),
            ));
        };
        let detached = record.lease.status != EnvironmentAttachmentStatus::Detached;
        record.lease.status = EnvironmentAttachmentStatus::Detached;
        record.lease.readiness.transport = EnvironmentReadinessPhase::Skipped;
        record.lease.readiness.environment = EnvironmentReadinessPhase::Skipped;
        drop(leases);
        Ok(detached)
    }

    async fn probe_for_policy(
        &self,
        source: &EnvironmentAttachmentRef,
        request: &EnvironmentReadinessRequest,
    ) -> Result<(EnvironmentAttachmentStatus, EnvironmentReadiness), RpcError> {
        match request.policy {
            EnvironmentReadinessPolicy::Skip => Ok((
                EnvironmentAttachmentStatus::Degraded,
                skipped_readiness(source.mode),
            )),
            EnvironmentReadinessPolicy::Required => match self.probe(source, request).await {
                Ok(readiness) => Ok((EnvironmentAttachmentStatus::Ready, readiness)),
                Err(error) => Err(RpcError::new(ENVIRONMENT_UNAVAILABLE, error)),
            },
            EnvironmentReadinessPolicy::BestEffort => {
                Ok(self.probe_diagnostic(source, request).await)
            }
        }
    }

    async fn probe_diagnostic(
        &self,
        source: &EnvironmentAttachmentRef,
        request: &EnvironmentReadinessRequest,
    ) -> (EnvironmentAttachmentStatus, EnvironmentReadiness) {
        match self.probe(source, request).await {
            Ok(readiness) => (EnvironmentAttachmentStatus::Ready, readiness),
            Err(error) => (
                EnvironmentAttachmentStatus::Unavailable,
                unavailable_readiness(source.mode, error),
            ),
        }
    }

    async fn probe(
        &self,
        source: &EnvironmentAttachmentRef,
        request: &EnvironmentReadinessRequest,
    ) -> Result<EnvironmentReadiness, String> {
        match source.kind.as_str() {
            "local" => Ok(ready_readiness(source.mode)),
            "envd" => self.probe_envd(source, request).await,
            other => Err(format!("unsupported environment attachment kind: {other}")),
        }
    }

    async fn probe_envd(
        &self,
        source: &EnvironmentAttachmentRef,
        request: &EnvironmentReadinessRequest,
    ) -> Result<EnvironmentReadiness, String> {
        let endpoint = source
            .requested_endpoint_ref()
            .ok_or_else(|| "envd attachment requires endpointRef".to_string())?;
        let client = EnvdRpcClient::http(endpoint).map_err(|error| error.to_string())?;
        let environment_id = source
            .requested_environment_id()
            .unwrap_or(DEFAULT_ENVIRONMENT_ID)
            .to_string();
        let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_READINESS_TIMEOUT_MS);
        let probe = async {
            client
                .initialize(InitializeEnvdRequest {
                    client_name: Some("starweaver-rpc".to_string()),
                    metadata: starweaver_core::Metadata::default(),
                })
                .await?;
            client
                .open_environment(OpenEnvironmentRequest {
                    environment_id: Some(environment_id.clone()),
                    metadata: starweaver_core::Metadata::default(),
                })
                .await?;
            client
                .environment_state(EnvironmentRequest { environment_id })
                .await
        };
        timeout(Duration::from_millis(timeout_ms), probe)
            .await
            .map_err(|_| format!("envd readiness probe timed out after {timeout_ms}ms"))?
            .map_err(|error| error.to_string())?;
        Ok(ready_readiness(source.mode))
    }
}

fn validate_attachment_source(
    attachment: EnvironmentAttachmentRef,
    allow_lease_ref: bool,
) -> Result<EnvironmentAttachmentRef, RpcError> {
    if !is_valid_environment_attachment_id(&attachment.id) {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "invalid environment attachment id: {}; expected an ASCII slug",
                attachment.id
            ),
        ));
    }
    if attachment.attachment_lease_id.is_some() && !allow_lease_ref {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "attachmentLeaseId is not valid in an attachment source",
        ));
    }
    match attachment.kind.as_str() {
        "local" => Ok(attachment),
        "envd" => {
            let Some(endpoint) = attachment.requested_endpoint_ref() else {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "envd environment attachment requires endpointRef",
                ));
            };
            if endpoint.starts_with("http://") {
                Ok(attachment)
            } else {
                Err(RpcError::new(
                    UNSUPPORTED_FEATURE,
                    "envd environment attachment currently supports http:// endpoint refs",
                ))
            }
        }
        other => Err(RpcError::new(
            UNSUPPORTED_FEATURE,
            format!("unsupported environment attachment kind: {other}"),
        )),
    }
}

fn validate_scope(scope: &EnvironmentAttachmentScope) -> Result<(), RpcError> {
    match scope.kind {
        EnvironmentAttachmentScopeKind::Connection => Ok(()),
        EnvironmentAttachmentScopeKind::Session => {
            if scope
                .session_id
                .as_deref()
                .is_some_and(|session_id| !session_id.trim().is_empty())
            {
                Ok(())
            } else {
                Err(RpcError::new(
                    INVALID_PARAMS,
                    "session-scoped environment attachment requires sessionId",
                ))
            }
        }
    }
}

fn lease_from_source(
    attachment_lease_id: String,
    scope: EnvironmentAttachmentScope,
    source: &EnvironmentAttachmentRef,
    status: EnvironmentAttachmentStatus,
    readiness: EnvironmentReadiness,
) -> EnvironmentAttachmentLease {
    EnvironmentAttachmentLease {
        attachment_lease_id,
        scope,
        id: source.id.clone(),
        kind: source.kind.clone(),
        mode: source.mode,
        is_default: source.is_default,
        mount_root: format!("/environment/{}", source.id),
        status,
        readiness,
        endpoint_ref: source.endpoint_ref.clone(),
        environment_id: source.environment_id.clone(),
        metadata: source.metadata.clone(),
    }
}

fn ready_readiness(mode: EnvironmentAttachmentAccessMode) -> EnvironmentReadiness {
    EnvironmentReadiness {
        transport: EnvironmentReadinessPhase::Ready,
        environment: EnvironmentReadinessPhase::Ready,
        capabilities: capabilities_for_mode(mode),
        message: None,
    }
}

fn skipped_readiness(mode: EnvironmentAttachmentAccessMode) -> EnvironmentReadiness {
    EnvironmentReadiness {
        transport: EnvironmentReadinessPhase::Skipped,
        environment: EnvironmentReadinessPhase::Skipped,
        capabilities: capabilities_for_mode(mode),
        message: Some("readiness probe skipped".to_string()),
    }
}

fn unavailable_readiness(
    mode: EnvironmentAttachmentAccessMode,
    message: String,
) -> EnvironmentReadiness {
    EnvironmentReadiness {
        transport: EnvironmentReadinessPhase::Unavailable,
        environment: EnvironmentReadinessPhase::Unavailable,
        capabilities: capabilities_for_mode(mode),
        message: Some(message),
    }
}

fn capabilities_for_mode(
    mode: EnvironmentAttachmentAccessMode,
) -> EnvironmentReadinessCapabilities {
    let mut files = vec![
        "read".to_string(),
        "list".to_string(),
        "stat".to_string(),
        "glob".to_string(),
        "grep".to_string(),
    ];
    if mode == EnvironmentAttachmentAccessMode::ReadWrite {
        files.insert(1, "write".to_string());
    }
    let command = if mode == EnvironmentAttachmentAccessMode::ReadWrite {
        vec!["run".to_string()]
    } else {
        Vec::new()
    };
    let process = if mode == EnvironmentAttachmentAccessMode::ReadWrite {
        ["start", "wait", "input", "signal", "kill"]
            .into_iter()
            .map(ToString::to_string)
            .collect()
    } else {
        Vec::new()
    };
    EnvironmentReadinessCapabilities {
        files,
        command,
        process,
    }
}

fn scope_matches(left: &EnvironmentAttachmentScope, right: &EnvironmentAttachmentScope) -> bool {
    left.kind == right.kind && left.session_id == right.session_id
}

fn same_source(left: &EnvironmentAttachmentRef, right: &EnvironmentAttachmentRef) -> bool {
    left.kind == right.kind
        && left.mode == right.mode
        && left.endpoint_ref == right.endpoint_ref
        && left.environment_id == right.environment_id
}

fn idempotency_key(scope: &EnvironmentAttachmentScope, key: &str) -> String {
    format!(
        "{:?}:{}:{key}",
        scope.kind,
        scope.session_id.as_deref().unwrap_or_default()
    )
}

fn new_lease_id(id: &str) -> String {
    format!("envatt_{}_{}", id, Uuid::new_v4().simple())
}

fn lock_error(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(
        SERVER_ERROR,
        format!("environment attachment lock poisoned: {error}"),
    )
}
