use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::{
    DEFAULT_ENVIRONMENT_ID, EnvdService, EnvironmentRequest, InitializeEnvdRequest,
    OpenEnvironmentRequest, envd_protocol_identity,
};
use starweaver_rpc_core::{
    ALREADY_EXISTS, ENVIRONMENT_UNAVAILABLE, EnvironmentAttachParams,
    EnvironmentAttachmentAccessMode, EnvironmentAttachmentLease, EnvironmentAttachmentRef,
    EnvironmentAttachmentScope, EnvironmentAttachmentScopeKind, EnvironmentAttachmentStatus,
    EnvironmentDetachParams, EnvironmentHealthParams, EnvironmentListParams, EnvironmentReadiness,
    EnvironmentReadinessCapabilities, EnvironmentReadinessPhase, EnvironmentReadinessPolicy,
    EnvironmentReadinessRequest, IDEMPOTENCY_CONFLICT, INVALID_PARAMS,
    LOCAL_ENVIRONMENT_ATTACHMENT_ID, LOCAL_ENVIRONMENT_ATTACHMENT_KIND, RUN_CONFLICT, RpcError,
    SERVER_ERROR, UNSUPPORTED_FEATURE, environment_attachment_lease_result,
    environment_attachment_list_result, environment_health_result,
    is_valid_environment_attachment_id,
};
use tokio::{sync::Mutex as AsyncMutex, time::timeout};
use uuid::Uuid;

use crate::session_management::command_fingerprint;

const DEFAULT_READINESS_TIMEOUT_MS: u64 = 5_000;
const MAX_READINESS_TIMEOUT_MS: u64 = 60_000;

fn envd_client_for_attachment(
    attachment: &EnvironmentAttachmentRef,
) -> Result<EnvdRpcClient, String> {
    let endpoint = attachment
        .requested_endpoint_ref()
        .ok_or_else(|| "envd attachment requires endpointRef".to_string())?;
    EnvdRpcClient::from_local_endpoint_ref(endpoint, attachment.requested_auth_token())
        .map_err(|error| error.to_string())
}

fn validate_envd_attachment_transport(attachment: &EnvironmentAttachmentRef) -> Result<(), String> {
    let endpoint = attachment
        .requested_endpoint_ref()
        .ok_or_else(|| "envd environment attachment requires endpointRef".to_string())?;
    starweaver_envd_client::validate_local_endpoint_ref(endpoint, attachment.requested_auth_token())
        .map_err(|error| error.to_string())
}

fn redacted_envd_endpoint_ref(attachment: &EnvironmentAttachmentRef) -> Option<String> {
    starweaver_envd_client::redacted_endpoint_ref(attachment.requested_endpoint_ref()?)
}

/// RPC-owned environment attachment leases and active-run usage accounting.
#[derive(Clone)]
pub struct EnvironmentAttachmentManager {
    leases: Arc<Mutex<BTreeMap<String, LeaseRecord>>>,
    attach_idempotency: Arc<Mutex<BTreeMap<String, AttachIdempotencyRecord>>>,
    attach_serial: Arc<AsyncMutex<()>>,
    active_runs: Arc<Mutex<BTreeMap<String, BTreeMap<String, usize>>>>,
}

#[derive(Clone)]
struct LeaseRecord {
    lease: EnvironmentAttachmentLease,
    source: EnvironmentAttachmentRef,
    owner_connection_id: Option<String>,
}

#[derive(Clone)]
struct AttachIdempotencyRecord {
    command_fingerprint: String,
    lease_id: String,
}

