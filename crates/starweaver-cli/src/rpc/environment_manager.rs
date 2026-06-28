use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{json, Value};
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
    LOCAL_ENVIRONMENT_ATTACHMENT_ID, LOCAL_ENVIRONMENT_ATTACHMENT_KIND, RUN_CONFLICT, SERVER_ERROR,
    UNSUPPORTED_FEATURE,
};
use tokio::time::timeout;
use uuid::Uuid;

use crate::environment::{
    envd_client_for_attachment, redacted_envd_endpoint_ref, validate_envd_attachment_transport,
};

const DEFAULT_READINESS_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone)]
pub(super) struct EnvironmentAttachmentManager {
    leases: Arc<Mutex<BTreeMap<String, LeaseRecord>>>,
    attach_idempotency: Arc<Mutex<BTreeMap<String, String>>>,
    active_runs: Arc<Mutex<BTreeMap<String, BTreeMap<String, usize>>>>,
}

#[derive(Clone)]
struct LeaseRecord {
    lease: EnvironmentAttachmentLease,
    source: EnvironmentAttachmentRef,
}

impl EnvironmentAttachmentManager {
    pub(super) fn new() -> Self {
        Self {
            leases: Arc::new(Mutex::new(BTreeMap::new())),
            attach_idempotency: Arc::new(Mutex::new(BTreeMap::new())),
            active_runs: Arc::new(Mutex::new(BTreeMap::new())),
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
        let detached = self.detach_lease(&params.attachment_lease_id)?;
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
        run_session_id: Option<&str>,
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
    ) -> Result<EnvironmentAttachmentRef, RpcError> {
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

    pub(super) fn mark_run_mounted(
        &self,
        run_id: &str,
        attachment: &EnvironmentAttachmentRef,
    ) -> Result<(), RpcError> {
        let Some(lease_id) = attachment.requested_attachment_lease_id() else {
            return Ok(());
        };
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
        self.active_runs
            .lock()
            .map_err(lock_error)?
            .entry(run_id.to_string())
            .or_default()
            .entry(lease_id.to_string())
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        Ok(())
    }

    pub(super) fn mark_run_unmounted(
        &self,
        run_id: &str,
        attachment: &EnvironmentAttachmentRef,
    ) -> Result<(), RpcError> {
        let Some(lease_id) = attachment.requested_attachment_lease_id() else {
            return Ok(());
        };
        if let Some(lease_counts) = self.active_runs.lock().map_err(lock_error)?.get_mut(run_id) {
            if let Some(count) = lease_counts.get_mut(lease_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    lease_counts.remove(lease_id);
                }
            }
        }
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

    fn detach_lease(&self, lease_id: &str) -> Result<bool, RpcError> {
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
        Ok(ready_readiness(source.resolved_mode()))
    }
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
        metadata: source.metadata.clone(),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;
    use starweaver_rpc_core::environment_attachment_refs;

    #[tokio::test]
    async fn attach_rejects_reserved_local_id_for_envd_source() {
        let manager = EnvironmentAttachmentManager::new();
        let error = manager
            .attach(&json!({
                "attachment": {
                    "id": "local",
                    "kind": "envd",
                    "endpointRef": "http://127.0.0.1:8766/rpc",
                    "authToken": "secret"
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("reserved"));
    }

    #[tokio::test]
    async fn lease_ref_cannot_remap_envd_source_to_reserved_local_id() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(&json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": "http://127.0.0.1:8766/rpc",
                    "authToken": "secret"
                },
                "readiness": {"policy": "skip"}
            }))
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
            .attach(&json!({
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }))
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
            )
            .await
            .unwrap();

        manager.mark_run_started("run_1", &attachments).unwrap();
        let error = manager
            .detach(&json!({"attachmentLeaseId": lease_id}))
            .unwrap_err();
        assert_eq!(error.code, RUN_CONFLICT);
        assert!(error.message.contains("run_1"));

        manager.mark_run_finished("run_1").unwrap();
        let detached = manager
            .detach(&json!({"attachmentLeaseId": lease_id}))
            .unwrap();
        assert_eq!(detached["detached"], true);
    }

