//! Shared local run coordination for RPC and interactive clients.
#![allow(clippy::redundant_pub_crate)]

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use serde::Serialize;
use serde_json::{Value, json};
use starweaver_environment::{
    ShellProcessStatus, SwitchableEnvironmentProvider, SwitchableEnvironmentTarget,
};
use starweaver_rpc_core::{
    ALREADY_EXISTS, EnvironmentActiveMountParams, EnvironmentActiveUnmountParams,
    EnvironmentAttachmentRef, IDEMPOTENCY_CONFLICT, INVALID_PARAMS, RUN_CONFLICT,
};
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{RunRecord, RunStatus};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, EnvironmentLifecycleEvent, ReplayCursor, ReplayEvent,
    ReplayEventKind, ReplayScope,
};

use crate::{
    CliError, CliResult, CliService,
    args::RunCommand,
    config::CliConfig,
    display_preview::run_output_preview,
    environment::{ResolvedEnvironment, resolve_environment_target_for_session_with_attachments},
    local_store::{LocalStore, LocalStreamArchive},
    prompt_input::PromptInput,
    runner::CliSteeringMessage,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunStatusItem {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum RunStreamEvent {
    Output(Box<ReplayEvent>),
    Status(RunStatusItem),
    Raw(Box<AgentStreamRecord>),
}

pub(super) struct StartedRun {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) events: mpsc::Receiver<RunStreamEvent>,
}

pub(super) struct RunAttachment {
    pub(super) session_id: String,
    pub(super) run_id: Option<String>,
    pub(super) active: bool,
    pub(super) events: Vec<ReplayEvent>,
    pub(super) subscription: Option<mpsc::Receiver<RunStreamEvent>>,
}

#[derive(Clone)]
pub(super) struct CliRuntimeCoordinator {
    config: CliConfig,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunState>>>,
}

struct ActiveRunState {
    session_id: String,
    sequence_no: usize,
    status: String,
    output_preview: Option<String>,
    error: Option<String>,
    display_messages: Vec<DisplayMessage>,
    replay_events: Vec<ReplayEvent>,
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    subscribers: Vec<RunSubscriber>,
    environment: Option<ActiveEnvironmentBinding>,
}

struct ActiveEnvironmentBinding {
    attachments: Vec<EnvironmentAttachmentRef>,
    binding_version: u64,
    switchable: Arc<SwitchableEnvironmentProvider>,
    latest_environment_sequence: Option<usize>,
    idempotency: HashMap<String, ActiveMutationIdempotencyRecord>,
}

#[derive(Clone)]
struct ActiveMutationIdempotencyRecord {
    params_digest: String,
    result: Value,
}

#[derive(Debug)]
pub(super) struct ActiveMountOutcome {
    pub(super) result: Value,
    pub(super) applied: bool,
}

#[derive(Debug)]
pub(super) struct ActiveUnmountOutcome {
    pub(super) result: Value,
    pub(super) removed: EnvironmentAttachmentRef,
    pub(super) applied: bool,
}

struct ActiveMountMutation {
    mounted: EnvironmentAttachmentRef,
    previous_binding_version: u64,
    binding_version: u64,
    previous_default: Option<String>,
    current_default: Option<String>,
    previous_default_shell: Option<String>,
    current_default_shell: Option<String>,
    attachments: Vec<EnvironmentAttachmentRef>,
}

struct ActiveUnmountMutation {
    removed: EnvironmentAttachmentRef,
    previous_binding_version: u64,
    binding_version: u64,
    previous_default: Option<String>,
    current_default: Option<String>,
    previous_default_shell: Option<String>,
    current_default_shell: Option<String>,
    attachments: Vec<EnvironmentAttachmentRef>,
}

enum ActiveUnmountPreparation {
    Idempotent {
        result: Value,
        removed: EnvironmentAttachmentRef,
    },
    Mutation(PreparedActiveUnmount),
}

struct PreparedActiveUnmount {
    session_id: String,
    switchable: Arc<SwitchableEnvironmentProvider>,
    removed: EnvironmentAttachmentRef,
    updated: Vec<EnvironmentAttachmentRef>,
    previous_binding_version: u64,
    previous_default: Option<String>,
    previous_default_shell: Option<String>,
}

struct RunSubscriber {
    include_raw: bool,
    cursor: SubscriberCursor,
    sender: mpsc::Sender<RunStreamEvent>,
}

enum SubscriberCursor {
    Run,
    Session { next_sequence: Arc<Mutex<usize>> },
}

struct BackgroundRunWorker {
    config: CliConfig,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunState>>>,
    command: RunCommand,
    prompt_input: Option<PromptInput>,
    include_raw: bool,
    started_sender: mpsc::Sender<CliResult<(String, String)>>,
    event_sender: mpsc::Sender<RunStreamEvent>,
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    steering_receiver: mpsc::Receiver<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    cancel_receiver: mpsc::Receiver<()>,
}

impl BackgroundRunWorker {
    #[allow(clippy::too_many_lines)]
    fn run(self) {
        let mut service = match CliService::open(self.config.clone()) {
            Ok(service) => service,
            Err(error) => {
                let _ = self.started_sender.send(Err(error));
                return;
            }
        };
        let prepared = match service.prepare_prompt_run(&self.command, self.prompt_input) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = self.started_sender.send(Err(error));
                return;
            }
        };
        let run_on_error = prepared.run.clone();
        let session_id = prepared.session_id.clone();
        let run_id = prepared.run_id.clone();
        let active_environment = active_environment_binding(&prepared.environment);
        insert_active_run(
            &self.active_runs,
            &session_id,
            &run_id,
            self.steering_sender,
            self.cancel_sender,
            prepared.run.sequence_no,
            RunSubscriber {
                include_raw: self.include_raw,
                cursor: SubscriberCursor::Run,
                sender: self.event_sender,
            },
            active_environment,
        );
        publish_status(
            &self.active_runs,
            &session_id,
            &run_id,
            "running",
            None,
            None,
        );
        publish_display_messages(
            &self.active_runs,
            &run_id,
            vec![queued_display_message(&prepared.run)],
        );
        publish_initial_environment_info(&self.active_runs, &run_id);
        let _ = self
            .started_sender
            .send(Ok((session_id.clone(), run_id.clone())));

        let (stream_sender, stream_receiver) = mpsc::channel::<AgentStreamRecord>();
        let active_for_stream = Arc::clone(&self.active_runs);
        let stream_run = prepared.run.clone();
        let stream_handle = thread::spawn(move || {
            forward_runtime_stream(stream_receiver, &active_for_stream, &stream_run);
        });
        let executed = CliService::run_prepared_prompt(
            prepared,
            Some(stream_sender),
            Some(self.steering_receiver),
            Some(self.cancel_receiver),
        );
        let _ = stream_handle.join();
        match executed {
            Ok(mut executed) => {
                executed.merge_display_message_inserts(environment_lifecycle_message_inserts(
                    &self.active_runs,
                    &run_id,
                ));
                match service.complete_prompt_run(executed) {
                    Ok(execution) => {
                        publish_status(
                            &self.active_runs,
                            &execution.session_id,
                            &execution.run_id,
                            &execution.status,
                            output_preview(&execution.messages),
                            None,
                        );
                        remove_active_if_terminal(&self.active_runs, &execution.run_id);
                    }
                    Err(error) => {
                        publish_status(
                            &self.active_runs,
                            &session_id,
                            &run_id,
                            "failed",
                            None,
                            Some(error.to_string()),
                        );
                        remove_active_if_terminal(&self.active_runs, &run_id);
                    }
                }
            }
            Err(error) => {
                let _ = service.fail_prepared_prompt_run(run_on_error, &error);
                publish_status(
                    &self.active_runs,
                    &session_id,
                    &run_id,
                    "failed",
                    None,
                    Some(error.to_string()),
                );
                remove_active_if_terminal(&self.active_runs, &run_id);
            }
        }
    }
}