impl EnvironmentAttachmentManager {
    pub(super) fn new() -> Self {
        Self {
            leases: Arc::new(Mutex::new(BTreeMap::new())),
            attach_idempotency: Arc::new(Mutex::new(BTreeMap::new())),
            attach_serial: Arc::new(AsyncMutex::new(())),
            active_runs: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(super) async fn attach(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentAttachParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid attach params: {error}"))
            })?;
        validate_scope(&params.scope)?;
        validate_readiness_timeout(params.readiness.timeout_ms)?;
        let owner_connection_id = connection_owner(&params.scope, connection_id)?;
        let source = validate_attachment_source(params.attachment, false)?;
        let fingerprint = attach_command_fingerprint(&params.scope, &source, &params.readiness)?;
        // Attachment creation is low-frequency control-plane work. Serializing the bounded probe
        // and commit closes duplicate-probe and duplicate-lease races without a cancellable
        // pending-state protocol.
        let _attach_guard = self.attach_serial.lock().await;
        if let Some(key) = params.idempotency_key.as_deref()
            && let Some(lease) = self.idempotent_attach_result(
                &params.scope,
                key,
                &fingerprint,
                owner_connection_id.as_deref(),
            )?
        {
            return Ok(environment_attachment_lease_result(&lease));
        }
        if let Some(existing) =
            self.find_active_by_scope_and_id(&params.scope, &source.id, connection_id)?
        {
            if same_source(&existing.source, &source) {
                if let Some(key) = params.idempotency_key.as_deref() {
                    self.store_attach_idempotency(
                        &params.scope,
                        key,
                        &fingerprint,
                        &existing.lease.attachment_lease_id,
                        owner_connection_id.as_deref(),
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
                owner_connection_id: owner_connection_id.clone(),
            },
        );
        if let Some(key) = params.idempotency_key.as_deref() {
            self.store_attach_idempotency(
                &lease.scope,
                key,
                &fingerprint,
                &lease.attachment_lease_id,
                owner_connection_id.as_deref(),
            )?;
        }
        Ok(environment_attachment_lease_result(&lease))
    }

    pub(super) fn detach(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentDetachParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid detach params: {error}"))
            })?;
        let detached = self.detach_lease(&params.attachment_lease_id, connection_id)?;
        Ok(json!({
            "attachmentLeaseId": params.attachment_lease_id,
            "detached": detached,
        }))
    }

    pub(super) fn list(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
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
            .filter(|record| connection_can_access(record, connection_id))
            .filter(|record| {
                params
                    .scope
                    .as_ref()
                    .is_none_or(|scope| scope_matches(scope, &record.lease.scope))
            })
            .filter(|record| {
                params
                    .status
                    .is_none_or(|status| status == record.lease.status)
            })
            .take(limit)
            .map(|record| record.lease.clone())
            .collect::<Vec<_>>();
        Ok(environment_attachment_list_result(&leases, None))
    }

