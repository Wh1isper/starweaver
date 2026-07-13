//! Background run coordination for the interactive CLI.
#![allow(clippy::redundant_pub_crate)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use starweaver_runtime::AgentStreamRecord;

use crate::{
    CliError, CliResult, CliService, args::RunCommand, config::CliConfig,
    prompt_input::PromptInput, runner::CliSteeringMessage,
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
}

pub(super) struct StartedRun {
    pub(super) run_id: String,
    pub(super) events: mpsc::Receiver<RunStreamEvent>,
}

#[derive(Clone)]
pub(super) struct CliRuntimeCoordinator {
    config: CliConfig,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunControl>>>,
}

struct ActiveRunControl {
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
}

struct BackgroundRunWorker {
    config: CliConfig,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunControl>>>,
    command: RunCommand,
    prompt_input: Option<PromptInput>,
    started_sender: mpsc::Sender<CliResult<(String, String)>>,
    event_sender: mpsc::Sender<RunStreamEvent>,
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    steering_receiver: mpsc::Receiver<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    cancel_receiver: mpsc::Receiver<()>,
}

impl BackgroundRunWorker {
    fn run(self) {
        let mut service = match CliService::open(self.config) {
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
        if let Ok(mut runs) = self.active_runs.lock() {
            runs.insert(
                run_id.clone(),
                ActiveRunControl {
                    steering_sender: self.steering_sender,
                    cancel_sender: self.cancel_sender,
                },
            );
        }
        let _ = self
            .event_sender
            .send(RunStreamEvent::Status(RunStatusItem {
                session_id: session_id.clone(),
                run_id: run_id.clone(),
                status: "running".to_string(),
                error: None,
            }));
        if self
            .started_sender
            .send(Ok((session_id.clone(), run_id.clone())))
            .is_err()
        {
            remove_active_run(&self.active_runs, &run_id);
            return;
        }

        let (stream_sender, stream_receiver) = mpsc::channel::<AgentStreamRecord>();
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
            Some(self.steering_receiver),
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
                    session_id,
                    run_id: run_id.clone(),
                    status: "failed".to_string(),
                    error: Some(error.to_string()),
                },
            },
            Err(error) => {
                let _ = service.fail_prepared_prompt_run(run_on_error, &error);
                RunStatusItem {
                    session_id,
                    run_id: run_id.clone(),
                    status: "failed".to_string(),
                    error: Some(error.to_string()),
                }
            }
        };
        let _ = self.event_sender.send(RunStreamEvent::Status(status));
        remove_active_run(&self.active_runs, &run_id);
    }
}

impl CliRuntimeCoordinator {
    pub(super) fn new(config: CliConfig) -> Self {
        Self {
            config,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn start_run_with_raw(
        &self,
        command: RunCommand,
        prompt_input: Option<PromptInput>,
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
            started_sender,
            event_sender,
            steering_sender,
            steering_receiver,
            cancel_sender,
            cancel_receiver,
        };
        thread::spawn(move || worker.run());
        let (_session_id, run_id) = started_receiver
            .recv()
            .map_err(|error| CliError::Run(error.to_string()))??;
        Ok(StartedRun {
            run_id,
            events: event_receiver,
        })
    }

    pub(super) fn steer_run(&self, run_id: &str, message: CliSteeringMessage) -> CliResult<()> {
        let sender = self
            .active_runs
            .lock()
            .map_err(|error| CliError::Run(error.to_string()))?
            .get(run_id)
            .map(|control| control.steering_sender.clone())
            .ok_or_else(|| CliError::NotFound(run_id.to_string()))?;
        sender
            .send(message)
            .map_err(|error| CliError::Run(error.to_string()))
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
