//! Background run coordination for the interactive CLI.
#![allow(clippy::redundant_pub_crate)]

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use chrono::Utc;
use starweaver_agent::{
    BackgroundSubagentDeliveryClaim, BackgroundSubagentDeliveryStatus,
    BackgroundSubagentSupervisor, BackgroundSubagentTaskResult,
};
use starweaver_core::{RunId, SubagentAttemptId};
use starweaver_runtime::AgentStreamRecord;

use crate::{
    CliError, CliResult, CliService,
    args::RunCommand,
    config::CliConfig,
    prompt_input::PromptInput,
    runner::{CliAgentExecutionHost, CliSteeringChannel, CliSteeringMessage},
};

#[derive(Clone, Debug)]
pub(super) struct RunStatusItem {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) status: String,
    pub(super) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum RunStreamEvent {
    Status(RunStatusItem),
    Raw(Box<AgentStreamRecord>),
    StartFailed(String),
}

pub(super) struct StartedRun {
    pub(super) control_id: String,
    pub(super) events: mpsc::Receiver<RunStreamEvent>,
}

#[derive(Clone)]
pub(super) struct CliRuntimeCoordinator {
    config: CliConfig,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunControl>>>,
    worker_handles: Arc<Mutex<Vec<BackgroundWorkerHandle>>>,
    interactive_host: Arc<CliInteractiveExecutionHost>,
    closing: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub(super) struct BackgroundCompletion {
    pub(super) session_id: String,
    pub(super) attempt_id: String,
}

struct CliInteractiveExecutionHost {
    runtime: Arc<tokio::runtime::Runtime>,
    supervisors: Mutex<HashMap<String, Arc<BackgroundSubagentSupervisor>>>,
    completions: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
}

struct BackgroundWorkerHandle {
    join: thread::JoinHandle<()>,
    completed: mpsc::Receiver<()>,
}

struct ActiveRunControl {
    steering_channel: CliSteeringChannel,
    cancel_sender: mpsc::Sender<()>,
}

fn reap_finished_worker_handles(
    worker_handles: &Mutex<Vec<BackgroundWorkerHandle>>,
) -> Result<(), String> {
    let handles = worker_handles.lock().map_or_else(
        |error| std::mem::take(&mut *error.into_inner()),
        |mut handles| std::mem::take(&mut *handles),
    );
    let mut pending = Vec::new();
    let mut worker_error = None;
    for handle in handles {
        let BackgroundWorkerHandle { join, completed } = handle;
        match completed.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                if join.join().is_err() {
                    worker_error
                        .get_or_insert_with(|| "CLI background worker panicked".to_string());
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                pending.push(BackgroundWorkerHandle { join, completed });
            }
        }
    }
    if !pending.is_empty() {
        match worker_handles.lock() {
            Ok(mut handles) => handles.extend(pending),
            Err(error) => error.into_inner().extend(pending),
        }
    }
    worker_error.map_or(Ok(()), Err)
}

fn drain_worker_handles_until(
    worker_handles: &Mutex<Vec<BackgroundWorkerHandle>>,
    deadline: std::time::Instant,
) -> Result<(), String> {
    let handles = worker_handles.lock().map_or_else(
        |error| std::mem::take(&mut *error.into_inner()),
        |mut handles| std::mem::take(&mut *handles),
    );
    let mut pending = Vec::new();
    let mut worker_error = None;
    for handle in handles {
        let BackgroundWorkerHandle { join, completed } = handle;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match completed.recv_timeout(remaining) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if join.join().is_err() {
                    worker_error
                        .get_or_insert_with(|| "CLI background worker panicked".to_string());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                pending.push(BackgroundWorkerHandle { join, completed });
            }
        }
    }
    if !pending.is_empty() {
        match worker_handles.lock() {
            Ok(mut handles) => handles.extend(pending),
            Err(error) => error.into_inner().extend(pending),
        }
        worker_error.get_or_insert_with(|| {
            "CLI shutdown exceeded its deadline with background workers still running".to_string()
        });
    }
    worker_error.map_or(Ok(()), Err)
}

