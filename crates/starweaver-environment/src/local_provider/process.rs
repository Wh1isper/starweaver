//! Local background process provider implementation.

use std::{
    collections::BTreeMap,
    io,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use command_group::GroupChild;
use starweaver_core::Metadata;

#[cfg(unix)]
use crate::signal_process_group;
use crate::{
    CapturedPipe, EnvironmentError, EnvironmentResult, LocalExecutionPermit, ProcessShellProvider,
    ProgramCommand, ShellCommand, ShellProcessSnapshot, ShellProcessStatus,
    checked_timeout_deadline, local_program_command, local_shell_command, program_process_metadata,
    read_child_pipe, refresh_local_shell_process, shell_process_metadata, spawn_group,
    terminate_process_group,
};

use super::LocalEnvironmentProvider;

#[derive(Debug)]
pub struct LocalShellProcess {
    pub(crate) command: String,
    pub(crate) child: GroupChild,
    pub(crate) stdout_handle: Option<thread::JoinHandle<io::Result<CapturedPipe>>>,
    pub(crate) stderr_handle: Option<thread::JoinHandle<io::Result<CapturedPipe>>>,
    pub(crate) metadata: Metadata,
    pub(crate) deadline: Option<Instant>,
    pub(crate) completed_at: Option<Instant>,
    pub(crate) completed: Option<ShellProcessSnapshot>,
    pub(crate) execution_permit: Option<LocalExecutionPermit>,
}

impl Drop for LocalShellProcess {
    fn drop(&mut self) {
        if self.completed.is_none() {
            // Provider teardown is the final owner boundary. Terminate and reap
            // the group leader before its scratch directory is released so the
            // child cannot survive as either a running process or a zombie.
            let _ = terminate_process_group(&mut self.child);
        }
        drop(self.stdout_handle.take());
        drop(self.stderr_handle.take());
    }
}

impl LocalEnvironmentProvider {
    fn spawn_local_process(
        &self,
        mut process: Command,
        display_command: String,
        mut metadata: Metadata,
        timeout_seconds: Option<u64>,
        execution_permit: LocalExecutionPermit,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let deadline = timeout_seconds.map(checked_timeout_deadline).transpose()?;
        self.reap_local_processes()?;
        metadata.insert(
            "output_limit_bytes_per_stream".to_string(),
            serde_json::json!(self.max_output_bytes),
        );
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_group(process)?;
        let stdout_reader = child.inner().stdout.take();
        let stderr_reader = child.inner().stderr.take();
        let max_output_bytes = self.max_output_bytes;
        let stdout_handle = thread::spawn(move || read_child_pipe(stdout_reader, max_output_bytes));
        let stderr_handle = thread::spawn(move || read_child_pipe(stderr_reader, max_output_bytes));
        let process_id = format!("process_{}", child.id());
        let snapshot = ShellProcessSnapshot {
            process_id: process_id.clone(),
            command: display_command.clone(),
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata: metadata.clone(),
        };
        let local_process = LocalShellProcess {
            command: display_command,
            child,
            stdout_handle: Some(stdout_handle),
            stderr_handle: Some(stderr_handle),
            metadata,
            deadline,
            completed_at: None,
            completed: None,
            execution_permit: Some(execution_permit),
        };
        self.resources
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(process_id, local_process);
        Ok(snapshot)
    }

    pub(super) fn reap_local_processes(&self) -> EnvironmentResult<()> {
        let mut processes = self
            .resources
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let ids = processes.keys().cloned().collect::<Vec<_>>();
        for process_id in ids {
            if let Some(process) = processes.get_mut(&process_id) {
                refresh_local_shell_process(&process_id, process, false)?;
            }
        }
        prune_completed_processes(&mut processes, self.completed_process_retention);
        Ok(())
    }
}

fn prune_completed_processes(
    processes: &mut BTreeMap<String, LocalShellProcess>,
    retention: usize,
) {
    let mut completed = processes
        .iter()
        .filter_map(|(process_id, process)| {
            process
                .completed_at
                .map(|completed_at| (completed_at, process_id.clone()))
        })
        .collect::<Vec<_>>();
    completed.sort_by_key(|(completed_at, _)| *completed_at);
    let remove_count = completed.len().saturating_sub(retention);
    for (_, process_id) in completed.into_iter().take(remove_count) {
        processes.remove(&process_id);
    }
}

#[async_trait]
#[allow(clippy::significant_drop_tightening)]
impl ProcessShellProvider for LocalEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let provider = self.clone();
        crate::blocking::run(move || {
            if !provider.policy.shell.permits_shell() {
                return Err(EnvironmentError::AccessDenied(command.command));
            }
            let cwd = provider.resolve_shell_cwd(command.cwd.as_deref())?;
            let environment = provider.shell_environment(&command.environment)?;
            provider.reap_local_processes()?;
            let execution_permit = provider.execution_limiter.try_acquire()?;
            let mut process = local_shell_command(&command.command);
            process.current_dir(cwd).envs(&environment);
            provider.spawn_local_process(
                process,
                command.command.clone(),
                shell_process_metadata(&command),
                command.timeout_seconds,
                execution_permit,
            )
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn start_program(
        &self,
        command: ProgramCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let provider = self.clone();
        crate::blocking::run(move || {
            if !provider.policy.shell.permits_program(&command.program) {
                return Err(EnvironmentError::AccessDenied(command.display_command()));
            }
            if !command.environment.is_empty()
                && !provider
                    .policy
                    .shell
                    .permits_program_environment_overrides()
            {
                return Err(EnvironmentError::InvalidRequest(
                    "environment overrides are not allowed for allowlisted direct programs"
                        .to_string(),
                ));
            }
            let cwd = provider.resolve_shell_cwd(command.cwd.as_deref())?;
            let environment = provider.shell_environment(&command.environment)?;
            provider.reap_local_processes()?;
            let execution_permit = provider.execution_limiter.try_acquire()?;
            let mut process = local_program_command(&command)?;
            process.current_dir(cwd).envs(&environment);
            provider.spawn_local_process(
                process,
                command.display_command(),
                program_process_metadata(&command),
                command.timeout_seconds,
                execution_permit,
            )
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let provider = self.clone();
        let process_id = process_id.to_string();
        crate::blocking::run_cancellable(move |cancelled| {
            let deadline = checked_timeout_deadline(timeout_seconds)?;
            loop {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(EnvironmentError::Provider(
                        "process wait cancelled".to_string(),
                    ));
                }
                let snapshot = {
                    let mut processes = provider
                        .resources
                        .processes
                        .lock()
                        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                    let process = processes
                        .get_mut(&process_id)
                        .ok_or_else(|| EnvironmentError::NotFound(process_id.clone()))?;
                    let snapshot = refresh_local_shell_process(&process_id, process, false)?;
                    prune_completed_processes(&mut processes, provider.completed_process_retention);
                    snapshot
                };
                if snapshot.status != ShellProcessStatus::Running || timeout_seconds == 0 {
                    return Ok(snapshot);
                }
                if Instant::now() >= deadline {
                    return Ok(snapshot);
                }
                thread::sleep(Duration::from_millis(25));
            }
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        let provider = self.clone();
        crate::blocking::run(move || {
            let snapshots = {
                let mut processes = provider
                    .resources
                    .processes
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                let ids = processes.keys().cloned().collect::<Vec<_>>();
                let mut snapshots = Vec::with_capacity(ids.len());
                for process_id in ids {
                    if let Some(process) = processes.get_mut(&process_id) {
                        snapshots.push(refresh_local_shell_process(&process_id, process, false)?);
                    }
                }
                prune_completed_processes(&mut processes, provider.completed_process_retention);
                snapshots
            };
            Ok(snapshots)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let provider = self.clone();
        let process_id = process_id.to_string();
        let text = text.to_string();
        crate::blocking::run(move || {
            let snapshot = {
                let mut processes = provider
                    .resources
                    .processes
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                let process = processes
                    .get_mut(&process_id)
                    .ok_or_else(|| EnvironmentError::NotFound(process_id.clone()))?;
                if let Some(stdin) = process.child.inner().stdin.as_mut() {
                    use std::io::Write as _;
                    stdin
                        .write_all(text.as_bytes())
                        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                    stdin
                        .write_all(b"\n")
                        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                    if close_stdin {
                        process.child.inner().stdin.take();
                    }
                } else {
                    return Err(EnvironmentError::InvalidRequest(format!(
                        "stdin is closed for process: {process_id}"
                    )));
                }
                refresh_local_shell_process(&process_id, process, false)?
            };
            Ok(snapshot)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let provider = self.clone();
        let process_id = process_id.to_string();
        crate::blocking::run(move || {
            #[cfg(unix)]
            {
                let snapshot = {
                    let mut processes = provider
                        .resources
                        .processes
                        .lock()
                        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                    let process = processes
                        .get_mut(&process_id)
                        .ok_or_else(|| EnvironmentError::NotFound(process_id.clone()))?;
                    signal_process_group(&process.child, signal)?;
                    refresh_local_shell_process(&process_id, process, false)?
                };
                Ok(snapshot)
            }

            #[cfg(not(unix))]
            {
                let _ = (provider, process_id, signal);
                Err(EnvironmentError::InvalidRequest(
                    "shell_signal is only supported on Unix local providers; shell_kill uses the \
                     platform process-group fallback"
                        .to_string(),
                ))
            }
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        let provider = self.clone();
        let process_id = process_id.to_string();
        crate::blocking::run(move || {
            let snapshot = {
                let mut processes = provider
                    .resources
                    .processes
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                let process = processes
                    .get_mut(&process_id)
                    .ok_or_else(|| EnvironmentError::NotFound(process_id.clone()))?;
                let snapshot = refresh_local_shell_process(&process_id, process, true)?;
                prune_completed_processes(&mut processes, provider.completed_process_retention);
                snapshot
            };
            Ok(snapshot)
        })
        .await
        .map_err(EnvironmentError::Provider)?
    }
}
