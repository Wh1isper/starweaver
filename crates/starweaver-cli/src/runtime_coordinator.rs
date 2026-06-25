//! Shared local run coordination for RPC and interactive clients.
#![allow(clippy::redundant_pub_crate)]

use std::{
    collections::{HashMap, HashSet},
    sync::{mpsc, Arc, Mutex},
    thread,
};

use serde::Serialize;
use serde_json::{json, Value};
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{RunRecord, RunStatus};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayScope,
};

use crate::{
    args::RunCommand,
    config::CliConfig,
    local_store::{LocalStore, LocalStreamArchive},
    prompt_input::PromptInput,
    runner::CliSteeringMessage,
    CliError, CliResult, CliService,
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
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    subscribers: Vec<RunSubscriber>,
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
            Ok(executed) => match service.complete_prompt_run(executed) {
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
            },
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
        let mut live_messages = Vec::new();
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
                live_messages.extend(
                    state
                        .display_messages
                        .iter()
                        .filter(|message| {
                            !persisted_messages.contains(&display_message_key(message))
                        })
                        .cloned(),
                );
            }
            let all_live_events =
                append_session_replay_events(&scope, store_event_count, live_messages);
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

    pub(super) fn run_status(&self, session_id: &str, run_id: &str) -> CliResult<RunStatusItem> {
        if let Ok(runs) = self.active_runs.lock() {
            if let Some(state) = runs.get(run_id) {
                return Ok(RunStatusItem {
                    session_id: state.session_id.clone(),
                    run_id: run_id.to_string(),
                    status: state.status.clone(),
                    output_preview: state.output_preview.clone(),
                    error: state.error.clone(),
                });
            }
        }
        let store = LocalStore::open(&self.config)?;
        let run = store.load_run(session_id, run_id)?;
        Ok(RunStatusItem {
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            status: run_status_name(run.status).to_string(),
            output_preview: run.output_preview,
            error: None,
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
                .display_messages
                .iter()
                .filter(|message| cursor.is_none_or(|cursor| message.sequence > cursor.sequence))
                .cloned()
                .map(|message| run_replay_event(run_id, message))
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

fn insert_active_run(
    active_runs: &Arc<Mutex<HashMap<String, ActiveRunState>>>,
    session_id: &str,
    run_id: &str,
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    sequence_no: usize,
    subscriber: RunSubscriber,
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
                steering_sender,
                cancel_sender,
                subscribers: vec![subscriber],
            },
        );
    }
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
    for message in messages {
        let terminal = terminal_status(message.kind);
        state.display_messages.push(message.clone());
        state.subscribers.retain(|subscriber| {
            let event = replay_event_for_subscriber(run_id, message.clone(), subscriber);
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

fn display_message_key(message: &DisplayMessage) -> (String, usize) {
    (message.run_id.as_str().to_string(), message.sequence)
}

fn append_session_replay_events(
    scope: &ReplayScope,
    start_sequence: usize,
    messages: Vec<DisplayMessage>,
) -> Vec<ReplayEvent> {
    messages
        .into_iter()
        .enumerate()
        .map(|(offset, message)| display_replay_event(scope, start_sequence + offset, message))
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

fn replay_event_for_subscriber(
    run_id: &str,
    message: DisplayMessage,
    subscriber: &RunSubscriber,
) -> ReplayEvent {
    match &subscriber.cursor {
        SubscriberCursor::Run => run_replay_event(run_id, message),
        SubscriberCursor::Session { next_sequence } => {
            let sequence = next_session_sequence(next_sequence);
            display_replay_event(
                &ReplayScope::session(message.session_id.as_str()),
                sequence,
                message,
            )
        }
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
    messages.iter().rev().find_map(|message| {
        message
            .payload
            .get("output")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| message.preview.clone())
    })
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

    use super::*;
    use crate::{args, ConfigResolver};

    fn test_config(root: &std::path::Path) -> CliConfig {
        let cli = args::parse(["starweaver-cli".to_string(), "rpc".to_string()]).unwrap();
        ConfigResolver::for_tests(root).resolve(&cli).unwrap()
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
        );
        publish_display_messages(
            &coordinator.active_runs,
            "run_1",
            vec![DisplayMessage::new(
                1,
                starweaver_core::SessionId::from_string("session_1"),
                starweaver_core::RunId::from_string("run_1"),
                DisplayMessageKind::RunCompleted,
            )
            .with_payload(json!({"output": "done"}))],
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
        );
        publish_display_messages(
            &coordinator.active_runs,
            "run_1",
            vec![DisplayMessage::new(
                1,
                starweaver_core::SessionId::from_string(session_id.clone()),
                starweaver_core::RunId::from_string("run_1"),
                DisplayMessageKind::AssistantTextDelta,
            )
            .with_payload(json!({"delta": "first"}))],
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
            vec![DisplayMessage::new(
                2,
                starweaver_core::SessionId::from_string(session_id.clone()),
                starweaver_core::RunId::from_string("run_1"),
                DisplayMessageKind::AssistantTextDelta,
            )
            .with_payload(json!({"delta": "second"}))],
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