impl CliInteractiveExecutionHost {
    fn new() -> CliResult<Self> {
        Ok(Self {
            runtime: Arc::new(
                tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?,
            ),
            supervisors: Mutex::new(HashMap::new()),
            completions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn execution_host(&self, session_id: &str) -> CliAgentExecutionHost {
        let supervisor = {
            let mut supervisors = self
                .supervisors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            supervisors
                .entry(session_id.to_string())
                .or_insert_with(|| {
                    let completions = Arc::clone(&self.completions);
                    let completion_session_id = session_id.to_string();
                    Arc::new(
                        BackgroundSubagentSupervisor::new().with_completion_callback(Arc::new(
                            move |result: &BackgroundSubagentTaskResult| {
                                let mut pending = completions
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let attempts =
                                    pending.entry(completion_session_id.clone()).or_default();
                                if attempts
                                    .iter()
                                    .any(|attempt_id| attempt_id == result.attempt_id.as_str())
                                {
                                    return;
                                }
                                attempts.push_back(result.attempt_id.as_str().to_string());
                                drop(pending);
                            },
                        )),
                    )
                })
                .clone()
        };
        CliAgentExecutionHost::interactive(supervisor, Arc::clone(&self.runtime))
    }

    fn take_completions(&self, session_id: &str) -> Vec<BackgroundCompletion> {
        let attempt_ids = self
            .completions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id)
            .unwrap_or_default();
        attempt_ids
            .into_iter()
            .filter(|attempt_id| self.completion_is_undelivered(session_id, attempt_id))
            .map(|attempt_id| BackgroundCompletion {
                session_id: session_id.to_string(),
                attempt_id,
            })
            .collect()
    }

    fn completion_is_undelivered(&self, session_id: &str, attempt_id: &str) -> bool {
        self.supervisors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .and_then(|supervisor| {
                supervisor.task_result(&SubagentAttemptId::from_string(attempt_id))
            })
            .is_some_and(|result| {
                result.delivery_status == BackgroundSubagentDeliveryStatus::Undelivered
            })
    }

    fn claim_continuation(
        &self,
        session_id: &str,
        attempt_id: &str,
        continuation_run_id: &str,
    ) -> CliResult<String> {
        let supervisor = self
            .supervisors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
            .ok_or_else(|| CliError::NotFound(session_id.to_string()))?;
        let claim_id = format!("tui:{continuation_run_id}:{attempt_id}");
        supervisor
            .claim_delivery(
                &SubagentAttemptId::from_string(attempt_id),
                BackgroundSubagentDeliveryClaim {
                    claim_id: claim_id.clone(),
                    continuation_run_id: Some(RunId::from_string(continuation_run_id)),
                    deadline: Utc::now() + chrono::Duration::seconds(60),
                },
            )
            .map_err(|error| CliError::Run(error.to_string()))?;
        Ok(claim_id)
    }

    fn release_continuation(&self, session_id: &str, attempt_id: &str, claim_id: &str) {
        let supervisor = self
            .supervisors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned();
        if let Some(supervisor) = supervisor {
            let _ = supervisor
                .release_delivery_claim(&SubagentAttemptId::from_string(attempt_id), claim_id);
        }
    }

    fn shutdown(&self, timeout: Duration) -> Result<(), String> {
        let supervisors = self.supervisors.lock().map_or_else(
            |error| error.into_inner().values().cloned().collect::<Vec<_>>(),
            |supervisors| supervisors.values().cloned().collect::<Vec<_>>(),
        );
        self.runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + timeout;
            let mut shutdown_error = None;
            for supervisor in supervisors {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if let Err(error) = supervisor.shutdown_checked(Some(remaining)).await {
                    shutdown_error.get_or_insert_with(|| error.to_string());
                }
            }
            shutdown_error.map_or(Ok(()), Err)
        })
    }
}

struct BackgroundRunWorker {
    config: CliConfig,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunControl>>>,
    interactive_host: Arc<CliInteractiveExecutionHost>,
    command: RunCommand,
    prompt_input: Option<PromptInput>,
    control_id: String,
    event_sender: mpsc::SyncSender<RunStreamEvent>,
    steering_channel: CliSteeringChannel,
    cancel_receiver: mpsc::Receiver<()>,
    background_attempt_id: Option<String>,
}

