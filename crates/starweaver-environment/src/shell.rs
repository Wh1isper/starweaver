//! Local shell process helpers.

use std::{
    collections::BTreeMap,
    env,
    io::{self, Read},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use command_group::{CommandGroup as _, GroupChild};
use starweaver_core::Metadata;

use crate::{
    EnvironmentError, EnvironmentResult, ProgramCommand, ShellCommand, ShellOutput,
    ShellProcessSnapshot, ShellProcessStatus, local_provider::LocalShellProcess,
};

/// Default simultaneous local foreground/background process allowance.
pub const DEFAULT_LOCAL_PROCESS_CONCURRENCY: usize = 8;
/// Default maximum retained bytes for each stdout/stderr stream.
pub const DEFAULT_LOCAL_OUTPUT_BYTES: usize = 1024 * 1024;
/// Default number of completed background snapshots kept by a provider.
pub const DEFAULT_LOCAL_COMPLETED_PROCESS_RETENTION: usize = 64;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
const PIPE_READER_JOIN_GRACE_PERIOD: Duration = Duration::from_secs(1);
#[cfg(unix)]
const TERMINATION_GRACE_PERIOD: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct LocalExecutionLimiter {
    max: usize,
    active: Mutex<usize>,
}

impl LocalExecutionLimiter {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            active: Mutex::new(0),
        }
    }

    pub(crate) fn try_acquire(self: &Arc<Self>) -> EnvironmentResult<LocalExecutionPermit> {
        let mut active = self
            .active
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        if *active >= self.max {
            return Err(EnvironmentError::Provider(format!(
                "local process concurrency limit exhausted (maximum {})",
                self.max
            )));
        }
        *active += 1;
        drop(active);
        Ok(LocalExecutionPermit {
            limiter: Arc::clone(self),
        })
    }
}

#[derive(Debug)]
pub struct LocalExecutionPermit {
    limiter: Arc<LocalExecutionLimiter>,
}

impl Drop for LocalExecutionPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.limiter.active.lock() {
            *active = active.saturating_sub(1);
        }
    }
}

#[derive(Debug)]
pub struct CapturedPipe {
    text: String,
    total_bytes: u64,
    captured_bytes: usize,
    truncated: bool,
    drain_timed_out: bool,
    capture_failed: bool,
}

impl CapturedPipe {
    const fn failed() -> Self {
        Self {
            text: String::new(),
            total_bytes: 0,
            captured_bytes: 0,
            truncated: true,
            drain_timed_out: false,
            capture_failed: true,
        }
    }
}

pub fn shell_process_metadata(command: &ShellCommand) -> Metadata {
    process_metadata(
        command.timeout_seconds,
        command.cwd.as_deref(),
        &command.environment,
        "shell",
    )
}

pub fn program_process_metadata(command: &ProgramCommand) -> Metadata {
    process_metadata(
        command.timeout_seconds,
        command.cwd.as_deref(),
        &command.environment,
        "program",
    )
}

fn process_metadata(
    timeout_seconds: Option<u64>,
    cwd: Option<&str>,
    environment: &BTreeMap<String, String>,
    execution_mode: &str,
) -> Metadata {
    let mut metadata = Metadata::default();
    if let Some(timeout_seconds) = timeout_seconds {
        metadata.insert(
            "timeout_seconds".to_string(),
            serde_json::json!(timeout_seconds),
        );
    }
    if let Some(cwd) = cwd {
        metadata.insert("cwd".to_string(), serde_json::json!(cwd));
    }
    if !environment.is_empty() {
        metadata.insert(
            "environment_variables".to_string(),
            serde_json::json!(environment.keys().collect::<Vec<_>>()),
        );
    }
    metadata.insert(
        "execution_mode".to_string(),
        serde_json::json!(execution_mode),
    );
    metadata.insert(
        "process_isolation".to_string(),
        serde_json::json!(process_isolation()),
    );
    metadata
}

const fn process_isolation() -> &'static str {
    #[cfg(unix)]
    {
        "unix_process_group"
    }
    #[cfg(windows)]
    {
        "windows_job_object"
    }
    #[cfg(not(any(unix, windows)))]
    {
        "group_fallback"
    }
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

