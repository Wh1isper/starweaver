//! Standalone RPC method dispatch.

use std::{
    cell::Cell,
    collections::{BTreeSet, HashMap},
    future::Future,
    ops::Deref,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use starweaver_agent::{ContinuationMaterializationMode, Usage};
use starweaver_context::MessageBus;
use starweaver_core::{RunId, SessionId, TraceContext};
use starweaver_runtime::AgentInput;
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, AttachEnvironment,
    ClarificationAnswer as DomainClarificationAnswer, DecideApproval, DeferredMutationOutcome,
    DeferredToolRecord, DetachEnvironment, DurableEnvironmentAttachment,
    DurableEnvironmentMountStatus, DurableEnvironmentScope, DurableEnvironmentStatus,
    DurableHostEventClass, DurableHostEventQuery, DurableHostEventRecord, DurableHostEventScope,
    ENVIRONMENT_ATTACH_OPERATION, EnvironmentAttachmentPageKey, EnvironmentAttachmentQuery,
    EnvironmentHostEventContext, EnvironmentMountQuery, EnvironmentMutationContext,
    ExecutionStatus, InputPart, InteractionMutationContext, InteractionPageKey,
    InteractionPageQuery, MountEnvironmentResource, PendingHostEventPublication,
    ResolveClarification, ResolveDeferredTool, RunRecord, SessionDeletionFence, SessionPageKey,
    SessionPageQuery, SessionRecord, SessionSearchError, SessionSearchFilter,
    SessionSearchGranularity, SessionSearchProvider, SessionSearchQuery, SessionSearchQueryMode,
    SessionSearchScope, SessionSearchSort, SessionStatus, SessionStore, SessionStoreResult,
    UnmountEnvironmentResource,
};
use starweaver_storage::{LocalSessionSearchLimits, LocalSessionSearchProvider, SqliteStorage};
use tokio::{
    runtime::{Builder as RuntimeBuilder, Handle, Runtime},
    sync::{mpsc, oneshot, watch},
};
use uuid::Uuid;

use starweaver_rpc_core::generated as host;

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHitlResumeRequest, RpcHostError, RpcHostResult, RpcRunRequest,
    RpcRuntimeCoordinator,
    environment_contract::{EnvironmentAttachmentAccessMode, EnvironmentAttachmentRef},
    environment_manager::EnvironmentAttachmentManager,
    error::{
        INVALID_PARAMS, RpcError, SERVER_ERROR, SESSION_SEARCH_UNAVAILABLE, UNSUPPORTED_FEATURE,
    },
    host_cursor::{CursorAdmissionError, HostCursorCodec},
    session_tools::{DeferredToolDefinition as LegacyDeferredToolDefinition, bind_deferred_tools},
};

const MAX_CONNECTION_SUBSCRIPTIONS: usize = 32;
const SUBSCRIPTION_REPLAY_PAGE: usize = 256;
const RPC_RUNTIME_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

std::thread_local! {
    static RPC_RUNTIME_WORKER: Cell<bool> = const { Cell::new(false) };
}

struct RpcExecutionRuntime {
    runtime: Option<Runtime>,
}

impl RpcExecutionRuntime {
    const fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Some(runtime),
        }
    }
}

impl Deref for RpcExecutionRuntime {
    type Target = Runtime;

    fn deref(&self) -> &Self::Target {
        let Some(runtime) = self.runtime.as_ref() else {
            unreachable!("RPC execution runtime is unavailable during shutdown");
        };
        runtime
    }
}

impl Drop for RpcExecutionRuntime {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        if Handle::try_current().is_ok() {
            runtime.shutdown_background();
        } else {
            drop(runtime);
        }
    }
}

async fn run_storage<T, F>(storage: SqliteStorage, operation: F) -> Result<T, RpcError>
where
    T: Send + 'static,
    F: FnOnce(SqliteStorage) -> SessionStoreResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || operation(storage))
        .await
        .map_err(|error| {
            rpc_error(RpcHostError::Runtime(format!(
                "storage task failed: {error}"
            )))
        })?
        .map_err(rpc_error)
}

/// Notification capability of the selected transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RpcNotificationMode {
    /// Transport can emit out-of-band notifications.
    Live,
    /// Transport supports request/response replay only.
    ReplayOnly,
}

/// RPC-owned application service.
#[derive(Clone)]
pub struct RpcService {
    config: Arc<RpcConfig>,
    catalog: Arc<RpcAgentCatalog>,
    storage: SqliteStorage,
    coordinator: Arc<RpcRuntimeCoordinator>,
    environment_manager: EnvironmentAttachmentManager,
    session_search: Option<Arc<dyn SessionSearchProvider>>,
    session_search_scope: SessionSearchScope,
    notifications: RpcNotificationMode,
    cursor_codec: HostCursorCodec,
    runtime: Arc<RpcExecutionRuntime>,
    startup_repaired_runs: u64,
}

/// Result of processing one strict generated JSON-RPC request frame.
#[derive(Debug)]
pub struct RpcFrameOutcome {
    /// Encoded JSON-RPC response object.
    pub response: Option<Value>,
    /// Whether a successful shutdown request was handled.
    pub shutdown: bool,
}

/// One generated notification plus a transport flush acknowledgement.
pub(crate) struct RpcNotificationOutput {
    pub(crate) value: Value,
    pub(crate) flushed: oneshot::Sender<()>,
}

/// Per-connection host protocol negotiation state.
#[derive(Clone)]
pub struct RpcConnection {
    service: RpcService,
    state: Arc<RpcConnectionState>,
}

struct RpcConnectionState {
    initialized: AtomicBool,
    closed: AtomicBool,
    cleanup_completed: AtomicBool,
    connection_id: String,
    authority_identity: String,
    transport: host::Transport,
    scopes: BTreeSet<String>,
    output: Option<mpsc::Sender<RpcNotificationOutput>>,
    subscriptions: Arc<Mutex<HashMap<String, ConnectionSubscription>>>,
    pending_activations: Mutex<Vec<oneshot::Sender<()>>>,
    negotiated_features: Mutex<BTreeSet<String>>,
    environment_manager: EnvironmentAttachmentManager,
    storage: SqliteStorage,
}

struct ConnectionSubscription {
    cancel: watch::Sender<bool>,
    ready: watch::Sender<bool>,
    progress: Arc<Mutex<SubscriptionProgress>>,
    tail_stopped: oneshot::Receiver<()>,
}