impl BackgroundRunWorker {
    #[allow(clippy::too_many_lines)]
    fn run(self) {
        let mut service = match CliService::open(self.config) {
            Ok(service) => service,
            Err(error) => {
                let _ = self
                    .event_sender
                    .send(RunStreamEvent::StartFailed(error.to_string()));
                remove_active_run(&self.active_runs, &self.control_id);
                return;
            }
        };
        let mut prepared = match service.prepare_prompt_run(&self.command, self.prompt_input) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = self
                    .event_sender
                    .send(RunStreamEvent::StartFailed(error.to_string()));
                remove_active_run(&self.active_runs, &self.control_id);
                return;
            }
        };
        let mut run_on_error = prepared.run.clone();
        let session_id = prepared.session_id.clone();
        prepared.set_execution_host(self.interactive_host.execution_host(&session_id));
        let run_id = prepared.run_id.clone();
        let delivery_claim = if let Some(attempt_id) = self.background_attempt_id.as_deref() {
            match self
                .interactive_host
                .claim_continuation(&session_id, attempt_id, &run_id)
            {
                Ok(claim_id) => Some((attempt_id.to_string(), claim_id)),
                Err(error) => {
                    let _ = service.fail_prepared_prompt_run(run_on_error, &error);
                    let _ = self
                        .event_sender
                        .send(RunStreamEvent::StartFailed(error.to_string()));
                    remove_active_run(&self.active_runs, &self.control_id);
                    return;
                }
            }
        } else {
            None
        };
        if let Err(error) = service.start_prepared_hitl_resume(&mut prepared) {
            let _ = service.fail_prepared_prompt_run(run_on_error, &error);
            let _ = self
                .event_sender
                .send(RunStreamEvent::StartFailed(error.to_string()));
            remove_active_run(&self.active_runs, &self.control_id);
            return;
        }
        run_on_error = prepared.run.clone();
        let _ = self
            .event_sender
            .send(RunStreamEvent::Status(RunStatusItem {
                session_id: session_id.clone(),
                run_id: run_id.clone(),
                status: "running".to_string(),
                error: None,
            }));
        let (stream_sender, stream_receiver) = mpsc::sync_channel::<AgentStreamRecord>(256);
        let stream_event_sender = self.event_sender.clone();
        let stream_handle = thread::spawn(move || {
            for record in stream_receiver {
                if stream_event_sender
                    .send(RunStreamEvent::Raw(Box::new(record)))
                    .is_err()
                {
                    break;
                }
            }
        });
        let executed = CliService::run_prepared_prompt(
            prepared,
            Some(stream_sender),
            Some(self.steering_channel),
            Some(self.cancel_receiver),
        );
        let _ = stream_handle.join();
        let status = match executed {
            Ok(executed) => match service.complete_prompt_run(executed) {
                Ok(execution) => RunStatusItem {
                    session_id: execution.session_id,
                    run_id: execution.run_id,
                    status: execution.status,
                    error: None,
                },
                Err(error) => RunStatusItem {
                    session_id: session_id.clone(),
                    run_id,
                    status: "failed".to_string(),
                    error: Some(error.to_string()),
                },
            },
            Err(error) => {
                let _ = service.fail_prepared_prompt_run(run_on_error, &error);
                RunStatusItem {
                    session_id: session_id.clone(),
                    run_id,
                    status: "failed".to_string(),
                    error: Some(error.to_string()),
                }
            }
        };
        if let Some((attempt_id, claim_id)) = delivery_claim {
            self.interactive_host
                .release_continuation(&session_id, &attempt_id, &claim_id);
        }
        let _ = self.event_sender.send(RunStreamEvent::Status(status));
        remove_active_run(&self.active_runs, &self.control_id);
    }
}