impl CliRuntimeCoordinator {
    pub(super) fn new(config: CliConfig) -> Self {
        Self {
            config,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn start_run(
        &self,
        command: RunCommand,
        prompt_input: Option<PromptInput>,
    ) -> CliResult<StartedRun> {
        self.start_run_with_options(command, prompt_input, false)
    }

    pub(super) fn start_run_with_raw(
        &self,
        command: RunCommand,
        prompt_input: Option<PromptInput>,
    ) -> CliResult<StartedRun> {
        self.start_run_with_options(command, prompt_input, true)
    }

    fn start_run_with_options(
        &self,
        command: RunCommand,
        prompt_input: Option<PromptInput>,
        include_raw: bool,
    ) -> CliResult<StartedRun> {
        let (started_sender, started_receiver) = mpsc::channel::<CliResult<(String, String)>>();
        let (event_sender, event_receiver) = mpsc::channel::<RunStreamEvent>();
        let (steering_sender, steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, cancel_receiver) = mpsc::channel::<()>();
        let worker = BackgroundRunWorker {
            config: self.config.clone(),
            active_runs: Arc::clone(&self.active_runs),
            command,
            prompt_input,
            include_raw,
            started_sender,
            event_sender,
            steering_sender,
            steering_receiver,
            cancel_sender,
            cancel_receiver,
        };
        thread::spawn(move || worker.run());
        let (session_id, run_id) = started_receiver
            .recv()
            .map_err(|error| CliError::Run(error.to_string()))??;
        Ok(StartedRun {
            session_id,
            run_id,
            events: event_receiver,
        })
    }

    pub(super) fn attach_run(
        &self,
        session_id: &str,
        run_id: &str,
        cursor: Option<&ReplayCursor>,
    ) -> CliResult<RunAttachment> {
        let scope = ReplayScope::run(run_id);
        validate_cursor_scope(cursor, &scope)?;
        if let Some(attachment) = self.attach_active_run(session_id, run_id, cursor) {
            return Ok(attachment);
        }
        let archive = LocalStreamArchive::new(self.config.clone());
        let window = archive.replay_display_window(session_id, Some(run_id), cursor)?;
        Ok(RunAttachment {
            session_id: session_id.to_string(),
            run_id: Some(run_id.to_string()),
            active: false,
            events: window.events,
            subscription: None,
        })
    }

    pub(super) fn session_output(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        cursor: Option<&ReplayCursor>,
    ) -> CliResult<RunAttachment> {
        if let Some(run_id) = run_id {
            return self.attach_run(session_id, run_id, cursor);
        }
        let scope = ReplayScope::session(session_id);
        validate_cursor_scope(cursor, &scope)?;
        let mut active = false;
        let (subscription_sender, subscription_receiver) = mpsc::channel::<RunStreamEvent>();
        let mut live_events = Vec::new();
        let (store_event_count, store_events) = {
            let archive = LocalStreamArchive::new(self.config.clone());
            let window = archive.replay_display_window(session_id, None, cursor)?;
            (window.next_sequence, window.events)
        };
        if let Ok(mut runs) = self.active_runs.lock() {
            let mut matching_runs = runs
                .values_mut()
                .filter(|state| state.session_id == session_id)
                .collect::<Vec<_>>();
            matching_runs.sort_by_key(|state| state.sequence_no);
            let persisted_messages = persisted_display_message_keys(&store_events);
            for state in &mut matching_runs {
                if is_subscribable_status(&state.status) {
                    active = true;
                }
                live_events.extend(
                    state
                        .replay_events
                        .iter()
                        .filter(|event| {
                            replay_event_not_persisted_as_display(event, &persisted_messages)
                        })
                        .cloned(),
                );
            }
            let all_live_events =
                append_session_replay_events(&scope, store_event_count, live_events);
            let next_sequence = Arc::new(Mutex::new(store_event_count + all_live_events.len()));
            for state in matching_runs
                .into_iter()
                .filter(|state| is_subscribable_status(&state.status))
            {
                state.subscribers.push(RunSubscriber {
                    include_raw: false,
                    cursor: SubscriberCursor::Session {
                        next_sequence: Arc::clone(&next_sequence),
                    },
                    sender: subscription_sender.clone(),
                });
            }
            let mut events = store_events;
            events.extend(filter_replay_events(all_live_events, cursor));
            return Ok(RunAttachment {
                session_id: session_id.to_string(),
                run_id: None,
                active,
                events,
                subscription: active.then_some(subscription_receiver),
            });
        }
        Ok(RunAttachment {
            session_id: session_id.to_string(),
            run_id: None,
            active,
            events: store_events,
            subscription: active.then_some(subscription_receiver),
        })
    }

    pub(super) fn steer_run(&self, run_id: &str, message: CliSteeringMessage) -> CliResult<()> {
        let sender = {
            let runs = self
                .active_runs
                .lock()
                .map_err(|error| CliError::Run(error.to_string()))?;
            let Some(state) = runs.get(run_id) else {
                return Err(CliError::NotFound(run_id.to_string()));
            };
            let sender = state.steering_sender.clone();
            drop(runs);
            sender
        };

        sender
            .send(message)
            .map_err(|error| CliError::Run(error.to_string()))
    }

    pub(super) fn steer_session(
        &self,
        session_id: &str,
        message: CliSteeringMessage,
    ) -> CliResult<String> {
        let (run_id, sender) = {
            let runs = self
                .active_runs
                .lock()
                .map_err(|error| CliError::Run(error.to_string()))?;
            let Some((run_id, state)) = runs
                .iter()
                .find(|(_, state)| state.session_id == session_id && state.status == "running")
            else {
                return Err(CliError::NotFound(format!("active run for {session_id}")));
            };
            let run_id = run_id.clone();
            let sender = state.steering_sender.clone();
            drop(runs);
            (run_id, sender)
        };

        sender
            .send(message)
            .map_err(|error| CliError::Run(error.to_string()))?;
        Ok(run_id)
    }

    pub(super) fn cancel_run(&self, run_id: &str) -> CliResult<()> {
        let sender = {
            let runs = self
                .active_runs
                .lock()
                .map_err(|error| CliError::Run(error.to_string()))?;
            let Some(state) = runs.get(run_id) else {
                return Err(CliError::NotFound(run_id.to_string()));
            };
            let sender = state.cancel_sender.clone();
            drop(runs);
            sender
        };

        sender
            .send(())
            .map_err(|error| CliError::Run(error.to_string()))
    }

    pub(super) fn active_environment_list(&self, run_id: &str) -> CliResult<Value> {
        let runs = self
            .active_runs
            .lock()
            .map_err(|error| CliError::Run(error.to_string()))?;
        let Some(state) = runs.get(run_id) else {
            return Err(CliError::NotFound(run_id.to_string()));
        };
        let Some(environment) = state.environment.as_ref() else {
            return Err(CliError::Run(
                "active run has no mutable environment binding".to_string(),
            ));
        };
        let result = json!({
            "runId": run_id,
            "environment": environment_summary(
                environment.binding_version,
                &environment.attachments,
            ),
            "latestEnvironmentCursor": environment.latest_environment_sequence.map(|sequence| {
                json!({
                    "scope": ReplayScope::run(run_id).as_str(),
                    "sequence": sequence,
                })
            }),
        });
        drop(runs);
        Ok(result)
    }

    pub(super) fn active_run_session_id(&self, run_id: &str) -> CliResult<String> {
        let runs = self
            .active_runs
            .lock()
            .map_err(|error| CliError::Run(error.to_string()))?;
        let Some(state) = runs.get(run_id) else {
            return Err(CliError::NotFound(run_id.to_string()));
        };
        let session_id = state.session_id.clone();
        drop(runs);
        Ok(session_id)
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn active_environment_mount(
        &self,
        params: &EnvironmentActiveMountParams,
        attachment: EnvironmentAttachmentRef,
        params_digest: &str,
    ) -> Result<ActiveMountOutcome, starweaver_rpc_core::RpcError> {
        let mut runs = self
            .active_runs
            .lock()
            .map_err(|error| starweaver_rpc_core::RpcError::new(RUN_CONFLICT, error.to_string()))?;
        let Some(state) = runs.get_mut(&params.run_id) else {
            return Err(starweaver_rpc_core::RpcError::new(
                RUN_CONFLICT,
                format!("active run not found: {}", params.run_id),
            ));
        };
        let operation_id = format!("envop_{}", chrono::Utc::now().timestamp_micros());
        let event_sequence = next_display_sequence(state);
        let mutation = {
            let Some(environment) = state.environment.as_mut() else {
                return Err(starweaver_rpc_core::RpcError::new(
                    RUN_CONFLICT,
                    "active run has no mutable environment binding",
                ));
            };
            if let Some(key) = params.idempotency_key.as_deref()
                && let Some(record) = environment.idempotency.get(&mutation_key("mount", key))
            {
                ensure_idempotency_digest(record, params_digest)?;
                return Ok(ActiveMountOutcome {
                    result: record.result.clone(),
                    applied: false,
                });
            }
            ensure_expected_binding(environment, params.expected_binding_version)?;
            let previous_binding_version = environment.binding_version;
            let previous_default = default_mount_id(&environment.attachments);
            let previous_default_shell = default_shell_mount_id(&environment.attachments);
            let existing_index = environment
                .attachments
                .iter()
                .position(|current| current.id == attachment.id);
            if existing_index.is_some() && !params.replace {
                return Err(starweaver_rpc_core::RpcError::new(
                    ALREADY_EXISTS,
                    format!("environment mount already exists: {}", attachment.id),
                ));
            }
            let mut updated = environment.attachments.clone();
            let mut mounted = attachment;
            if let Some(index) = existing_index {
                if updated[index].is_default && !mounted.is_default {
                    mounted.is_default = true;
                }
                if updated[index].is_default_for_shell && !mounted.is_default_for_shell {
                    mounted.is_default_for_shell = true;
                }
                updated[index] = mounted.clone();
            } else {
                updated.push(mounted.clone());
            }
            normalize_default_flags(&mut updated)?;
            let target = resolve_environment_target_for_session_with_attachments(
                &self.config,
                &state.session_id,
                &updated,
            )
            .map_err(configuration_rpc_error)?;
            environment
                .switchable
                .replace_target(SwitchableEnvironmentTarget::new(
                    target.provider,
                    target.process_provider,
                ))
                .map_err(environment_rpc_error)?;
            environment.attachments = updated;
            environment.binding_version = environment.binding_version.saturating_add(1);
            let binding_version = environment.binding_version;
            let attachments = environment.attachments.clone();
            let current_default = default_mount_id(&attachments);
            let current_default_shell = default_shell_mount_id(&attachments);
            ActiveMountMutation {
                mounted,
                previous_binding_version,
                binding_version,
                previous_default,
                current_default,
                previous_default_shell,
                current_default_shell,
                attachments,
            }
        };
        let lifecycle_extra = json!({
            "action": "mounted",
            "mount": mount_summary(&mutation.mounted, "ready"),
            "previousBindingVersion": mutation.previous_binding_version,
            "previousDefaultMountId": mutation.previous_default,
            "currentDefaultMountId": mutation.current_default,
            "previousDefaultShellMountId": mutation.previous_default_shell,
            "currentDefaultShellMountId": mutation.current_default_shell,
        });
        let lifecycle_environment =
            environment_summary(mutation.binding_version, &mutation.attachments);
        let lifecycle = environment_lifecycle_event(
            &params.run_id,
            &state.session_id,
            &operation_id,
            "environment_mounted",
            mutation.binding_version,
            &lifecycle_environment,
            &lifecycle_extra,
        );
        publish_environment_lifecycle_to_state(&params.run_id, state, event_sequence, lifecycle);
        let mut warnings = Vec::new();
        let steering_cursor = if params.inject_context {
            match state.steering_sender.send(CliSteeringMessage {
                id: operation_id.clone(),
                text: render_environment_steering_text("mounted", &mutation.mounted),
            }) {
                Ok(()) => Some(event_sequence),
                Err(error) => {
                    warnings.push(json!({
                        "code": "steering_injection_failed",
                        "message": error.to_string(),
                    }));
                    None
                }
            }
        } else {
            None
        };
        let result = active_mount_result(
            &params.run_id,
            &operation_id,
            &mutation.mounted,
            mutation.previous_binding_version,
            mutation.binding_version,
            params.replace,
            mutation.previous_default,
            mutation.current_default,
            mutation.previous_default_shell,
            mutation.current_default_shell,
            &mutation.attachments,
            event_sequence,
            steering_cursor,
            warnings,
        );
        if let Some(key) = params.idempotency_key.as_deref()
            && let Some(environment) = state.environment.as_mut()
        {
            environment.idempotency.insert(
                mutation_key("mount", key),
                ActiveMutationIdempotencyRecord {
                    params_digest: params_digest.to_string(),
                    result: result.clone(),
                },
            );
        }
        drop(runs);
        Ok(ActiveMountOutcome {
            result,
            applied: true,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn active_environment_unmount(
        &self,
        params: &EnvironmentActiveUnmountParams,
        params_digest: &str,
    ) -> Result<ActiveUnmountOutcome, starweaver_rpc_core::RpcError> {
        let prepared = match self.prepare_active_unmount(params, params_digest)? {
            ActiveUnmountPreparation::Idempotent { result, removed } => {
                return Ok(ActiveUnmountOutcome {
                    result,
                    removed,
                    applied: false,
                });
            }
            ActiveUnmountPreparation::Mutation(prepared) => prepared,
        };
        ensure_mount_has_no_live_processes(&prepared.switchable, &params.mount_id).await?;
        let target = resolve_environment_target_for_session_with_attachments(
            &self.config,
            &prepared.session_id,
            &prepared.updated,
        )
        .map_err(configuration_rpc_error)?;
        let mut runs = self
            .active_runs
            .lock()
            .map_err(|error| starweaver_rpc_core::RpcError::new(RUN_CONFLICT, error.to_string()))?;
        let Some(state) = runs.get_mut(&params.run_id) else {
            return Err(starweaver_rpc_core::RpcError::new(
                RUN_CONFLICT,
                format!("active run not found: {}", params.run_id),
            ));
        };
        let operation_id = format!("envop_{}", chrono::Utc::now().timestamp_micros());
        let event_sequence = next_display_sequence(state);
        let mutation = {
            let Some(environment) = state.environment.as_mut() else {
                return Err(starweaver_rpc_core::RpcError::new(
                    RUN_CONFLICT,
                    "active run has no mutable environment binding",
                ));
            };
            if let Some(key) = params.idempotency_key.as_deref()
                && let Some(record) = environment.idempotency.get(&mutation_key("unmount", key))
            {
                ensure_idempotency_digest(record, params_digest)?;
                let removed = environment
                    .attachments
                    .iter()
                    .find(|attachment| attachment.id == params.mount_id)
                    .cloned()
                    .unwrap_or_else(|| tombstone_attachment(&params.mount_id));
                return Ok(ActiveUnmountOutcome {
                    result: record.result.clone(),
                    removed,
                    applied: false,
                });
            }
            if environment.binding_version != prepared.previous_binding_version {
                return Err(starweaver_rpc_core::RpcError::new(
                    RUN_CONFLICT,
                    format!(
                        "environment binding changed during unmount: expected {}, current {}",
                        prepared.previous_binding_version, environment.binding_version
                    ),
                ));
            }
            environment
                .switchable
                .replace_target(SwitchableEnvironmentTarget::new(
                    target.provider,
                    target.process_provider,
                ))
                .map_err(environment_rpc_error)?;
            environment.attachments = prepared.updated;
            environment.binding_version = environment.binding_version.saturating_add(1);
            let binding_version = environment.binding_version;
            let attachments = environment.attachments.clone();
            let current_default = default_mount_id(&attachments);
            let current_default_shell = default_shell_mount_id(&attachments);
            ActiveUnmountMutation {
                removed: prepared.removed,
                previous_binding_version: prepared.previous_binding_version,
                binding_version,
                previous_default: prepared.previous_default,
                current_default,
                previous_default_shell: prepared.previous_default_shell,
                current_default_shell,
                attachments,
            }
        };
        let lifecycle_extra = json!({
            "action": "unmounted",
            "mount": mount_summary(&mutation.removed, "detached"),
            "previousBindingVersion": mutation.previous_binding_version,
            "previousDefaultMountId": mutation.previous_default,
            "currentDefaultMountId": mutation.current_default,
            "previousDefaultShellMountId": mutation.previous_default_shell,
            "currentDefaultShellMountId": mutation.current_default_shell,
        });
        let lifecycle_environment =
            environment_summary(mutation.binding_version, &mutation.attachments);
        let lifecycle = environment_lifecycle_event(
            &params.run_id,
            &state.session_id,
            &operation_id,
            "environment_unmounted",
            mutation.binding_version,
            &lifecycle_environment,
            &lifecycle_extra,
        );
        publish_environment_lifecycle_to_state(&params.run_id, state, event_sequence, lifecycle);
        let mut warnings = Vec::new();
        let steering_cursor = if params.inject_context {
            match state.steering_sender.send(CliSteeringMessage {
                id: operation_id.clone(),
                text: render_environment_steering_text("unmounted", &mutation.removed),
            }) {
                Ok(()) => Some(event_sequence),
                Err(error) => {
                    warnings.push(json!({
                        "code": "steering_injection_failed",
                        "message": error.to_string(),
                    }));
                    None
                }
            }
        } else {
            None
        };
        let result = active_unmount_result(
            &params.run_id,
            &operation_id,
            &mutation.removed,
            mutation.previous_binding_version,
            mutation.binding_version,
            mutation.previous_default,
            mutation.current_default,
            mutation.previous_default_shell,
            mutation.current_default_shell,
            &mutation.attachments,
            event_sequence,
            steering_cursor,
            warnings,
        );
        if let Some(key) = params.idempotency_key.as_deref()
            && let Some(environment) = state.environment.as_mut()
        {
            environment.idempotency.insert(
                mutation_key("unmount", key),
                ActiveMutationIdempotencyRecord {
                    params_digest: params_digest.to_string(),
                    result: result.clone(),
                },
            );
        }
        drop(runs);
        Ok(ActiveUnmountOutcome {
            result,
            removed: mutation.removed,
            applied: true,
        })
    }

    fn prepare_active_unmount(
        &self,
        params: &EnvironmentActiveUnmountParams,
        params_digest: &str,
    ) -> Result<ActiveUnmountPreparation, starweaver_rpc_core::RpcError> {
        let mut runs = self
            .active_runs
            .lock()
            .map_err(|error| starweaver_rpc_core::RpcError::new(RUN_CONFLICT, error.to_string()))?;
        let Some(state) = runs.get_mut(&params.run_id) else {
            return Err(starweaver_rpc_core::RpcError::new(
                RUN_CONFLICT,
                format!("active run not found: {}", params.run_id),
            ));
        };
        let Some(environment) = state.environment.as_mut() else {
            return Err(starweaver_rpc_core::RpcError::new(
                RUN_CONFLICT,
                "active run has no mutable environment binding",
            ));
        };
        if let Some(key) = params.idempotency_key.as_deref()
            && let Some(record) = environment.idempotency.get(&mutation_key("unmount", key))
        {
            ensure_idempotency_digest(record, params_digest)?;
            let result = record.result.clone();
            let removed = environment
                .attachments
                .iter()
                .find(|attachment| attachment.id == params.mount_id)
                .cloned()
                .unwrap_or_else(|| tombstone_attachment(&params.mount_id));
            drop(runs);
            return Ok(ActiveUnmountPreparation::Idempotent { result, removed });
        }
        ensure_expected_binding(environment, params.expected_binding_version)?;
        let previous_binding_version = environment.binding_version;
        let previous_default = default_mount_id(&environment.attachments);
        let previous_default_shell = default_shell_mount_id(&environment.attachments);
        let Some(index) = environment
            .attachments
            .iter()
            .position(|attachment| attachment.id == params.mount_id)
        else {
            return Err(starweaver_rpc_core::RpcError::new(
                INVALID_PARAMS,
                format!("unknown environment mount: {}", params.mount_id),
            ));
        };
        let removed = environment.attachments[index].clone();
        let mut updated = environment.attachments.clone();
        updated.remove(index);
        if updated.is_empty() {
            return Err(starweaver_rpc_core::RpcError::new(
                INVALID_PARAMS,
                "active environment binding must keep at least one mount",
            ));
        }
        apply_unmount_defaults(&mut updated, &removed, params)?;
        normalize_default_flags(&mut updated)?;
        let preparation = ActiveUnmountPreparation::Mutation(PreparedActiveUnmount {
            session_id: state.session_id.clone(),
            switchable: environment.switchable.clone(),
            removed,
            updated,
            previous_binding_version,
            previous_default,
            previous_default_shell,
        });
        drop(runs);
        Ok(preparation)
    }

    pub(super) fn run_status(&self, session_id: &str, run_id: &str) -> CliResult<RunStatusItem> {
        if let Ok(runs) = self.active_runs.lock()
            && let Some(state) = runs.get(run_id)
        {
            return Ok(RunStatusItem {
                session_id: state.session_id.clone(),
                run_id: run_id.to_string(),
                status: state.status.clone(),
                output_preview: state.output_preview.clone(),
                error: state.error.clone(),
            });
        }
        let store = LocalStore::open(&self.config)?;
        let run = store.load_run(session_id, run_id)?;
        let error = (run.status == RunStatus::Failed)
            .then(|| run.output_preview.clone())
            .flatten();
        Ok(RunStatusItem {
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            status: run_status_name(run.status).to_string(),
            output_preview: run.output_preview,
            error,
        })
    }

    fn attach_active_run(
        &self,
        session_id: &str,
        run_id: &str,
        cursor: Option<&ReplayCursor>,
    ) -> Option<RunAttachment> {
        let (sender, receiver) = mpsc::channel::<RunStreamEvent>();
        let (active, events) = {
            let mut runs = self.active_runs.lock().ok()?;
            let state = runs.get_mut(run_id)?;
            if state.session_id != session_id {
                return None;
            }
            let active = is_subscribable_status(&state.status);
            let events = state
                .replay_events
                .iter()
                .filter(|event| cursor.is_none_or(|cursor| event.sequence > cursor.sequence))
                .cloned()
                .collect();
            if active {
                state.subscribers.push(RunSubscriber {
                    include_raw: false,
                    cursor: SubscriberCursor::Run,
                    sender,
                });
            }
            drop(runs);
            (active, events)
        };
        Some(RunAttachment {
            session_id: session_id.to_string(),
            run_id: Some(run_id.to_string()),
            active,
            events,
            subscription: active.then_some(receiver),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_active_run(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    session_id: &str,
    run_id: &str,
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    sequence_no: usize,
    subscriber: RunSubscriber,
    environment: Option<ActiveEnvironmentBinding>,
) {
    if let Ok(mut runs) = active_runs.lock() {
        runs.insert(
            run_id.to_string(),
            ActiveRunState {
                session_id: session_id.to_string(),
                sequence_no,
                status: "running".to_string(),
                output_preview: None,
                error: None,
                display_messages: Vec::new(),
                replay_events: Vec::new(),
                steering_sender,
                cancel_sender,
                subscribers: vec![subscriber],
                environment,
            },
        );
    }
}

fn active_environment_binding(
    environment: &ResolvedEnvironment,
) -> Option<ActiveEnvironmentBinding> {
    Some(ActiveEnvironmentBinding {
        attachments: environment.attachments.clone(),
        binding_version: 1,
        switchable: environment.switchable.clone()?,
        latest_environment_sequence: None,
        idempotency: HashMap::new(),
    })
}

fn ensure_idempotency_digest(
    record: &ActiveMutationIdempotencyRecord,
    params_digest: &str,
) -> Result<(), starweaver_rpc_core::RpcError> {
    if record.params_digest == params_digest {
        Ok(())
    } else {
        Err(starweaver_rpc_core::RpcError::new(
            IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with different params",
        ))
    }
}

fn ensure_expected_binding(
    environment: &ActiveEnvironmentBinding,
    expected: Option<u64>,
) -> Result<(), starweaver_rpc_core::RpcError> {
    if expected.is_some_and(|expected| expected != environment.binding_version) {
        return Err(starweaver_rpc_core::RpcError::new(
            RUN_CONFLICT,
            format!(
                "environment binding version mismatch: expected {}, current {}",
                expected.unwrap_or_default(),
                environment.binding_version
            ),
        ));
    }
    Ok(())
}

async fn ensure_mount_has_no_live_processes(
    switchable: &SwitchableEnvironmentProvider,
    mount_id: &str,
) -> Result<(), starweaver_rpc_core::RpcError> {
    let Some(process_provider) = switchable
        .process_provider()
        .map_err(environment_rpc_error)?
    else {
        return Ok(());
    };
    let process_prefix = format!("{mount_id}:");
    let processes = process_provider
        .list_processes()
        .await
        .map_err(environment_rpc_error)?;
    if let Some(process) = processes.into_iter().find(|process| {
        process.status == ShellProcessStatus::Running
            && process.process_id.starts_with(&process_prefix)
    }) {
        return Err(starweaver_rpc_core::RpcError::new(
            RUN_CONFLICT,
            format!(
                "environment mount owns a live background process: {}",
                process.process_id
            ),
        ));
    }
    Ok(())
}

fn normalize_default_flags(
    attachments: &mut [EnvironmentAttachmentRef],
) -> Result<(), starweaver_rpc_core::RpcError> {
    let default_ids = attachments
        .iter()
        .enumerate()
        .filter_map(|(index, attachment)| attachment.is_default.then_some(index))
        .collect::<Vec<_>>();
    match default_ids.as_slice() {
        [default_index] => {
            for (index, attachment) in attachments.iter_mut().enumerate() {
                attachment.is_default = index == *default_index;
            }
        }
        [] if attachments.len() == 1 => {
            attachments[0].is_default = true;
        }
        [] => {
            return Err(starweaver_rpc_core::RpcError::new(
                INVALID_PARAMS,
                "active environment binding requires one default mount",
            ));
        }
        _ => {
            let last_default = *default_ids.last().unwrap_or(&0);
            for (index, attachment) in attachments.iter_mut().enumerate() {
                attachment.is_default = index == last_default;
            }
        }
    }

    let default_shell_ids = attachments
        .iter()
        .enumerate()
        .filter_map(|(index, attachment)| attachment.is_default_for_shell.then_some(index))
        .collect::<Vec<_>>();
    match default_shell_ids.as_slice() {
        [default_shell_index] => {
            if !attachment_supports_shell_default(&attachments[*default_shell_index]) {
                return Err(starweaver_rpc_core::RpcError::new(
                    INVALID_PARAMS,
                    format!(
                        "environment mount cannot be defaultForShell: {}",
                        attachments[*default_shell_index].id
                    ),
                ));
            }
            for (index, attachment) in attachments.iter_mut().enumerate() {
                attachment.is_default_for_shell = index == *default_shell_index;
            }
        }
        [] => {
            let default_index = attachments
                .iter()
                .position(|attachment| attachment.is_default)
                .ok_or_else(|| {
                    starweaver_rpc_core::RpcError::new(
                        INVALID_PARAMS,
                        "active environment binding requires one default mount",
                    )
                })?;
            if attachment_supports_shell_default(&attachments[default_index]) {
                attachments[default_index].is_default_for_shell = true;
            }
        }
        _ => {
            let last_default_shell = *default_shell_ids.last().unwrap_or(&0);
            if !attachment_supports_shell_default(&attachments[last_default_shell]) {
                return Err(starweaver_rpc_core::RpcError::new(
                    INVALID_PARAMS,
                    format!(
                        "environment mount cannot be defaultForShell: {}",
                        attachments[last_default_shell].id
                    ),
                ));
            }
            for (index, attachment) in attachments.iter_mut().enumerate() {
                attachment.is_default_for_shell = index == last_default_shell;
            }
        }
    }
    Ok(())
}

fn attachment_supports_shell_default(attachment: &EnvironmentAttachmentRef) -> bool {
    attachment.resolved_mode() == starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadWrite
}

fn apply_unmount_defaults(
    attachments: &mut [EnvironmentAttachmentRef],
    removed: &EnvironmentAttachmentRef,
    params: &EnvironmentActiveUnmountParams,
) -> Result<(), starweaver_rpc_core::RpcError> {
    if removed.is_default {
        let Some(new_default) = params.new_default_mount_id.as_deref() else {
            return Err(starweaver_rpc_core::RpcError::new(
                INVALID_PARAMS,
                "newDefaultMountId is required when unmounting the default mount",
            ));
        };
        set_default_mount(attachments, new_default)?;
    }
    if removed.is_default_for_shell {
        if let Some(new_default_shell) = params.new_default_shell_mount_id.as_deref() {
            set_default_shell_mount(attachments, new_default_shell)?;
        } else {
            for attachment in attachments.iter_mut() {
                attachment.is_default_for_shell = false;
            }
        }
    }
    Ok(())
}

fn set_default_mount(
    attachments: &mut [EnvironmentAttachmentRef],
    id: &str,
) -> Result<(), starweaver_rpc_core::RpcError> {
    if !attachments.iter().any(|attachment| attachment.id == id) {
        return Err(starweaver_rpc_core::RpcError::new(
            INVALID_PARAMS,
            format!("unknown new default mount: {id}"),
        ));
    }
    for attachment in attachments {
        attachment.is_default = attachment.id == id;
    }
    Ok(())
}

fn set_default_shell_mount(
    attachments: &mut [EnvironmentAttachmentRef],
    id: &str,
) -> Result<(), starweaver_rpc_core::RpcError> {
    if !attachments.iter().any(|attachment| attachment.id == id) {
        return Err(starweaver_rpc_core::RpcError::new(
            INVALID_PARAMS,
            format!("unknown new default shell mount: {id}"),
        ));
    }
    for attachment in attachments {
        attachment.is_default_for_shell = attachment.id == id;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn environment_lifecycle_event(
    run_id: &str,
    session_id: &str,
    operation_id: &str,
    kind: &str,
    binding_version: u64,
    environment: &Value,
    extra: &Value,
) -> EnvironmentLifecycleEvent {
    EnvironmentLifecycleEvent {
        operation_kind: kind.to_string(),
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        binding_version,
        environment: environment.clone(),
        operation_id: Some(operation_id.to_string()),
        extra: extra.as_object().cloned().unwrap_or_default(),
    }
}

fn publish_environment_lifecycle_to_state(
    run_id: &str,
    state: &mut ActiveRunState,
    sequence: usize,
    lifecycle: EnvironmentLifecycleEvent,
) {
    let display_message = lifecycle.to_display_message(sequence);
    let event = ReplayEvent::new(
        ReplayScope::run(run_id),
        sequence,
        ReplayEventKind::EnvironmentLifecycle(Box::new(lifecycle)),
    );
    state.display_messages.push(display_message);
    state.replay_events.push(event.clone());
    if let Some(environment) = state.environment.as_mut() {
        environment.latest_environment_sequence = Some(sequence);
    }
    state.subscribers.retain(|subscriber| {
        let event = replay_event_for_subscriber(event.clone(), subscriber);
        subscriber
            .sender
            .send(RunStreamEvent::Output(Box::new(event)))
            .is_ok()
    });
}

fn publish_initial_environment_info(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    run_id: &str,
) {
    let Ok(mut runs) = active_runs.lock() else {
        return;
    };
    let Some(state) = runs.get_mut(run_id) else {
        return;
    };
    let Some(environment) = state.environment.as_ref() else {
        return;
    };
    let binding_version = environment.binding_version;
    let attachments = environment.attachments.clone();
    let sequence = next_display_sequence(state);
    let operation_id = format!("envop_{}", chrono::Utc::now().timestamp_micros());
    let lifecycle_environment = environment_summary(binding_version, &attachments);
    let lifecycle_extra = json!({});
    let lifecycle = environment_lifecycle_event(
        run_id,
        &state.session_id,
        &operation_id,
        "environment_info",
        binding_version,
        &lifecycle_environment,
        &lifecycle_extra,
    );
    publish_environment_lifecycle_to_state(run_id, state, sequence, lifecycle);
}

fn environment_lifecycle_message_inserts(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    run_id: &str,
) -> Vec<(usize, DisplayMessage)> {
    let Ok(runs) = active_runs.lock() else {
        return Vec::new();
    };
    let Some(state) = runs.get(run_id) else {
        return Vec::new();
    };
    let mut non_lifecycle_count = 0_usize;
    let mut inserts = Vec::new();
    for message in &state.display_messages {
        if is_environment_lifecycle_message(message) {
            inserts.push((non_lifecycle_count, message.clone()));
        } else {
            non_lifecycle_count = non_lifecycle_count.saturating_add(1);
        }
    }
    inserts
}

fn is_environment_lifecycle_kind(kind: &str) -> bool {
    matches!(
        kind,
        "environment_info" | "environment_mounted" | "environment_unmounted"
    )
}

fn is_environment_lifecycle_message(message: &DisplayMessage) -> bool {
    message.kind == DisplayMessageKind::HostEvent
        && message
            .payload
            .get("operationKind")
            .and_then(Value::as_str)
            .is_some_and(is_environment_lifecycle_kind)
}

fn next_display_sequence(state: &ActiveRunState) -> usize {
    state
        .display_messages
        .iter()
        .map(|message| message.sequence)
        .max()
        .map_or(0, |sequence| sequence.saturating_add(1))
}

fn environment_summary(binding_version: u64, attachments: &[EnvironmentAttachmentRef]) -> Value {
    json!({
        "bindingVersion": binding_version,
        "defaultMountId": default_mount_id(attachments),
        "defaultShellMountId": default_shell_mount_id(attachments),
        "mounts": attachments
            .iter()
            .map(|attachment| mount_summary(attachment, "ready"))
            .collect::<Vec<_>>(),
    })
}

fn mount_summary(attachment: &EnvironmentAttachmentRef, status: &str) -> Value {
    json!({
        "id": attachment.id,
        "kind": attachment.kind,
        "root": format!("/environment/{}", attachment.id),
        "mode": attachment.resolved_mode(),
        "default": attachment.is_default,
        "defaultForShell": attachment.is_default_for_shell,
        "status": status,
        "readiness": {},
        "environmentId": attachment.environment_id.clone(),
        "metadata": attachment.metadata.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn active_mount_result(
    run_id: &str,
    operation_id: &str,
    mounted: &EnvironmentAttachmentRef,
    previous_binding_version: u64,
    binding_version: u64,
    replace: bool,
    previous_default: Option<String>,
    current_default: Option<String>,
    previous_default_shell: Option<String>,
    current_default_shell: Option<String>,
    attachments: &[EnvironmentAttachmentRef],
    event_sequence: usize,
    steering_sequence: Option<usize>,
    warnings: Vec<Value>,
) -> Value {
    mutation_result(
        run_id,
        operation_id,
        &mounted.id,
        previous_binding_version,
        binding_version,
        previous_default,
        current_default,
        previous_default_shell,
        current_default_shell,
        attachments,
        event_sequence,
        steering_sequence,
        warnings,
        json!({
            "replace": replace,
            "mount": mount_summary(mounted, "ready"),
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn active_unmount_result(
    run_id: &str,
    operation_id: &str,
    removed: &EnvironmentAttachmentRef,
    previous_binding_version: u64,
    binding_version: u64,
    previous_default: Option<String>,
    current_default: Option<String>,
    previous_default_shell: Option<String>,
    current_default_shell: Option<String>,
    attachments: &[EnvironmentAttachmentRef],
    event_sequence: usize,
    steering_sequence: Option<usize>,
    warnings: Vec<Value>,
) -> Value {
    mutation_result(
        run_id,
        operation_id,
        &removed.id,
        previous_binding_version,
        binding_version,
        previous_default,
        current_default,
        previous_default_shell,
        current_default_shell,
        attachments,
        event_sequence,
        steering_sequence,
        warnings,
        json!({
            "removedMount": mount_summary(removed, "detached"),
        }),
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_pass_by_value)]
fn mutation_result(
    run_id: &str,
    operation_id: &str,
    mount_id: &str,
    previous_binding_version: u64,
    binding_version: u64,
    previous_default: Option<String>,
    current_default: Option<String>,
    previous_default_shell: Option<String>,
    current_default_shell: Option<String>,
    attachments: &[EnvironmentAttachmentRef],
    event_sequence: usize,
    steering_sequence: Option<usize>,
    warnings: Vec<Value>,
    extra: Value,
) -> Value {
    let mut result = json!({
        "runId": run_id,
        "operationId": operation_id,
        "mountId": mount_id,
        "previousBindingVersion": previous_binding_version,
        "bindingVersion": binding_version,
        "previousDefaultMountId": previous_default,
        "currentDefaultMountId": current_default,
        "previousDefaultShellMountId": previous_default_shell,
        "currentDefaultShellMountId": current_default_shell,
        "environment": environment_summary(binding_version, attachments),
        "eventCursor": cursor_value(run_id, event_sequence),
    });
    if let Some(sequence) = steering_sequence {
        result["steeringCursor"] = cursor_value(run_id, sequence);
    }
    if !warnings.is_empty() {
        result["warnings"] = Value::Array(warnings);
    }
    merge_json_object(&mut result, &extra);
    result
}

fn cursor_value(run_id: &str, sequence: usize) -> Value {
    json!({
        "scope": ReplayScope::run(run_id).as_str(),
        "sequence": sequence,
    })
}

fn default_mount_id(attachments: &[EnvironmentAttachmentRef]) -> Option<String> {
    attachments
        .iter()
        .find(|attachment| attachment.is_default)
        .map(|attachment| attachment.id.clone())
}

fn default_shell_mount_id(attachments: &[EnvironmentAttachmentRef]) -> Option<String> {
    attachments
        .iter()
        .find(|attachment| attachment.is_default_for_shell)
        .map(|attachment| attachment.id.clone())
}

fn render_environment_steering_text(action: &str, attachment: &EnvironmentAttachmentRef) -> String {
    format!(
        "<environment-context action=\"{}\" mount=\"{}\" root=\"/environment/{}\" mode=\"{:?}\" />",
        action,
        attachment.id,
        attachment.id,
        attachment.resolved_mode()
    )
}

fn mutation_key(action: &str, key: &str) -> String {
    format!("{action}:{key}")
}

fn tombstone_attachment(mount_id: &str) -> EnvironmentAttachmentRef {
    EnvironmentAttachmentRef {
        id: mount_id.to_string(),
        kind: "unknown".to_string(),
        mode: None,
        is_default: false,
        is_default_for_shell: false,
        attachment_lease_id: None,
        endpoint_ref: None,
        environment_id: None,
        auth_token: None,
        metadata: serde_json::Map::new(),
    }
}

fn merge_json_object(target: &mut Value, source: &Value) {
    let Some(target) = target.as_object_mut() else {
        return;
    };
    let Some(source) = source.as_object() else {
        return;
    };
    for (key, value) in source {
        target.insert(key.clone(), value.clone());
    }
}

#[allow(clippy::needless_pass_by_value)]
fn configuration_rpc_error(error: CliError) -> starweaver_rpc_core::RpcError {
    starweaver_rpc_core::RpcError::new(starweaver_rpc_core::CONFIGURATION_FAILED, error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn environment_rpc_error(
    error: starweaver_environment::EnvironmentError,
) -> starweaver_rpc_core::RpcError {
    starweaver_rpc_core::RpcError::new(
        starweaver_rpc_core::ENVIRONMENT_UNAVAILABLE,
        error.to_string(),
    )
}

fn forward_runtime_stream(
    receiver: mpsc::Receiver<AgentStreamRecord>,
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    run: &RunRecord,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let projector = DefaultDisplayMessageProjector;
    let context = DisplayProjectionContext::new(run.session_id.clone(), run.run_id.clone());
    for record in receiver {
        publish_raw_record(active_runs, run.run_id.as_str(), &record);
        let messages = runtime.block_on(projector.project(&context, &record));
        if !messages.is_empty() {
            publish_display_messages(active_runs, run.run_id.as_str(), messages);
        }
    }
}

fn publish_raw_record(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    run_id: &str,
    record: &AgentStreamRecord,
) {
    let Ok(mut runs) = active_runs.lock() else {
        return;
    };
    let Some(state) = runs.get_mut(run_id) else {
        return;
    };
    state.subscribers.retain(|subscriber| {
        if subscriber.include_raw {
            subscriber
                .sender
                .send(RunStreamEvent::Raw(Box::new(record.clone())))
                .is_ok()
        } else {
            true
        }
    });
}

fn publish_display_messages(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    run_id: &str,
    messages: Vec<DisplayMessage>,
) {
    let Ok(mut runs) = active_runs.lock() else {
        return;
    };
    let Some(state) = runs.get_mut(run_id) else {
        return;
    };
    for mut message in messages {
        message.sequence = next_display_sequence(state);
        let terminal = terminal_status(message.kind);
        let event = run_replay_event(run_id, message.clone());
        state.display_messages.push(message.clone());
        state.replay_events.push(event.clone());
        state.subscribers.retain(|subscriber| {
            let event = replay_event_for_subscriber(event.clone(), subscriber);
            subscriber
                .sender
                .send(RunStreamEvent::Output(Box::new(event)))
                .is_ok()
        });
        if let Some(status) = terminal {
            state.status = status.to_string();
            state.output_preview.clone_from(&message.preview);
        }
    }
}

fn publish_status(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    session_id: &str,
    run_id: &str,
    status: &str,
    output_preview: Option<String>,
    error: Option<String>,
) {
    let item = RunStatusItem {
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        status: status.to_string(),
        output_preview,
        error,
    };
    let Ok(mut runs) = active_runs.lock() else {
        return;
    };
    if let Some(state) = runs.get_mut(run_id) {
        state.status.clone_from(&item.status);
        state.output_preview.clone_from(&item.output_preview);
        state.error.clone_from(&item.error);
        state.subscribers.retain(|subscriber| {
            subscriber
                .sender
                .send(RunStreamEvent::Status(item.clone()))
                .is_ok()
        });
    }
}

fn remove_active_if_terminal(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    run_id: &str,
) {
    if let Ok(mut runs) = active_runs.lock() {
        runs.remove(run_id);
    }
}

fn queued_display_message(run: &RunRecord) -> DisplayMessage {
    DisplayMessage::new(
        0,
        run.session_id.clone(),
        run.run_id.clone(),
        DisplayMessageKind::RunQueued,
    )
    .with_payload(json!({"sequence_no": run.sequence_no}))
    .with_preview("run queued")
}

fn validate_cursor_scope(cursor: Option<&ReplayCursor>, scope: &ReplayScope) -> CliResult<()> {
    cursor
        .map_or(Ok(()), |cursor| cursor.validate_scope(scope))
        .map_err(|error| CliError::Usage(error.to_string()))
}

fn run_replay_event(run_id: &str, message: DisplayMessage) -> ReplayEvent {
    display_replay_event(&ReplayScope::run(run_id), message.sequence, message)
}

fn persisted_display_message_keys(events: &[ReplayEvent]) -> HashSet<(String, usize)> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            ReplayEventKind::DisplayMessage(message) => Some(display_message_key(message)),
            _ => None,
        })
        .collect()
}

fn replay_event_not_persisted_as_display(
    event: &ReplayEvent,
    persisted_messages: &HashSet<(String, usize)>,
) -> bool {
    match &event.event {
        ReplayEventKind::DisplayMessage(message) => {
            !persisted_messages.contains(&display_message_key(message))
        }
        ReplayEventKind::EnvironmentLifecycle(lifecycle) => {
            let display_message = lifecycle.to_display_message(event.sequence);
            !persisted_messages.contains(&display_message_key(&display_message))
        }
        _ => true,
    }
}

fn display_message_key(message: &DisplayMessage) -> (String, usize) {
    (message.run_id.as_str().to_string(), message.sequence)
}

fn append_session_replay_events(
    scope: &ReplayScope,
    start_sequence: usize,
    events: Vec<ReplayEvent>,
) -> Vec<ReplayEvent> {
    events
        .into_iter()
        .enumerate()
        .map(|(offset, event)| rebase_replay_event(event, scope.clone(), start_sequence + offset))
        .collect()
}

fn filter_replay_events(
    events: Vec<ReplayEvent>,
    cursor: Option<&ReplayCursor>,
) -> Vec<ReplayEvent> {
    events
        .into_iter()
        .filter(|event| cursor.is_none_or(|cursor| event.sequence > cursor.sequence))
        .collect()
}

fn replay_event_for_subscriber(event: ReplayEvent, subscriber: &RunSubscriber) -> ReplayEvent {
    match &subscriber.cursor {
        SubscriberCursor::Run => event,
        SubscriberCursor::Session { next_sequence } => {
            let sequence = next_session_sequence(next_sequence);
            let scope = ReplayScope::session(replay_event_session_id(&event));
            rebase_replay_event(event, scope, sequence)
        }
    }
}

fn rebase_replay_event(mut event: ReplayEvent, scope: ReplayScope, sequence: usize) -> ReplayEvent {
    event.scope = scope;
    event.sequence = sequence;
    event
}

fn replay_event_session_id(event: &ReplayEvent) -> &str {
    match &event.event {
        ReplayEventKind::DisplayMessage(message) => message.session_id.as_str(),
        ReplayEventKind::EnvironmentLifecycle(lifecycle) => &lifecycle.session_id,
        _ => event.scope.as_str(),
    }
}

fn next_session_sequence(next_sequence: &Arc<Mutex<usize>>) -> usize {
    let Ok(mut sequence) = next_sequence.lock() else {
        return 0;
    };
    let current = *sequence;
    *sequence = sequence.saturating_add(1);
    current
}

fn display_replay_event(
    scope: &ReplayScope,
    sequence: usize,
    message: DisplayMessage,
) -> ReplayEvent {
    ReplayEvent::new(
        scope.clone(),
        sequence,
        ReplayEventKind::DisplayMessage(Box::new(message)),
    )
}

fn output_preview(messages: &[DisplayMessage]) -> Option<String> {
    run_output_preview(messages)
}

const fn terminal_status(kind: DisplayMessageKind) -> Option<&'static str> {
    match kind {
        DisplayMessageKind::RunCompleted => Some("completed"),
        DisplayMessageKind::RunFailed => Some("failed"),
        DisplayMessageKind::RunCancelled => Some("cancelled"),
        _ => None,
    }
}

fn is_subscribable_status(status: &str) -> bool {
    !matches!(status, "completed" | "failed" | "cancelled")
}

const fn run_status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;
    use starweaver_environment::EnvironmentProvider;
    use starweaver_rpc_core::{
        EnvironmentActiveMountParams, EnvironmentActiveUnmountParams,
        EnvironmentAttachmentAccessMode, EnvironmentAttachmentRef, IDEMPOTENCY_CONFLICT,
    };

    use super::*;
    use crate::{
        ConfigResolver, args, environment::resolve_environment_for_session_with_attachments,
    };

    fn test_config(root: &std::path::Path) -> CliConfig {
        let cli = args::parse(["starweaver-cli".to_string(), "rpc".to_string()]).unwrap();
        ConfigResolver::for_tests(root).resolve(&cli).unwrap()
    }

    fn local_attachment(
        id: &str,
        is_default: bool,
        is_default_for_shell: bool,
    ) -> EnvironmentAttachmentRef {
        EnvironmentAttachmentRef {
            id: id.to_string(),
            kind: "local".to_string(),
            mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
            is_default,
            is_default_for_shell,
            attachment_lease_id: None,
            endpoint_ref: None,
            environment_id: None,
            auth_token: None,
            metadata: serde_json::Map::new(),
        }
    }

    #[test]
    fn active_run_registry_routes_steering_and_cancel_messages() {
        let temp = tempfile::tempdir().unwrap();
        let coordinator = CliRuntimeCoordinator::new(test_config(temp.path()));
        let (steering_sender, steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, _subscriber_receiver) = mpsc::channel::<RunStreamEvent>();

        insert_active_run(
            &coordinator.active_runs,
            "session_1",
            "run_1",
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            None,
        );

        let run_id = coordinator
            .steer_session(
                "session_1",
                CliSteeringMessage {
                    id: "steer_1".to_string(),
                    text: "continue".to_string(),
                },
            )
            .unwrap();
        assert_eq!(run_id, "run_1");
        let session_steering = steering_receiver.recv().unwrap();
        assert_eq!(session_steering.id, "steer_1");
        assert_eq!(session_steering.text, "continue");

        coordinator
            .steer_run(
                "run_1",
                CliSteeringMessage {
                    id: "steer_2".to_string(),
                    text: "refine".to_string(),
                },
            )
            .unwrap();
        let run_steering = steering_receiver.recv().unwrap();
        assert_eq!(run_steering.id, "steer_2");
        assert_eq!(run_steering.text, "refine");

        coordinator.cancel_run("run_1").unwrap();
        cancel_receiver.recv().unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn active_environment_mutations_update_binding_and_stream_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let coordinator = CliRuntimeCoordinator::new(config.clone());
        let attachments = vec![local_attachment("local", true, true)];
        let environment =
            resolve_environment_for_session_with_attachments(&config, "session_1", &attachments)
                .unwrap();
        let (steering_sender, steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, _cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, subscriber_receiver) = mpsc::channel::<RunStreamEvent>();
        insert_active_run(
            &coordinator.active_runs,
            "session_1",
            "run_1",
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            active_environment_binding(&environment),
        );

        let initial = coordinator.active_environment_list("run_1").unwrap();
        assert_eq!(initial["environment"]["bindingVersion"], 1);
        assert_eq!(initial["environment"]["defaultMountId"], "local");

        let mount_params = EnvironmentActiveMountParams {
            run_id: "run_1".to_string(),
            attachment: local_attachment("scratch", false, false),
            replace: false,
            inject_context: true,
            expected_binding_version: Some(1),
            idempotency_key: Some("mount-scratch".to_string()),
        };
        let mounted = coordinator
            .active_environment_mount(
                &mount_params,
                mount_params.attachment.clone(),
                "mount-digest",
            )
            .unwrap();
        assert!(mounted.applied);
        assert_eq!(mounted.result["bindingVersion"], 2);
        assert_eq!(mounted.result["mountId"], "scratch");
        assert_eq!(
            mounted.result["environment"]["mounts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let RunStreamEvent::Output(event) = subscriber_receiver.recv().unwrap() else {
            panic!("expected lifecycle output event");
        };
        let ReplayEventKind::EnvironmentLifecycle(lifecycle) = &event.event else {
            panic!("expected environment lifecycle event");
        };
        assert_eq!(lifecycle.operation_kind, "environment_mounted");
        assert_eq!(lifecycle.extra["mount"]["id"], "scratch");
        assert!(
            steering_receiver
                .recv()
                .unwrap()
                .text
                .contains("mount=\"scratch\"")
        );

        let replayed = coordinator
            .active_environment_mount(
                &mount_params,
                mount_params.attachment.clone(),
                "mount-digest",
            )
            .unwrap();
        assert!(!replayed.applied);
        assert_eq!(replayed.result, mounted.result);
        let mut conflicting_mount_params = mount_params.clone();
        conflicting_mount_params.attachment = local_attachment("other", false, false);
        let conflict = coordinator
            .active_environment_mount(
                &conflicting_mount_params,
                conflicting_mount_params.attachment.clone(),
                "different-mount-digest",
            )
            .unwrap_err();
        assert_eq!(conflict.code, IDEMPOTENCY_CONFLICT);

        let unmount_params = EnvironmentActiveUnmountParams {
            run_id: "run_1".to_string(),
            mount_id: "scratch".to_string(),
            new_default_mount_id: None,
            new_default_shell_mount_id: None,
            inject_context: false,
            expected_binding_version: Some(2),
            idempotency_key: Some("unmount-scratch".to_string()),
        };
        let unmounted = coordinator
            .active_environment_unmount(&unmount_params, "unmount-digest")
            .await
            .unwrap();
        assert!(unmounted.applied);
        assert_eq!(unmounted.removed.id, "scratch");
        assert_eq!(unmounted.result["bindingVersion"], 3);
        assert_eq!(
            unmounted.result["environment"]["mounts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let replayed_unmount = coordinator
            .active_environment_unmount(&unmount_params, "unmount-digest")
            .await
            .unwrap();
        assert!(!replayed_unmount.applied);
        assert_eq!(replayed_unmount.result, unmounted.result);
        let current = coordinator.active_environment_list("run_1").unwrap();
        assert_eq!(current["environment"]["bindingVersion"], 3);
        assert_eq!(current["environment"]["mounts"][0]["id"], "local");
    }

    #[tokio::test]
    async fn active_environment_unmount_rejects_mount_with_live_processes() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let coordinator = CliRuntimeCoordinator::new(config);
        let attachments = vec![
            local_attachment("local", true, true),
            local_attachment("scratch", false, false),
        ];
        let local_provider = Arc::new(starweaver_environment::VirtualEnvironmentProvider::new(
            "local",
        ));
        let scratch_provider = Arc::new(
            starweaver_environment::VirtualEnvironmentProvider::new("scratch").with_process(
                starweaver_environment::ShellProcessSnapshot {
                    process_id: "process_1".to_string(),
                    command: "sleep 5".to_string(),
                    status: ShellProcessStatus::Running,
                    stdout: String::new(),
                    stderr: String::new(),
                    return_code: None,
                    metadata: starweaver_core::Metadata::default(),
                },
            ),
        );
        let composite = Arc::new(
            starweaver_environment::CompositeEnvironmentProvider::new(vec![
                starweaver_environment::EnvironmentMount::new("local", local_provider)
                    .unwrap()
                    .with_default(true)
                    .with_default_for_shell(true),
                starweaver_environment::EnvironmentMount::new("scratch", scratch_provider).unwrap(),
            ])
            .unwrap(),
        );
        let target_process_provider = composite.clone().process_shell_provider();
        let switchable = Arc::new(SwitchableEnvironmentProvider::new(
            "test-active-environment",
            SwitchableEnvironmentTarget::new(composite, target_process_provider),
        ));
        let environment = ResolvedEnvironment {
            provider: switchable.clone() as starweaver_environment::DynEnvironmentProvider,
            process_provider: Some(
                switchable.clone() as starweaver_environment::DynProcessShellProvider
            ),
            switchable: Some(switchable),
            attachments,
        };
        let (steering_sender, _steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, _cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, _subscriber_receiver) = mpsc::channel::<RunStreamEvent>();
        insert_active_run(
            &coordinator.active_runs,
            "session_1",
            "run_1",
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            active_environment_binding(&environment),
        );

        let error = coordinator
            .active_environment_unmount(
                &EnvironmentActiveUnmountParams {
                    run_id: "run_1".to_string(),
                    mount_id: "scratch".to_string(),
                    new_default_mount_id: None,
                    new_default_shell_mount_id: None,
                    inject_context: false,
                    expected_binding_version: Some(1),
                    idempotency_key: None,
                },
                "unmount-live-process",
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, RUN_CONFLICT);
        assert!(error.message.contains("scratch:process_1"));
        let current = coordinator.active_environment_list("run_1").unwrap();
        assert_eq!(current["environment"]["bindingVersion"], 1);
        assert_eq!(
            current["environment"]["mounts"].as_array().unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn active_environment_mount_accepts_stdio_envd_target() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let coordinator = CliRuntimeCoordinator::new(config);
        let environment =
            resolve_environment_for_session_with_attachments(&coordinator.config, "session_1", &[])
                .unwrap();
        let (steering_sender, _steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, _cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, _subscriber_receiver) = mpsc::channel::<RunStreamEvent>();
        insert_active_run(
            &coordinator.active_runs,
            "session_1",
            "run_1",
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            active_environment_binding(&environment),
        );

        let mount_params = EnvironmentActiveMountParams {
            run_id: "run_1".to_string(),
            attachment: EnvironmentAttachmentRef {
                id: "stdio_data".to_string(),
                kind: "envd".to_string(),
                mode: Some(EnvironmentAttachmentAccessMode::ReadOnly),
                is_default: false,
                is_default_for_shell: false,
                attachment_lease_id: None,
                endpoint_ref: Some(format!(
                    "stdio://{}?arg=--help",
                    std::env::current_exe().unwrap().display()
                )),
                environment_id: Some("default".to_string()),
                auth_token: None,
                metadata: serde_json::Map::new(),
            },
            replace: false,
            inject_context: false,
            expected_binding_version: Some(1),
            idempotency_key: Some("mount-stdio-envd".to_string()),
        };

        let mounted = coordinator
            .active_environment_mount(
                &mount_params,
                mount_params.attachment.clone(),
                "stdio-digest",
            )
            .unwrap();

        assert!(mounted.applied);
        assert_eq!(mounted.result["bindingVersion"], 2);
        assert_eq!(mounted.result["mount"]["kind"], "envd");
        assert_eq!(mounted.result["mount"]["id"], "stdio_data");
    }

    #[test]
    fn run_replay_event_uses_run_scope_cursor() {
        let message = DisplayMessage::new(
            3,
            starweaver_core::SessionId::from_string("session_1"),
            starweaver_core::RunId::from_string("run_1"),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({"delta": "hello"}));

        let output = run_replay_event("run_1", message);

        assert_eq!(output.scope, ReplayScope::run("run_1"));
        assert_eq!(output.sequence, 3);
        let ReplayEventKind::DisplayMessage(message) = output.event else {
            panic!("expected display message");
        };
        assert_eq!(message.payload["delta"], "hello");
    }

    #[test]
    fn output_preview_skips_internal_compaction_messages() {
        let session_id = starweaver_core::SessionId::from_string("session_1");
        let run_id = starweaver_core::RunId::from_string("run_1");
        let messages = vec![
            DisplayMessage::new(
                0,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::RunCompleted,
            )
            .with_payload(json!({"output": "final answer"}))
            .with_preview("final answer"),
            DisplayMessage::new(
                1,
                session_id,
                run_id,
                DisplayMessageKind::CompactionCompleted,
            )
            .with_preview("display compaction completed"),
        ];

        assert_eq!(output_preview(&messages).as_deref(), Some("final answer"));
    }

    #[test]
    fn terminal_active_run_window_returns_events_without_new_subscribers() {
        let temp = tempfile::tempdir().unwrap();
        let coordinator = CliRuntimeCoordinator::new(test_config(temp.path()));
        let (steering_sender, _steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, _cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, _subscriber_receiver) = mpsc::channel::<RunStreamEvent>();

        insert_active_run(
            &coordinator.active_runs,
            "session_1",
            "run_1",
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            None,
        );
        publish_display_messages(
            &coordinator.active_runs,
            "run_1",
            vec![
                DisplayMessage::new(
                    1,
                    starweaver_core::SessionId::from_string("session_1"),
                    starweaver_core::RunId::from_string("run_1"),
                    DisplayMessageKind::RunCompleted,
                )
                .with_payload(json!({"output": "done"})),
            ],
        );

        let attachment = coordinator
            .attach_active_run("session_1", "run_1", None)
            .unwrap();

        assert!(!attachment.active);
        assert!(attachment.subscription.is_none());
        assert_eq!(attachment.events.len(), 1);
        let ReplayEventKind::DisplayMessage(message) = &attachment.events[0].event else {
            panic!("expected display message");
        };
        assert_eq!(message.payload["output"], "done");
        assert_eq!(
            coordinator
                .active_runs
                .lock()
                .unwrap()
                .get("run_1")
                .unwrap()
                .subscribers
                .len(),
            1
        );
        assert_eq!(
            coordinator
                .active_runs
                .lock()
                .unwrap()
                .get("run_1")
                .unwrap()
                .status,
            "completed"
        );
    }

    #[test]
    fn session_output_replays_active_messages_and_streams_live_tail() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let mut store = crate::LocalStore::open(&config).unwrap();
        let session = store
            .create_session(&config.default_profile, Some("active session".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        drop(store);

        let coordinator = CliRuntimeCoordinator::new(config);
        let (steering_sender, _steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, _cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, _subscriber_receiver) = mpsc::channel::<RunStreamEvent>();
        insert_active_run(
            &coordinator.active_runs,
            &session_id,
            "run_1",
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            None,
        );
        publish_display_messages(
            &coordinator.active_runs,
            "run_1",
            vec![
                DisplayMessage::new(
                    1,
                    starweaver_core::SessionId::from_string(session_id.clone()),
                    starweaver_core::RunId::from_string("run_1"),
                    DisplayMessageKind::AssistantTextDelta,
                )
                .with_payload(json!({"delta": "first"})),
            ],
        );

        let mut attachment = coordinator.session_output(&session_id, None, None).unwrap();
        assert!(attachment.active);
        assert_eq!(attachment.events.len(), 1);
        assert_eq!(
            attachment.events[0].scope,
            ReplayScope::session(&session_id)
        );
        assert_eq!(attachment.events[0].sequence, 0);
        let ReplayEventKind::DisplayMessage(message) = &attachment.events[0].event else {
            panic!("expected display message");
        };
        assert_eq!(message.payload["delta"], "first");
        let receiver = attachment.subscription.take().unwrap();

        publish_display_messages(
            &coordinator.active_runs,
            "run_1",
            vec![
                DisplayMessage::new(
                    2,
                    starweaver_core::SessionId::from_string(session_id.clone()),
                    starweaver_core::RunId::from_string("run_1"),
                    DisplayMessageKind::AssistantTextDelta,
                )
                .with_payload(json!({"delta": "second"})),
            ],
        );

        let event = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let RunStreamEvent::Output(output) = event else {
            panic!("expected live output event");
        };
        assert_eq!(output.scope, ReplayScope::session(&session_id));
        assert_eq!(output.sequence, 1);
        let ReplayEventKind::DisplayMessage(message) = &output.event else {
            panic!("expected display message");
        };
        assert_eq!(message.payload["delta"], "second");
    }

    #[test]
    fn persisted_failed_run_status_preserves_error_detail() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let mut store = crate::LocalStore::open(&config).unwrap();
        let session = store
            .create_session(&config.default_profile, Some("failed session".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        let mut run = store
            .append_run(
                &session_id,
                "hello".to_string(),
                None,
                &config.default_profile,
            )
            .unwrap();
        let run_id = run.run_id.as_str().to_string();
        store
            .fail_run(
                &mut run,
                "websocket closed before response.completed".to_string(),
            )
            .unwrap();
        drop(store);

        let status = CliRuntimeCoordinator::new(config)
            .run_status(&session_id, &run_id)
            .unwrap();

        assert_eq!(status.status, "failed");
        assert_eq!(
            status.output_preview.as_deref(),
            Some("websocket closed before response.completed")
        );
        assert_eq!(
            status.error.as_deref(),
            Some("websocket closed before response.completed")
        );
    }

    #[test]
    fn session_output_dedupes_active_messages_already_persisted() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let mut store = crate::LocalStore::open(&config).unwrap();
        let session = store
            .create_session(&config.default_profile, Some("dedupe session".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        let mut run = store
            .append_run(
                &session_id,
                "hello".to_string(),
                None,
                &config.default_profile,
            )
            .unwrap();
        let run_id = run.run_id.as_str().to_string();
        let persisted_message = DisplayMessage::new(
            0,
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({"delta": "persisted"}));
        store
            .complete_run(
                &mut run,
                "persisted".to_string(),
                crate::local_store::RunArtifacts {
                    state: starweaver_context::ResumableState::default(),
                    environment_state: None,
                    raw_records: Vec::new(),
                    display_messages: vec![persisted_message.clone()],
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
                },
            )
            .unwrap();
        drop(store);

        let coordinator = CliRuntimeCoordinator::new(config);
        let (steering_sender, _steering_receiver) = mpsc::channel::<CliSteeringMessage>();
        let (cancel_sender, _cancel_receiver) = mpsc::channel::<()>();
        let (subscriber_sender, _subscriber_receiver) = mpsc::channel::<RunStreamEvent>();
        insert_active_run(
            &coordinator.active_runs,
            &session_id,
            &run_id,
            steering_sender,
            cancel_sender,
            0,
            RunSubscriber {
                include_raw: false,
                cursor: SubscriberCursor::Run,
                sender: subscriber_sender,
            },
            None,
        );
        publish_display_messages(&coordinator.active_runs, &run_id, vec![persisted_message]);

        let attachment = coordinator.session_output(&session_id, None, None).unwrap();

        assert!(attachment.active);
        assert_eq!(attachment.events.len(), 1);
        let ReplayEventKind::DisplayMessage(message) = &attachment.events[0].event else {
            panic!("expected display message");
        };
        assert_eq!(message.payload["delta"], "persisted");
    }
}