#[derive(Default)]
struct SubscriptionProgress {
    last_flushed_cursor: Option<host::HostEventCursor>,
    last_flushed_sequence: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionCursorPosition {
    updated_at: String,
    session_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InteractionCursorPosition {
    updated_at: String,
    interaction_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvironmentCursorPosition {
    updated_at: String,
    attachment_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InteractionCursorView<'a> {
    session_id: Option<&'a str>,
    run_id: Option<&'a str>,
}

struct HostEventTail {
    subscription_id: host::SubscriptionId,
    stopped: oneshot::Sender<()>,
    view: host::EventViewRequest,
    scope: DurableHostEventScope,
    event_classes: Vec<DurableHostEventClass>,
    after_position: u64,
    fence_position: u64,
    output: mpsc::Sender<RpcNotificationOutput>,
    cancel: watch::Receiver<bool>,
    ready: watch::Receiver<bool>,
    progress: Arc<Mutex<SubscriptionProgress>>,
}

impl RpcConnectionState {
    fn close(&self) -> RpcHostResult<()> {
        self.closed.store(true, Ordering::Release);
        let mut subscriptions = self
            .subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for subscription in subscriptions.values() {
            let _ = subscription.cancel.send(true);
        }
        subscriptions.clear();
        drop(subscriptions);
        if self.cleanup_completed.load(Ordering::Acquire) {
            return Ok(());
        }
        let attachments = self.storage.list_connection_environment_attachments(
            &self.authority_identity,
            &self.connection_id,
        )?;
        let occurred_at = chrono::Utc::now();
        let mut commands = Vec::new();
        for attachment in attachments {
            if attachment.status == DurableEnvironmentStatus::Detached {
                continue;
            }
            let idempotency_key = format!(
                "connection-revoke:{}:{}",
                self.connection_id, attachment.attachment_id
            );
            let transition =
                environment_transition_identity(&self.authority_identity, &idempotency_key);
            commands.push(DetachEnvironment {
                context: EnvironmentMutationContext {
                    authority_binding: self.authority_identity.clone(),
                    idempotency_key,
                    command_fingerprint: format!(
                        "connection-revoke:{}:{}",
                        attachment.attachment_id, attachment.revision
                    ),
                    occurred_at,
                    host_event: Some(EnvironmentHostEventContext {
                        transition_identity: transition,
                        scope: DurableHostEventScope::Global,
                    }),
                },
                attachment_id: attachment.attachment_id,
            });
        }
        self.storage.detach_connection_environments(
            &self.authority_identity,
            &self.connection_id,
            commands,
        )?;
        self.cleanup_completed.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for RpcConnectionState {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl RpcConnection {
    /// Explicitly close this transport-owned connection and revoke its ephemeral authority.
    pub(crate) fn close(&self) -> RpcHostResult<()> {
        self.state.close()
    }

    /// Handle one frame using this connection's initialization state.
    #[must_use]
    pub fn handle_text(&self, text: &str) -> RpcFrameOutcome {
        let connection = self.clone();
        let task_text = text.to_string();
        match execute_on_runtime(&self.service.runtime, async move {
            connection.handle_text_async(&task_text).await
        }) {
            Ok(outcome) => outcome,
            Err(error) => runtime_failure_outcome(text, &error),
        }
    }

    async fn handle_text_async(&self, text: &str) -> RpcFrameOutcome {
        if self.state.closed.load(Ordering::Acquire) {
            return runtime_failure_outcome(
                text,
                &RpcHostError::Runtime("RPC connection is closed".to_string()),
            );
        }
        let request = match host::decode_request_frame(text.as_bytes()) {
            Ok(request) => request,
            Err(error) => {
                return RpcFrameOutcome {
                    response: Some(encoded_error_response_value(error.into_response())),
                    shutdown: false,
                };
            }
        };
        let method = request.call.method();
        let initializing = method == host::Method::Initialize;
        let requested_shutdown = method == host::Method::Shutdown;
        if initializing
            && self.state.initialized.load(Ordering::Acquire)
            && self.state.transport == host::Transport::Stdio
        {
            return RpcFrameOutcome {
                response: Some(encoded_response_value(host::HostResponse {
                    id: request.id,
                    result: Err(invalid_params_error(
                        "initialize may be called only once per stdio connection",
                    )),
                })),
                shutdown: false,
            };
        }
        if !initializing && !self.state.initialized.load(Ordering::Acquire) {
            return RpcFrameOutcome {
                response: Some(encoded_response_value(host::HostResponse {
                    id: request.id,
                    result: Err(not_initialized_error()),
                })),
                shutdown: false,
            };
        }
        if let Err(error) = self.admit_method(method) {
            return RpcFrameOutcome {
                response: Some(encoded_response_value(host::HostResponse {
                    id: request.id,
                    result: Err(error),
                })),
                shutdown: false,
            };
        }
        let response = host::dispatch(self, &(), request).await;
        let succeeded = response.result.is_ok();
        if initializing && succeeded {
            self.state.initialized.store(true, Ordering::Release);
        }
        RpcFrameOutcome {
            response: Some(encoded_response_value(response)),
            shutdown: requested_shutdown && succeeded,
        }
    }

    fn admit_method(&self, method: host::Method) -> Result<(), host::HostError> {
        let metadata = method.metadata();
        if !metadata.transports.contains(&self.state.transport) {
            return Err(unsupported_feature_error(
                "method is unavailable on this transport",
            ));
        }
        if metadata
            .scopes
            .iter()
            .any(|scope| *scope != "public" && !self.state.scopes.contains(*scope))
        {
            return Err(authorization_denied_error(
                "connection authority does not admit this method",
            ));
        }
        let negotiated = self
            .state
            .negotiated_features
            .lock()
            .map_err(|_| internal_error("feature negotiation state unavailable", true))?
            .clone();
        if method != host::Method::Initialize
            && metadata
                .features
                .iter()
                .any(|feature| !negotiated.contains(*feature))
        {
            return Err(unsupported_feature_error(
                "method requires a feature that was not negotiated",
            ));
        }
        let feature_refs = negotiated.iter().map(String::as_str).collect::<Vec<_>>();
        let scope_refs = self
            .state
            .scopes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !method.is_admitted(&feature_refs, &scope_refs, self.state.transport) {
            return Err(internal_error(
                "generated method admission invariant was not satisfied",
                true,
            ));
        }
        Ok(())
    }

    fn admitted_event_classes(
        &self,
        view: &host::EventViewRequest,
    ) -> Result<Vec<DurableHostEventClass>, host::HostError> {
        let negotiated = self
            .state
            .negotiated_features
            .lock()
            .map_err(|_| internal_error("feature negotiation state unavailable", true))?
            .clone();
        if view
            .optional_features
            .iter()
            .any(|feature| !negotiated.contains(feature))
        {
            return Err(unsupported_feature_error(
                "event view requests an optional feature that was not negotiated",
            ));
        }
        for event_class in view.profile.metadata().event_classes {
            let metadata = event_class.metadata();
            if metadata
                .scopes
                .iter()
                .any(|scope| !self.state.scopes.contains(*scope))
            {
                return Err(authorization_denied_error(
                    "connection authority does not admit the requested event profile",
                ));
            }
            if metadata
                .feature
                .is_some_and(|feature| !negotiated.contains(feature))
            {
                return Err(unsupported_feature_error(
                    "requested event profile requires a feature that was not negotiated",
                ));
            }
        }
        let feature_refs = negotiated.iter().map(String::as_str).collect::<Vec<_>>();
        let scope_refs = self
            .state
            .scopes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !view.profile.is_admitted(&feature_refs, &scope_refs) {
            return Err(internal_error(
                "generated event-profile admission invariant was not satisfied",
                true,
            ));
        }
        Ok(durable_event_classes(view.profile))
    }

    async fn catalog_list_generated(
        &self,
        _params: host::CatalogListParams,
    ) -> Result<host::CatalogListResult, host::HostError> {
        let selection = self.current_model_selection().await?;
        let profiles = self
            .service
            .catalog
            .profiles()
            .into_iter()
            .map(|profile| host::ProfileSummary {
                label: profile.label,
                model_id: profile.model_id,
                name: profile.name,
                source: profile.source.to_string(),
            })
            .collect();
        Ok(host::CatalogListResult {
            profiles,
            selection,
        })
    }

    async fn model_selection_get_generated(
        &self,
        _params: host::ModelSelectionGetParams,
    ) -> Result<host::ModelSelectionGetResult, host::HostError> {
        Ok(host::ModelSelectionGetResult {
            selection: self.current_model_selection().await?,
        })
    }

    async fn current_model_selection(&self) -> Result<host::ModelSelection, host::HostError> {
        let default_profile = self.service.catalog.default_profile().to_string();
        let default_model = self
            .service
            .catalog
            .profile(&default_profile)
            .map_err(rpc_host_to_generated_error)?
            .model_id
            .clone();
        let authority_binding = self.state.authority_identity.clone();
        let selection = run_storage(self.service.storage.clone(), move |storage| {
            storage.load_or_initialize_model_selection(
                starweaver_session::InitializeModelSelection {
                    authority_binding,
                    selected_profile: default_profile,
                    model_id: default_model,
                    initialized_at: chrono::Utc::now(),
                },
            )
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        generated_model_selection(&selection)
    }

    async fn model_select_generated(
        &self,
        params: host::ModelSelectParams,
    ) -> Result<host::ModelSelectResult, host::HostError> {
        let profile = self
            .service
            .catalog
            .profile(&params.profile)
            .map_err(rpc_host_to_generated_error)?;
        let fingerprint = mutation_fingerprint("model.select", &params)?;
        let command = starweaver_session::SelectModel {
            authority_binding: self.state.authority_identity.clone(),
            selected_profile: params.profile,
            model_id: profile.model_id.clone(),
            idempotency_key: params.idempotency_key.as_str().to_string(),
            command_fingerprint: fingerprint,
            occurred_at: chrono::Utc::now(),
            host_event_publication: None,
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.select_model(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::ModelSelectResult {
            receipt: generated_mutation_receipt(&result.receipt)?,
            selection: generated_model_selection(&result.selection)?,
        })
    }

    async fn profile_get_generated(
        &self,
        params: host::ProfileGetParams,
    ) -> Result<host::ProfileGetResult, host::HostError> {
        let profile = self
            .service
            .catalog
            .profile(&params.name)
            .map_err(rpc_host_to_generated_error)?;
        Ok(host::ProfileGetResult {
            profile: host::ProfileDetail {
                instructions: profile.instructions.clone(),
                label: profile.label.clone(),
                mcp_servers: self.service.catalog.effective_mcp_server_names(profile),
                model_id: profile.model_id.clone(),
                name: params.name,
                subagents: profile.subagents.clone(),
                toolsets: profile.toolsets.clone(),
            },
        })
    }

    async fn session_create_generated(
        &self,
        params: host::SessionCreateParams,
    ) -> Result<host::SessionCreateResult, host::HostError> {
        let fingerprint = mutation_fingerprint("session.create", &params)?;
        let idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let store = self.service.storage.session_store();
        if let Some(session) = store
            .load_session_mutation_receipt(
                starweaver_session::LOCAL_SESSION_NAMESPACE,
                &idempotency_key,
                &fingerprint,
            )
            .await
            .map_err(session_store_to_generated_error)?
        {
            return Ok(host::SessionCreateResult {
                receipt: mutation_receipt(
                    "session.create",
                    &params.idempotency_key,
                    &fingerprint,
                    session.session_id.as_str(),
                    "committed",
                    true,
                    false,
                )?,
                session: session_summary(&session)?,
            });
        }
        let profile = params
            .profile
            .clone()
            .unwrap_or_else(|| self.service.config.default_profile.clone());
        self.service
            .catalog
            .profile(&profile)
            .map_err(rpc_host_to_generated_error)?;
        let deferred_tools = params
            .deferred_tools
            .iter()
            .map(legacy_deferred_tool)
            .collect::<Result<Vec<_>, _>>()?;
        let workspace = std::fs::canonicalize(&self.service.config.workspace_root)
            .unwrap_or_else(|_| self.service.config.workspace_root.clone())
            .to_string_lossy()
            .into_owned();
        let candidate_id = deterministic_session_id(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let mut session = SessionRecord::new(candidate_id);
        let candidate_created_at = session.created_at;
        session.profile = Some(profile);
        session.title = params.title;
        session.workspace = Some(workspace);
        session.metadata.insert(
            starweaver_storage::SESSION_SOURCE_PRODUCT_METADATA_KEY.to_string(),
            json!("rpc"),
        );
        bind_deferred_tools(&mut session, deferred_tools).map_err(rpc_host_to_generated_error)?;
        let publication = session_changed_publication(
            &mutation_transition_identity(
                "session.create",
                params.idempotency_key.as_str(),
                &fingerprint,
            ),
            &session,
        )?;
        let session = store
            .create_session_idempotent_with_host_events(
                session,
                &idempotency_key,
                &fingerprint,
                vec![publication],
            )
            .await
            .map_err(session_store_to_generated_error)?;
        let replayed = session.created_at != candidate_created_at;
        Ok(host::SessionCreateResult {
            receipt: mutation_receipt(
                "session.create",
                &params.idempotency_key,
                &fingerprint,
                session.session_id.as_str(),
                "committed",
                replayed,
                false,
            )?,
            session: session_summary(&session)?,
        })
    }

    async fn session_delete_generated(
        &self,
        params: host::SessionDeleteParams,
    ) -> Result<host::SessionDeleteResult, host::HostError> {
        let session_id = SessionId::from_string(params.session_id.as_str());
        let fingerprint = mutation_fingerprint("session.delete", &params)?;
        let idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let fence_id = deterministic_deletion_fence_id(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
            &fingerprint,
        );
        let store = self.service.storage.session_store();

        let fenced = if let Some(receipt) = store
            .load_session_mutation_receipt(
                starweaver_session::LOCAL_SESSION_NAMESPACE,
                &idempotency_key,
                &fingerprint,
            )
            .await
            .map_err(session_store_to_generated_error)?
        {
            if receipt.status == SessionStatus::Deleted {
                return Ok(host::SessionDeleteResult {
                    receipt: mutation_receipt(
                        "session.delete",
                        &params.idempotency_key,
                        &fingerprint,
                        receipt.session_id.as_str(),
                        "deleted",
                        true,
                        false,
                    )?,
                    session: session_summary(&receipt)?,
                });
            }
            match &receipt.deletion_fence {
                SessionDeletionFence::Deleting {
                    fence_id: current, ..
                } if current == &fence_id => receipt,
                _ => {
                    return Err(internal_error(
                        "session deletion receipt is not resumable",
                        true,
                    ));
                }
            }
        } else {
            store
                .acquire_session_deletion_fence(
                    &session_id,
                    params.expected_revision.get(),
                    &fence_id,
                    &self.state.authority_identity,
                    &idempotency_key,
                    &fingerprint,
                )
                .await
                .map_err(session_store_to_generated_error)?
        };

        let requested_at = match &fenced.deletion_fence {
            SessionDeletionFence::Deleting {
                fence_id: current,
                started_at,
                ..
            } if current == &fence_id => *started_at,
            _ => {
                return Err(internal_error(
                    "session deletion fence was not durably acquired",
                    true,
                ));
            }
        };
        self.service
            .coordinator
            .quiesce_session_for_deletion(&session_id, Duration::from_secs(10))
            .await
            .map_err(rpc_host_to_generated_error)?;

        let mut deleted_projection = fenced;
        deleted_projection.status = SessionStatus::Deleted;
        deleted_projection.active_run_id = None;
        deleted_projection.deletion_fence = SessionDeletionFence::Deleted {
            fence_id: fence_id.clone(),
            deleted_at: requested_at,
        };
        deleted_projection.revision = deleted_projection.revision.saturating_add(1);
        deleted_projection.updated_at = requested_at;
        let publication = session_changed_publication(
            &mutation_transition_identity(
                "session.delete",
                params.idempotency_key.as_str(),
                &fingerprint,
            ),
            &deleted_projection,
        )?;
        let session = store
            .tombstone_session_idempotent_with_host_events(
                &session_id,
                &fence_id,
                &idempotency_key,
                &fingerprint,
                vec![publication],
            )
            .await
            .map_err(session_store_to_generated_error)?;
        self.service
            .coordinator
            .forget_deleted_session(&session_id)
            .map_err(rpc_host_to_generated_error)?;
        Ok(host::SessionDeleteResult {
            receipt: mutation_receipt(
                "session.delete",
                &params.idempotency_key,
                &fingerprint,
                session.session_id.as_str(),
                "deleted",
                false,
                false,
            )?,
            session: session_summary(&session)?,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn session_fork_generated(
        &self,
        params: host::SessionForkParams,
    ) -> Result<host::SessionForkResult, host::HostError> {
        let fingerprint = mutation_fingerprint("session.fork", &params)?;
        let idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let store = self.service.storage.session_store();
        if let Some(session) = store
            .load_session_mutation_receipt(
                starweaver_session::LOCAL_SESSION_NAMESPACE,
                &idempotency_key,
                &fingerprint,
            )
            .await
            .map_err(session_store_to_generated_error)?
        {
            let (source_session_id, source_run_id) = fork_lineage(&session)?;
            return Ok(host::SessionForkResult {
                receipt: mutation_receipt(
                    "session.fork",
                    &params.idempotency_key,
                    &fingerprint,
                    session.session_id.as_str(),
                    "committed",
                    true,
                    false,
                )?,
                session: session_summary(&session)?,
                source_run_id,
                source_session_id,
            });
        }

        let source_id = SessionId::from_string(params.session_id.as_str());
        let source = store
            .load_session(&source_id)
            .await
            .map_err(session_store_to_generated_error)?;
        if source.status == SessionStatus::Deleted {
            return Err(invalid_params_error("deleted sessions cannot be forked"));
        }
        let source_run_id = source.head_success_run_id.clone();
        let mut state = match source_run_id.as_ref() {
            Some(run_id) => {
                let storage = self.service.storage.clone();
                let session_id = source.session_id.clone();
                let run_id = run_id.clone();
                run_storage(storage, move |storage| {
                    storage.load_run_context(&session_id, &run_id)
                })
                .await
                .map_err(rpc_error_to_host_error)?
                .ok_or_else(|| {
                    rpc_host_to_generated_error(RpcHostError::NotFound(
                        "source session head context is unavailable".to_string(),
                    ))
                })?
            }
            None if source.head_run_id.is_none() => source.state.clone(),
            None => {
                return Err(invalid_params_error(
                    "sessions with runs but no successful run cannot be forked",
                ));
            }
        };

        let target_id = deterministic_fork_session_id(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let mut target = SessionRecord::new(target_id);
        let candidate_created_at = target.created_at;
        target.namespace_id.clone_from(&source.namespace_id);
        target.owner_id.clone_from(&source.owner_id);
        target.profile.clone_from(&source.profile);
        target.workspace.clone_from(&source.workspace);
        target.parent_session_id = Some(source.session_id.clone());
        target.title = params.title.or_else(|| {
            source
                .title
                .as_deref()
                .map(|title| format!("Fork of {title}"))
        });

        state.run_id = None;
        state.session_id = Some(target.session_id.clone());
        state.parent_run_id = None;
        state.parent_task_id = None;
        state.pending_tool_returns.clear();
        state.user_prompts = None;
        state.steering_messages.clear();
        state.deferred_tool_metadata.clear();
        state.usage = Usage::default();
        state.usage_snapshot_entries.clear();
        state.message_bus = MessageBus::default();
        state.trace_snapshot = TraceContext::default();
        state.started_at = target.created_at;
        state.metadata.remove("starweaver.durable_run_id");
        state.metadata.insert(
            "starweaver.durable_session_id".to_string(),
            json!(target.session_id.as_str()),
        );
        target.state = state;
        target.metadata.insert(
            starweaver_storage::SESSION_SOURCE_PRODUCT_METADATA_KEY.to_string(),
            json!("rpc"),
        );
        if let Some(binding) = source
            .metadata
            .get(crate::session_tools::RPC_DEFERRED_TOOLSET_METADATA_KEY)
        {
            target.metadata.insert(
                crate::session_tools::RPC_DEFERRED_TOOLSET_METADATA_KEY.to_string(),
                binding.clone(),
            );
        }
        target.metadata.insert(
            "rpc.fork".to_string(),
            json!({
                "kind": "session_forked",
                "source_session_id": source.session_id,
                "source_run_id": source_run_id,
                "source_revision": source.revision,
            }),
        );
        let publication = session_changed_publication(
            &mutation_transition_identity(
                "session.fork",
                params.idempotency_key.as_str(),
                &fingerprint,
            ),
            &target,
        )?;
        let session = store
            .create_session_idempotent_with_host_events(
                target,
                &idempotency_key,
                &fingerprint,
                vec![publication],
            )
            .await
            .map_err(session_store_to_generated_error)?;
        let replayed = session.created_at != candidate_created_at;
        let (source_session_id, source_run_id) = fork_lineage(&session)?;
        Ok(host::SessionForkResult {
            receipt: mutation_receipt(
                "session.fork",
                &params.idempotency_key,
                &fingerprint,
                session.session_id.as_str(),
                "committed",
                replayed,
                false,
            )?,
            session: session_summary(&session)?,
            source_run_id,
            source_session_id,
        })
    }

    async fn session_list_generated(
        &self,
        params: host::SessionListParams,
    ) -> Result<host::SessionListResult, host::HostError> {
        let after = params
            .cursor
            .as_deref()
            .map(|cursor| {
                self.service
                    .cursor_codec
                    .decode_page::<SessionCursorPosition, _>(
                        "session.list",
                        cursor,
                        &self.state.authority_identity,
                        &"all-sessions",
                    )
            })
            .transpose()
            .map_err(cursor_invalid_error)?
            .map(session_page_key)
            .transpose()?;
        let query = SessionPageQuery::new(after, params.limit as usize)
            .map_err(|_| invalid_params_error("invalid session page limit"))?;
        let page = self
            .service
            .storage
            .session_store()
            .list_session_page(query)
            .await
            .map_err(|_| storage_unavailable_error())?;
        let sessions = page
            .sessions
            .iter()
            .map(session_summary)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if page.has_more {
            page.next_key
                .as_ref()
                .map(|key| {
                    self.service.cursor_codec.encode_page(
                        "session.list",
                        &session_cursor_position(key),
                        &self.state.authority_identity,
                        &"all-sessions",
                    )
                })
                .transpose()
                .map_err(|_| internal_error("failed to encode session page cursor", true))?
        } else {
            None
        };
        Ok(host::SessionListResult {
            page: host::PageInfo {
                has_more: page.has_more,
                next_cursor,
            },
            sessions,
        })
    }

    async fn session_get_generated(
        &self,
        params: host::SessionGetParams,
    ) -> Result<host::SessionGetResult, host::HostError> {
        let session_id = SessionId::from_string(params.session_id.as_str());
        let store = self.service.storage.session_store();
        let session = store
            .load_session(&session_id)
            .await
            .map_err(session_store_to_generated_error)?;
        let mut runs = store
            .list_runs(&session_id)
            .await
            .map_err(session_store_to_generated_error)?;
        let limit = params.run_limit as usize;
        if runs.len() > limit {
            runs = runs.split_off(runs.len() - limit);
        }
        Ok(host::SessionGetResult {
            runs: runs
                .iter()
                .map(run_summary)
                .collect::<Result<Vec<_>, _>>()?,
            session: session_summary(&session)?,
        })
    }

    async fn session_search_generated(
        &self,
        params: host::SessionSearchParams,
    ) -> Result<host::SessionSearchResult, host::HostError> {
        let provider = self
            .service
            .session_search
            .as_ref()
            .ok_or_else(|| unsupported_feature_error("session search is not installed"))?;
        let session_statuses = params
            .status
            .map(|status| {
                vec![match status {
                    host::SessionStatus::Active => SessionStatus::Active,
                    host::SessionStatus::Archived => SessionStatus::Archived,
                    host::SessionStatus::Failed => SessionStatus::Failed,
                    host::SessionStatus::Deleted => SessionStatus::Deleted,
                }]
            })
            .unwrap_or_default();
        let filter = SessionSearchFilter {
            profile: params.profile.clone(),
            session_statuses,
            ..SessionSearchFilter::default()
        };
        let query = SessionSearchQuery {
            text: params.query,
            mode: match params.mode {
                host::SessionSearchMode::Literal => SessionSearchQueryMode::Literal,
                host::SessionSearchMode::Hybrid => SessionSearchQueryMode::Phrase,
            },
            filter,
            sources: Default::default(),
            granularity: SessionSearchGranularity::Session,
            sort: SessionSearchSort::Auto,
            limit: params.limit,
            cursor: params.cursor,
        };
        let page = provider
            .search(&self.service.session_search_scope, query)
            .await
            .map_err(session_search_error)
            .map_err(rpc_error_to_host_error)?;
        let store = self.service.storage.session_store();
        let mut hits = Vec::with_capacity(page.hits.len());
        for hit in page.hits {
            let session = store
                .load_session(&hit.session.session_id)
                .await
                .map_err(session_store_to_generated_error)?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let score_basis_points = hit
                .score
                .map_or(0, |score| (score.clamp(0.0, 1.0) * 10_000.0).round() as u32);
            let highlights = hit
                .snippet
                .map(|snippet| vec![snippet.text])
                .unwrap_or_default();
            hits.push(host::SessionSearchHit {
                highlights,
                score_basis_points,
                session: session_summary(&session)?,
            });
        }
        Ok(host::SessionSearchResult {
            hits,
            page: host::PageInfo {
                has_more: page.next_cursor.is_some(),
                next_cursor: page.next_cursor,
            },
        })
    }

    fn admit_environment_attachment(
        &self,
        attachment: &DurableEnvironmentAttachment,
    ) -> Result<(), host::HostError> {
        if let DurableEnvironmentScope::Connection { connection_id } = &attachment.scope
            && connection_id != &self.state.connection_id
        {
            return Err(not_found_error("environment attachment was not found"));
        }
        Ok(())
    }

    fn durable_environment_scope(
        &self,
        scope: &host::AttachmentScope,
    ) -> Result<DurableEnvironmentScope, host::HostError> {
        match scope {
            host::AttachmentScope::ConnectionAttachmentScope(_) => {
                if self.state.transport == host::Transport::Http {
                    return Err(unsupported_feature_error(
                        "connection-scoped environment attachments require stdio",
                    ));
                }
                Ok(DurableEnvironmentScope::Connection {
                    connection_id: self.state.connection_id.clone(),
                })
            }
            host::AttachmentScope::SessionAttachmentScope(scope) => {
                Ok(DurableEnvironmentScope::Session {
                    session_id: scope.session_id.as_str().to_string(),
                })
            }
            host::AttachmentScope::RunAttachmentScope(scope) => Ok(DurableEnvironmentScope::Run {
                session_id: scope.session_id.as_str().to_string(),
                run_id: scope.run_id.as_str().to_string(),
            }),
        }
    }

    async fn resolve_run_environment_attachments(
        &self,
        attachment_ids: &[host::AttachmentId],
        session_id: Option<&SessionId>,
        permitted_run_id: Option<&RunId>,
    ) -> Result<Vec<EnvironmentAttachmentRef>, host::HostError> {
        if attachment_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut seen = BTreeSet::new();
        let mut resolved = Vec::with_capacity(attachment_ids.len());
        for (index, attachment_id) in attachment_ids.iter().enumerate() {
            if !seen.insert(attachment_id.as_str()) {
                return Err(invalid_params_error(
                    "run environment attachment IDs must be unique",
                ));
            }
            let authority_binding = self.state.authority_identity.clone();
            let durable_attachment_id = attachment_id.as_str().to_string();
            let attachment = run_storage(self.service.storage.clone(), move |storage| {
                storage.get_environment_attachment(&authority_binding, &durable_attachment_id)
            })
            .await
            .map_err(rpc_error_to_host_error)?
            .ok_or_else(|| not_found_error("environment attachment was not found"))?;
            self.admit_environment_attachment(&attachment)?;
            if matches!(
                attachment.status,
                DurableEnvironmentStatus::Attaching | DurableEnvironmentStatus::Detached
            ) {
                return Err(rpc_error_to_host_error(RpcError::new(
                    -32_031,
                    "environment attachment is not ready",
                )));
            }
            let scope_permitted = match (&attachment.scope, session_id, permitted_run_id) {
                (DurableEnvironmentScope::Connection { connection_id }, _, _) => {
                    connection_id == &self.state.connection_id
                }
                (DurableEnvironmentScope::Session { session_id: owner }, Some(session_id), _) => {
                    owner == session_id.as_str()
                }
                (
                    DurableEnvironmentScope::Run {
                        session_id: owner_session,
                        run_id: owner_run,
                    },
                    Some(session_id),
                    Some(run_id),
                ) => owner_session == session_id.as_str() && owner_run == run_id.as_str(),
                _ => false,
            };
            if !scope_permitted {
                return Err(authorization_denied_error(
                    "environment attachment scope does not permit this run",
                ));
            }
            resolved.push(configured_environment_attachment_ref(
                &self.service.config,
                &attachment,
                index == 0,
            )?);
        }
        self.state
            .environment_manager
            .materialize_run_attachments(
                resolved,
                session_id.map(SessionId::as_str),
                Some(&self.state.connection_id),
            )
            .await
            .map_err(rpc_error_to_host_error)
    }

    async fn environment_attach_generated(
        &self,
        params: host::EnvironmentAttachParams,
    ) -> Result<host::EnvironmentAttachResult, host::HostError> {
        let fingerprint = mutation_fingerprint(ENVIRONMENT_ATTACH_OPERATION, &params)?;
        let scope = self.durable_environment_scope(&params.scope)?;
        let client_idempotency_key = params.idempotency_key.clone();
        let durable_idempotency_key =
            if matches!(&scope, DurableEnvironmentScope::Connection { .. }) {
                authority_scoped_idempotency_key(
                    &self.state.connection_id,
                    params.idempotency_key.as_str(),
                )
            } else {
                params.idempotency_key.as_str().to_string()
            };
        let replay = run_storage(self.service.storage.clone(), {
            let authority_binding = self.state.authority_identity.clone();
            let durable_idempotency_key = durable_idempotency_key.clone();
            let fingerprint = fingerprint.clone();
            move |storage| {
                storage.replay_environment_attachment_mutation(
                    &authority_binding,
                    &durable_idempotency_key,
                    &fingerprint,
                    ENVIRONMENT_ATTACH_OPERATION,
                )
            }
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        if let Some(result) = replay {
            let mut receipt = generated_mutation_receipt(&result.receipt)?;
            receipt.idempotency_key = client_idempotency_key;
            return Ok(host::EnvironmentAttachResult {
                attachment: generated_environment_attachment(&result.attachment)?,
                receipt,
            });
        }

        let environment_id = params.environment_id.as_str().to_string();
        self.service
            .config
            .resolve_environment_source(&environment_id)
            .map_err(rpc_host_to_generated_error)?;
        let display_name = self
            .service
            .config
            .environments
            .get(&environment_id)
            .and_then(|entry| entry.display_name.clone());
        let occurred_at = chrono::Utc::now();
        let attachment_id = deterministic_environment_id(
            "attachment",
            &self.state.authority_identity,
            &durable_idempotency_key,
        );
        let projected = DurableEnvironmentAttachment {
            authority_binding: self.state.authority_identity.clone(),
            attachment_id: attachment_id.clone(),
            environment_id: environment_id.clone(),
            display_name: display_name.clone(),
            scope: scope.clone(),
            status: DurableEnvironmentStatus::Ready,
            revision: 1,
            updated_at: occurred_at,
        };
        let provider_ref =
            configured_environment_attachment_ref(&self.service.config, &projected, true)?;
        self.state
            .environment_manager
            .materialize_run_attachments(vec![provider_ref], None, Some(&self.state.connection_id))
            .await
            .map_err(rpc_error_to_host_error)?;
        let transition_identity = environment_transition_identity(
            &self.state.authority_identity,
            &durable_idempotency_key,
        );
        let host_event = EnvironmentHostEventContext {
            transition_identity,
            scope: environment_event_scope(&scope),
        };
        let command = AttachEnvironment {
            context: EnvironmentMutationContext {
                authority_binding: self.state.authority_identity.clone(),
                idempotency_key: durable_idempotency_key,
                command_fingerprint: fingerprint,
                occurred_at,
                host_event: Some(host_event),
            },
            attachment_id,
            environment_id,
            display_name,
            scope,
            status: DurableEnvironmentStatus::Ready,
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.attach_environment(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let mut receipt = generated_mutation_receipt(&result.receipt)?;
        receipt.idempotency_key = client_idempotency_key;
        Ok(host::EnvironmentAttachResult {
            attachment: generated_environment_attachment(&result.attachment)?,
            receipt,
        })
    }

    async fn environment_detach_generated(
        &self,
        params: host::EnvironmentDetachParams,
    ) -> Result<host::EnvironmentDetachResult, host::HostError> {
        let fingerprint = mutation_fingerprint("environment.detach", &params)?;
        let authority_binding = self.state.authority_identity.clone();
        let attachment_id = params.attachment_id.as_str().to_string();
        let current = run_storage(self.service.storage.clone(), {
            let authority_binding = authority_binding.clone();
            let attachment_id = attachment_id.clone();
            move |storage| storage.get_environment_attachment(&authority_binding, &attachment_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?
        .ok_or_else(|| not_found_error("environment attachment was not found"))?;
        self.admit_environment_attachment(&current)?;
        let occurred_at = chrono::Utc::now();
        let transition_identity =
            environment_transition_identity(&authority_binding, params.idempotency_key.as_str());
        let event_scope = environment_event_scope(&current.scope);
        let command = DetachEnvironment {
            context: EnvironmentMutationContext {
                authority_binding,
                idempotency_key: params.idempotency_key.as_str().to_string(),
                command_fingerprint: fingerprint,
                occurred_at,
                host_event: Some(EnvironmentHostEventContext {
                    transition_identity,
                    scope: event_scope,
                }),
            },
            attachment_id,
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.detach_environment(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::EnvironmentDetachResult {
            attachment: generated_environment_attachment(&result.attachment)?,
            receipt: generated_mutation_receipt(&result.receipt)?,
        })
    }

    async fn environment_health_generated(
        &self,
        params: host::EnvironmentHealthParams,
    ) -> Result<host::EnvironmentHealthResult, host::HostError> {
        let authority_binding = self.state.authority_identity.clone();
        let attachment_id = params.attachment_id.as_str().to_string();
        let attachment = run_storage(self.service.storage.clone(), move |storage| {
            storage.get_environment_attachment(&authority_binding, &attachment_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?
        .ok_or_else(|| not_found_error("environment attachment was not found"))?;
        self.admit_environment_attachment(&attachment)?;
        if attachment.status != DurableEnvironmentStatus::Detached {
            let provider_ref =
                configured_environment_attachment_ref(&self.service.config, &attachment, true)?;
            self.state
                .environment_manager
                .materialize_run_attachments(
                    vec![provider_ref],
                    None,
                    Some(&self.state.connection_id),
                )
                .await
                .map_err(rpc_error_to_host_error)?;
        }
        Ok(host::EnvironmentHealthResult {
            attachment: generated_environment_attachment(&attachment)?,
            checked_at: generated_timestamp(chrono::Utc::now())?,
        })
    }

    async fn environment_list_generated(
        &self,
        params: host::EnvironmentListParams,
    ) -> Result<host::EnvironmentListResult, host::HostError> {
        let scope = params
            .scope
            .as_ref()
            .map(|scope| self.durable_environment_scope(scope))
            .transpose()?;
        let cursor_view = scope.clone();
        let after = params
            .cursor
            .as_deref()
            .map(|cursor| {
                self.service
                    .cursor_codec
                    .decode_page::<EnvironmentCursorPosition, _>(
                        "environment.list",
                        cursor,
                        &self.state.authority_identity,
                        &cursor_view,
                    )
            })
            .transpose()
            .map_err(cursor_invalid_error)?
            .map(environment_page_key)
            .transpose()?;
        let query = EnvironmentAttachmentQuery {
            authority_binding: self.state.authority_identity.clone(),
            scope,
            connection_id: Some(self.state.connection_id.clone()),
            limit: params.limit,
            after,
        };
        let page = run_storage(self.service.storage.clone(), move |storage| {
            storage.list_environment_attachments(query)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let next_cursor = page
            .next
            .as_ref()
            .map(|next| {
                self.service.cursor_codec.encode_page(
                    "environment.list",
                    &EnvironmentCursorPosition {
                        updated_at: next.updated_at.to_rfc3339(),
                        attachment_id: next.attachment_id.clone(),
                    },
                    &self.state.authority_identity,
                    &cursor_view,
                )
            })
            .transpose()
            .map_err(|_| internal_error("failed to encode environment page cursor", true))?;
        Ok(host::EnvironmentListResult {
            attachments: page
                .items
                .iter()
                .map(generated_environment_attachment)
                .collect::<Result<Vec<_>, _>>()?,
            page: host::PageInfo {
                has_more: next_cursor.is_some(),
                next_cursor,
            },
        })
    }

    async fn environment_mount_generated(
        &self,
        params: host::EnvironmentMountParams,
    ) -> Result<host::EnvironmentMountResult, host::HostError> {
        let authority_binding = self.state.authority_identity.clone();
        let attachment_id = params.attachment_id.as_str().to_string();
        let attachment = run_storage(self.service.storage.clone(), {
            let authority_binding = authority_binding.clone();
            let attachment_id = attachment_id.clone();
            move |storage| storage.get_environment_attachment(&authority_binding, &attachment_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?
        .ok_or_else(|| not_found_error("environment attachment was not found"))?;
        self.admit_environment_attachment(&attachment)?;
        if matches!(
            &attachment.scope,
            DurableEnvironmentScope::Connection { .. }
        ) {
            return Err(unsupported_feature_error(
                "connection-scoped attachments cannot own durable resource mounts",
            ));
        }
        let session_id = params.session_id.as_str().to_string();
        let run_id = params.run_id.as_str().to_string();
        if !attachment
            .scope
            .permits_run(Some(&self.state.connection_id), &session_id, &run_id)
        {
            return Err(authorization_denied_error(
                "environment attachment scope does not permit this run",
            ));
        }
        self.service
            .storage
            .session_store()
            .load_run(
                &SessionId::from_string(&session_id),
                &RunId::from_string(&run_id),
            )
            .await
            .map_err(session_store_to_generated_error)?;
        let resource = self
            .service
            .config
            .resolve_environment_resource(&attachment.environment_id, &params.resource_ref)
            .map_err(rpc_host_to_generated_error)?;
        let fingerprint = mutation_fingerprint("environment.mount", &params)?;
        let mount_id = deterministic_environment_id(
            "mount",
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let occurred_at = chrono::Utc::now();
        let transition_identity = environment_transition_identity(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let event_scope = DurableHostEventScope::run(
            SessionId::from_string(&session_id),
            RunId::from_string(&run_id),
        );
        let command = MountEnvironmentResource {
            context: EnvironmentMutationContext {
                authority_binding,
                idempotency_key: params.idempotency_key.as_str().to_string(),
                command_fingerprint: fingerprint,
                occurred_at,
                host_event: Some(EnvironmentHostEventContext {
                    transition_identity,
                    scope: event_scope,
                }),
            },
            mount_id: mount_id.clone(),
            attachment_id,
            session_id,
            run_id,
            connection_id: Some(self.state.connection_id.clone()),
            resource_label: resource.label,
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.mount_environment_resource(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::EnvironmentMountResult {
            mount_id,
            receipt: generated_mutation_receipt(&result.receipt)?,
        })
    }

    async fn environment_mounts_list_generated(
        &self,
        params: host::EnvironmentMountListParams,
    ) -> Result<host::EnvironmentMountListResult, host::HostError> {
        let query = EnvironmentMountQuery {
            authority_binding: self.state.authority_identity.clone(),
            session_id: params.session_id.as_str().to_string(),
            run_id: params.run_id.as_str().to_string(),
            limit: 128,
        };
        let mounts = run_storage(self.service.storage.clone(), move |storage| {
            storage.list_environment_mounts(query)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::EnvironmentMountListResult {
            mounts: mounts
                .into_iter()
                .filter(|mount| mount.status == DurableEnvironmentMountStatus::Mounted)
                .map(|mount| {
                    Ok(host::EnvironmentMountSummary {
                        attachment_id: host::AttachmentId::new(mount.attachment_id).map_err(
                            |_| {
                                internal_error("attachment identity violated protocol schema", true)
                            },
                        )?,
                        mount_id: mount.mount_id,
                        resource_label: mount.resource_label,
                    })
                })
                .collect::<Result<Vec<_>, host::HostError>>()?,
        })
    }

    async fn environment_unmount_generated(
        &self,
        params: host::EnvironmentUnmountParams,
    ) -> Result<host::EnvironmentUnmountResult, host::HostError> {
        let fingerprint = mutation_fingerprint("environment.unmount", &params)?;
        let authority_binding = self.state.authority_identity.clone();
        let mount_id = params.mount_id.clone();
        let mount = run_storage(self.service.storage.clone(), {
            let authority_binding = authority_binding.clone();
            let mount_id = mount_id.clone();
            move |storage| storage.get_environment_mount(&authority_binding, &mount_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?
        .ok_or_else(|| not_found_error("environment mount was not found"))?;
        run_storage(self.service.storage.clone(), {
            let authority_binding = authority_binding.clone();
            let attachment_id = mount.attachment_id.clone();
            move |storage| storage.get_environment_attachment(&authority_binding, &attachment_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?
        .ok_or_else(|| internal_error("environment mount references a missing attachment", true))?;
        let occurred_at = chrono::Utc::now();
        let transition_identity = environment_transition_identity(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let event_scope = DurableHostEventScope::run(
            SessionId::from_string(&mount.session_id),
            RunId::from_string(&mount.run_id),
        );
        let command = UnmountEnvironmentResource {
            context: EnvironmentMutationContext {
                authority_binding,
                idempotency_key: params.idempotency_key.as_str().to_string(),
                command_fingerprint: fingerprint,
                occurred_at,
                host_event: Some(EnvironmentHostEventContext {
                    transition_identity,
                    scope: event_scope,
                }),
            },
            mount_id: mount_id.clone(),
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.unmount_environment_resource(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::EnvironmentUnmountResult {
            mount_id,
            receipt: generated_mutation_receipt(&result.receipt)?,
            removed: result.mount.status == DurableEnvironmentMountStatus::Unmounted,
        })
    }

    async fn run_start_generated(
        &self,
        params: host::RunStartParams,
    ) -> Result<host::RunStartResult, host::HostError> {
        let fingerprint = mutation_fingerprint("run.start", &params)?;
        let storage_idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        if let Some(started) = self
            .service
            .coordinator
            .lookup_started_run(&storage_idempotency_key, &fingerprint)
            .await
            .map_err(rpc_host_to_generated_error)?
        {
            let run = self
                .service
                .storage
                .session_store()
                .load_run(&started.session_id, &started.run_id)
                .await
                .map_err(session_store_to_generated_error)?;
            return Ok(host::RunStartResult {
                receipt: durable_wire_receipt(
                    &started.admission_id,
                    &params.idempotency_key,
                    &fingerprint,
                    "run.start",
                    started.status.as_str(),
                    &format!(
                        "run:{}/{}",
                        started.session_id.as_str(),
                        started.run_id.as_str()
                    ),
                    true,
                    false,
                )?,
                run: run_summary(&run)?,
            });
        }
        let (durable_input, input) = generated_run_input(&params.input)?;
        let profile = match params.profile.clone() {
            Some(profile) => profile,
            None => self.current_model_selection().await?.selected_profile,
        };
        self.service
            .catalog
            .profile(&profile)
            .map_err(rpc_host_to_generated_error)?;
        let session_id = params
            .session_id
            .as_ref()
            .map(|id| SessionId::from_string(id.as_str()));
        let restore_from_run_id = params
            .restore_from_run_id
            .as_ref()
            .map(|id| RunId::from_string(id.as_str()));
        let environment_attachments = self
            .resolve_run_environment_attachments(
                &params.environment_attachments,
                session_id.as_ref(),
                restore_from_run_id.as_ref(),
            )
            .await?;
        let mut request = RpcRunRequest {
            durable_input,
            input,
            session_id,
            restore_from_run_id,
            profile,
            environment_attachments,
            idempotency_key: storage_idempotency_key,
            command_fingerprint: fingerprint.clone(),
            continuation_mode: generated_continuation_mode(params.continuation_mode),
            install_session_management: false,
        };
        request.install_session_management =
            self.service.notifications == RpcNotificationMode::Live;
        let started = self
            .service
            .coordinator
            .start(request)
            .await
            .map_err(rpc_host_to_generated_error)?;
        let run = self
            .service
            .storage
            .session_store()
            .load_run(&started.session_id, &started.run_id)
            .await
            .map_err(session_store_to_generated_error)?;
        Ok(host::RunStartResult {
            receipt: durable_wire_receipt(
                &started.admission_id,
                &params.idempotency_key,
                &fingerprint,
                "run.start",
                started.status.as_str(),
                &format!(
                    "run:{}/{}",
                    started.session_id.as_str(),
                    started.run_id.as_str()
                ),
                started.idempotent_replay,
                false,
            )?,
            run: run_summary(&run)?,
        })
    }

    async fn run_resume_generated(
        &self,
        params: host::RunResumeParams,
    ) -> Result<host::RunResumeResult, host::HostError> {
        let fingerprint = mutation_fingerprint("run.resume", &params)?;
        let storage_idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        if let Some(started) = self
            .service
            .coordinator
            .lookup_started_run(&storage_idempotency_key, &fingerprint)
            .await
            .map_err(rpc_host_to_generated_error)?
        {
            let run = self
                .service
                .storage
                .session_store()
                .load_run(&started.session_id, &started.run_id)
                .await
                .map_err(session_store_to_generated_error)?;
            return Ok(host::RunResumeResult {
                receipt: durable_wire_receipt(
                    &started.admission_id,
                    &params.idempotency_key,
                    &fingerprint,
                    "run.resume",
                    started.status.as_str(),
                    &format!(
                        "run:{}/{}",
                        started.session_id.as_str(),
                        started.run_id.as_str()
                    ),
                    true,
                    false,
                )?,
                run: run_summary(&run)?,
                source_run_id: params.run_id,
            });
        }
        let session_id = SessionId::from_string(params.session_id.as_str());
        let source_run_id = RunId::from_string(params.run_id.as_str());
        let store = self.service.storage.session_store();
        let source = store
            .load_run(&session_id, &source_run_id)
            .await
            .map_err(session_store_to_generated_error)?;
        let session = store
            .load_session(&session_id)
            .await
            .map_err(session_store_to_generated_error)?;
        let profile = params
            .profile
            .clone()
            .or(source.profile)
            .or(session.profile)
            .unwrap_or_else(|| self.service.catalog.default_profile().to_string());
        self.service
            .catalog
            .profile(&profile)
            .map_err(rpc_host_to_generated_error)?;
        let environment_attachments = self
            .resolve_run_environment_attachments(
                &params.environment_attachments,
                Some(&session_id),
                Some(&source_run_id),
            )
            .await?;
        let started = self
            .service
            .coordinator
            .resume_waiting(RpcHitlResumeRequest {
                session_id: session_id.clone(),
                source_run_id: source_run_id.clone(),
                profile,
                environment_attachments,
                idempotency_key: storage_idempotency_key,
                command_fingerprint: fingerprint.clone(),
                continuation_mode: generated_continuation_mode(params.continuation_mode),
                install_session_management: self.service.notifications == RpcNotificationMode::Live,
            })
            .await
            .map_err(rpc_host_to_generated_error)?;
        let run = store
            .load_run(&started.session_id, &started.run_id)
            .await
            .map_err(session_store_to_generated_error)?;
        Ok(host::RunResumeResult {
            receipt: durable_wire_receipt(
                &started.admission_id,
                &params.idempotency_key,
                &fingerprint,
                "run.resume",
                started.status.as_str(),
                &format!(
                    "run:{}/{}",
                    started.session_id.as_str(),
                    started.run_id.as_str()
                ),
                started.idempotent_replay,
                false,
            )?,
            run: run_summary(&run)?,
            source_run_id: host::RunId::new(source_run_id.as_str()).map_err(|_| {
                internal_error("source run identity violated protocol schema", true)
            })?,
        })
    }

    async fn run_interrupt_generated(
        &self,
        params: host::RunInterruptParams,
    ) -> Result<host::RunInterruptResult, host::HostError> {
        let fingerprint = mutation_fingerprint("run.interrupt", &params)?;
        let session_id = SessionId::from_string(params.session_id.as_str());
        let run_id = RunId::from_string(params.run_id.as_str());
        let operation_id = deterministic_run_control_id(
            "interrupt",
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let storage_idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let outcome = self
            .service
            .coordinator
            .cancel_idempotent_bound(
                &session_id,
                &run_id,
                operation_id,
                params.reason,
                Some(storage_idempotency_key),
                self.state.authority_identity.clone(),
            )
            .await
            .map_err(rpc_host_to_generated_error)?;
        let receipt_id = outcome
            .get("receiptId")
            .and_then(Value::as_str)
            .ok_or_else(|| internal_error("interrupt receipt projection is incomplete", true))?;
        let replayed = outcome
            .get("idempotent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let run = self
            .service
            .storage
            .session_store()
            .load_run(&session_id, &run_id)
            .await
            .map_err(session_store_to_generated_error)?;
        Ok(host::RunInterruptResult {
            receipt: durable_wire_receipt(
                receipt_id,
                &params.idempotency_key,
                &fingerprint,
                "run.interrupt",
                "accepted",
                &format!("run:{}/{}", session_id.as_str(), run_id.as_str()),
                replayed,
                false,
            )?,
            run: run_summary(&run)?,
        })
    }

    async fn run_steer_generated(
        &self,
        params: host::RunSteerParams,
    ) -> Result<host::RunSteerResult, host::HostError> {
        let fingerprint = mutation_fingerprint("run.steer", &params)?;
        let session_id = SessionId::from_string(params.session_id.as_str());
        let run_id = RunId::from_string(params.run_id.as_str());
        let operation_id = deterministic_run_control_id(
            "steer",
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let storage_idempotency_key = authority_scoped_idempotency_key(
            &self.state.authority_identity,
            params.idempotency_key.as_str(),
        );
        let outcome = self
            .service
            .coordinator
            .steer_idempotent_bound(
                &session_id,
                &run_id,
                operation_id,
                params.text,
                Some(storage_idempotency_key),
                self.state.authority_identity.clone(),
            )
            .await
            .map_err(rpc_host_to_generated_error)?;
        let receipt_id = outcome
            .get("receiptId")
            .and_then(Value::as_str)
            .ok_or_else(|| internal_error("steering receipt projection is incomplete", true))?;
        let replayed = outcome
            .get("idempotent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let accepted = outcome
            .get("queued")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(host::RunSteerResult {
            accepted,
            receipt: durable_wire_receipt(
                receipt_id,
                &params.idempotency_key,
                &fingerprint,
                "run.steer",
                "accepted",
                &format!("run:{}/{}", session_id.as_str(), run_id.as_str()),
                replayed,
                false,
            )?,
        })
    }

    async fn run_status_generated(
        &self,
        params: host::RunStatusParams,
    ) -> Result<host::RunStatusResult, host::HostError> {
        let run = self
            .service
            .storage
            .session_store()
            .load_run(
                &SessionId::from_string(params.session_id.as_str()),
                &RunId::from_string(params.run_id.as_str()),
            )
            .await
            .map_err(session_store_to_generated_error)?;
        Ok(host::RunStatusResult {
            run: run_summary(&run)?,
        })
    }

    async fn approval_decide_generated(
        &self,
        params: host::ApprovalDecideParams,
    ) -> Result<host::ApprovalDecideResult, host::HostError> {
        let status = match params.decision.as_str() {
            "approved" => ApprovalStatus::Approved,
            "denied" => ApprovalStatus::Denied,
            _ => return Err(invalid_params_error("decision must be approved or denied")),
        };
        let fingerprint = mutation_fingerprint("approval.decide", &params)?;
        let approval_id = params.approval_id.as_str().to_string();
        let current = run_storage(self.service.storage.clone(), {
            let approval_id = approval_id.clone();
            move |storage| storage.load_approval(&approval_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let occurred_at = chrono::Utc::now();
        let mut projected = current.clone();
        projected.revision = projected.revision.saturating_add(1);
        projected.status = status;
        projected.updated_at = occurred_at;
        projected.decision = Some(ApprovalDecision {
            status,
            decided_by: Some(self.state.authority_identity.clone()),
            decided_at: occurred_at,
            reason: params.reason.clone(),
            metadata: Default::default(),
        });
        let transition = mutation_transition_identity(
            "approval.decide",
            params.idempotency_key.as_str(),
            &fingerprint,
        );
        let command = DecideApproval {
            context: InteractionMutationContext {
                authority_binding: self.state.authority_identity.clone(),
                expected_revision: params.expected_revision.get(),
                idempotency_key: params.idempotency_key.as_str().to_string(),
                command_fingerprint: fingerprint,
                occurred_at,
                host_event_publication: Some(approval_changed_publication(
                    &transition,
                    &projected,
                )?),
            },
            session_id: current.session_id,
            run_id: current.run_id,
            approval_id,
            decision: projected
                .decision
                .clone()
                .ok_or_else(|| internal_error("approval decision projection failed", true))?,
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.decide_approval_atomic(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::ApprovalDecideResult {
            approval: approval_summary(&result.approval)?,
            receipt: generated_mutation_receipt(&result.receipt)?,
        })
    }

    async fn clarification_resolve_generated(
        &self,
        params: host::ClarificationResolveParams,
    ) -> Result<host::ClarificationResolveResult, host::HostError> {
        let fingerprint = mutation_fingerprint("clarification.resolve", &params)?;
        let clarification_id = params.clarification_id.as_str().to_string();
        let current = run_storage(self.service.storage.clone(), {
            let clarification_id = clarification_id.clone();
            move |storage| storage.load_approval(&clarification_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let answers = params
            .answers
            .iter()
            .map(|answer| DomainClarificationAnswer {
                question: answer.question.clone(),
                selected_options: answer.selected_options.clone(),
                free_text: answer.free_text.clone(),
            })
            .collect::<Vec<_>>();
        let (questions, _) =
            starweaver_session::validate_clarification_answers(&current.request, &answers)
                .map_err(session_store_to_generated_error)?;
        let occurred_at = chrono::Utc::now();
        let summary = clarification_summary(
            &current,
            &questions,
            host::ClarificationStatus::Resolved,
            current.revision.saturating_add(1),
            occurred_at,
        )?;
        let transition = mutation_transition_identity(
            "clarification.resolve",
            params.idempotency_key.as_str(),
            &fingerprint,
        );
        let command = ResolveClarification {
            context: InteractionMutationContext {
                authority_binding: self.state.authority_identity.clone(),
                expected_revision: params.expected_revision.get(),
                idempotency_key: params.idempotency_key.as_str().to_string(),
                command_fingerprint: fingerprint,
                occurred_at,
                host_event_publication: Some(clarification_changed_publication(
                    &transition,
                    &current.session_id,
                    &current.run_id,
                    &summary,
                    occurred_at,
                )?),
            },
            session_id: current.session_id,
            run_id: current.run_id,
            clarification_id,
            answers,
            response: params.response,
            resolved_by: Some(self.state.authority_identity.clone()),
        };
        let result = run_storage(self.service.storage.clone(), move |storage| {
            storage.resolve_clarification_atomic(command)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::ClarificationResolveResult {
            clarification: clarification_summary(
                &result.approval,
                &result.clarification.questions,
                host::ClarificationStatus::Resolved,
                result.clarification.revision,
                result.clarification.resolved_at,
            )?,
            receipt: generated_mutation_receipt(&result.receipt)?,
        })
    }

    async fn deferred_complete_generated(
        &self,
        params: host::DeferredCompleteParams,
    ) -> Result<host::DeferredCompleteResult, host::HostError> {
        self.resolve_deferred_generated(
            params.deferred_id.as_str(),
            params.expected_revision.get(),
            &params.idempotency_key,
            "deferred.complete",
            DeferredMutationOutcome::Completed {
                response: Value::String(params.result_text.clone()),
                metadata: Default::default(),
            },
            &params,
        )
        .await
        .and_then(|result| {
            Ok(host::DeferredCompleteResult {
                deferred: deferred_summary(&result.deferred)?,
                receipt: generated_mutation_receipt(&result.receipt)?,
            })
        })
    }

    async fn deferred_fail_generated(
        &self,
        params: host::DeferredFailParams,
    ) -> Result<host::DeferredFailResult, host::HostError> {
        self.resolve_deferred_generated(
            params.deferred_id.as_str(),
            params.expected_revision.get(),
            &params.idempotency_key,
            "deferred.fail",
            DeferredMutationOutcome::Failed {
                response: json!({"error": params.error.clone()}),
                metadata: Default::default(),
            },
            &params,
        )
        .await
        .and_then(|result| {
            Ok(host::DeferredFailResult {
                deferred: deferred_summary(&result.deferred)?,
                receipt: generated_mutation_receipt(&result.receipt)?,
            })
        })
    }

    async fn resolve_deferred_generated(
        &self,
        deferred_id: &str,
        expected_revision: u64,
        idempotency_key: &host::IdempotencyKey,
        operation: &str,
        outcome: DeferredMutationOutcome,
        fingerprint_params: &(impl Serialize + Sync),
    ) -> Result<starweaver_session::DeferredMutationResult, host::HostError> {
        let fingerprint = mutation_fingerprint(operation, fingerprint_params)?;
        let current = run_storage(self.service.storage.clone(), {
            let deferred_id = deferred_id.to_string();
            move |storage| storage.load_deferred_tool(&deferred_id)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let occurred_at = chrono::Utc::now();
        let mut projected = current.clone();
        projected.revision = projected.revision.saturating_add(1);
        projected.status = outcome.status();
        projected.updated_at = occurred_at;
        let (response, metadata) = outcome.parts();
        projected.response = response.clone();
        projected.metadata.extend(metadata.clone());
        let transition =
            mutation_transition_identity(operation, idempotency_key.as_str(), &fingerprint);
        let command = ResolveDeferredTool {
            context: InteractionMutationContext {
                authority_binding: self.state.authority_identity.clone(),
                expected_revision,
                idempotency_key: idempotency_key.as_str().to_string(),
                command_fingerprint: fingerprint,
                occurred_at,
                host_event_publication: Some(deferred_changed_publication(
                    &transition,
                    &projected,
                )?),
            },
            session_id: current.session_id,
            run_id: current.run_id,
            deferred_id: deferred_id.to_string(),
            outcome,
        };
        run_storage(self.service.storage.clone(), move |storage| {
            storage.resolve_deferred_tool_atomic(command)
        })
        .await
        .map_err(rpc_error_to_host_error)
    }

    async fn approval_list_generated(
        &self,
        params: host::InteractionListParams,
    ) -> Result<host::ApprovalListResult, host::HostError> {
        let view = InteractionCursorView {
            session_id: params.session_id.as_ref().map(host::SessionId::as_str),
            run_id: params.run_id.as_ref().map(host::RunId::as_str),
        };
        let after = self.interaction_cursor("approval.list", params.cursor.as_deref(), &view)?;
        let query = InteractionPageQuery::new(
            params
                .session_id
                .as_ref()
                .map(|id| SessionId::from_string(id.as_str())),
            params
                .run_id
                .as_ref()
                .map(|id| RunId::from_string(id.as_str())),
            after,
            params.limit as usize,
        )
        .map_err(|_| invalid_params_error("invalid approval page limit"))?;
        let page = run_storage(self.service.storage.clone(), move |storage| {
            storage.list_approval_page(query)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let next_cursor = self.next_interaction_cursor("approval.list", &view, &page)?;
        Ok(host::ApprovalListResult {
            approvals: page
                .records
                .iter()
                .map(approval_summary)
                .collect::<Result<Vec<_>, _>>()?,
            page: host::PageInfo {
                has_more: page.has_more,
                next_cursor,
            },
        })
    }

    async fn approval_show_generated(
        &self,
        params: host::ApprovalShowParams,
    ) -> Result<host::ApprovalShowResult, host::HostError> {
        let id = params.approval_id.into_string();
        let approval = run_storage(self.service.storage.clone(), move |storage| {
            storage.load_approval(&id)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::ApprovalShowResult {
            approval: approval_summary(&approval)?,
        })
    }

    async fn deferred_list_generated(
        &self,
        params: host::InteractionListParams,
    ) -> Result<host::DeferredListResult, host::HostError> {
        let view = InteractionCursorView {
            session_id: params.session_id.as_ref().map(host::SessionId::as_str),
            run_id: params.run_id.as_ref().map(host::RunId::as_str),
        };
        let after = self.interaction_cursor("deferred.list", params.cursor.as_deref(), &view)?;
        let query = InteractionPageQuery::new(
            params
                .session_id
                .as_ref()
                .map(|id| SessionId::from_string(id.as_str())),
            params
                .run_id
                .as_ref()
                .map(|id| RunId::from_string(id.as_str())),
            after,
            params.limit as usize,
        )
        .map_err(|_| invalid_params_error("invalid deferred page limit"))?;
        let page = run_storage(self.service.storage.clone(), move |storage| {
            storage.list_deferred_tool_page(query)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        let next_cursor = self.next_interaction_cursor("deferred.list", &view, &page)?;
        Ok(host::DeferredListResult {
            deferred: page
                .records
                .iter()
                .map(deferred_summary)
                .collect::<Result<Vec<_>, _>>()?,
            page: host::PageInfo {
                has_more: page.has_more,
                next_cursor,
            },
        })
    }

    async fn deferred_show_generated(
        &self,
        params: host::DeferredShowParams,
    ) -> Result<host::DeferredShowResult, host::HostError> {
        let id = params.deferred_id.into_string();
        let deferred = run_storage(self.service.storage.clone(), move |storage| {
            storage.load_deferred_tool(&id)
        })
        .await
        .map_err(rpc_error_to_host_error)?;
        Ok(host::DeferredShowResult {
            deferred: deferred_summary(&deferred)?,
        })
    }

    fn interaction_cursor(
        &self,
        kind: &str,
        cursor: Option<&str>,
        view: &InteractionCursorView<'_>,
    ) -> Result<Option<InteractionPageKey>, host::HostError> {
        cursor
            .map(|cursor| {
                self.service
                    .cursor_codec
                    .decode_page::<InteractionCursorPosition, _>(
                        kind,
                        cursor,
                        &self.state.authority_identity,
                        view,
                    )
            })
            .transpose()
            .map_err(cursor_invalid_error)?
            .map(interaction_page_key)
            .transpose()
    }

    fn next_interaction_cursor<T>(
        &self,
        kind: &str,
        view: &InteractionCursorView<'_>,
        page: &starweaver_session::InteractionPage<T>,
    ) -> Result<Option<String>, host::HostError> {
        if !page.has_more {
            return Ok(None);
        }
        page.next_key
            .as_ref()
            .map(|key| {
                self.service.cursor_codec.encode_page(
                    kind,
                    &interaction_cursor_position(key),
                    &self.state.authority_identity,
                    view,
                )
            })
            .transpose()
            .map_err(|_| internal_error("failed to encode interaction page cursor", true))
    }

    async fn shutdown_generated(
        &self,
        params: host::ShutdownParams,
    ) -> Result<host::ShutdownResult, host::HostError> {
        let deadline = Duration::from_millis(u64::from(params.deadline_ms));
        self.service
            .coordinator
            .shutdown(deadline.min(Duration::from_secs(30)))
            .await
            .map_err(rpc_host_to_generated_error)?;
        Ok(host::ShutdownResult {
            status: "shutdown".to_string(),
        })
    }

    async fn initialize_generated(
        &self,
        params: host::InitializeParams,
    ) -> Result<host::InitializeResult, host::HostError> {
        if params.protocol.major != host::PROTOCOL_MAJOR
            || params.protocol.revision != host::PROTOCOL_REVISION
            || params.protocol.schema_digest.as_str() != host::SCHEMA_DIGEST
        {
            return Err(unsupported_feature_error(
                "client protocol identity does not match this host",
            ));
        }
        let supported = self.supported_features();
        if params
            .required_features
            .iter()
            .any(|feature| !supported.contains(feature.as_str()))
        {
            return Err(unsupported_feature_error(
                "one or more required client features are unavailable",
            ));
        }
        let client_supported = params
            .supported_features
            .into_iter()
            .map(host::FeatureId::into_string)
            .collect::<BTreeSet<_>>();
        if params
            .required_features
            .iter()
            .any(|feature| !client_supported.contains(feature.as_str()))
        {
            return Err(unsupported_feature_error(
                "required client features must also be declared as supported",
            ));
        }
        let negotiated = supported
            .intersection(&client_supported)
            .cloned()
            .collect::<BTreeSet<_>>();
        self.state
            .negotiated_features
            .lock()
            .map_err(|_| internal_error("feature negotiation state unavailable", true))?
            .clone_from(&negotiated);
        let result = host::InitializeResult {
            launch: host::LaunchCompatibility {
                accepted_maximum_version: host::LAUNCH_SCHEMA_VERSION,
                accepted_minimum_version: host::LAUNCH_SCHEMA_VERSION,
                configuration_generation: host::DecimalU64::new(
                    self.service.config.launch.configuration_generation,
                ),
                effective_schema: host::LaunchSchemaIdentity {
                    name: host::LaunchSchemaIdentityName::Value,
                    version: self.service.config.launch.schema_version,
                },
                envelope_digest: host::SchemaDigest::new(
                    &self.service.config.launch.envelope_digest,
                )
                .map_err(|_| internal_error("launch digest violated protocol schema", true))?,
                mode: self.service.config.launch.mode.clone(),
            },
            negotiated_features: negotiated
                .into_iter()
                .map(host::FeatureId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| internal_error("server feature id violated protocol schema", true))?,
            protocol: generated_protocol_identity()?,
            runtime_build: host::RuntimeBuildIdentity {
                build_revision: option_env!("STARWEAVER_BUILD_REVISION")
                    .unwrap_or("source")
                    .to_string(),
                target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            runtime_status: "ready".to_string(),
            server_info: host::ServerInfo {
                name: "starweaver-rpc".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            startup_reconciliation: host::StartupReconciliation {
                changed_run_state: self.service.startup_repaired_runs > 0,
                repaired_runs: host::DecimalU64::new(self.service.startup_repaired_runs),
            },
            storage: host::StorageCompatibility {
                current_generation: host::DecimalU64::new(1),
                maintenance_barrier_generation: host::DecimalU64::new(0),
                maximum_readable_generation: host::DecimalU64::new(1),
                maximum_writable_generation: host::DecimalU64::new(1),
                minimum_readable_generation: host::DecimalU64::new(1),
                minimum_writable_generation: host::DecimalU64::new(1),
            },
            supported_features: supported
                .into_iter()
                .map(host::FeatureId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| internal_error("server feature id violated protocol schema", true))?,
            workspace: host::WorkspaceCompatibility {
                execution_domain_id: self.service.config.launch.execution_domain_id.clone(),
                workspace_identity: self.service.config.launch.workspace_identity.clone(),
            },
        };
        Ok(result)
    }

    fn supported_features(&self) -> BTreeSet<String> {
        host::METHODS
            .iter()
            .filter(|metadata| {
                (self.state.output.is_some()
                    || !matches!(
                        metadata.method,
                        host::Method::EventsSubscribe | host::Method::EventsUnsubscribe
                    ))
                    && (self.service.session_search.is_some()
                        || metadata.method != host::Method::SessionSearch)
                    && (self.service.config.client_capabilities.clarifying_questions
                        || metadata.method != host::Method::ClarificationResolve)
            })
            .flat_map(|metadata| metadata.features.iter().copied())
            .map(str::to_string)
            .collect()
    }

    async fn diagnostics_generated(
        &self,
        _params: host::DiagnosticsGetParams,
    ) -> Result<host::DiagnosticsGetResult, host::HostError> {
        Ok(host::DiagnosticsGetResult {
            diagnostic_ref: None,
            pending_recovery_items: host::DecimalU64::new(0),
            protocol: generated_protocol_identity()?,
            runtime_status: "ready".to_string(),
            sdk: starweaver_core::sdk_name().to_string(),
            storage_current: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    async fn events_replay(
        &self,
        params: host::EventsReplayParams,
    ) -> Result<host::EventsReplayResult, host::HostError> {
        let event_classes = self.admitted_event_classes(&params.view)?;
        self.service.drain_host_event_outbox().await?;
        let scope = durable_event_scope(&params.view.scope);
        let after_position = params
            .cursor
            .as_ref()
            .map(|cursor| {
                self.service.cursor_codec.decode(
                    cursor,
                    &self.state.authority_identity,
                    &params.view,
                )
            })
            .transpose()
            .map_err(cursor_invalid_error)?;
        let query =
            DurableHostEventQuery::new(scope, event_classes, after_position, params.limit as usize)
                .map_err(|_| internal_error("invalid durable event query", false))?;
        let page = self
            .service
            .storage
            .session_store()
            .replay_host_events(query)
            .await
            .map_err(|_| storage_unavailable_error())?;
        let mut deliveries = Vec::with_capacity(page.records.len());
        for record in page.records {
            deliveries.push(self.event_delivery(record, &params.view)?);
        }
        let next_position = page.next_position.or(after_position).unwrap_or(0);
        let next_cursor = self
            .service
            .cursor_codec
            .encode(next_position, &self.state.authority_identity, &params.view)
            .map_err(|_| internal_error("failed to encode durable event cursor", true))?;
        Ok(host::EventsReplayResult {
            deliveries,
            has_more: page.has_more,
            next_cursor,
        })
    }

    fn event_delivery(
        &self,
        record: DurableHostEventRecord,
        view: &host::EventViewRequest,
    ) -> Result<host::EventDelivery, host::HostError> {
        let cursor = self
            .service
            .cursor_codec
            .encode(record.position, &self.state.authority_identity, view)
            .map_err(|_| internal_error("failed to encode durable event cursor", true))?;
        let event = serde_json::from_value::<host::HostEvent>(record.projection).map_err(|_| {
            internal_error("durable event projection violated protocol schema", true)
        })?;
        let event_id = host::EventId::new(record.event_id)
            .map_err(|_| internal_error("durable event identity violated protocol schema", true))?;
        let occurred_at = host::Timestamp::new(
            record
                .occurred_at
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        )
        .map_err(|_| internal_error("durable event timestamp violated protocol schema", true))?;
        Ok(host::EventDelivery {
            cursor,
            record: host::EventRecord {
                event,
                event_id,
                occurred_at,
                scope: generated_event_scope(record.scope)?,
            },
        })
    }

    async fn events_subscribe(
        &self,
        params: host::EventsSubscribeParams,
    ) -> Result<host::EventsSubscribeResult, host::HostError> {
        let Some(output) = self.state.output.clone() else {
            return Err(unsupported_feature_error(
                "durable event subscriptions require the stdio transport",
            ));
        };
        let event_classes = self.admitted_event_classes(&params.view)?;
        self.service.drain_host_event_outbox().await?;
        let scope = durable_event_scope(&params.view.scope);
        let after_position = params
            .cursor
            .as_ref()
            .map(|cursor| {
                self.service.cursor_codec.decode(
                    cursor,
                    &self.state.authority_identity,
                    &params.view,
                )
            })
            .transpose()
            .map_err(cursor_invalid_error)?
            .unwrap_or(0);
        let fence_position = self
            .service
            .storage
            .session_store()
            .host_event_fence(&scope, &event_classes)
            .await
            .map_err(|_| storage_unavailable_error())?
            .unwrap_or(after_position)
            .max(after_position);
        let accepted_cursor = self
            .service
            .cursor_codec
            .encode(after_position, &self.state.authority_identity, &params.view)
            .map_err(|_| internal_error("failed to encode accepted event cursor", true))?;
        let fence_cursor = self
            .service
            .cursor_codec
            .encode(fence_position, &self.state.authority_identity, &params.view)
            .map_err(|_| internal_error("failed to encode event fence cursor", true))?;
        let subscription_id = host::SubscriptionId::new(format!("sub_{}", Uuid::new_v4()))
            .map_err(|_| internal_error("failed to create subscription identity", true))?;
        let (cancel, cancel_receiver) = watch::channel(false);
        let (ready, ready_receiver) = watch::channel(false);
        let progress = Arc::new(Mutex::new(SubscriptionProgress::default()));
        let (tail_stopped, tail_stopped_receiver) = oneshot::channel();
        {
            let mut subscriptions = self
                .state
                .subscriptions
                .lock()
                .map_err(|_| internal_error("subscription registry unavailable", true))?;
            if subscriptions.len() >= MAX_CONNECTION_SUBSCRIPTIONS {
                return Err(already_exists_error("subscription limit reached"));
            }
            subscriptions.insert(
                subscription_id.as_str().to_string(),
                ConnectionSubscription {
                    cancel,
                    ready,
                    progress: Arc::clone(&progress),
                    tail_stopped: tail_stopped_receiver,
                },
            );
        }
        self.spawn_host_event_tail(HostEventTail {
            subscription_id: subscription_id.clone(),
            stopped: tail_stopped,
            view: params.view,
            scope,
            event_classes,
            after_position,
            fence_position,
            output,
            cancel: cancel_receiver,
            ready: ready_receiver,
            progress,
        });
        Ok(host::EventsSubscribeResult {
            accepted_cursor,
            fence_cursor,
            next_delivery_sequence: host::EventsSubscribeResultNextDeliverySequence::Value,
            subscription_id,
        })
    }

    async fn events_unsubscribe(
        &self,
        params: host::EventsUnsubscribeParams,
    ) -> Result<host::EventsUnsubscribeResult, host::HostError> {
        let removed = self
            .state
            .subscriptions
            .lock()
            .map_err(|_| internal_error("subscription registry unavailable", true))?
            .remove(params.subscription_id.as_str());
        if let Some(subscription) = removed {
            let _ = subscription.cancel.send(true);
            let _ = subscription.tail_stopped.await;
            if let Some(output) = self.state.output.clone() {
                let (activate, activated) = oneshot::channel();
                self.state
                    .pending_activations
                    .lock()
                    .map_err(|_| internal_error("subscription activation queue unavailable", true))?
                    .push(activate);
                let subscription_id = params.subscription_id.clone();
                self.service.runtime.spawn(async move {
                    if activated.await.is_ok() {
                        let (last_cursor, last_sequence) = {
                            let progress = subscription.progress.lock().ok();
                            (
                                progress
                                    .as_ref()
                                    .and_then(|progress| progress.last_flushed_cursor.clone()),
                                progress
                                    .as_ref()
                                    .and_then(|progress| progress.last_flushed_sequence)
                                    .map(host::DecimalU64::new),
                            )
                        };
                        let _ = send_generated_notification(
                            &output,
                            host::HostNotificationParams::SubscriptionClosed(Box::new(
                                host::SubscriptionClosedNotificationParams {
                                    last_flushed_cursor: last_cursor,
                                    last_flushed_delivery_sequence: last_sequence,
                                    reason: host::SubscriptionClosedReason::Unsubscribed,
                                    subscription_id,
                                },
                            )),
                        )
                        .await;
                    }
                });
            }
            Ok(host::EventsUnsubscribeResult {
                closed: true,
                subscription_id: params.subscription_id,
            })
        } else {
            Ok(host::EventsUnsubscribeResult {
                closed: false,
                subscription_id: params.subscription_id,
            })
        }
    }

    fn spawn_host_event_tail(&self, mut tail: HostEventTail) {
        let connection = self.clone();
        let subscriptions = Arc::clone(&self.state.subscriptions);
        self.service.runtime.spawn(async move {
            while !*tail.ready.borrow() {
                if tail.ready.changed().await.is_err() || *tail.cancel.borrow() {
                    return;
                }
            }
            let mut position = tail.after_position;
            let mut delivery_sequence = 1_u64;
            let mut catch_up = position < tail.fence_position;
            let mut close_reason = None;
            'delivery: loop {
                if *tail.cancel.borrow() {
                    break;
                }
                if connection.service.drain_host_event_outbox().await.is_err() {
                    close_reason = Some(host::SubscriptionClosedReason::Overflow);
                    break;
                }
                let query = match DurableHostEventQuery::new(
                    tail.scope.clone(),
                    tail.event_classes.clone(),
                    Some(position),
                    SUBSCRIPTION_REPLAY_PAGE,
                ) {
                    Ok(query) => query,
                    Err(_) => {
                        close_reason = Some(host::SubscriptionClosedReason::Overflow);
                        break;
                    }
                };
                let page = match connection
                    .service
                    .storage
                    .session_store()
                    .replay_host_events(query)
                    .await
                {
                    Ok(page) => page,
                    Err(_) => {
                        close_reason = Some(host::SubscriptionClosedReason::Overflow);
                        break;
                    }
                };
                let mut delivered = 0_usize;
                for record in page.records {
                    if catch_up && record.position > tail.fence_position {
                        break;
                    }
                    let delivery = match connection.event_delivery(record, &tail.view) {
                        Ok(delivery) => delivery,
                        Err(_) => {
                            close_reason = Some(host::SubscriptionClosedReason::Overflow);
                            break 'delivery;
                        }
                    };
                    let cursor = delivery.cursor.clone();
                    let terminal = event_delivery_is_terminal(&delivery, &tail.scope);
                    let frame =
                        match generated_notification_value(host::HostNotificationParams::HostEvent(
                            Box::new(host::HostEventNotificationParams {
                                delivery,
                                delivery_sequence: host::DecimalU64::new(delivery_sequence),
                                subscription_id: tail.subscription_id.clone(),
                            }),
                        )) {
                            Ok(frame) => frame,
                            Err(_) => {
                                close_reason = Some(host::SubscriptionClosedReason::Overflow);
                                break 'delivery;
                            }
                        };
                    if !send_subscription_frame(&tail.output, &mut tail.cancel, frame).await {
                        break 'delivery;
                    }
                    position = connection
                        .service
                        .cursor_codec
                        .decode(&cursor, &connection.state.authority_identity, &tail.view)
                        .unwrap_or(position);
                    if let Ok(mut progress) = tail.progress.lock() {
                        progress.last_flushed_cursor = Some(cursor);
                        progress.last_flushed_sequence = Some(delivery_sequence);
                    }
                    delivered += 1;
                    if terminal {
                        close_reason = Some(host::SubscriptionClosedReason::Terminal);
                        break 'delivery;
                    }
                    let Some(next_sequence) = delivery_sequence.checked_add(1) else {
                        close_reason = Some(host::SubscriptionClosedReason::SequenceExhausted);
                        break 'delivery;
                    };
                    delivery_sequence = next_sequence;
                }
                if catch_up && position >= tail.fence_position {
                    catch_up = false;
                }
                if delivered == 0 {
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_millis(50)) => {}
                        changed = tail.cancel.changed() => {
                            if changed.is_err() || *tail.cancel.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
            if let Ok(mut registry) = subscriptions.lock() {
                registry.remove(tail.subscription_id.as_str());
            }
            if let Some(reason) = close_reason {
                let (last_flushed_cursor, last_flushed_delivery_sequence) = {
                    let progress = tail.progress.lock().ok();
                    (
                        progress
                            .as_ref()
                            .and_then(|progress| progress.last_flushed_cursor.clone()),
                        progress
                            .as_ref()
                            .and_then(|progress| progress.last_flushed_sequence)
                            .map(host::DecimalU64::new),
                    )
                };
                let _ = send_generated_notification(
                    &tail.output,
                    host::HostNotificationParams::SubscriptionClosed(Box::new(
                        host::SubscriptionClosedNotificationParams {
                            last_flushed_cursor,
                            last_flushed_delivery_sequence,
                            reason,
                            subscription_id: tail.subscription_id,
                        },
                    )),
                )
                .await;
            }
            let _ = tail.stopped.send(());
        });
    }

    /// Release notifications only after the corresponding JSON-RPC response was flushed.
    pub fn activate_pending_subscriptions(&self) {
        if let Ok(subscriptions) = self.state.subscriptions.lock() {
            for subscription in subscriptions.values() {
                let _ = subscription.ready.send(true);
            }
        }
        if let Ok(mut activations) = self.state.pending_activations.lock() {
            for activation in activations.drain(..) {
                let _ = activation.send(());
            }
        }
    }
}

#[async_trait::async_trait]
impl host::HostServer for RpcConnection {
    type Context = ();

    async fn approval_decide(
        &self,
        _context: &Self::Context,
        params: host::ApprovalDecideParams,
    ) -> Result<host::ApprovalDecideResult, host::ApprovalDecideError> {
        self.approval_decide_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn approval_list(
        &self,
        _context: &Self::Context,
        params: host::InteractionListParams,
    ) -> Result<host::ApprovalListResult, host::ApprovalListError> {
        self.approval_list_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn approval_show(
        &self,
        _context: &Self::Context,
        params: host::ApprovalShowParams,
    ) -> Result<host::ApprovalShowResult, host::ApprovalShowError> {
        self.approval_show_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn catalog_list(
        &self,
        _context: &Self::Context,
        params: host::CatalogListParams,
    ) -> Result<host::CatalogListResult, host::CatalogListError> {
        self.catalog_list_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn clarification_resolve(
        &self,
        _context: &Self::Context,
        params: host::ClarificationResolveParams,
    ) -> Result<host::ClarificationResolveResult, host::ClarificationResolveError> {
        self.clarification_resolve_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn deferred_complete(
        &self,
        _context: &Self::Context,
        params: host::DeferredCompleteParams,
    ) -> Result<host::DeferredCompleteResult, host::DeferredCompleteError> {
        self.deferred_complete_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn deferred_fail(
        &self,
        _context: &Self::Context,
        params: host::DeferredFailParams,
    ) -> Result<host::DeferredFailResult, host::DeferredFailError> {
        self.deferred_fail_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn deferred_list(
        &self,
        _context: &Self::Context,
        params: host::InteractionListParams,
    ) -> Result<host::DeferredListResult, host::DeferredListError> {
        self.deferred_list_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn deferred_show(
        &self,
        _context: &Self::Context,
        params: host::DeferredShowParams,
    ) -> Result<host::DeferredShowResult, host::DeferredShowError> {
        self.deferred_show_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn diagnostics_get(
        &self,
        _context: &Self::Context,
        params: host::DiagnosticsGetParams,
    ) -> Result<host::DiagnosticsGetResult, host::DiagnosticsGetError> {
        self.diagnostics_generated(params).await.map_err(Into::into)
    }

    async fn environment_attach(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentAttachParams,
    ) -> Result<host::EnvironmentAttachResult, host::EnvironmentAttachError> {
        self.environment_attach_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn environment_detach(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentDetachParams,
    ) -> Result<host::EnvironmentDetachResult, host::EnvironmentDetachError> {
        self.environment_detach_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn environment_health(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentHealthParams,
    ) -> Result<host::EnvironmentHealthResult, host::EnvironmentHealthError> {
        self.environment_health_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn environment_list(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentListParams,
    ) -> Result<host::EnvironmentListResult, host::EnvironmentListError> {
        self.environment_list_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn environment_mount(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentMountParams,
    ) -> Result<host::EnvironmentMountResult, host::EnvironmentMountError> {
        self.environment_mount_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn environment_mounts_list(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentMountListParams,
    ) -> Result<host::EnvironmentMountListResult, host::EnvironmentMountsListError> {
        self.environment_mounts_list_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn environment_unmount(
        &self,
        _context: &Self::Context,
        params: host::EnvironmentUnmountParams,
    ) -> Result<host::EnvironmentUnmountResult, host::EnvironmentUnmountError> {
        self.environment_unmount_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn events_replay(
        &self,
        _context: &Self::Context,
        params: host::EventsReplayParams,
    ) -> Result<host::EventsReplayResult, host::EventsReplayError> {
        self.events_replay(params).await.map_err(Into::into)
    }

    async fn events_subscribe(
        &self,
        _context: &Self::Context,
        params: host::EventsSubscribeParams,
    ) -> Result<host::EventsSubscribeResult, host::EventsSubscribeError> {
        self.events_subscribe(params).await.map_err(Into::into)
    }

    async fn events_unsubscribe(
        &self,
        _context: &Self::Context,
        params: host::EventsUnsubscribeParams,
    ) -> Result<host::EventsUnsubscribeResult, host::EventsUnsubscribeError> {
        self.events_unsubscribe(params).await.map_err(Into::into)
    }

    async fn initialize(
        &self,
        _context: &Self::Context,
        params: host::InitializeParams,
    ) -> Result<host::InitializeResult, host::InitializeError> {
        self.initialize_generated(params).await.map_err(Into::into)
    }

    async fn model_select(
        &self,
        _context: &Self::Context,
        params: host::ModelSelectParams,
    ) -> Result<host::ModelSelectResult, host::ModelSelectError> {
        self.model_select_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn model_selection_get(
        &self,
        _context: &Self::Context,
        params: host::ModelSelectionGetParams,
    ) -> Result<host::ModelSelectionGetResult, host::ModelSelectionGetError> {
        self.model_selection_get_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn profile_get(
        &self,
        _context: &Self::Context,
        params: host::ProfileGetParams,
    ) -> Result<host::ProfileGetResult, host::ProfileGetError> {
        self.profile_get_generated(params).await.map_err(Into::into)
    }

    async fn run_interrupt(
        &self,
        _context: &Self::Context,
        params: host::RunInterruptParams,
    ) -> Result<host::RunInterruptResult, host::RunInterruptError> {
        self.run_interrupt_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn run_resume(
        &self,
        _context: &Self::Context,
        params: host::RunResumeParams,
    ) -> Result<host::RunResumeResult, host::RunResumeError> {
        self.run_resume_generated(params).await.map_err(Into::into)
    }

    async fn run_start(
        &self,
        _context: &Self::Context,
        params: host::RunStartParams,
    ) -> Result<host::RunStartResult, host::RunStartError> {
        self.run_start_generated(params).await.map_err(Into::into)
    }

    async fn run_status(
        &self,
        _context: &Self::Context,
        params: host::RunStatusParams,
    ) -> Result<host::RunStatusResult, host::RunStatusError> {
        self.run_status_generated(params).await.map_err(Into::into)
    }

    async fn run_steer(
        &self,
        _context: &Self::Context,
        params: host::RunSteerParams,
    ) -> Result<host::RunSteerResult, host::RunSteerError> {
        self.run_steer_generated(params).await.map_err(Into::into)
    }

    async fn session_create(
        &self,
        _context: &Self::Context,
        params: host::SessionCreateParams,
    ) -> Result<host::SessionCreateResult, host::SessionCreateError> {
        self.session_create_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn session_delete(
        &self,
        _context: &Self::Context,
        params: host::SessionDeleteParams,
    ) -> Result<host::SessionDeleteResult, host::SessionDeleteError> {
        self.session_delete_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn session_fork(
        &self,
        _context: &Self::Context,
        params: host::SessionForkParams,
    ) -> Result<host::SessionForkResult, host::SessionForkError> {
        self.session_fork_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn session_get(
        &self,
        _context: &Self::Context,
        params: host::SessionGetParams,
    ) -> Result<host::SessionGetResult, host::SessionGetError> {
        self.session_get_generated(params).await.map_err(Into::into)
    }

    async fn session_list(
        &self,
        _context: &Self::Context,
        params: host::SessionListParams,
    ) -> Result<host::SessionListResult, host::SessionListError> {
        self.session_list_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn session_search(
        &self,
        _context: &Self::Context,
        params: host::SessionSearchParams,
    ) -> Result<host::SessionSearchResult, host::SessionSearchError> {
        self.session_search_generated(params)
            .await
            .map_err(Into::into)
    }

    async fn shutdown(
        &self,
        _context: &Self::Context,
        params: host::ShutdownParams,
    ) -> Result<host::ShutdownResult, host::ShutdownError> {
        self.shutdown_generated(params).await.map_err(Into::into)
    }
}
fn generated_notification_value(
    params: host::HostNotificationParams,
) -> Result<Value, host::HostError> {
    let frame = host::encode_notification_frame(&host::HostNotification { params })
        .map_err(|_| internal_error("failed to encode generated notification", true))?;
    serde_json::from_slice(&frame)
        .map_err(|_| internal_error("generated notification was not valid JSON", true))
}

async fn send_generated_notification(
    output: &mpsc::Sender<RpcNotificationOutput>,
    params: host::HostNotificationParams,
) -> bool {
    let Ok(value) = generated_notification_value(params) else {
        return false;
    };
    let (flushed, flushed_receiver) = oneshot::channel();
    if output
        .send(RpcNotificationOutput { value, flushed })
        .await
        .is_err()
    {
        return false;
    }
    flushed_receiver.await.is_ok()
}

fn event_delivery_is_terminal(
    delivery: &host::EventDelivery,
    scope: &DurableHostEventScope,
) -> bool {
    if !matches!(scope, DurableHostEventScope::Run { .. }) {
        return false;
    }
    matches!(
        &delivery.record.event,
        host::HostEvent::RunChangedEvent(host::RunChangedEvent {
            run: host::RunSummary {
                status: host::RunStatus::Completed
                    | host::RunStatus::Failed
                    | host::RunStatus::Cancelled,
                ..
            },
            ..
        })
    )
}

async fn send_subscription_frame(
    output: &mpsc::Sender<RpcNotificationOutput>,
    cancel: &mut watch::Receiver<bool>,
    frame: Value,
) -> bool {
    if *cancel.borrow() {
        return false;
    }
    let (flushed, flushed_receiver) = oneshot::channel();
    let output_frame = RpcNotificationOutput {
        value: frame,
        flushed,
    };
    let sent = tokio::select! {
        result = output.send(output_frame) => result.is_ok(),
        changed = cancel.changed() => changed.is_ok() && !*cancel.borrow(),
    };
    if !sent {
        return false;
    }
    flushed_receiver.await.is_ok()
}

fn execute_on_runtime<T, F>(runtime: &Runtime, future: F) -> RpcHostResult<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    if RPC_RUNTIME_WORKER.with(Cell::get) {
        return Err(RpcHostError::Runtime(
            "blocking RPC service APIs cannot run on an RPC runtime worker".to_string(),
        ));
    }
    // Tokio never polls a spawned future synchronously, so the caller waits only
    // for completion and never carries the request state machine on its stack.
    let (result_sender, result_receiver) = std_mpsc::sync_channel(1);
    drop(runtime.spawn(async move {
        let result = future.await;
        let _ = result_sender.send(result);
    }));
    result_receiver.recv().map_err(|_| {
        RpcHostError::Runtime("RPC runtime task stopped before returning a result".to_string())
    })
}

fn runtime_failure_outcome(text: &str, _error: &RpcHostError) -> RpcFrameOutcome {
    let id = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|request| {
            request
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .and_then(|id| host::RequestId::new(id).ok());
    RpcFrameOutcome {
        response: Some(encoded_error_response_value(host::HostErrorResponse {
            id,
            error: internal_error("request execution failed", true),
        })),
        shutdown: false,
    }
}

fn encoded_response_value(response: host::HostResponse) -> Value {
    let id = response.id.as_str().to_string();
    host::encode_response_frame(&response)
        .ok()
        .and_then(|frame| serde_json::from_slice(&frame).ok())
        .unwrap_or_else(|| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": internal_error("response encoding failed", true),
            })
        })
}

fn encoded_error_response_value(response: host::HostErrorResponse) -> Value {
    let id = response.id.as_ref().map(host::RequestId::as_str);
    host::encode_error_response_frame(&response)
        .ok()
        .and_then(|frame| serde_json::from_slice(&frame).ok())
        .unwrap_or_else(|| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": internal_error("error response encoding failed", true),
            })
        })
}

fn mutation_fingerprint(
    operation: &str,
    params: &impl Serialize,
) -> Result<String, host::HostError> {
    let payload = serde_json::to_vec(&(operation, params))
        .map_err(|_| invalid_params_error("failed to canonicalize mutation parameters"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(payload)))
}

fn mutation_transition_identity(
    operation: &str,
    idempotency_key: &str,
    fingerprint: &str,
) -> String {
    let digest = Sha256::digest(
        serde_json::to_vec(&(operation, idempotency_key, fingerprint))
            .unwrap_or_else(|_| Vec::new()),
    );
    format!("receipt-sha256:{digest:x}")
}

fn authority_scoped_idempotency_key(authority_identity: &str, idempotency_key: &str) -> String {
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "starweaver.host.authority-idempotency.v1",
            authority_identity,
            idempotency_key,
        ))
        .unwrap_or_default(),
    );
    format!("authority-sha256:{digest:x}")
}

fn deterministic_session_id(authority_identity: &str, idempotency_key: &str) -> SessionId {
    let digest = Sha256::digest(
        serde_json::to_vec(&("session.create", authority_identity, idempotency_key))
            .unwrap_or_else(|_| Vec::new()),
    );
    SessionId::from_string(format!("session_{digest:x}"))
}

fn deterministic_fork_session_id(authority_identity: &str, idempotency_key: &str) -> SessionId {
    let digest = Sha256::digest(
        serde_json::to_vec(&("session.fork", authority_identity, idempotency_key))
            .unwrap_or_else(|_| Vec::new()),
    );
    SessionId::from_string(format!("session_{digest:x}"))
}

fn deterministic_run_control_id(
    operation: &str,
    authority_identity: &str,
    idempotency_key: &str,
) -> String {
    let digest = Sha256::digest(
        serde_json::to_vec(&(operation, authority_identity, idempotency_key))
            .unwrap_or_else(|_| Vec::new()),
    );
    format!("{operation}-sha256:{digest:x}")
}

fn deterministic_deletion_fence_id(
    authority_identity: &str,
    idempotency_key: &str,
    fingerprint: &str,
) -> String {
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "session.delete",
            authority_identity,
            idempotency_key,
            fingerprint,
        ))
        .unwrap_or_else(|_| Vec::new()),
    );
    format!("rpc-delete-sha256:{digest:x}")
}

fn fork_lineage(
    session: &SessionRecord,
) -> Result<(host::SessionId, Option<host::RunId>), host::HostError> {
    let lineage = session
        .metadata
        .get("rpc.fork")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            internal_error("forked session is missing durable lineage evidence", true)
        })?;
    let source_session_id = lineage
        .get("source_session_id")
        .and_then(Value::as_str)
        .and_then(|value| host::SessionId::new(value).ok())
        .ok_or_else(|| internal_error("forked session has invalid source session lineage", true))?;
    let source_run_id = lineage
        .get("source_run_id")
        .and_then(Value::as_str)
        .map(host::RunId::new)
        .transpose()
        .map_err(|_| internal_error("forked session has invalid source run lineage", true))?;
    Ok((source_session_id, source_run_id))
}

fn generated_environment_scope(
    scope: &DurableEnvironmentScope,
) -> Result<host::AttachmentScope, host::HostError> {
    match scope {
        DurableEnvironmentScope::Connection { .. } => Ok(
            host::AttachmentScope::ConnectionAttachmentScope(host::ConnectionAttachmentScope {
                kind: host::ConnectionAttachmentScopeKind::Value,
            }),
        ),
        DurableEnvironmentScope::Session { session_id } => Ok(
            host::AttachmentScope::SessionAttachmentScope(host::SessionAttachmentScope {
                kind: host::SessionAttachmentScopeKind::Value,
                session_id: host::SessionId::new(session_id).map_err(|_| {
                    internal_error("environment session scope violated protocol schema", true)
                })?,
            }),
        ),
        DurableEnvironmentScope::Run { session_id, run_id } => Ok(
            host::AttachmentScope::RunAttachmentScope(host::RunAttachmentScope {
                kind: host::RunAttachmentScopeKind::Value,
                run_id: host::RunId::new(run_id).map_err(|_| {
                    internal_error("environment run scope violated protocol schema", true)
                })?,
                session_id: host::SessionId::new(session_id).map_err(|_| {
                    internal_error("environment session scope violated protocol schema", true)
                })?,
            }),
        ),
    }
}

fn generated_environment_attachment(
    attachment: &DurableEnvironmentAttachment,
) -> Result<host::EnvironmentAttachment, host::HostError> {
    let status = match attachment.status {
        DurableEnvironmentStatus::Attaching => host::EnvironmentStatus::Attaching,
        DurableEnvironmentStatus::Ready => host::EnvironmentStatus::Ready,
        DurableEnvironmentStatus::Degraded => host::EnvironmentStatus::Degraded,
        DurableEnvironmentStatus::Detached => host::EnvironmentStatus::Detached,
    };
    Ok(host::EnvironmentAttachment {
        attachment_id: host::AttachmentId::new(&attachment.attachment_id).map_err(|_| {
            internal_error(
                "environment attachment identity violated protocol schema",
                true,
            )
        })?,
        display_name: attachment.display_name.clone(),
        environment_id: host::EnvironmentId::new(&attachment.environment_id).map_err(|_| {
            internal_error(
                "environment catalog identity violated protocol schema",
                true,
            )
        })?,
        revision: host::DecimalU64::new(attachment.revision),
        scope: generated_environment_scope(&attachment.scope)?,
        status,
    })
}

fn environment_event_scope(scope: &DurableEnvironmentScope) -> DurableHostEventScope {
    match scope {
        DurableEnvironmentScope::Connection { .. } => DurableHostEventScope::Global,
        DurableEnvironmentScope::Session { session_id } => {
            DurableHostEventScope::session(SessionId::from_string(session_id))
        }
        DurableEnvironmentScope::Run { session_id, run_id } => DurableHostEventScope::run(
            SessionId::from_string(session_id),
            RunId::from_string(run_id),
        ),
    }
}

fn deterministic_environment_id(kind: &str, authority_identity: &str, key: &str) -> String {
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "starweaver.host.environment.v1",
            kind,
            authority_identity,
            key,
        ))
        .unwrap_or_default(),
    );
    format!("{kind}_{digest:x}")
}

fn environment_transition_identity(authority_identity: &str, key: &str) -> String {
    mutation_transition_identity("environment.mutation", key, authority_identity)
}

fn configured_environment_attachment_ref(
    config: &RpcConfig,
    attachment: &DurableEnvironmentAttachment,
    is_default: bool,
) -> Result<EnvironmentAttachmentRef, host::HostError> {
    let (kind, endpoint_ref, environment_id, auth_token) = match config
        .resolve_environment_source(&attachment.environment_id)
        .map_err(rpc_host_to_generated_error)?
    {
        crate::ResolvedRpcEnvironmentSource::Local { .. } => {
            ("local".to_string(), None, None, None)
        }
        crate::ResolvedRpcEnvironmentSource::Envd {
            endpoint_ref,
            environment_id,
            auth_token,
        } => (
            "envd".to_string(),
            Some(endpoint_ref),
            Some(environment_id),
            auth_token,
        ),
    };
    Ok(EnvironmentAttachmentRef {
        id: attachment.attachment_id.clone(),
        kind,
        mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
        is_default,
        is_default_for_shell: is_default,
        endpoint_ref,
        environment_id,
        auth_token,
        metadata: serde_json::Map::new(),
    })
}

fn session_changed_publication(
    transition_identity: &str,
    session: &SessionRecord,
) -> Result<PendingHostEventPublication, host::HostError> {
    let projection = serde_json::to_value(host::SessionChangedEvent {
        kind: host::SessionChangedEventKind::Value,
        session: session_summary(session)?,
    })
    .map_err(|_| internal_error("failed to project session event", true))?;
    PendingHostEventPublication::new(
        transition_identity,
        0,
        DurableHostEventScope::session(session.session_id.clone()),
        DurableHostEventClass::SessionChanged,
        projection,
        session.updated_at,
    )
    .map_err(session_store_to_generated_error)
}

fn approval_changed_publication(
    transition_identity: &str,
    approval: &ApprovalRecord,
) -> Result<PendingHostEventPublication, host::HostError> {
    let projection = serde_json::to_value(host::ApprovalChangedEvent {
        approval: approval_summary(approval)?,
        kind: host::ApprovalChangedEventKind::Value,
    })
    .map_err(|_| internal_error("failed to project approval event", true))?;
    PendingHostEventPublication::new(
        transition_identity,
        0,
        DurableHostEventScope::run(approval.session_id.clone(), approval.run_id.clone()),
        DurableHostEventClass::ApprovalChanged,
        projection,
        approval.updated_at,
    )
    .map_err(session_store_to_generated_error)
}

fn deferred_changed_publication(
    transition_identity: &str,
    deferred: &DeferredToolRecord,
) -> Result<PendingHostEventPublication, host::HostError> {
    let projection = serde_json::to_value(host::DeferredChangedEvent {
        deferred: deferred_summary(deferred)?,
        kind: host::DeferredChangedEventKind::Value,
    })
    .map_err(|_| internal_error("failed to project deferred event", true))?;
    PendingHostEventPublication::new(
        transition_identity,
        0,
        DurableHostEventScope::run(deferred.session_id.clone(), deferred.run_id.clone()),
        DurableHostEventClass::DeferredChanged,
        projection,
        deferred.updated_at,
    )
    .map_err(session_store_to_generated_error)
}

fn clarification_changed_publication(
    transition_identity: &str,
    session_id: &SessionId,
    run_id: &RunId,
    clarification: &host::ClarificationSummary,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> Result<PendingHostEventPublication, host::HostError> {
    let projection = serde_json::to_value(host::ClarificationChangedEvent {
        clarification: clarification.clone(),
        kind: host::ClarificationChangedEventKind::Value,
    })
    .map_err(|_| internal_error("failed to project clarification event", true))?;
    PendingHostEventPublication::new(
        transition_identity,
        0,
        DurableHostEventScope::run(session_id.clone(), run_id.clone()),
        DurableHostEventClass::ClarificationChanged,
        projection,
        occurred_at,
    )
    .map_err(session_store_to_generated_error)
}

fn clarification_summary(
    approval: &ApprovalRecord,
    questions: &[starweaver_session::ClarificationQuestion],
    status: host::ClarificationStatus,
    revision: u64,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> Result<host::ClarificationSummary, host::HostError> {
    Ok(host::ClarificationSummary {
        clarification_id: host::ClarificationId::new(&approval.approval_id)
            .map_err(|_| internal_error("clarification identity violated protocol schema", true))?,
        questions: questions
            .iter()
            .map(|question| {
                Ok(host::ClarificationQuestion {
                    header: question.header.clone(),
                    multi_select: question.multi_select,
                    options: question
                        .options
                        .iter()
                        .map(|option| host::ClarificationQuestionOption {
                            description: option.description.clone(),
                            label: option.label.clone(),
                            preview: option.preview.clone(),
                        })
                        .collect(),
                    question: question.question.clone(),
                })
            })
            .collect::<Result<Vec<_>, host::HostError>>()?,
        revision: host::DecimalU64::new(revision),
        run_id: host::RunId::new(approval.run_id.as_str())
            .map_err(|_| internal_error("run identity violated protocol schema", true))?,
        session_id: host::SessionId::new(approval.session_id.as_str())
            .map_err(|_| internal_error("session identity violated protocol schema", true))?,
        status,
        updated_at: generated_timestamp(updated_at)?,
    })
}

fn generated_model_selection(
    selection: &starweaver_session::DurableModelSelection,
) -> Result<host::ModelSelection, host::HostError> {
    Ok(host::ModelSelection {
        model_id: selection.model_id.clone(),
        revision: host::DecimalU64::new(selection.revision),
        selected_profile: selection.selected_profile.clone(),
    })
}

fn generated_mutation_receipt(
    receipt: &starweaver_session::MutationReceipt,
) -> Result<host::MutationReceipt, host::HostError> {
    let operation = if receipt.operation == starweaver_session::MODEL_SELECTION_OPERATION {
        "model.select"
    } else {
        &receipt.operation
    };
    Ok(host::MutationReceipt {
        fingerprint: host::SchemaDigest::new(&receipt.fingerprint)
            .map_err(|_| internal_error("mutation fingerprint violated protocol schema", true))?,
        idempotency_key: host::IdempotencyKey::new(&receipt.idempotency_key)
            .map_err(|_| internal_error("idempotency key violated protocol schema", true))?,
        operation: operation.to_string(),
        receipt_id: host::ReceiptId::new(&receipt.receipt_id).map_err(|_| {
            internal_error("mutation receipt identity violated protocol schema", true)
        })?,
        reconciliation_required: receipt.reconciliation_required,
        replayed: receipt.replayed,
        state: receipt.state.clone(),
        target_ref: receipt.target_ref.clone(),
    })
}

fn durable_wire_receipt(
    receipt_id: &str,
    idempotency_key: &host::IdempotencyKey,
    fingerprint: &str,
    operation: &str,
    state: &str,
    target_ref: &str,
    replayed: bool,
    reconciliation_required: bool,
) -> Result<host::MutationReceipt, host::HostError> {
    Ok(host::MutationReceipt {
        fingerprint: host::SchemaDigest::new(fingerprint)
            .map_err(|_| internal_error("mutation fingerprint violated protocol schema", true))?,
        idempotency_key: idempotency_key.clone(),
        operation: operation.to_string(),
        receipt_id: host::ReceiptId::new(receipt_id).map_err(|_| {
            internal_error("mutation receipt identity violated protocol schema", true)
        })?,
        reconciliation_required,
        replayed,
        state: state.to_string(),
        target_ref: target_ref.to_string(),
    })
}

fn mutation_receipt(
    operation: &str,
    idempotency_key: &host::IdempotencyKey,
    fingerprint: &str,
    target_ref: &str,
    state: &str,
    replayed: bool,
    reconciliation_required: bool,
) -> Result<host::MutationReceipt, host::HostError> {
    let receipt_digest = Sha256::digest(
        serde_json::to_vec(&(operation, idempotency_key.as_str(), fingerprint, target_ref))
            .map_err(|_| internal_error("failed to derive mutation receipt identity", true))?,
    );
    Ok(host::MutationReceipt {
        fingerprint: host::SchemaDigest::new(fingerprint)
            .map_err(|_| internal_error("mutation fingerprint violated protocol schema", true))?,
        idempotency_key: idempotency_key.clone(),
        operation: operation.to_string(),
        receipt_id: host::ReceiptId::new(format!("receipt-sha256:{receipt_digest:x}")).map_err(
            |_| internal_error("mutation receipt identity violated protocol schema", true),
        )?,
        reconciliation_required,
        replayed,
        state: state.to_string(),
        target_ref: target_ref.to_string(),
    })
}

fn legacy_deferred_tool(
    tool: &host::DeferredToolDefinition,
) -> Result<LegacyDeferredToolDefinition, host::HostError> {
    let canonical = canonical_json(&tool.input_schema).map_err(rpc_error_to_host_error)?;
    let digest = format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()));
    if digest != tool.input_schema_digest.as_str() {
        return Err(invalid_params_error(
            "deferred tool inputSchemaDigest does not match inputSchema",
        ));
    }
    Ok(LegacyDeferredToolDefinition {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
        instructions: tool.instructions.clone(),
    })
}

fn rpc_host_to_generated_error(error: RpcHostError) -> host::HostError {
    rpc_error_to_host_error(rpc_error(error))
}

fn session_store_to_generated_error(
    error: starweaver_session::SessionStoreError,
) -> host::HostError {
    rpc_host_to_generated_error(error.into())
}

fn invalid_params_error(message: &str) -> host::HostError {
    host::HostError {
        code: -32_602,
        message: message.to_string(),
        data: host::HostErrorData::InvalidParams(host::InvalidParamsData {
            diagnostic_ref: None,
            kind: host::InvalidParamsDataKind::Value,
            reconciliation_required: false,
            resource_kind: None,
            retryable: false,
        }),
    }
}

fn generated_timestamp(
    value: chrono::DateTime<chrono::Utc>,
) -> Result<host::Timestamp, host::HostError> {
    host::Timestamp::new(value.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
        .map_err(|_| internal_error("durable timestamp violated protocol schema", true))
}

fn session_summary(session: &SessionRecord) -> Result<host::SessionSummary, host::HostError> {
    let status = match session.status {
        SessionStatus::Active => host::SessionStatus::Active,
        SessionStatus::Archived => host::SessionStatus::Archived,
        SessionStatus::Failed => host::SessionStatus::Failed,
        SessionStatus::Deleted => host::SessionStatus::Deleted,
    };
    Ok(host::SessionSummary {
        created_at: generated_timestamp(session.created_at)?,
        profile: session.profile.clone(),
        revision: host::DecimalU64::new(session.revision),
        session_id: host::SessionId::new(session.session_id.as_str()).map_err(|_| {
            internal_error("durable session identity violated protocol schema", true)
        })?,
        status,
        title: session.title.clone(),
        updated_at: generated_timestamp(session.updated_at)?,
        workspace_label: session.workspace.clone(),
    })
}

fn run_summary(run: &RunRecord) -> Result<host::RunSummary, host::HostError> {
    let status = match run.status.as_str() {
        "queued" => host::RunStatus::Queued,
        "starting" => host::RunStatus::Starting,
        "running" => host::RunStatus::Running,
        "waiting" => host::RunStatus::Waiting,
        "completed" => host::RunStatus::Completed,
        "failed" => host::RunStatus::Failed,
        "cancelled" => host::RunStatus::Cancelled,
        _ => return Err(internal_error("unknown durable run status", true)),
    };
    Ok(host::RunSummary {
        created_at: generated_timestamp(run.created_at)?,
        diagnostic_ref: run.terminal_error.as_ref().map(|error| error.code.clone()),
        output_preview: run.output_preview.clone(),
        revision: host::DecimalU64::new(run.revision),
        run_id: host::RunId::new(run.run_id.as_str())
            .map_err(|_| internal_error("durable run identity violated protocol schema", true))?,
        session_id: host::SessionId::new(run.session_id.as_str()).map_err(|_| {
            internal_error("durable session identity violated protocol schema", true)
        })?,
        status,
        updated_at: generated_timestamp(run.updated_at)?,
    })
}

fn approval_summary(approval: &ApprovalRecord) -> Result<host::ApprovalSummary, host::HostError> {
    let status = match approval.status {
        ApprovalStatus::Pending => host::ApprovalStatus::Pending,
        ApprovalStatus::Approved => host::ApprovalStatus::Approved,
        ApprovalStatus::Denied => host::ApprovalStatus::Denied,
        ApprovalStatus::Expired => host::ApprovalStatus::Expired,
        ApprovalStatus::Cancelled => host::ApprovalStatus::Cancelled,
    };
    Ok(host::ApprovalSummary {
        approval_id: host::ApprovalId::new(&approval.approval_id).map_err(|_| {
            internal_error("durable approval identity violated protocol schema", true)
        })?,
        revision: host::DecimalU64::new(approval.revision),
        run_id: host::RunId::new(approval.run_id.as_str())
            .map_err(|_| internal_error("durable run identity violated protocol schema", true))?,
        session_id: host::SessionId::new(approval.session_id.as_str()).map_err(|_| {
            internal_error("durable session identity violated protocol schema", true)
        })?,
        status,
        title: approval.action_name.clone(),
        updated_at: generated_timestamp(approval.updated_at)?,
    })
}

fn deferred_summary(
    deferred: &DeferredToolRecord,
) -> Result<host::DeferredSummary, host::HostError> {
    let status = match deferred.status {
        ExecutionStatus::Pending => host::DeferredStatus::Pending,
        ExecutionStatus::Running => host::DeferredStatus::Running,
        ExecutionStatus::Waiting => host::DeferredStatus::Waiting,
        ExecutionStatus::Completed => host::DeferredStatus::Completed,
        ExecutionStatus::Failed => host::DeferredStatus::Failed,
        ExecutionStatus::Cancelled => host::DeferredStatus::Cancelled,
    };
    Ok(host::DeferredSummary {
        deferred_id: host::DeferredId::new(&deferred.deferred_id).map_err(|_| {
            internal_error("durable deferred identity violated protocol schema", true)
        })?,
        revision: host::DecimalU64::new(deferred.revision),
        run_id: host::RunId::new(deferred.run_id.as_str())
            .map_err(|_| internal_error("durable run identity violated protocol schema", true))?,
        session_id: host::SessionId::new(deferred.session_id.as_str()).map_err(|_| {
            internal_error("durable session identity violated protocol schema", true)
        })?,
        status,
        tool_name: deferred.tool_name.clone(),
        updated_at: generated_timestamp(deferred.updated_at)?,
    })
}

fn session_cursor_position(key: &SessionPageKey) -> SessionCursorPosition {
    SessionCursorPosition {
        updated_at: key
            .updated_at
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        session_id: key.session_id.as_str().to_string(),
    }
}

fn session_page_key(position: SessionCursorPosition) -> Result<SessionPageKey, host::HostError> {
    Ok(SessionPageKey {
        updated_at: parse_cursor_timestamp(&position.updated_at)?,
        session_id: SessionId::from_string(position.session_id),
    })
}

fn interaction_cursor_position(key: &InteractionPageKey) -> InteractionCursorPosition {
    InteractionCursorPosition {
        updated_at: key
            .updated_at
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        interaction_id: key.interaction_id.clone(),
    }
}

fn interaction_page_key(
    position: InteractionCursorPosition,
) -> Result<InteractionPageKey, host::HostError> {
    Ok(InteractionPageKey {
        updated_at: parse_cursor_timestamp(&position.updated_at)?,
        interaction_id: position.interaction_id,
    })
}

fn environment_page_key(
    position: EnvironmentCursorPosition,
) -> Result<EnvironmentAttachmentPageKey, host::HostError> {
    Ok(EnvironmentAttachmentPageKey {
        updated_at: parse_cursor_timestamp(&position.updated_at)?,
        attachment_id: position.attachment_id,
    })
}

fn parse_cursor_timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>, host::HostError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&chrono::Utc))
        .map_err(|_| cursor_invalid_error(CursorAdmissionError::Malformed))
}

fn not_initialized_error() -> host::HostError {
    host::HostError {
        code: -32_001,
        message: "host protocol initialize must succeed before calling other methods".to_string(),
        data: host::HostErrorData::NotInitialized(host::NotInitializedData {
            diagnostic_ref: None,
            kind: host::NotInitializedDataKind::Value,
            reconciliation_required: false,
            resource_kind: None,
            retryable: false,
        }),
    }
}

fn internal_error(message: &str, reconciliation_required: bool) -> host::HostError {
    host::HostError {
        code: -32_000,
        message: message.to_string(),
        data: host::HostErrorData::InternalError(host::InternalErrorData {
            diagnostic_ref: None,
            kind: host::InternalErrorDataKind::Value,
            reconciliation_required,
            resource_kind: None,
            retryable: false,
        }),
    }
}

fn cursor_invalid_error(reason: CursorAdmissionError) -> host::HostError {
    let reason = match reason {
        CursorAdmissionError::Malformed => host::CursorInvalidReason::Malformed,
        CursorAdmissionError::IntegrityFailed => host::CursorInvalidReason::IntegrityFailed,
        CursorAdmissionError::ScopeMismatch => host::CursorInvalidReason::ScopeMismatch,
        CursorAdmissionError::ViewMismatch => host::CursorInvalidReason::ViewMismatch,
        CursorAdmissionError::StorageMismatch => host::CursorInvalidReason::StorageMismatch,
    };
    host::HostError {
        code: -32_016,
        message: "cursor is invalid for the requested event view".to_string(),
        data: host::HostErrorData::CursorInvalid(host::CursorInvalidData {
            diagnostic_ref: None,
            kind: host::CursorInvalidDataKind::Value,
            reason,
            reconciliation_required: false,
            resource_kind: None,
            retryable: false,
        }),
    }
}

fn not_found_error(message: &str) -> host::HostError {
    rpc_error_to_host_error(RpcError::new(-32_010, message))
}

fn authorization_denied_error(message: &str) -> host::HostError {
    rpc_error_to_host_error(RpcError::new(-32_017, message))
}

fn already_exists_error(message: &str) -> host::HostError {
    host::HostError {
        code: -32_011,
        message: message.to_string(),
        data: host::HostErrorData::AlreadyExists(host::AlreadyExistsData {
            diagnostic_ref: None,
            kind: host::AlreadyExistsDataKind::Value,
            reconciliation_required: false,
            resource_kind: Some("subscription".to_string()),
            retryable: false,
        }),
    }
}

fn storage_unavailable_error() -> host::HostError {
    host::HostError {
        code: -32_015,
        message: "storage unavailable".to_string(),
        data: host::HostErrorData::StorageUnavailable(host::StorageUnavailableData {
            diagnostic_ref: None,
            kind: host::StorageUnavailableDataKind::Value,
            reconciliation_required: true,
            resource_kind: None,
            retryable: true,
        }),
    }
}

fn unsupported_feature_error(message: &str) -> host::HostError {
    host::HostError {
        code: -32_002,
        message: message.to_string(),
        data: host::HostErrorData::UnsupportedFeature(host::UnsupportedFeatureData {
            diagnostic_ref: None,
            kind: host::UnsupportedFeatureDataKind::Value,
            reconciliation_required: false,
            resource_kind: None,
            retryable: false,
        }),
    }
}

fn generated_protocol_identity() -> Result<host::ProtocolIdentity, host::HostError> {
    Ok(host::ProtocolIdentity {
        major: host::PROTOCOL_MAJOR,
        name: host::ProtocolIdentityName::Value,
        revision: host::PROTOCOL_REVISION.to_string(),
        schema_digest: host::SchemaDigest::new(host::SCHEMA_DIGEST)
            .map_err(|_| internal_error("generated protocol digest is invalid", true))?,
    })
}

fn durable_event_scope(scope: &host::ResourceScope) -> DurableHostEventScope {
    match scope {
        host::ResourceScope::GlobalResourceScope(_) => DurableHostEventScope::Global,
        host::ResourceScope::SessionResourceScope(scope) => {
            DurableHostEventScope::session(SessionId::from_string(scope.session_id.as_str()))
        }
        host::ResourceScope::RunResourceScope(scope) => DurableHostEventScope::run(
            SessionId::from_string(scope.session_id.as_str()),
            RunId::from_string(scope.run_id.as_str()),
        ),
    }
}

fn durable_event_classes(profile: host::EventProfile) -> Vec<DurableHostEventClass> {
    profile
        .metadata()
        .event_classes
        .iter()
        .map(|event_class| match *event_class {
            host::EventClass::SessionChanged => DurableHostEventClass::SessionChanged,
            host::EventClass::RunChanged => DurableHostEventClass::RunChanged,
            host::EventClass::OutputAvailable => DurableHostEventClass::OutputAvailable,
            host::EventClass::ApprovalChanged => DurableHostEventClass::ApprovalChanged,
            host::EventClass::DeferredChanged => DurableHostEventClass::DeferredChanged,
            host::EventClass::ClarificationChanged => DurableHostEventClass::ClarificationChanged,
            host::EventClass::EnvironmentChanged => DurableHostEventClass::EnvironmentChanged,
            host::EventClass::Diagnostic => DurableHostEventClass::Diagnostic,
        })
        .collect()
}

fn generated_event_scope(
    scope: DurableHostEventScope,
) -> Result<host::ResourceScope, host::HostError> {
    match scope {
        DurableHostEventScope::Global => Ok(host::ResourceScope::GlobalResourceScope(
            host::GlobalResourceScope {
                kind: host::GlobalResourceScopeKind::Value,
            },
        )),
        DurableHostEventScope::Session { session_id } => Ok(
            host::ResourceScope::SessionResourceScope(host::SessionResourceScope {
                kind: host::SessionResourceScopeKind::Value,
                session_id: host::SessionId::new(session_id.as_str()).map_err(|_| {
                    internal_error("durable session identity violated protocol schema", true)
                })?,
            }),
        ),
        DurableHostEventScope::Run { session_id, run_id } => Ok(
            host::ResourceScope::RunResourceScope(host::RunResourceScope {
                kind: host::RunResourceScopeKind::Value,
                session_id: host::SessionId::new(session_id.as_str()).map_err(|_| {
                    internal_error("durable session identity violated protocol schema", true)
                })?,
                run_id: host::RunId::new(run_id.as_str()).map_err(|_| {
                    internal_error("durable run identity violated protocol schema", true)
                })?,
            }),
        ),
    }
}

fn rpc_error_to_host_error(error: RpcError) -> host::HostError {
    let (code, kind, message, retryable, reconciliation_required) = match error.code {
        -32_602 => (-32_602, "invalid_params", "invalid params", false, false),
        -32_601 => (
            -32_601,
            "method_not_found",
            "method not found",
            false,
            false,
        ),
        -32_001 => (-32_001, "not_initialized", "not initialized", false, false),
        -32_002 => (
            -32_002,
            "unsupported_feature",
            "unsupported feature",
            false,
            false,
        ),
        -32_010 => (-32_010, "not_found", "resource not found", false, false),
        -32_011 => (
            -32_011,
            "already_exists",
            "resource already exists",
            false,
            false,
        ),
        -32_012 => (
            -32_012,
            "idempotency_conflict",
            "idempotency conflict",
            false,
            true,
        ),
        -32_013 => (-32_013, "run_conflict", "run conflict", false, true),
        -32_014 => (-32_014, "stale_fence", "stale fence", false, true),
        -32_015 => (
            -32_015,
            "storage_unavailable",
            "storage unavailable",
            true,
            true,
        ),
        -32_017 => (
            -32_017,
            "authorization_denied",
            "authorization denied",
            false,
            false,
        ),
        -32_031 => (
            -32_031,
            "environment_unavailable",
            "environment unavailable",
            true,
            true,
        ),
        -32_032 => (
            -32_032,
            "session_search_unavailable",
            "session search unavailable",
            true,
            false,
        ),
        -32_050 => (
            -32_050,
            "configuration_failed",
            "configuration failed",
            false,
            false,
        ),
        _ => return internal_error("internal error", true),
    };
    serde_json::from_value(json!({
        "code": code,
        "message": message,
        "data": {
            "kind": kind,
            "retryable": retryable,
            "reconciliationRequired": reconciliation_required,
        },
    }))
    .unwrap_or_else(|_| internal_error("internal error", true))
}

impl RpcService {
    /// Construct a service for stdio/live transports.
    ///
    /// # Errors
    ///
    /// Returns storage or Tokio runtime initialization errors.
    pub fn live(config: RpcConfig) -> Result<Self, RpcHostError> {
        Self::new(config, RpcNotificationMode::Live)
    }

    /// Construct a service for unary replay-only transports.
    ///
    /// # Errors
    ///
    /// Returns storage or Tokio runtime initialization errors.
    pub fn replay_only(config: RpcConfig) -> Result<Self, RpcHostError> {
        Self::new(config, RpcNotificationMode::ReplayOnly)
    }

    fn new(config: RpcConfig, notifications: RpcNotificationMode) -> Result<Self, RpcHostError> {
        if let Some(parent) = config.database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&config.workspace_root)?;
        let storage = SqliteStorage::open(&config.database_path)?;
        let catalog = RpcAgentCatalog::new(config.clone())?;
        let environment_manager = EnvironmentAttachmentManager::new();
        let coordinator = Arc::new(RpcRuntimeCoordinator::new(
            config.clone(),
            catalog.clone(),
            storage.clone(),
            environment_manager,
        ));
        let session_search_scope =
            SessionSearchScope::local(config.database_path.to_string_lossy().into_owned());
        let session_search = if config.session_search.enabled {
            let limits = LocalSessionSearchLimits {
                max_query_bytes: config.session_search.max_query_bytes,
                max_page_size: config.session_search.max_page_size,
                max_display_files: config.session_search.max_display_files,
                max_total_display_bytes: config.session_search.max_total_display_bytes,
                max_display_hits: config.session_search.max_display_hits,
                max_scan_duration: Duration::from_millis(config.session_search.scan_timeout_ms),
                ..LocalSessionSearchLimits::default()
            };
            let mut provider = LocalSessionSearchProvider::new(
                Arc::new(storage.session_store()),
                &session_search_scope,
            )
            .with_limits(limits);
            if let Some(root) = config.session_search.display_root.as_ref() {
                provider = provider.with_display_root(root.clone());
            }
            Some(Arc::new(provider) as Arc<dyn SessionSearchProvider>)
        } else {
            None
        };
        let runtime = RuntimeBuilder::new_multi_thread()
            .enable_all()
            .thread_name("starweaver-rpc-runtime")
            .thread_stack_size(RPC_RUNTIME_THREAD_STACK_SIZE)
            .on_thread_start(|| RPC_RUNTIME_WORKER.with(|worker| worker.set(true)))
            .on_thread_stop(|| RPC_RUNTIME_WORKER.with(|worker| worker.set(false)))
            .build()
            .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let runtime = Arc::new(RpcExecutionRuntime::new(runtime));
        let startup_coordinator = Arc::clone(&coordinator);
        let startup_store = storage.session_store();
        let startup_repaired_runs = execute_on_runtime(&runtime, async move {
            let reconciled = startup_coordinator.reconcile_startup().await?;
            loop {
                let materialized = startup_store
                    .materialize_host_event_publications(500)
                    .await?;
                if materialized.len() < 500 {
                    break;
                }
            }
            Ok::<u64, RpcHostError>(u64::try_from(reconciled.len()).unwrap_or(u64::MAX))
        })??;
        let storage_identity = std::fs::canonicalize(&config.database_path)
            .unwrap_or_else(|_| config.database_path.clone())
            .to_string_lossy()
            .into_owned();
        let cursor_codec = HostCursorCodec::load_or_create(&config.state_dir, &storage_identity)?;
        Ok(Self {
            config: Arc::new(config),
            catalog: Arc::new(catalog),
            storage,
            coordinator,
            environment_manager,
            session_search,
            session_search_scope,
            notifications,
            cursor_codec,
            runtime,
            startup_repaired_runs,
        })
    }

    async fn drain_host_event_outbox(&self) -> Result<(), host::HostError> {
        let store = self.storage.session_store();
        loop {
            let materialized = store
                .materialize_host_event_publications(500)
                .await
                .map_err(|_| storage_unavailable_error())?;
            if materialized.len() < 500 {
                return Ok(());
            }
        }
    }

    pub(crate) fn shutdown_owned_runtime(&self, timeout: Duration) -> RpcHostResult<()> {
        let coordinator = self.coordinator.clone();
        execute_on_runtime(
            &self.runtime,
            async move { coordinator.shutdown(timeout).await },
        )?
    }

    /// Open an uninitialized stdio connection with a bounded notification sink.
    #[must_use]
    pub(crate) fn live_connection(
        &self,
        output: mpsc::Sender<RpcNotificationOutput>,
    ) -> RpcConnection {
        self.new_connection(
            Some(output),
            "local-stdio",
            host::Transport::Stdio,
            all_connection_scopes(),
        )
    }

    fn new_connection(
        &self,
        output: Option<mpsc::Sender<RpcNotificationOutput>>,
        authority_identity: &str,
        transport: host::Transport,
        scopes: BTreeSet<String>,
    ) -> RpcConnection {
        RpcConnection {
            service: self.clone(),
            state: Arc::new(RpcConnectionState {
                initialized: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                cleanup_completed: AtomicBool::new(false),
                connection_id: format!("connection_{}", Uuid::new_v4()),
                authority_identity: authority_identity.to_string(),
                transport,
                scopes,
                output,
                subscriptions: Arc::new(Mutex::new(HashMap::new())),
                pending_activations: Mutex::new(Vec::new()),
                negotiated_features: Mutex::new(BTreeSet::new()),
                environment_manager: self.environment_manager,
                storage: self.storage.clone(),
            }),
        }
    }

    pub(crate) fn handle_text_for_authority(
        &self,
        text: &str,
        authority_identity: &str,
        scopes: BTreeSet<String>,
    ) -> RpcFrameOutcome {
        let connection =
            self.new_connection(None, authority_identity, host::Transport::Http, scopes);
        connection.state.initialized.store(true, Ordering::Release);
        if let Ok(mut negotiated) = connection.state.negotiated_features.lock() {
            *negotiated = connection.supported_features();
        }
        connection.handle_text(text)
    }
}

fn all_connection_scopes() -> BTreeSet<String> {
    ["read", "run", "approval", "admin", "shutdown"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn generated_continuation_mode(mode: host::ContinuationMode) -> ContinuationMaterializationMode {
    match mode {
        host::ContinuationMode::Preserve => ContinuationMaterializationMode::Preserve,
        host::ContinuationMode::Compatible => ContinuationMaterializationMode::Compatible,
        host::ContinuationMode::Switch => ContinuationMaterializationMode::Switch,
    }
}

fn generated_run_input(
    parts: &[host::InputPart],
) -> Result<(Vec<InputPart>, AgentInput), host::HostError> {
    if parts.is_empty() {
        return Err(invalid_params_error("run input must not be empty"));
    }
    let durable_input = parts
        .iter()
        .map(|part| match part {
            host::InputPart::TextInputPart(part) => Ok(InputPart::text(part.text.clone())),
            host::InputPart::ResourceInputPart(part) => {
                let mut resource_metadata = starweaver_core::Metadata::default();
                if let Some(name) = &part.name {
                    resource_metadata.insert("name".to_string(), json!(name));
                }
                Ok(InputPart::ResourceRef {
                    uri: part.uri.clone(),
                    media_type: part.media_type.clone(),
                    resource_type: resource_type_for_media_type(&part.media_type).to_string(),
                    resource_metadata,
                    metadata: Default::default(),
                })
            }
        })
        .collect::<Result<Vec<_>, host::HostError>>()?;
    let content = durable_input
        .iter()
        .cloned()
        .map(starweaver_model::ContentPart::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_params_error("run input cannot be converted to model content"))?;
    Ok((durable_input, AgentInput::parts(content)))
}

fn resource_type_for_media_type(media_type: &str) -> &'static str {
    if media_type.starts_with("image/") {
        "image"
    } else if media_type.starts_with("audio/") {
        "audio"
    } else if media_type.starts_with("video/") {
        "video"
    } else {
        "document"
    }
}

fn canonical_json(value: &Value) -> Result<String, RpcError> {
    serde_json::to_string(value).map_err(|error| {
        RpcError::new(
            INVALID_PARAMS,
            format!("invalid active environment params: {error}"),
        )
    })
}

fn rpc_error(error: impl Into<RpcHostError>) -> RpcError {
    error.into().into()
}

fn session_search_error(error: SessionSearchError) -> RpcError {
    match error {
        SessionSearchError::InvalidQuery(message) | SessionSearchError::InvalidCursor(message) => {
            RpcError::new(INVALID_PARAMS, message)
        }
        SessionSearchError::Unsupported(message) => RpcError::new(UNSUPPORTED_FEATURE, message),
        SessionSearchError::Unavailable(message) => {
            RpcError::new(SESSION_SEARCH_UNAVAILABLE, message)
        }
        SessionSearchError::PermissionDenied => {
            RpcError::new(SERVER_ERROR, "session search permission denied")
        }
        SessionSearchError::Failed(_) => RpcError::new(SERVER_ERROR, "session search failed"),
    }
}

#[cfg(test)]
mod generated_service_tests {
    #![allow(clippy::similar_names, clippy::too_many_lines, clippy::unwrap_used)]

    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use starweaver_rpc_core::generated::HostServer as _;

    use super::*;
    use crate::config::RpcEnvironmentResourceConfig;

    fn typed<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn negotiated_features_and_event_profile_scopes_are_enforced_before_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let service = RpcService::replay_only(RpcConfig::for_tests(temp.path())).unwrap();
        let connection = service.new_connection(
            None,
            "stdio-admission-authority",
            host::Transport::Stdio,
            all_connection_scopes(),
        );
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": "initialize-empty",
            "method": "initialize",
            "params": {
                "clientInfo": {"name": "admission-test", "version": "1"},
                "protocol": {
                    "major": host::PROTOCOL_MAJOR,
                    "name": host::PROTOCOL_NAME,
                    "revision": host::PROTOCOL_REVISION,
                    "schemaDigest": host::SCHEMA_DIGEST
                },
                "requiredFeatures": [],
                "supportedFeatures": []
            }
        });
        let initialized = connection.handle_text(&initialize.to_string());
        assert!(initialized.response.unwrap().get("result").is_some());
        let denied = connection.handle_text(
            &json!({
                "jsonrpc": "2.0",
                "id": "session-list-without-feature",
                "method": "session.list",
                "params": {"limit": 10}
            })
            .to_string(),
        );
        assert_eq!(
            denied.response.unwrap()["error"]["code"],
            json!(host::ERROR_CODE_UNSUPPORTED_FEATURE)
        );

        let replay = service.handle_text_for_authority(
            &json!({
                "jsonrpc": "2.0",
                "id": "read-only-operations",
                "method": "events.replay",
                "params": {
                    "limit": 10,
                    "view": {
                        "optionalFeatures": [],
                        "profile": "operations.v1",
                        "scope": {"kind": "global"}
                    }
                }
            })
            .to_string(),
            "read-only-authority",
            BTreeSet::from(["read".to_string()]),
        );
        assert_eq!(
            replay.response.unwrap()["error"]["code"],
            json!(host::ERROR_CODE_AUTHORIZATION_DENIED)
        );

        let http_connection_scope = service.handle_text_for_authority(
            &json!({
                "jsonrpc": "2.0",
                "id": "http-connection-scope",
                "method": "environment.attach",
                "params": {
                    "environmentId": "local",
                    "idempotencyKey": "http-connection-scope",
                    "scope": {"kind": "connection"}
                }
            })
            .to_string(),
            "http-authority",
            all_connection_scopes(),
        );
        assert_eq!(
            http_connection_scope.response.unwrap()["error"]["code"],
            json!(host::ERROR_CODE_UNSUPPORTED_FEATURE)
        );
    }

    #[test]
    fn session_and_run_receipt_keys_are_bound_to_trusted_authority() {
        assert_eq!(
            authority_scoped_idempotency_key("authority-a", "same-key"),
            authority_scoped_idempotency_key("authority-a", "same-key")
        );
        assert_ne!(
            authority_scoped_idempotency_key("authority-a", "same-key"),
            authority_scoped_idempotency_key("authority-b", "same-key")
        );

        let temp = tempfile::tempdir().unwrap();
        let service = RpcService::replay_only(RpcConfig::for_tests(temp.path())).unwrap();
        let first = service.new_connection(
            None,
            "authority-a",
            host::Transport::Http,
            all_connection_scopes(),
        );
        let second = service.new_connection(
            None,
            "authority-b",
            host::Transport::Http,
            all_connection_scopes(),
        );
        let runtime = service.runtime.runtime.as_ref().unwrap();
        let (first_id, first_replay_id, second_id) = execute_on_runtime(runtime, async move {
            let params: host::SessionCreateParams = typed(json!({
                "deferredTools": [],
                "idempotencyKey": "same-key",
                "profile": "default",
                "title": "Authority scoped receipt"
            }));
            let first_result = first.session_create(&(), params.clone()).await.unwrap();
            let first_replay = first.session_create(&(), params.clone()).await.unwrap();
            let second_result = second.session_create(&(), params).await.unwrap();
            (
                first_result.session.session_id,
                first_replay.session.session_id,
                second_result.session.session_id,
            )
        })
        .unwrap();
        assert_eq!(first_id, first_replay_id);
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn run_control_receipts_are_authority_scoped_at_the_generated_adapter() {
        let temp = tempfile::tempdir().unwrap();
        let service = RpcService::replay_only(RpcConfig::for_tests(temp.path())).unwrap();
        let authority_a = service.new_connection(
            None,
            "authority-a",
            host::Transport::Http,
            all_connection_scopes(),
        );
        let authority_b = service.new_connection(
            None,
            "authority-b",
            host::Transport::Http,
            all_connection_scopes(),
        );
        let runtime = service.runtime.runtime.as_ref().unwrap();
        execute_on_runtime(runtime, async move {
            let (interrupt_session, interrupt_run, interrupt_handle, interrupt_release) = service
                .coordinator
                .install_active_run_control_fixture()
                .await
                .unwrap();
            let interrupt_params: host::RunInterruptParams = typed(json!({
                "idempotencyKey": "shared-interrupt-key",
                "reason": "stop now",
                "runId": interrupt_run.as_str(),
                "sessionId": interrupt_session.as_str()
            }));
            let first_interrupt = authority_a
                .run_interrupt(&(), interrupt_params.clone())
                .await
                .unwrap();
            let replayed_interrupt = authority_a
                .run_interrupt(&(), interrupt_params.clone())
                .await
                .unwrap();
            let other_interrupt = authority_b
                .run_interrupt(&(), interrupt_params.clone())
                .await
                .unwrap();
            assert_eq!(
                first_interrupt.receipt.idempotency_key.as_str(),
                "shared-interrupt-key"
            );
            assert_eq!(
                replayed_interrupt.receipt.idempotency_key.as_str(),
                "shared-interrupt-key"
            );
            assert_eq!(
                other_interrupt.receipt.idempotency_key.as_str(),
                "shared-interrupt-key"
            );
            assert!(!first_interrupt.receipt.replayed);
            assert!(replayed_interrupt.receipt.replayed);
            assert!(!other_interrupt.receipt.replayed);
            assert_eq!(
                first_interrupt.receipt.receipt_id,
                replayed_interrupt.receipt.receipt_id
            );
            assert_ne!(
                first_interrupt.receipt.receipt_id,
                other_interrupt.receipt.receipt_id
            );
            let conflicting_interrupt: host::RunInterruptParams = typed(json!({
                "idempotencyKey": "shared-interrupt-key",
                "reason": "different reason",
                "runId": interrupt_run.as_str(),
                "sessionId": interrupt_session.as_str()
            }));
            assert!(
                authority_a
                    .run_interrupt(&(), conflicting_interrupt)
                    .await
                    .is_err()
            );
            interrupt_release.notify_one();
            let _ = interrupt_handle.complete().await;

            let (steer_session, steer_run, steer_handle, steer_release) = service
                .coordinator
                .install_active_run_control_fixture()
                .await
                .unwrap();
            let steer_params: host::RunSteerParams = typed(json!({
                "idempotencyKey": "shared-steer-key",
                "runId": steer_run.as_str(),
                "sessionId": steer_session.as_str(),
                "text": "new direction"
            }));
            let first_steer = authority_a
                .run_steer(&(), steer_params.clone())
                .await
                .unwrap();
            let replayed_steer = authority_a
                .run_steer(&(), steer_params.clone())
                .await
                .unwrap();
            let other_steer = authority_b
                .run_steer(&(), steer_params.clone())
                .await
                .unwrap();
            assert_eq!(
                first_steer.receipt.idempotency_key.as_str(),
                "shared-steer-key"
            );
            assert_eq!(
                replayed_steer.receipt.idempotency_key.as_str(),
                "shared-steer-key"
            );
            assert_eq!(
                other_steer.receipt.idempotency_key.as_str(),
                "shared-steer-key"
            );
            assert!(!first_steer.receipt.replayed);
            assert!(replayed_steer.receipt.replayed);
            assert!(!other_steer.receipt.replayed);
            assert_eq!(
                first_steer.receipt.receipt_id,
                replayed_steer.receipt.receipt_id
            );
            assert_ne!(
                first_steer.receipt.receipt_id,
                other_steer.receipt.receipt_id
            );
            let conflicting_steer: host::RunSteerParams = typed(json!({
                "idempotencyKey": "shared-steer-key",
                "runId": steer_run.as_str(),
                "sessionId": steer_session.as_str(),
                "text": "conflicting direction"
            }));
            assert!(authority_a.run_steer(&(), conflicting_steer).await.is_err());
            steer_release.notify_one();
            let completion = steer_handle.complete().await;
            assert!(completion.error.is_none(), "{completion:?}");
        })
        .unwrap();
    }

    #[test]
    fn environment_attach_replays_before_current_provider_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let first_service = RpcService::replay_only(RpcConfig::for_tests(temp.path())).unwrap();
        let first_connection = first_service.new_connection(
            None,
            "receipt-first-authority",
            host::Transport::Http,
            all_connection_scopes(),
        );
        *first_connection.state.negotiated_features.lock().unwrap() =
            first_connection.supported_features();
        let params: host::EnvironmentAttachParams = typed(json!({
            "environmentId": "local",
            "idempotencyKey": "receipt-first-attach",
            "scope": {"kind": "session", "sessionId": "session-replay"}
        }));
        let first = execute_on_runtime(first_service.runtime.runtime.as_ref().unwrap(), {
            let first_connection = first_connection.clone();
            let params = params.clone();
            async move {
                first_connection
                    .environment_attach(&(), params)
                    .await
                    .unwrap()
            }
        })
        .unwrap();
        drop(first_connection);
        drop(first_service);

        let mut unavailable_config = RpcConfig::for_tests(temp.path());
        unavailable_config.environments.remove("local");
        let replay_service = RpcService::replay_only(unavailable_config).unwrap();
        let replay_connection = replay_service.new_connection(
            None,
            "receipt-first-authority",
            host::Transport::Http,
            all_connection_scopes(),
        );
        *replay_connection.state.negotiated_features.lock().unwrap() =
            replay_connection.supported_features();
        let replay = execute_on_runtime(
            replay_service.runtime.runtime.as_ref().unwrap(),
            async move {
                replay_connection
                    .environment_attach(&(), params)
                    .await
                    .unwrap()
            },
        )
        .unwrap();

        assert_eq!(replay.attachment, first.attachment);
        assert_eq!(replay.receipt.receipt_id, first.receipt.receipt_id);
        assert_eq!(
            replay.receipt.idempotency_key,
            first.receipt.idempotency_key
        );
        assert!(replay.receipt.replayed);
    }

    #[test]
    fn connection_scoped_attachments_are_revoked_on_owner_drop() {
        let temp = tempfile::tempdir().unwrap();
        let service = RpcService::replay_only(RpcConfig::for_tests(temp.path())).unwrap();
        let connection = service.new_connection(
            None,
            "shared-stdio-authority",
            host::Transport::Stdio,
            all_connection_scopes(),
        );
        let connection_id = connection.state.connection_id.clone();
        let authority = connection.state.authority_identity.clone();
        let runtime = service.runtime.runtime.as_ref().unwrap();
        let attachment_id = execute_on_runtime(runtime, {
            let connection = connection.clone();
            async move {
                connection
                    .environment_attach(
                        &(),
                        typed(json!({
                            "environmentId": "local",
                            "idempotencyKey": "connection-attachment",
                            "scope": {"kind": "connection"}
                        })),
                    )
                    .await
                    .unwrap()
                    .attachment
                    .attachment_id
                    .into_string()
            }
        })
        .unwrap();
        let attached = service
            .storage
            .get_environment_attachment(&authority, &attachment_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            attached.scope,
            DurableEnvironmentScope::Connection {
                connection_id: connection_id.clone()
            }
        );
        connection.close().unwrap();
        drop(connection);
        let detached = service
            .storage
            .get_environment_attachment(&authority, &attachment_id)
            .unwrap()
            .unwrap();
        assert_eq!(detached.status, DurableEnvironmentStatus::Detached);
        assert!(
            service
                .storage
                .list_connection_environment_attachments(&authority, &connection_id)
                .unwrap()
                .iter()
                .all(|attachment| attachment.status == DurableEnvironmentStatus::Detached)
        );
    }

    #[test]
    fn generated_environment_boundary_is_durable_replayable_and_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        config
            .environments
            .get_mut("local")
            .unwrap()
            .resources
            .insert(
                "workspace-data".to_string(),
                RpcEnvironmentResourceConfig {
                    label: "Workspace data".to_string(),
                    source_ref: "data".to_string(),
                },
            );
        let service = RpcService::replay_only(config).unwrap();
        let connection = service.new_connection(
            None,
            "generated-test-authority",
            host::Transport::Http,
            all_connection_scopes(),
        );
        *connection.state.negotiated_features.lock().unwrap() = connection.supported_features();
        let runtime = service.runtime.runtime.as_ref().unwrap();

        execute_on_runtime(runtime, async move {
            let session = connection
                .session_create(
                    &(),
                    typed(json!({
                        "deferredTools": [],
                        "idempotencyKey": "create-environment-test-session",
                        "profile": "default",
                        "title": "Generated environment test"
                    })),
                )
                .await
                .unwrap();
            let session_id = session.session.session_id;

            let attach_params: host::EnvironmentAttachParams = typed(json!({
                "environmentId": "local",
                "idempotencyKey": "attach-local-environment",
                "scope": {"kind": "session", "sessionId": session_id.as_str()}
            }));
            let attached = connection
                .environment_attach(&(), attach_params.clone())
                .await
                .unwrap();
            let replayed_attach = connection
                .environment_attach(&(), attach_params.clone())
                .await
                .unwrap();
            assert_eq!(replayed_attach.attachment, attached.attachment);
            assert_eq!(
                replayed_attach.receipt.receipt_id,
                attached.receipt.receipt_id
            );
            assert!(!attached.receipt.replayed);
            assert!(replayed_attach.receipt.replayed);
            assert_eq!(attached.attachment.status, host::EnvironmentStatus::Ready);
            assert_eq!(attached.attachment.revision.get(), 1);

            let conflicting_attach: host::EnvironmentAttachParams = typed(json!({
                "environmentId": "local",
                "idempotencyKey": "attach-local-environment",
                "scope": {"kind": "connection"}
            }));
            assert!(
                connection
                    .environment_attach(&(), conflicting_attach)
                    .await
                    .is_err(),
                "same idempotency key must reject a different command fingerprint"
            );

            let listed = connection
                .environment_list(
                    &(),
                    typed(json!({
                        "limit": 10,
                        "scope": {"kind": "session", "sessionId": session_id.as_str()}
                    })),
                )
                .await
                .unwrap();
            assert_eq!(listed.attachments, vec![attached.attachment.clone()]);
            assert!(!listed.page.has_more);

            let health = connection
                .environment_health(
                    &(),
                    typed(json!({"attachmentId": attached.attachment.attachment_id.as_str()})),
                )
                .await
                .unwrap();
            assert_eq!(health.attachment, attached.attachment);

            let run_params: host::RunStartParams = typed(json!({
                "continuationMode": "preserve",
                "environmentAttachments": [attached.attachment.attachment_id.as_str()],
                "idempotencyKey": "start-with-durable-environment",
                "input": [{"kind": "text", "text": "test generated environment"}],
                "profile": "default",
                "sessionId": session_id.as_str()
            }));
            let started = connection.run_start(&(), run_params.clone()).await.unwrap();
            let replayed_start = connection.run_start(&(), run_params).await.unwrap();
            assert_eq!(replayed_start.run.run_id, started.run.run_id);
            assert_eq!(
                replayed_start.receipt.receipt_id,
                started.receipt.receipt_id
            );
            assert!(!started.receipt.replayed);
            assert!(replayed_start.receipt.replayed);
            let run_id = started.run.run_id;

            let mount_params: host::EnvironmentMountParams = typed(json!({
                "attachmentId": attached.attachment.attachment_id.as_str(),
                "idempotencyKey": "mount-workspace-data",
                "resourceRef": "workspace-data",
                "runId": run_id.as_str(),
                "sessionId": session_id.as_str()
            }));
            let mounted = connection
                .environment_mount(&(), mount_params.clone())
                .await
                .unwrap();
            let replayed_mount = connection
                .environment_mount(&(), mount_params)
                .await
                .unwrap();
            assert_eq!(replayed_mount.mount_id, mounted.mount_id);
            assert_eq!(
                replayed_mount.receipt.receipt_id,
                mounted.receipt.receipt_id
            );
            assert!(!mounted.receipt.replayed);
            assert!(replayed_mount.receipt.replayed);

            let mounts_params: host::EnvironmentMountListParams = typed(json!({
                "runId": run_id.as_str(),
                "sessionId": session_id.as_str()
            }));
            let mounts = connection
                .environment_mounts_list(&(), mounts_params.clone())
                .await
                .unwrap();
            assert_eq!(mounts.mounts.len(), 1);
            assert_eq!(mounts.mounts[0].mount_id, mounted.mount_id);
            assert_eq!(mounts.mounts[0].resource_label, "Workspace data");

            let run_view = json!({
                "optionalFeatures": [],
                "profile": "operations.v1",
                "scope": {
                    "kind": "run",
                    "runId": run_id.as_str(),
                    "sessionId": session_id.as_str()
                }
            });
            let after_mount = host::HostServer::events_replay(
                &connection,
                &(),
                typed(json!({"limit": 100, "view": run_view.clone()})),
            )
            .await
            .unwrap();
            assert_eq!(after_mount.deliveries.len(), 1);
            assert_eq!(
                serde_json::to_value(&after_mount.deliveries[0].record.event).unwrap()["kind"],
                "environment_changed"
            );

            let unmount_params: host::EnvironmentUnmountParams = typed(json!({
                "idempotencyKey": "unmount-workspace-data",
                "mountId": mounted.mount_id
            }));
            let unmounted = connection
                .environment_unmount(&(), unmount_params.clone())
                .await
                .unwrap();
            let replayed_unmount = connection
                .environment_unmount(&(), unmount_params)
                .await
                .unwrap();
            assert_eq!(replayed_unmount.mount_id, unmounted.mount_id);
            assert_eq!(
                replayed_unmount.receipt.receipt_id,
                unmounted.receipt.receipt_id
            );
            assert!(!unmounted.receipt.replayed);
            assert!(replayed_unmount.receipt.replayed);
            assert!(unmounted.removed);
            assert!(
                connection
                    .environment_mounts_list(&(), mounts_params)
                    .await
                    .unwrap()
                    .mounts
                    .is_empty()
            );

            let after_unmount = host::HostServer::events_replay(
                &connection,
                &(),
                typed(json!({"limit": 100, "view": run_view})),
            )
            .await
            .unwrap();
            assert_eq!(after_unmount.deliveries.len(), 2);
            assert_ne!(
                after_unmount.deliveries[0].cursor,
                after_unmount.deliveries[1].cursor
            );

            let detach_params: host::EnvironmentDetachParams = typed(json!({
                "attachmentId": attached.attachment.attachment_id.as_str(),
                "idempotencyKey": "detach-local-environment"
            }));
            let detached = connection
                .environment_detach(&(), detach_params.clone())
                .await
                .unwrap();
            let replayed_detach = connection
                .environment_detach(&(), detach_params)
                .await
                .unwrap();
            assert_eq!(replayed_detach.attachment, detached.attachment);
            assert_eq!(
                replayed_detach.receipt.receipt_id,
                detached.receipt.receipt_id
            );
            assert!(!detached.receipt.replayed);
            assert!(replayed_detach.receipt.replayed);
            assert_eq!(
                detached.attachment.status,
                host::EnvironmentStatus::Detached
            );
        })
        .unwrap();

        service
            .shutdown_owned_runtime(Duration::from_secs(2))
            .unwrap();
    }
}