impl CliRuntimeCoordinator {
    pub(super) fn new(config: CliConfig) -> CliResult<Self> {
        Ok(Self {
            config,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            worker_handles: Arc::new(Mutex::new(Vec::new())),
            interactive_host: Arc::new(CliInteractiveExecutionHost::new()?),
            closing: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(super) fn start_run_with_raw(
        &self,
        command: RunCommand,
        prompt_input: Option<PromptInput>,
        background_attempt_id: Option<String>,
    ) -> CliResult<StartedRun> {
        reap_finished_worker_handles(&self.worker_handles).map_err(CliError::Run)?;
        if self.closing.load(Ordering::Acquire) {
            return Err(CliError::Run(
                "interactive runtime coordinator is shutting down".to_string(),
            ));
        }
        let (event_sender, event_receiver) = mpsc::sync_channel::<RunStreamEvent>(256);
        let steering_channel = CliSteeringChannel::new();
        let (cancel_sender, cancel_receiver) = mpsc::channel::<()>();
        let control_id = RunId::new().as_str().to_string();
        self.active_runs
            .lock()
            .map_err(|error| CliError::Run(error.to_string()))?
            .insert(
                control_id.clone(),
                ActiveRunControl {
                    steering_channel: steering_channel.clone(),
                    cancel_sender,
                },
            );
        let worker = BackgroundRunWorker {
            config: self.config.clone(),
            active_runs: Arc::clone(&self.active_runs),
            interactive_host: Arc::clone(&self.interactive_host),
            command,
            prompt_input,
            control_id: control_id.clone(),
            event_sender,
            steering_channel,
            cancel_receiver,
            background_attempt_id,
        };
        let (completed_sender, completed_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            worker.run();
            let _ = completed_sender.send(());
        });
        self.worker_handles
            .lock()
            .map_err(|error| CliError::Run(error.to_string()))?
            .push(BackgroundWorkerHandle {
                join: handle,
                completed: completed_receiver,
            });
        Ok(StartedRun {
            control_id,
            events: event_receiver,
        })
    }

    pub(super) fn steer_run(
        &self,
        run_id: &str,
        message: CliSteeringMessage,
    ) -> Result<(), CliSteeringMessage> {
        let channel = self
            .active_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .map(|control| control.steering_channel.clone());
        let Some(channel) = channel else {
            return Err(message);
        };
        channel.submit(message)
    }

    pub(super) fn take_background_completions(
        &self,
        session_id: &str,
    ) -> Vec<BackgroundCompletion> {
        self.interactive_host.take_completions(session_id)
    }

    pub(super) fn background_completion_is_undelivered(
        &self,
        session_id: &str,
        attempt_id: &str,
    ) -> bool {
        self.interactive_host
            .completion_is_undelivered(session_id, attempt_id)
    }

    pub(super) fn shutdown(&self, timeout: Duration) -> CliResult<()> {
        let deadline = std::time::Instant::now() + timeout;
        self.closing.store(true, Ordering::Release);
        let foreground_cancellations = self.active_runs.lock().map_or_else(
            |error| {
                error
                    .into_inner()
                    .values()
                    .map(|control| control.cancel_sender.clone())
                    .collect::<Vec<_>>()
            },
            |runs| {
                runs.values()
                    .map(|control| control.cancel_sender.clone())
                    .collect::<Vec<_>>()
            },
        );
        for sender in foreground_cancellations {
            let _ = sender.send(());
        }
        let supervisor_result = self
            .interactive_host
            .shutdown(deadline.saturating_duration_since(std::time::Instant::now()));
        let worker_result = drain_worker_handles_until(&self.worker_handles, deadline);
        match (supervisor_result, worker_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(supervisor), Ok(())) => Err(CliError::Run(supervisor)),
            (Ok(()), Err(worker)) => Err(CliError::Run(worker)),
            (Err(supervisor), Err(worker)) => Err(CliError::Run(format!("{supervisor}; {worker}"))),
        }
    }

    pub(super) fn cancel_run(&self, run_id: &str) -> CliResult<()> {
        let sender = self
            .active_runs
            .lock()
            .map_err(|error| CliError::Run(error.to_string()))?
            .get(run_id)
            .map(|control| control.cancel_sender.clone())
            .ok_or_else(|| CliError::NotFound(run_id.to_string()))?;
        sender
            .send(())
            .map_err(|error| CliError::Run(error.to_string()))
    }
}

fn remove_active_run(active_runs: &Arc<Mutex<HashMap<String, ActiveRunControl>>>, run_id: &str) {
    if let Ok(mut runs) = active_runs.lock() {
        runs.remove(run_id);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn completed_worker_handles_are_reaped_without_waiting_for_shutdown() {
        let handles = Mutex::new(Vec::new());
        let (completed_tx, completed_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            completed_tx.send(()).expect("completion receiver");
        });
        handles.lock().unwrap().push(BackgroundWorkerHandle {
            join,
            completed: completed_rx,
        });

        for _ in 0..20 {
            reap_finished_worker_handles(&handles).unwrap();
            if handles.lock().unwrap().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(handles.lock().unwrap().is_empty());
    }

    #[test]
    fn active_worker_handle_is_retained_by_non_blocking_reap() {
        let handles = Mutex::new(Vec::new());
        let (completed_tx, completed_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            completed_tx.send(()).expect("completion receiver");
        });
        handles.lock().unwrap().push(BackgroundWorkerHandle {
            join,
            completed: completed_rx,
        });

        reap_finished_worker_handles(&handles).unwrap();
        assert_eq!(handles.lock().unwrap().len(), 1);
        drain_worker_handles_until(&handles, std::time::Instant::now() + Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn timed_out_worker_handle_is_retained_for_a_later_shutdown() {
        let handles = Mutex::new(Vec::new());
        let (completed_tx, completed_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            completed_tx.send(()).expect("completion receiver");
        });
        handles.lock().unwrap().push(BackgroundWorkerHandle {
            join,
            completed: completed_rx,
        });

        assert!(
            drain_worker_handles_until(
                &handles,
                std::time::Instant::now() + Duration::from_millis(10),
            )
            .is_err()
        );
        assert_eq!(handles.lock().unwrap().len(), 1);
        drain_worker_handles_until(&handles, std::time::Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert!(handles.lock().unwrap().is_empty());
    }
}
