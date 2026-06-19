//! Local shell process helpers.

use std::{
    collections::BTreeMap,
    env,
    io::{self, Read},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use starweaver_core::Metadata;

use crate::{
    local_provider::LocalShellProcess, EnvironmentError, EnvironmentResult, ShellCommand,
    ShellOutput, ShellProcessSnapshot, ShellProcessStatus,
};

pub fn shell_process_metadata(command: &ShellCommand) -> Metadata {
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
    metadata
}

pub fn local_shell_executable() -> String {
    #[cfg(windows)]
    {
        if let Some(shell) = env::var_os("SHELL").as_deref().and_then(valid_shell_value) {
            return shell;
        }
        if let Some(shell) = find_executable_in_path(&["bash.exe", "bash", "sh.exe", "sh"]) {
            return shell;
        }
        for candidate in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\sh.exe",
        ] {
            let path = std::path::PathBuf::from(candidate);
            if path.is_file() {
                return path.to_string_lossy().to_string();
            }
        }
        env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }

    #[cfg(not(windows))]
    {
        env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

pub fn local_shell_command(command: &str) -> Command {
    let executable = local_shell_executable();
    let mut process = Command::new(&executable);
    if uses_windows_cmd(&executable) {
        process.arg("/C");
    } else {
        process.arg("-lc");
    }
    process.arg(command);
    process
}

#[cfg(windows)]
fn valid_shell_value(value: &std::ffi::OsStr) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let shell = Path::new(value);
    if shell.components().count() > 1 && !shell.is_file() {
        return None;
    }
    Some(value.to_string_lossy().to_string())
}

#[cfg(windows)]
fn find_executable_in_path(names: &[&str]) -> Option<String> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn uses_windows_cmd(executable: &str) -> bool {
    Path::new(executable)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("cmd"))
}

pub fn refresh_local_shell_process(
    process_id: &str,
    process: &mut LocalShellProcess,
    kill: bool,
) -> EnvironmentResult<ShellProcessSnapshot> {
    if let Some(snapshot) = &process.completed {
        return Ok(snapshot.clone());
    }
    let status = if kill {
        let _ = process.child.kill();
        Some(
            process
                .child
                .wait()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?,
        )
    } else {
        process
            .child
            .try_wait()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
    };
    let Some(status) = status else {
        return Ok(ShellProcessSnapshot {
            process_id: process_id.to_string(),
            command: process.command.clone(),
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata: process.metadata.clone(),
        });
    };
    let stdout_handle = process.stdout_handle.take().ok_or_else(|| {
        EnvironmentError::Provider(format!("stdout reader missing for process: {process_id}"))
    })?;
    let stderr_handle = process.stderr_handle.take().ok_or_else(|| {
        EnvironmentError::Provider(format!("stderr reader missing for process: {process_id}"))
    })?;
    let snapshot = ShellProcessSnapshot {
        process_id: process_id.to_string(),
        command: process.command.clone(),
        status: if kill {
            ShellProcessStatus::Killed
        } else if status.success() {
            ShellProcessStatus::Completed
        } else {
            ShellProcessStatus::Failed
        },
        stdout: join_pipe_reader(stdout_handle)?,
        stderr: join_pipe_reader(stderr_handle)?,
        return_code: status.code(),
        metadata: process.metadata.clone(),
    };
    process.completed = Some(snapshot.clone());
    Ok(snapshot)
}

pub fn run_local_shell_command(
    command: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    timeout_seconds: Option<u64>,
) -> EnvironmentResult<ShellOutput> {
    let mut process = local_shell_command(command);
    let mut child = process
        .current_dir(cwd)
        .envs(environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
    let mut stdout_reader = child.stdout.take();
    let mut stderr_reader = child.stderr.take();
    let stdout_handle = thread::spawn(move || read_child_pipe(stdout_reader.take()));
    let stderr_handle = thread::spawn(move || read_child_pipe(stderr_reader.take()));

    let mut timed_out = false;
    let status = if let Some(seconds) = timeout_seconds {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    let _ = child.kill();
                    break child
                        .wait()
                        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                }
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
            }
        }
    } else {
        child
            .wait()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
    };

    let stdout = join_pipe_reader(stdout_handle)?;
    let mut stderr = join_pipe_reader(stderr_handle)?;
    let mut metadata = Metadata::default();
    if timed_out {
        metadata.insert("timed_out".to_string(), serde_json::json!(true));
        metadata.insert(
            "timeout_seconds".to_string(),
            serde_json::json!(timeout_seconds),
        );
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str("shell command timed out");
    }
    Ok(ShellOutput {
        status: status.code().unwrap_or(-1),
        stdout,
        stderr,
        metadata,
    })
}

pub fn read_child_pipe(pipe: Option<impl Read>) -> io::Result<String> {
    let mut output = String::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_string(&mut output)?;
    }
    Ok(output)
}

pub fn join_pipe_reader(
    handle: thread::JoinHandle<io::Result<String>>,
) -> EnvironmentResult<String> {
    handle
        .join()
        .map_err(|_| EnvironmentError::Provider("failed to join shell output reader".to_string()))?
        .map_err(|error| EnvironmentError::Provider(error.to_string()))
}