    pub(super) async fn health(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<EnvironmentHealthParams>(params.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid health params: {error}"))
            })?;
        validate_readiness_timeout(params.timeout_ms)?;
        let source = match (params.attachment_lease_id.as_deref(), params.attachment) {
            (Some(_), Some(_)) => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "environment.health accepts either attachmentLeaseId or attachment, not both",
                ));
            }
            (Some(lease_id), None) => self
                .authorized_lease_record(lease_id, connection_id)?
                .map(|record| record.source)
                .ok_or_else(|| {
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
                ));
            }
        };
        let request = EnvironmentReadinessRequest {
            policy: EnvironmentReadinessPolicy::BestEffort,
            timeout_ms: params.timeout_ms,
        };
        let (status, readiness) = self.probe_diagnostic(&source, &request).await;
        Ok(environment_health_result(status, &readiness))
    }

    pub(super) fn authorize_run_attachment_replay(
        &self,
        refs: &[EnvironmentAttachmentRef],
        run_session_id: Option<&str>,
        connection_id: Option<&str>,
    ) -> Result<(), RpcError> {
        for attachment in refs {
            let Some(lease_id) = attachment.requested_attachment_lease_id() else {
                continue;
            };
            let record = self
                .authorized_lease_record(lease_id, connection_id)?
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("unknown environment attachment lease: {lease_id}"),
                    )
                })?;
            validate_lease_scope_for_run(&record.lease, run_session_id)?;
        }
        Ok(())
    }

    pub(super) async fn materialize_run_attachments(
        &self,
        refs: Vec<EnvironmentAttachmentRef>,
        run_session_id: Option<&str>,
        connection_id: Option<&str>,
    ) -> Result<Vec<EnvironmentAttachmentRef>, RpcError> {
        let mut materialized = Vec::with_capacity(refs.len());
        for attachment in refs {
            if let Some(lease_id) = attachment
                .requested_attachment_lease_id()
                .map(ToString::to_string)
            {
                let record = self
                    .authorized_lease_record(&lease_id, connection_id)?
                    .ok_or_else(|| {
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
                validate_lease_scope_for_run(&record.lease, run_session_id)?;
                let mut source = record.source;
                if let Some(mode) = attachment.requested_mode() {
                    ensure_mode_override(source.resolved_mode(), mode)?;
                    source.mode = Some(mode);
                }
                source.id = attachment.id;
                source.is_default = attachment.is_default;
                source.is_default_for_shell = attachment.is_default_for_shell;
                source.attachment_lease_id = Some(lease_id);
                ensure_reserved_local_kind(&source)?;
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

    pub(super) async fn materialize_active_attachment(
        &self,
        attachment: EnvironmentAttachmentRef,
        run_session_id: Option<&str>,
        connection_id: Option<&str>,
    ) -> Result<EnvironmentAttachmentRef, RpcError> {
        if let Some(lease_id) = attachment
            .requested_attachment_lease_id()
            .map(ToString::to_string)
        {
            let record = self
                .authorized_lease_record(&lease_id, connection_id)?
                .ok_or_else(|| {
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
            validate_lease_scope_for_run(&record.lease, run_session_id)?;
            let mut source = record.source;
            if let Some(mode) = attachment.requested_mode() {
                ensure_mode_override(source.resolved_mode(), mode)?;
                source.mode = Some(mode);
            }
            source.id = attachment.id;
            source.is_default = attachment.is_default;
            source.is_default_for_shell = attachment.is_default_for_shell;
            source.attachment_lease_id = Some(lease_id);
            ensure_reserved_local_kind(&source)?;
            let request = EnvironmentReadinessRequest {
                policy: EnvironmentReadinessPolicy::Required,
                timeout_ms: Some(DEFAULT_READINESS_TIMEOUT_MS),
            };
            self.probe_for_policy(&source, &request).await?;
            return Ok(source);
        }
        let source = validate_attachment_source(attachment, false)?;
        let request = EnvironmentReadinessRequest {
            policy: EnvironmentReadinessPolicy::Required,
            timeout_ms: Some(DEFAULT_READINESS_TIMEOUT_MS),
        };
        self.probe_for_policy(&source, &request).await?;
        Ok(source)
    }

    pub(super) fn mark_run_started(
        &self,
        run_id: &str,
        attachments: &[EnvironmentAttachmentRef],
    ) -> Result<(), RpcError> {
        let mut lease_counts = BTreeMap::<String, usize>::new();
        for lease_id in attachments
            .iter()
            .filter_map(|attachment| attachment.requested_attachment_lease_id())
        {
            *lease_counts.entry(lease_id.to_string()).or_default() += 1;
        }
        if lease_counts.is_empty() {
            return Ok(());
        }
        let mut active_runs = self.active_runs.lock().map_err(lock_error)?;
        let leases = self.leases.lock().map_err(lock_error)?;
        for lease_id in lease_counts.keys() {
            let Some(record) = leases.get(lease_id) else {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("unknown environment attachment lease: {lease_id}"),
                ));
            };
            if record.lease.status == EnvironmentAttachmentStatus::Detached {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("environment attachment lease is detached: {lease_id}"),
                ));
            }
        }
        active_runs.insert(run_id.to_string(), lease_counts);
        drop(leases);
        drop(active_runs);
        Ok(())
    }

    pub(super) fn replace_run_attachment(
        &self,
        run_id: &str,
        previous: Option<&EnvironmentAttachmentRef>,
        replacement: Option<&EnvironmentAttachmentRef>,
    ) -> Result<(), RpcError> {
        let previous_lease = previous
            .and_then(EnvironmentAttachmentRef::requested_attachment_lease_id)
            .map(ToString::to_string);
        let replacement_lease = replacement
            .and_then(EnvironmentAttachmentRef::requested_attachment_lease_id)
            .map(ToString::to_string);
        if previous_lease == replacement_lease {
            return Ok(());
        }
        if let Some(lease_id) = replacement_lease.as_deref() {
            let leases = self.leases.lock().map_err(lock_error)?;
            let Some(record) = leases.get(lease_id) else {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("unknown environment attachment lease: {lease_id}"),
                ));
            };
            if record.lease.status == EnvironmentAttachmentStatus::Detached {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("environment attachment lease is detached: {lease_id}"),
                ));
            }
            drop(leases);
        }
        let mut active_runs = self.active_runs.lock().map_err(lock_error)?;
        let lease_counts = active_runs.entry(run_id.to_string()).or_default();
        if let Some(lease_id) = previous_lease.as_deref()
            && let Some(count) = lease_counts.get_mut(lease_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                lease_counts.remove(lease_id);
            }
        }
        if let Some(lease_id) = replacement_lease {
            lease_counts
                .entry(lease_id)
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        if lease_counts.is_empty() {
            active_runs.remove(run_id);
        }
        drop(active_runs);
        Ok(())
    }

    pub(super) fn mark_run_finished(&self, run_id: &str) -> Result<(), RpcError> {
        self.active_runs.lock().map_err(lock_error)?.remove(run_id);
        Ok(())
    }

    fn idempotent_attach_result(
        &self,
        scope: &EnvironmentAttachmentScope,
        key: &str,
        command_fingerprint: &str,
        owner_connection_id: Option<&str>,
    ) -> Result<Option<EnvironmentAttachmentLease>, RpcError> {
        let Some(receipt) = self
            .attach_idempotency
            .lock()
            .map_err(lock_error)?
            .get(&idempotency_key(scope, key, owner_connection_id))
            .cloned()
        else {
            return Ok(None);
        };
        if receipt.command_fingerprint != command_fingerprint {
            return Err(RpcError::new(
                IDEMPOTENCY_CONFLICT,
                "environment.attach idempotency key is bound to different params",
            ));
        }
        Ok(self
            .lease_record(&receipt.lease_id)?
            .map(|record| record.lease))
    }

    fn store_attach_idempotency(
        &self,
        scope: &EnvironmentAttachmentScope,
        key: &str,
        command_fingerprint: &str,
        lease_id: &str,
        owner_connection_id: Option<&str>,
    ) -> Result<(), RpcError> {
        self.attach_idempotency.lock().map_err(lock_error)?.insert(
            idempotency_key(scope, key, owner_connection_id),
            AttachIdempotencyRecord {
                command_fingerprint: command_fingerprint.to_string(),
                lease_id: lease_id.to_string(),
            },
        );
        Ok(())
    }

    fn find_active_by_scope_and_id(
        &self,
        scope: &EnvironmentAttachmentScope,
        id: &str,
        connection_id: Option<&str>,
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
                    && connection_can_access(record, connection_id)
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

    fn authorized_lease_record(
        &self,
        lease_id: &str,
        connection_id: Option<&str>,
    ) -> Result<Option<LeaseRecord>, RpcError> {
        let record = self.lease_record(lease_id)?;
        if record
            .as_ref()
            .is_some_and(|record| !connection_can_access(record, connection_id))
        {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("environment attachment lease is owned by another connection: {lease_id}"),
            ));
        }
        Ok(record)
    }

    pub(super) fn release_connection_leases(&self, connection_id: &str) {
        if let Ok(mut leases) = self.leases.lock() {
            for record in leases
                .values_mut()
                .filter(|record| record.owner_connection_id.as_deref() == Some(connection_id))
            {
                record.lease.status = EnvironmentAttachmentStatus::Detached;
                record.lease.readiness.transport = EnvironmentReadinessPhase::Skipped;
                record.lease.readiness.environment = EnvironmentReadinessPhase::Skipped;
            }
        }
    }

    fn detach_lease(&self, lease_id: &str, connection_id: Option<&str>) -> Result<bool, RpcError> {
        let active_runs = self.active_runs.lock().map_err(lock_error)?;
        if let Some((run_id, _)) = active_runs
            .iter()
            .find(|(_, lease_counts)| lease_counts.contains_key(lease_id))
        {
            return Err(RpcError::new(
                RUN_CONFLICT,
                format!("environment attachment lease is still used by active run: {run_id}"),
            ));
        }
        let mut leases = self.leases.lock().map_err(lock_error)?;
        let Some(record) = leases.get_mut(lease_id) else {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown environment attachment lease: {lease_id}"),
            ));
        };
        if !connection_can_access(record, connection_id) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("environment attachment lease is owned by another connection: {lease_id}"),
            ));
        }
        let detached = record.lease.status != EnvironmentAttachmentStatus::Detached;
        record.lease.status = EnvironmentAttachmentStatus::Detached;
        record.lease.readiness.transport = EnvironmentReadinessPhase::Skipped;
        record.lease.readiness.environment = EnvironmentReadinessPhase::Skipped;
        drop(leases);
        drop(active_runs);
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
                skipped_readiness(source.resolved_mode()),
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
                unavailable_readiness(source.resolved_mode(), error),
            ),
        }
    }

    async fn probe(
        &self,
        source: &EnvironmentAttachmentRef,
        request: &EnvironmentReadinessRequest,
    ) -> Result<EnvironmentReadiness, String> {
        match source.kind.as_str() {
            "local" => Ok(ready_readiness(source.resolved_mode())),
            "envd" => self.probe_envd(source, request).await,
            other => Err(format!("unsupported environment attachment kind: {other}")),
        }
    }

    async fn probe_envd(
        &self,
        source: &EnvironmentAttachmentRef,
        request: &EnvironmentReadinessRequest,
    ) -> Result<EnvironmentReadiness, String> {
        let client = envd_client_for_attachment(source)?;
        let environment_id = source
            .requested_environment_id()
            .unwrap_or(DEFAULT_ENVIRONMENT_ID)
            .to_string();
        let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_READINESS_TIMEOUT_MS);
        if timeout_ms > MAX_READINESS_TIMEOUT_MS {
            return Err(format!(
                "environment readiness timeout exceeds the host maximum of {MAX_READINESS_TIMEOUT_MS}ms"
            ));
        }
        let probe = async {
            client
                .initialize(InitializeEnvdRequest {
                    protocol: Some(envd_protocol_identity()),
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
        Ok(ready_readiness(source.resolved_mode()))
    }
}

fn validate_readiness_timeout(timeout_ms: Option<u64>) -> Result<(), RpcError> {
    if timeout_ms.is_some_and(|timeout_ms| timeout_ms > MAX_READINESS_TIMEOUT_MS) {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!("environment readiness timeout must not exceed {MAX_READINESS_TIMEOUT_MS}ms"),
        ));
    }
    Ok(())
}