    #[tokio::test]
    async fn active_run_lease_usage_is_reference_counted() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(&json!({
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }))
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
            )
            .await
            .unwrap();

        manager.mark_run_started("run_1", &attachments).unwrap();
        manager
            .mark_run_unmounted("run_1", &attachments[0])
            .unwrap();
        let error = manager
            .detach(&json!({"attachmentLeaseId": lease_id}))
            .unwrap_err();
        assert_eq!(error.code, RUN_CONFLICT);

        manager
            .mark_run_unmounted("run_1", &attachments[1])
            .unwrap();
        let detached = manager
            .detach(&json!({"attachmentLeaseId": lease_id}))
            .unwrap();
        assert_eq!(detached["detached"], true);
    }

    #[tokio::test]
    async fn mark_run_started_rejects_lease_detached_after_materialization() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(&json!({
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }))
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
            )
            .await
            .unwrap();
        manager
            .detach(&json!({"attachmentLeaseId": lease_id}))
            .unwrap();

        let error = manager.mark_run_started("run_1", &attachments).unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("detached"));
    }

    #[tokio::test]
    async fn lease_refs_can_only_downgrade_run_local_mode() {
        let manager = EnvironmentAttachmentManager::new();
        let read_write = manager
            .attach(&json!({
                "attachment": {"id": "workspace", "kind": "local", "mode": "read_write"},
                "readiness": {"policy": "required"}
            }))
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
            )
            .await
            .unwrap();
        assert_eq!(
            downgraded[0].resolved_mode(),
            EnvironmentAttachmentAccessMode::ReadOnly
        );

        let read_only = manager
            .attach(&json!({
                "attachment": {"id": "data", "kind": "local", "mode": "read_only"},
                "readiness": {"policy": "required"}
            }))
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
            .attach(&json!({
                "scope": {"kind": "session", "sessionId": "session_a"},
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }))
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
            .materialize_run_attachments(refs(), None)
            .await
            .unwrap_err();
        assert_eq!(missing_session.code, INVALID_PARAMS);
        assert!(missing_session.message.contains("requires run sessionId"));

        let wrong_session = manager
            .materialize_run_attachments(refs(), Some("session_b"))
            .await
            .unwrap_err();
        assert_eq!(wrong_session.code, INVALID_PARAMS);
        assert!(wrong_session.message.contains("session_a"));
        assert!(wrong_session.message.contains("session_b"));

        let materialized = manager
            .materialize_run_attachments(refs(), Some("session_a"))
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
            .attach(&json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": "http://127.0.0.1:8766/rpc",
                    "authToken": " "
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap_err();
        assert_eq!(empty.code, INVALID_PARAMS);
        assert!(empty.message.contains("cannot be empty"));

        let newline = manager
            .attach(&json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": "http://127.0.0.1:8766/rpc",
                    "authToken": "line\nbreak"
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap_err();
        assert_eq!(newline.code, INVALID_PARAMS);
        assert!(newline.message.contains("cannot contain newlines"));
    }

    #[tokio::test]
    async fn envd_attach_rejects_unsafe_http_endpoint_refs() {
        let manager = EnvironmentAttachmentManager::new();

        let non_loopback = manager
            .attach(&json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": "http://example.com:8766/rpc",
                    "authToken": "secret"
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap_err();
        assert_eq!(non_loopback.code, INVALID_PARAMS);
        assert!(non_loopback.message.contains("must be loopback"));

        let query = manager
            .attach(&json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": "http://127.0.0.1:8766/rpc?token=secret",
                    "authToken": "secret"
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap_err();
        assert_eq!(query.code, INVALID_PARAMS);
        assert!(query.message.contains("query strings"));

        let userinfo = manager
            .attach(&json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": "http://user:pass@127.0.0.1:8766/rpc",
                    "authToken": "secret"
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap_err();
        assert_eq!(userinfo.code, INVALID_PARAMS);
        assert!(userinfo.message.contains("userinfo"));
    }

    #[tokio::test]
    async fn envd_attach_redacts_stdio_endpoint_ref_in_lease_result() {
        let manager = EnvironmentAttachmentManager::new();
        let attached = manager
            .attach(&json!({
                "attachment": {
                    "id": "stdio_data",
                    "kind": "envd",
                    "endpointRef": "stdio:///tmp/envd-fixture?arg=--token&arg=secret",
                    "environmentId": "default"
                },
                "readiness": {"policy": "skip"}
            }))
            .await
            .unwrap();

        assert_eq!(attached["attachment"]["endpointRef"], "stdio://<redacted>");
        assert!(!attached["attachment"].to_string().contains("envd-fixture"));
        assert!(!attached["attachment"].to_string().contains("secret"));
    }
}