pub fn local_program_command(command: &ProgramCommand) -> EnvironmentResult<Command> {
    if command.program.is_empty() {
        return Err(EnvironmentError::InvalidRequest(
            "program must not be empty".to_string(),
        ));
    }
    let mut process = Command::new(&command.program);
    process.args(&command.arguments);
    Ok(process)
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

pub fn spawn_group(mut process: Command) -> EnvironmentResult<GroupChild> {
    process
        .group_spawn()
        .map_err(|error| EnvironmentError::Provider(error.to_string()))
}

pub fn refresh_local_shell_process(
    process_id: &str,
    process: &mut LocalShellProcess,
    kill: bool,
) -> EnvironmentResult<ShellProcessSnapshot> {
    if let Some(snapshot) = &process.completed {
        return Ok(snapshot.clone());
    }

    let timed_out = process
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline);
    let (status, termination_cleanup_failed) = if kill || timed_out {
        match terminate_process_group(&mut process.child) {
            Ok(status) => (Some(status), false),
            Err(termination_error) => match process.child.try_wait() {
                Ok(Some(status)) => (Some(status), true),
                Ok(None) | Err(_) => return Err(termination_error),
            },
        }
    } else {
        (
            process
                .child
                .try_wait()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?,
            false,
        )
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

    // The leader may exit while descendants still own stdout/stderr. Always
    // attempt to close out the group before joining readers. Capture cleanup
    // degradation in the terminal snapshot instead of retaining the permit.
    let group_cleanup_failed =
        termination_cleanup_failed || kill_remaining_process_group(&mut process.child).is_err();
    let stdout = join_pipe_reader(process.stdout_handle.take(), process_id, "stdout")
        .unwrap_or_else(|_| CapturedPipe::failed());
    let stderr = join_pipe_reader(process.stderr_handle.take(), process_id, "stderr")
        .unwrap_or_else(|_| CapturedPipe::failed());
    let mut metadata = process.metadata.clone();
    if timed_out {
        metadata.insert("timed_out".to_string(), serde_json::json!(true));
    }
    if group_cleanup_failed {
        metadata.insert(
            "process_group_cleanup_failed".to_string(),
            serde_json::json!(true),
        );
    }
    add_capture_metadata(&mut metadata, "stdout", &stdout);
    add_capture_metadata(&mut metadata, "stderr", &stderr);
    let snapshot = ShellProcessSnapshot {
        process_id: process_id.to_string(),
        command: process.command.clone(),
        status: if kill || timed_out {
            ShellProcessStatus::Killed
        } else if status.success() {
            ShellProcessStatus::Completed
        } else {
            ShellProcessStatus::Failed
        },
        stdout: stdout.text,
        stderr: stderr.text,
        return_code: status.code(),
        metadata,
    };
    process.completed_at = Some(Instant::now());
    process.completed = Some(snapshot.clone());
    process.execution_permit.take();
    Ok(snapshot)
}

pub fn run_local_shell_command(
    command: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    timeout_seconds: Option<u64>,
    max_output_bytes: usize,
    cancelled: &AtomicBool,
) -> EnvironmentResult<ShellOutput> {
    run_local_command(
        local_shell_command(command),
        cwd,
        environment,
        timeout_seconds,
        max_output_bytes,
        "shell command timed out",
        cancelled,
    )
}

pub fn run_local_program_command(
    command: &ProgramCommand,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    max_output_bytes: usize,
    cancelled: &AtomicBool,
) -> EnvironmentResult<ShellOutput> {
    run_local_command(
        local_program_command(command)?,
        cwd,
        environment,
        command.timeout_seconds,
        max_output_bytes,
        "program timed out",
        cancelled,
    )
}

/// Resolve an untrusted timeout without allowing `Instant` overflow.
pub fn checked_timeout_deadline(seconds: u64) -> EnvironmentResult<Instant> {
    Instant::now()
        .checked_add(Duration::from_secs(seconds))
        .ok_or_else(|| {
            EnvironmentError::InvalidRequest(
                "timeout_seconds exceeds the supported duration".to_string(),
            )
        })
}

struct ForegroundProcessGuard {
    child: GroupChild,
    armed: bool,
}

impl ForegroundProcessGuard {
    const fn new(child: GroupChild) -> Self {
        Self { child, armed: true }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ForegroundProcessGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = kill_remaining_process_group(&mut self.child);
        }
    }
}

fn run_local_command(
    mut process: Command,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    timeout_seconds: Option<u64>,
    max_output_bytes: usize,
    timeout_message: &str,
    cancelled: &AtomicBool,
) -> EnvironmentResult<ShellOutput> {
    let deadline = timeout_seconds.map(checked_timeout_deadline).transpose()?;
    process
        .current_dir(cwd)
        .envs(environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ForegroundProcessGuard::new(spawn_group(process)?);
    let stdout_reader = child.child.inner().stdout.take();
    let stderr_reader = child.child.inner().stderr.take();
    let stdout_handle = thread::spawn(move || read_child_pipe(stdout_reader, max_output_bytes));
    let stderr_handle = thread::spawn(move || read_child_pipe(stderr_reader, max_output_bytes));

    let mut timed_out = false;
    let status = loop {
        match child.child.try_wait() {
            Ok(Some(status)) => {
                kill_remaining_process_group(&mut child.child)?;
                break status;
            }
            Ok(None) if cancelled.load(Ordering::Acquire) => {
                break terminate_process_group(&mut child.child)?;
            }
            Ok(None) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                timed_out = true;
                break terminate_process_group(&mut child.child)?;
            }
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
        }
    };
    child.disarm();

    let stdout = join_foreground_pipe_reader(stdout_handle)?;
    let mut stderr = join_foreground_pipe_reader(stderr_handle)?;
    let mut metadata = Metadata::default();
    add_capture_metadata(&mut metadata, "stdout", &stdout);
    add_capture_metadata(&mut metadata, "stderr", &stderr);
    if timed_out {
        metadata.insert("timed_out".to_string(), serde_json::json!(true));
        metadata.insert(
            "timeout_seconds".to_string(),
            serde_json::json!(timeout_seconds),
        );
        if !stderr.text.is_empty() && !stderr.text.ends_with('\n') {
            stderr.text.push('\n');
        }
        stderr.text.push_str(timeout_message);
    }
    Ok(ShellOutput {
        status: status.code().unwrap_or(-1),
        stdout: stdout.text,
        stderr: stderr.text,
        metadata,
    })
}