fn validate_attachment_source(
    mut attachment: EnvironmentAttachmentRef,
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
    ensure_reserved_local_kind(&attachment)?;
    if attachment.attachment_lease_id.is_some() && !allow_lease_ref {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "attachmentLeaseId is not valid in an attachment source",
        ));
    }
    match attachment.kind.as_str() {
        "local" => {
            attachment.mode = Some(attachment.resolved_mode());
            Ok(attachment)
        }
        "envd" => {
            validate_envd_attachment_transport(&attachment)
                .map_err(|error| RpcError::new(INVALID_PARAMS, error))?;
            attachment.mode = Some(attachment.resolved_mode());
            Ok(attachment)
        }
        other => Err(RpcError::new(
            UNSUPPORTED_FEATURE,
            format!("unsupported environment attachment kind: {other}"),
        )),
    }
}

fn ensure_reserved_local_kind(attachment: &EnvironmentAttachmentRef) -> Result<(), RpcError> {
    if attachment.id == LOCAL_ENVIRONMENT_ATTACHMENT_ID
        && attachment.kind != LOCAL_ENVIRONMENT_ATTACHMENT_KIND
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "reserved environment attachment id local requires kind local",
        ));
    }
    Ok(())
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

fn connection_owner(
    scope: &EnvironmentAttachmentScope,
    connection_id: Option<&str>,
) -> Result<Option<String>, RpcError> {
    match scope.kind {
        EnvironmentAttachmentScopeKind::Connection => connection_id
            .map(ToString::to_string)
            .ok_or_else(|| {
                RpcError::new(
                    UNSUPPORTED_FEATURE,
                    "connection-scoped environment attachments require a stateful live connection",
                )
            })
            .map(Some),
        EnvironmentAttachmentScopeKind::Session => Ok(None),
    }
}

