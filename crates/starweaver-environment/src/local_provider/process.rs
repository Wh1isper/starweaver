//! Local background process provider implementation.

use std::{
    io,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use starweaver_core::Metadata;

use crate::{
    read_child_pipe, refresh_local_shell_process, shell_process_metadata, EnvironmentError,
    EnvironmentResult, ProcessShellProvider, ShellCommand, ShellProcessSnapshot,
    ShellProcessStatus,
};

use super::LocalEnvironmentProvider;

#[derive(Debug)]
pub struct LocalShellProcess {
    pub(crate) command: String,
    pub(crate) child: Child,
    pub(crate) stdout_handle: Option<thread::JoinHandle<io::Result<String>>>,
    pub(crate) stderr_handle: Option<thread::JoinHandle<io::Result<String>>>,
    pub(crate) metadata: Metadata,
    pub(crate) completed: Option<ShellProcessSnapshot>,
}

#[async_trait]
#[allow(clippy::significant_drop_tightening)]
impl ProcessShellProvider for LocalEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        let cwd = self.resolve_shell_cwd(command.cwd.as_deref())?;
        let environment = self.shell_environment(&command.environment)?;
        let mut child = Command::new("/bin/sh")
            .arg("-lc")
            .arg(&command.command)
            .current_dir(cwd)
            .envs(&environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let mut stdout_reader = child.stdout.take();
        let mut stderr_reader = child.stderr.take();
        let stdout_handle = thread::spawn(move || read_child_pipe(stdout_reader.take()));
        let stderr_handle = thread::spawn(move || read_child_pipe(stderr_reader.take()));
        let process_id = format!("process_{}", child.id());
        let metadata = shell_process_metadata(&command);
        let snapshot = ShellProcessSnapshot {
            process_id: process_id.clone(),
            command: command.command.clone(),
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata: metadata.clone(),
        };
        self.processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(
                process_id,
                LocalShellProcess {
                    command: command.command,
                    child,
                    stdout_handle: Some(stdout_handle),
                    stderr_handle: Some(stderr_handle),
                    metadata,
                    completed: None,
                },
            );
        Ok(snapshot)
    }

    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
        loop {
            let snapshot = {
                let mut processes = self
                    .processes
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                let process = processes
                    .get_mut(process_id)
                    .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
                refresh_local_shell_process(process_id, process, false)?
            };
            if snapshot.status != ShellProcessStatus::Running || timeout_seconds == 0 {
                return Ok(snapshot);
            }
            if Instant::now() >= deadline {
                return Ok(snapshot);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        let snapshots = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let mut snapshots = Vec::new();
            for (process_id, process) in processes.iter_mut() {
                snapshots.push(refresh_local_shell_process(process_id, process, false)?);
            }
            snapshots
        };
        Ok(snapshots)
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let snapshot = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let process = processes
                .get_mut(process_id)
                .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
            if let Some(stdin) = process.child.stdin.as_mut() {
                use std::io::Write as _;
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                stdin
                    .write_all(b"\n")
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if close_stdin {
                    process.child.stdin.take();
                }
            } else {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "stdin is closed for process: {process_id}"
                )));
            }
            refresh_local_shell_process(process_id, process, false)?
        };
        Ok(snapshot)
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let snapshot = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let process = processes
                .get_mut(process_id)
                .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
            #[cfg(unix)]
            {
                let pid = process.child.id().to_string();
                let status = Command::new("kill")
                    .arg(format!("-{signal}"))
                    .arg(pid)
                    .status()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if !status.success() {
                    return Err(EnvironmentError::Provider(format!(
                        "failed to signal process {process_id} with signal {signal}"
                    )));
                }
            }
            #[cfg(not(unix))]
            {
                return Err(EnvironmentError::InvalidRequest(
                    "shell_signal is only supported on Unix local providers".to_string(),
                ));
            }
            refresh_local_shell_process(process_id, process, false)?
        };
        Ok(snapshot)
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        let snapshot = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let process = processes
                .get_mut(process_id)
                .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
            refresh_local_shell_process(process_id, process, true)?
        };
        Ok(snapshot)
    }
}
