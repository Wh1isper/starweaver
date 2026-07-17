use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use starweaver_environment::{
    DynProcessShellProvider, ShellCommand, ShellProcessSnapshot, ShellProcessStatus,
};

const SHELL_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SHELL_CLEANUP_RETRIES: usize = 3;
const SHELL_DROP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum TuiShellEvent {
    Started {
        process_id: String,
    },
    Finished {
        snapshot: ShellProcessSnapshot,
        elapsed: Duration,
    },
    Failed(String),
}

pub struct TuiShellRun {
    pub events: mpsc::Receiver<TuiShellEvent>,
    cancel_sender: mpsc::Sender<()>,
    done_receiver: mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<()>>,
    cleanup_wait_attempted: bool,
}

impl TuiShellRun {
    pub fn cancel(&self) -> Result<(), String> {
        self.cancel_sender
            .send(())
            .map_err(|error| error.to_string())
    }

    pub fn cancel_and_wait(&mut self, timeout: Duration) -> Result<(), String> {
        self.cleanup_wait_attempted = true;
        if self.worker.is_none() {
            return Ok(());
        }
        let _ = self.cancel_sender.send(());
        match self.done_receiver.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => self.join_worker(),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "shell cleanup did not finish within {:.1}s",
                timeout.as_secs_f64()
            )),
        }
    }

    fn join_worker(&mut self) -> Result<(), String> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| "shell worker panicked during cleanup".to_string())
    }
}

impl Drop for TuiShellRun {
    fn drop(&mut self) {
        if !self.cleanup_wait_attempted {
            let _ = self.cancel_and_wait(SHELL_DROP_TIMEOUT);
        }
    }
}

pub fn spawn_shell_run(provider: DynProcessShellProvider, command: String) -> TuiShellRun {
    let (event_sender, events) = mpsc::sync_channel(8);
    let (cancel_sender, cancel_receiver) = mpsc::channel();
    let (done_sender, done_receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_shell_worker(&provider, &command, &event_sender, &cancel_receiver);
        let _ = done_sender.send(());
    });
    TuiShellRun {
        events,
        cancel_sender,
        done_receiver,
        worker: Some(worker),
        cleanup_wait_attempted: false,
    }
}

fn run_shell_worker(
    provider: &DynProcessShellProvider,
    command: &str,
    event_sender: &mpsc::SyncSender<TuiShellEvent>,
    cancel_receiver: &mpsc::Receiver<()>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = event_sender.send(TuiShellEvent::Failed(error.to_string()));
            return;
        }
    };
    let started_at = Instant::now();
    let started = match runtime.block_on(provider.start_process(ShellCommand::shell(command))) {
        Ok(started) => started,
        Err(error) => {
            let _ = event_sender.send(TuiShellEvent::Failed(error.to_string()));
            return;
        }
    };
    let process_id = started.process_id;
    if event_sender
        .send(TuiShellEvent::Started {
            process_id: process_id.clone(),
        })
        .is_err()
    {
        let _ = kill_process_with_retries(&runtime, provider, &process_id);
        return;
    }

    loop {
        match cancel_receiver.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                let event = kill_process_with_retries(&runtime, provider, &process_id).map_or_else(
                    |error| {
                        TuiShellEvent::Failed(format!(
                            "could not clean up shell process {process_id}: {error}; manual cleanup may be required"
                        ))
                    },
                    |snapshot| TuiShellEvent::Finished {
                        snapshot,
                        elapsed: started_at.elapsed(),
                    },
                );
                let _ = event_sender.send(event);
                return;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match runtime.block_on(provider.wait_process(&process_id, 0)) {
            Ok(snapshot) if shell_status_is_terminal(&snapshot.status) => {
                let _ = event_sender.send(TuiShellEvent::Finished {
                    snapshot,
                    elapsed: started_at.elapsed(),
                });
                return;
            }
            Ok(_) => thread::sleep(SHELL_POLL_INTERVAL),
            Err(wait_error) => {
                let cleanup = kill_process_with_retries(&runtime, provider, &process_id)
                    .map_or_else(
                        |cleanup_error| {
                            format!(
                                "cleanup failed: {cleanup_error}; manual cleanup may be required for process {process_id}"
                            )
                        },
                        |snapshot| format!("cleanup status={:?}", snapshot.status),
                    );
                let _ = event_sender.send(TuiShellEvent::Failed(format!(
                    "shell process {process_id} polling failed: {wait_error}; {cleanup}"
                )));
                return;
            }
        }
    }
}

fn kill_process_with_retries(
    runtime: &tokio::runtime::Runtime,
    provider: &DynProcessShellProvider,
    process_id: &str,
) -> Result<ShellProcessSnapshot, String> {
    let mut last_error = None;
    for attempt in 0..SHELL_CLEANUP_RETRIES {
        match runtime.block_on(provider.kill_process(process_id)) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => last_error = Some(error.to_string()),
        }
        if attempt + 1 < SHELL_CLEANUP_RETRIES {
            thread::sleep(SHELL_POLL_INTERVAL);
        }
    }
    Err(last_error.unwrap_or_else(|| "unknown cleanup failure".to_string()))
}

const fn shell_status_is_terminal(status: &ShellProcessStatus) -> bool {
    matches!(
        status,
        ShellProcessStatus::Completed | ShellProcessStatus::Failed | ShellProcessStatus::Killed
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use starweaver_environment::{
        EnvironmentPolicy, FilePolicy, LocalEnvironmentProvider, ShellPolicy,
    };

    use super::*;

    fn local_process_provider(root: &std::path::Path) -> DynProcessShellProvider {
        Arc::new(
            LocalEnvironmentProvider::new(root).with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::allow_all(),
            }),
        )
    }

    #[test]
    fn shell_run_reports_started_and_finished() {
        let root = tempfile::tempdir().unwrap();
        let provider = local_process_provider(root.path());
        let run = spawn_shell_run(provider, "echo starweaver-shell".to_string());

        let started = run.events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(started, TuiShellEvent::Started { .. }));
        let finished = run.events.recv_timeout(Duration::from_secs(5)).unwrap();
        let TuiShellEvent::Finished { snapshot, .. } = finished else {
            panic!("shell worker must produce a terminal snapshot");
        };
        assert_eq!(snapshot.status, ShellProcessStatus::Completed);
        assert_eq!(snapshot.return_code, Some(0));
        assert!(snapshot.stdout.contains("starweaver-shell"));
    }

    #[cfg(unix)]
    #[test]
    fn shell_run_cancellation_returns_a_killed_snapshot_and_joins_worker() {
        let root = tempfile::tempdir().unwrap();
        let provider = local_process_provider(root.path());
        let mut run = spawn_shell_run(provider, "sleep 30".to_string());
        assert!(matches!(
            run.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            TuiShellEvent::Started { .. }
        ));

        run.cancel().unwrap();
        let finished = run.events.recv_timeout(Duration::from_secs(5)).unwrap();
        let TuiShellEvent::Finished { snapshot, .. } = finished else {
            panic!("cancelled shell worker must produce a terminal snapshot");
        };
        assert_eq!(snapshot.status, ShellProcessStatus::Killed);
        run.cancel_and_wait(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn shell_terminal_statuses_exclude_running() {
        assert!(!shell_status_is_terminal(&ShellProcessStatus::Running));
        assert!(shell_status_is_terminal(&ShellProcessStatus::Completed));
        assert!(shell_status_is_terminal(&ShellProcessStatus::Failed));
        assert!(shell_status_is_terminal(&ShellProcessStatus::Killed));
    }
}