fn connection_can_access(record: &LeaseRecord, connection_id: Option<&str>) -> bool {
    record
        .owner_connection_id
        .as_deref()
        .is_none_or(|owner| Some(owner) == connection_id)
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
        mode: source.resolved_mode(),
        is_default: source.is_default,
        is_default_for_shell: source.is_default_for_shell,
        mount_root: format!("/environment/{}", source.id),
        status,
        readiness,
        endpoint_ref: redacted_endpoint_ref(source),
        environment_id: source.environment_id.clone(),
        // Source metadata is an untyped extension map. Keep it provider-private until reviewed
        // host-visible keys are defined explicitly.
        metadata: serde_json::Map::new(),
    }
}

fn redacted_endpoint_ref(source: &EnvironmentAttachmentRef) -> Option<String> {
    if source.kind == "envd" {
        redacted_envd_endpoint_ref(source)
    } else {
        source.endpoint_ref.clone()
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
        && left.resolved_mode() == right.resolved_mode()
        && left.endpoint_ref == right.endpoint_ref
        && left.environment_id == right.environment_id
        && left.auth_token == right.auth_token
}

fn ensure_mode_override(
    lease_mode: EnvironmentAttachmentAccessMode,
    requested_mode: EnvironmentAttachmentAccessMode,
) -> Result<(), RpcError> {
    if lease_mode == EnvironmentAttachmentAccessMode::ReadOnly
        && requested_mode == EnvironmentAttachmentAccessMode::ReadWrite
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "run-local environment attachment mode cannot widen a read-only lease",
        ));
    }
    Ok(())
}

fn validate_lease_scope_for_run(
    lease: &EnvironmentAttachmentLease,
    run_session_id: Option<&str>,
) -> Result<(), RpcError> {
    if lease.scope.kind == EnvironmentAttachmentScopeKind::Connection {
        return Ok(());
    }
    let Some(lease_session_id) = lease.scope.session_id.as_deref() else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "session-scoped environment attachment lease is missing sessionId: {}",
                lease.attachment_lease_id
            ),
        ));
    };
    let Some(run_session_id) = run_session_id else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "session-scoped environment attachment lease requires run sessionId: {}",
                lease.attachment_lease_id
            ),
        ));
    };
    if lease_session_id != run_session_id {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "environment attachment lease belongs to session {lease_session_id}, not {run_session_id}"
            ),
        ));
    }
    Ok(())
}