pub fn read_child_pipe(
    pipe: Option<impl Read>,
    max_output_bytes: usize,
) -> io::Result<CapturedPipe> {
    let mut retained = Vec::with_capacity(max_output_bytes.min(8192));
    let mut total_bytes = 0_u64;
    if let Some(mut pipe) = pipe {
        let mut chunk = [0_u8; 8192];
        loop {
            let read = pipe.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            let remaining = max_output_bytes.saturating_sub(retained.len());
            retained.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
    let captured_bytes = retained.len();
    Ok(CapturedPipe {
        text: String::from_utf8_lossy(&retained).into_owned(),
        total_bytes,
        captured_bytes,
        truncated: total_bytes > u64::try_from(captured_bytes).unwrap_or(u64::MAX),
        drain_timed_out: false,
        capture_failed: false,
    })
}

fn add_capture_metadata(metadata: &mut Metadata, stream: &str, capture: &CapturedPipe) {
    metadata.insert(
        format!("{stream}_bytes"),
        serde_json::json!(capture.total_bytes),
    );
    metadata.insert(
        format!("{stream}_captured_bytes"),
        serde_json::json!(capture.captured_bytes),
    );
    metadata.insert(
        format!("{stream}_truncated"),
        serde_json::json!(capture.truncated),
    );
    if capture.drain_timed_out {
        metadata.insert(format!("{stream}_drain_timed_out"), serde_json::json!(true));
    }
    if capture.capture_failed {
        metadata.insert(format!("{stream}_capture_failed"), serde_json::json!(true));
    }
}

fn join_pipe_reader(
    handle: Option<thread::JoinHandle<io::Result<CapturedPipe>>>,
    process_id: &str,
    stream: &str,
) -> EnvironmentResult<CapturedPipe> {
    let handle = handle.ok_or_else(|| {
        EnvironmentError::Provider(format!("{stream} reader missing for process: {process_id}"))
    })?;
    join_foreground_pipe_reader(handle)
}

fn join_foreground_pipe_reader(
    handle: thread::JoinHandle<io::Result<CapturedPipe>>,
) -> EnvironmentResult<CapturedPipe> {
    let deadline = Instant::now() + PIPE_READER_JOIN_GRACE_PERIOD;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            drop(handle);
            return Ok(CapturedPipe {
                text: String::new(),
                total_bytes: 0,
                captured_bytes: 0,
                truncated: true,
                drain_timed_out: true,
                capture_failed: false,
            });
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    handle
        .join()
        .map_err(|_| EnvironmentError::Provider("failed to join shell output reader".to_string()))?
        .map_err(|error| EnvironmentError::Provider(error.to_string()))
}

pub fn terminate_process_group(child: &mut GroupChild) -> EnvironmentResult<ExitStatus> {
    #[cfg(unix)]
    {
        use command_group::{Signal, UnixChildExt as _};
        if let Err(error) = child.signal(Signal::SIGTERM)
            && !process_group_is_gone(&error)
        {
            return Err(EnvironmentError::Provider(error.to_string()));
        }
        let deadline = Instant::now() + TERMINATION_GRACE_PERIOD;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    kill_remaining_process_group(child)?;
                    return Ok(status);
                }
                Ok(None) if Instant::now() >= deadline => break,
                Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
                Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
            }
        }
    }

    kill_remaining_process_group(child)?;
    child
        .wait()
        .map_err(|error| EnvironmentError::Provider(error.to_string()))
}

pub fn kill_remaining_process_group(child: &mut GroupChild) -> EnvironmentResult<()> {
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if process_group_is_gone(&error) => Ok(()),
        Err(error) => Err(EnvironmentError::Provider(error.to_string())),
    }
}

fn process_group_is_gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidInput
        || error.kind() == io::ErrorKind::NotFound
        || error.raw_os_error() == Some(3)
}

#[cfg(unix)]
pub fn signal_process_group(child: &GroupChild, signal: i32) -> EnvironmentResult<()> {
    use command_group::{Signal, UnixChildExt as _};
    let signal = Signal::try_from(signal).map_err(|_| {
        EnvironmentError::InvalidRequest(format!("unsupported Unix signal: {signal}"))
    })?;
    child
        .signal(signal)
        .map_err(|error| EnvironmentError::Provider(error.to_string()))
}
