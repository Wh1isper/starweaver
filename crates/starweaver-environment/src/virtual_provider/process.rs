//! Virtual background process provider implementation.

use async_trait::async_trait;
use starweaver_core::Metadata;

use crate::{
    EnvironmentError, EnvironmentResult, ProcessShellProvider, ShellCommand, ShellProcessSnapshot,
    ShellProcessStatus,
};

use super::VirtualEnvironmentProvider;

#[async_trait]
impl ProcessShellProvider for VirtualEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        let process_id = format!(
            "process_{}",
            self.processes
                .lock()
                .map_or(0, |processes| processes.len() + 1)
        );
        let mut metadata = Metadata::default();
        if let Some(timeout_seconds) = command.timeout_seconds {
            metadata.insert(
                "timeout_seconds".to_string(),
                serde_json::json!(timeout_seconds),
            );
        }
        if let Some(cwd) = &command.cwd {
            metadata.insert("cwd".to_string(), serde_json::json!(cwd));
        }
        if !command.environment.is_empty() {
            metadata.insert(
                "environment".to_string(),
                serde_json::json!(command.environment),
            );
        }
        let snapshot = ShellProcessSnapshot {
            process_id: process_id.clone(),
            command: command.command,
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata,
        };
        self.processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(process_id, snapshot.clone());
        Ok(snapshot)
    }

    async fn wait_process(
        &self,
        process_id: &str,
        _timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(process_id)
            .cloned()
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        Ok(self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .values()
            .cloned()
            .collect())
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let snapshot = processes
            .get_mut(process_id)
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
        snapshot
            .metadata
            .insert("last_input".to_string(), serde_json::json!(text));
        snapshot
            .metadata
            .insert("close_stdin".to_string(), serde_json::json!(close_stdin));
        let snapshot = snapshot.clone();
        drop(processes);
        Ok(snapshot)
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let snapshot = processes
            .get_mut(process_id)
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
        snapshot
            .metadata
            .insert("last_signal".to_string(), serde_json::json!(signal));
        let snapshot = snapshot.clone();
        drop(processes);
        Ok(snapshot)
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let snapshot = processes
            .get_mut(process_id)
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
        snapshot.status = ShellProcessStatus::Killed;
        let snapshot = snapshot.clone();
        drop(processes);
        Ok(snapshot)
    }
}