fn attach_command_fingerprint(
    scope: &EnvironmentAttachmentScope,
    source: &EnvironmentAttachmentRef,
    readiness: &EnvironmentReadinessRequest,
) -> Result<String, RpcError> {
    let auth_token_digest = source
        .requested_auth_token()
        .map(|token| command_fingerprint("environment.attach.auth", &token))
        .transpose()
        .map_err(|error| RpcError::new(SERVER_ERROR, error.to_string()))?;
    command_fingerprint(
        "environment.attach",
        &json!({
            "scope": scope,
            "attachment": source,
            "authTokenDigest": auth_token_digest,
            "readiness": readiness,
        }),
    )
    .map_err(|error| RpcError::new(SERVER_ERROR, error.to_string()))
}

fn idempotency_key(
    scope: &EnvironmentAttachmentScope,
    key: &str,
    owner_connection_id: Option<&str>,
) -> String {
    format!(
        "{:?}:{}:{}:{key}",
        scope.kind,
        scope.session_id.as_deref().unwrap_or_default(),
        owner_connection_id.unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;
    use starweaver_rpc_core::environment_attachment_refs;

    const TEST_CONNECTION_ID: Option<&str> = Some("test-connection");

    #[tokio::test]
    async fn connection_scoped_leases_are_owned_and_revoked_with_the_connection() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "attachment": {"id": "workspace", "kind": "local"},
                    "readiness": {"policy": "required"}
                }),
                Some("connection-a"),
            )
            .await
            .unwrap();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();

        let owner_list = manager.list(&json!({}), Some("connection-a")).unwrap();
        assert_eq!(owner_list["attachments"].as_array().unwrap().len(), 1);
        let other_list = manager.list(&json!({}), Some("connection-b")).unwrap();
        assert!(other_list["attachments"].as_array().unwrap().is_empty());

        let refs = environment_attachment_refs(&json!({
            "environmentAttachments": [{
                "id": "workspace",
                "attachmentLeaseId": lease_id,
                "default": true
            }]
        }))
        .unwrap();
        let use_error = manager
            .materialize_run_attachments(refs, None, Some("connection-b"))
            .await
            .unwrap_err();
        assert_eq!(use_error.code, INVALID_PARAMS);
        assert!(use_error.message.contains("another connection"));
        let detach_error = manager
            .detach(
                &json!({"attachmentLeaseId": lease_id}),
                Some("connection-b"),
            )
            .unwrap_err();
        assert_eq!(detach_error.code, INVALID_PARAMS);
        assert!(detach_error.message.contains("another connection"));

        manager.release_connection_leases("connection-a");
        let owner_list = manager
            .list(&json!({"status": "detached"}), Some("connection-a"))
            .unwrap();
        assert_eq!(owner_list["attachments"].as_array().unwrap().len(), 1);
        let refs = environment_attachment_refs(&json!({
            "environmentAttachments": [{
                "id": "workspace",
                "attachmentLeaseId": lease_id,
                "default": true
            }]
        }))
        .unwrap();
        let detached_error = manager
            .materialize_run_attachments(refs, None, Some("connection-a"))
            .await
            .unwrap_err();
        assert_eq!(detached_error.code, INVALID_PARAMS);
        assert!(detached_error.message.contains("detached"));
    }

    #[tokio::test]
    async fn concurrent_attach_replays_one_fingerprint_bound_lease() {
        let manager = EnvironmentAttachmentManager::new();
        let params = json!({
            "attachment": {"id": "workspace", "kind": "local"},
            "readiness": {"policy": "required"},
            "idempotencyKey": "attach-once"
        });
        let (first, second) = tokio::join!(
            manager.attach(&params, TEST_CONNECTION_ID),
            manager.attach(&params, TEST_CONNECTION_ID),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(
            first["attachment"]["attachmentLeaseId"],
            second["attachment"]["attachmentLeaseId"]
        );
        assert_eq!(manager.leases.lock().unwrap().len(), 1);

        let conflict = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "workspace",
                        "kind": "local",
                        "mode": "read_only"
                    },
                    "readiness": {"policy": "required"},
                    "idempotencyKey": "attach-once"
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, IDEMPOTENCY_CONFLICT);
        assert_eq!(manager.leases.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn attach_rejects_readiness_timeout_above_host_maximum() {
        let manager = EnvironmentAttachmentManager::new();
        let error = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "workspace",
                        "kind": "envd",
                        "endpointRef": "stdio:///private/envd?arg=--wait"
                    },
                    "readiness": {
                        "policy": "required",
                        "timeoutMs": MAX_READINESS_TIMEOUT_MS + 1
                    }
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("must not exceed"));
        assert!(manager.leases.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn attach_idempotency_binds_request_only_auth_token() {
        let manager = EnvironmentAttachmentManager::new();
        let request = |token: &str| {
            json!({
                "attachment": {
                    "id": "workspace",
                    "kind": "local",
                    "authToken": token
                },
                "readiness": {"policy": "skip"},
                "idempotencyKey": "attach-token"
            })
        };
        manager
            .attach(&request("first-secret"), TEST_CONNECTION_ID)
            .await
            .unwrap();
        let conflict = manager
            .attach(&request("second-secret"), TEST_CONNECTION_ID)
            .await
            .unwrap_err();
        assert_eq!(conflict.code, IDEMPOTENCY_CONFLICT);
        assert!(!conflict.message.contains("secret"));
    }

    #[tokio::test]
    async fn attach_rejects_reserved_local_id_for_envd_source() {
        let manager = EnvironmentAttachmentManager::new();
        let error = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "local",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc",
                        "authToken": "secret"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("reserved"));
    }

    #[tokio::test]
    async fn lease_ref_cannot_remap_envd_source_to_reserved_local_id() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "remote",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc",
                        "authToken": "secret"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();

        let error = manager
            .materialize_run_attachments(
                environment_attachment_refs(&json!({
                    "environmentAttachments": [
                        {"id": "local", "attachmentLeaseId": lease_id, "default": true}
                    ]
                }))
                .unwrap(),
                None,
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("reserved"));
    }

    #[tokio::test]
    async fn detach_rejects_lease_used_by_active_run() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "attachment": {"id": "workspace", "kind": "local"},
                    "readiness": {"policy": "required"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let attachments = manager
            .materialize_run_attachments(
                environment_attachment_refs(&json!({
                    "environmentAttachments": [
                        {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                    ]
                }))
                .unwrap(),
                None,
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();

        manager.mark_run_started("run_1", &attachments).unwrap();
        let error = manager
            .detach(&json!({"attachmentLeaseId": lease_id}), TEST_CONNECTION_ID)
            .unwrap_err();
        assert_eq!(error.code, RUN_CONFLICT);
        assert!(error.message.contains("run_1"));

        manager.mark_run_finished("run_1").unwrap();
        let detached = manager
            .detach(&json!({"attachmentLeaseId": lease_id}), TEST_CONNECTION_ID)
            .unwrap();
        assert_eq!(detached["detached"], true);
    }

    #[tokio::test]
    async fn active_run_lease_usage_is_reference_counted() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "attachment": {"id": "workspace", "kind": "local"},
                    "readiness": {"policy": "required"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let attachments = manager
            .materialize_run_attachments(
                environment_attachment_refs(&json!({
                    "environmentAttachments": [
                        {"id": "workspace", "attachmentLeaseId": lease_id, "default": true},
                        {"id": "workspace-copy", "attachmentLeaseId": lease_id}
                    ]
                }))
                .unwrap(),
                None,
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();

        manager.mark_run_started("run_1", &attachments).unwrap();
        manager
            .replace_run_attachment("run_1", Some(&attachments[0]), None)
            .unwrap();
        let error = manager
            .detach(&json!({"attachmentLeaseId": lease_id}), TEST_CONNECTION_ID)
            .unwrap_err();
        assert_eq!(error.code, RUN_CONFLICT);

        manager
            .replace_run_attachment("run_1", Some(&attachments[1]), None)
            .unwrap();
        let detached = manager
            .detach(&json!({"attachmentLeaseId": lease_id}), TEST_CONNECTION_ID)
            .unwrap();
        assert_eq!(detached["detached"], true);
    }

    #[tokio::test]
    async fn mark_run_started_rejects_lease_detached_after_materialization() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "attachment": {"id": "workspace", "kind": "local"},
                    "readiness": {"policy": "required"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let attachments = manager
            .materialize_run_attachments(
                environment_attachment_refs(&json!({
                    "environmentAttachments": [
                        {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                    ]
                }))
                .unwrap(),
                None,
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        manager
            .detach(&json!({"attachmentLeaseId": lease_id}), TEST_CONNECTION_ID)
            .unwrap();

        let error = manager.mark_run_started("run_1", &attachments).unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("detached"));
    }

    #[tokio::test]
    async fn lease_refs_can_only_downgrade_run_local_mode() {
        let manager = EnvironmentAttachmentManager::new();
        let read_write = manager
            .attach(
                &json!({
                    "attachment": {"id": "workspace", "kind": "local", "mode": "read_write"},
                    "readiness": {"policy": "required"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let read_write_lease = read_write["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let downgraded = manager
            .materialize_run_attachments(
                environment_attachment_refs(&json!({
                    "environmentAttachments": [
                        {
                            "id": "workspace",
                            "attachmentLeaseId": read_write_lease,
                            "mode": "read_only",
                            "default": true
                        }
                    ]
                }))
                .unwrap(),
                None,
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        assert_eq!(
            downgraded[0].resolved_mode(),
            EnvironmentAttachmentAccessMode::ReadOnly
        );

        let read_only = manager
            .attach(
                &json!({
                    "attachment": {"id": "data", "kind": "local", "mode": "read_only"},
                    "readiness": {"policy": "required"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let read_only_lease = read_only["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let error = manager
            .materialize_run_attachments(
                environment_attachment_refs(&json!({
                    "environmentAttachments": [
                        {
                            "id": "data",
                            "attachmentLeaseId": read_only_lease,
                            "mode": "read_write",
                            "default": true
                        }
                    ]
                }))
                .unwrap(),
                None,
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("cannot widen"));
    }

    #[tokio::test]
    async fn session_scoped_lease_refs_must_match_run_session() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "scope": {"kind": "session", "sessionId": "session_a"},
                    "attachment": {"id": "workspace", "kind": "local"},
                    "readiness": {"policy": "required"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let refs = || {
            environment_attachment_refs(&json!({
                "environmentAttachments": [
                    {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                ]
            }))
            .unwrap()
        };

        let missing_session = manager
            .materialize_run_attachments(refs(), None, TEST_CONNECTION_ID)
            .await
            .unwrap_err();
        assert_eq!(missing_session.code, INVALID_PARAMS);
        assert!(missing_session.message.contains("requires run sessionId"));

        let wrong_session = manager
            .materialize_run_attachments(refs(), Some("session_b"), TEST_CONNECTION_ID)
            .await
            .unwrap_err();
        assert_eq!(wrong_session.code, INVALID_PARAMS);
        assert!(wrong_session.message.contains("session_a"));
        assert!(wrong_session.message.contains("session_b"));

        let materialized = manager
            .materialize_run_attachments(refs(), Some("session_a"), TEST_CONNECTION_ID)
            .await
            .unwrap();
        assert_eq!(
            materialized[0].requested_attachment_lease_id(),
            Some(lease_id)
        );
    }

    #[tokio::test]
    async fn envd_attach_rejects_invalid_auth_token_before_readiness() {
        let manager = EnvironmentAttachmentManager::new();

        let empty = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "remote",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc",
                        "authToken": " "
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(empty.code, INVALID_PARAMS);
        assert!(empty.message.contains("cannot be empty"));

        let newline = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "remote",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc",
                        "authToken": "line\nbreak"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(newline.code, INVALID_PARAMS);
        assert!(newline.message.contains("cannot contain newlines"));
    }

    #[tokio::test]
    async fn envd_attach_rejects_unsafe_http_endpoint_refs() {
        let manager = EnvironmentAttachmentManager::new();

        let non_loopback = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "remote",
                        "kind": "envd",
                        "endpointRef": "http://example.com:8766/rpc",
                        "authToken": "secret"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(non_loopback.code, INVALID_PARAMS);
        assert!(non_loopback.message.contains("must be loopback"));

        let query = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "remote",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc?token=secret",
                        "authToken": "secret"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(query.code, INVALID_PARAMS);
        assert!(query.message.contains("query strings"));

        let userinfo = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "remote",
                        "kind": "envd",
                        "endpointRef": "http://user:pass@127.0.0.1:8766/rpc",
                        "authToken": "secret"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap_err();
        assert_eq!(userinfo.code, INVALID_PARAMS);
        assert!(userinfo.message.contains("userinfo"));
    }

    #[tokio::test]
    async fn envd_attach_redacts_stdio_endpoint_ref_in_lease_result() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(
                &json!({
                    "attachment": {
                        "id": "stdio_data",
                        "kind": "envd",
                        "endpointRef": "stdio:///tmp/envd-fixture?arg=--token&arg=secret",
                        "environmentId": "default"
                    },
                    "readiness": {"policy": "skip"}
                }),
                TEST_CONNECTION_ID,
            )
            .await
            .unwrap();

        assert_eq!(attached["attachment"]["endpointRef"], "stdio://<redacted>");
        assert!(!attached["attachment"].to_string().contains("envd-fixture"));
        assert!(!attached["attachment"].to_string().contains("secret"));
    }
}
