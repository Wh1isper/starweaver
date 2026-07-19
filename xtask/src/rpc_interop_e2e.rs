use std::{
    env, fs,
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};

use crate::common::{binary_name, root, run_capture, run_command, target_dir};

pub fn check() -> Result<(), String> {
    let repository = root()?;
    let (cli, rpc) = build_binaries(&repository)?;
    check_with_binaries(&cli, &rpc)
}

pub fn build_binaries(repository: &Path) -> Result<(PathBuf, PathBuf), String> {
    run_command(Command::new("cargo").current_dir(repository).args([
        "build",
        "-p",
        "starweaver-cli",
        "--bin",
        "starweaver-cli",
        "-p",
        "starweaver-rpc",
        "--bin",
        "starweaver-rpc",
        "--locked",
    ]))?;
    let bin_dir = target_dir(repository).join("debug");
    let cli = bin_dir.join(binary_name("starweaver-cli"));
    let rpc = bin_dir.join(binary_name("starweaver-rpc"));
    for binary in [&cli, &rpc] {
        if !binary.is_file() {
            return Err(format!("missing E2E binary: {}", binary.display()));
        }
    }
    Ok((cli, rpc))
}

pub fn check_with_binaries(cli: &Path, rpc: &Path) -> Result<(), String> {
    for binary in [cli, rpc] {
        if !binary.is_file() {
            return Err(format!("missing E2E binary: {}", binary.display()));
        }
    }
    let temp = env::temp_dir().join(format!("starweaver-rpc-interop-e2e-{}", std::process::id()));
    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
    let result = (|| {
        check_native_default_paths(cli, rpc, &temp)?;
        run_e2e(cli, rpc, &temp.join("interop"))
    })();
    let _ = fs::remove_dir_all(&temp);
    result?;
    println!("CLI/RPC bidirectional subprocess interoperability validated");
    Ok(())
}

fn check_native_default_paths(cli: &Path, rpc: &Path, root: &Path) -> Result<(), String> {
    let home = root.join("native-home");
    let workspace = root.join("native-workspace");
    fs::create_dir_all(&home).map_err(|error| error.to_string())?;
    fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let expected = home.join(".starweaver").join("starweaver.sqlite");
    let mut cli_command = Command::new(cli);
    cli_command
        .current_dir(&workspace)
        .env_remove("STARWEAVER_CONFIG_DIR")
        .env_remove("STARWEAVER_SESSION_DB")
        .env_remove("STARWEAVER_STORE")
        .arg("diagnostics");
    set_native_home(&mut cli_command, &home);
    let cli_diagnostics = run_capture(&mut cli_command)?;
    if !cli_diagnostics.contains(&format!("database_path={}", expected.display())) {
        return Err(format!(
            "CLI native default database did not resolve to {}: {cli_diagnostics}",
            expected.display()
        ));
    }

    let mut rpc_command = Command::new(rpc);
    rpc_command
        .current_dir(&workspace)
        .env_remove("STARWEAVER_CONFIG_DIR")
        .env_remove("STARWEAVER_SESSION_DB")
        .env_remove("STARWEAVER_STORE")
        .arg("stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    set_native_home(&mut rpc_command, &home);
    let mut host = RpcHost::spawn_command(&mut rpc_command)?;
    host.initialize()?;
    let diagnostics = host.request(2, "diagnostics.get", json!({}))?;
    assert_no_rpc_error(&diagnostics)?;
    if diagnostics["result"]["databasePath"] != expected.to_string_lossy().as_ref() {
        return Err(format!(
            "RPC native default database did not match CLI default: {diagnostics}"
        ));
    }
    host.shutdown()
}

fn set_native_home(command: &mut Command, home: &Path) {
    if cfg!(windows) {
        command.env("USERPROFILE", home);
        command.env_remove("HOMEDRIVE");
        command.env_remove("HOMEPATH");
        command.env_remove("HOME");
    } else {
        command.env("HOME", home);
    }
}

#[allow(clippy::too_many_lines)]
fn run_e2e(cli: &Path, rpc: &Path, root: &Path) -> Result<(), String> {
    let global = root.join("config");
    let workspace = root.join("workspace");
    let project = workspace.join(".starweaver");
    let store = global.join("starweaver.sqlite");
    let legacy_store = project.join("starweaver.sqlite");
    let rpc_legacy_store = root.join("rpc-legacy.sqlite");
    fs::create_dir_all(&global).map_err(|error| error.to_string())?;
    fs::create_dir_all(&project).map_err(|error| error.to_string())?;
    fs::write(
        global.join("config.toml"),
        "[general]\nmodel = \"local_echo\"\ndefault_output = \"json\"\n",
    )
    .map_err(|error| error.to_string())?;
    let workspace_toml = toml::Value::String(workspace.to_string_lossy().into_owned()).to_string();
    fs::write(
        global.join("rpc.toml"),
        format!(
            "[server]\nworkspace_root = {workspace_toml}\ndefault_profile = \"local\"\n\n[profiles.local]\nmodel_id = \"local_echo\"\n"
        ),
    )
    .map_err(|error| error.to_string())?;

    let legacy_seed = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &legacy_store,
        [
            "run",
            "--prompt",
            "legacy seed",
            "--new-session",
            "--output",
            "json",
        ],
    )?;
    let legacy_seed = parse_single_json(&legacy_seed, "legacy CLI seed")?;
    let legacy_session_id = required_json_string(&legacy_seed, "sessionId")?;
    let before_import = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        ["session", "list", "--output", "json"],
    )?;
    if before_import.contains(&legacy_session_id) {
        return Err(
            "opening canonical CLI storage imported legacy evidence implicitly".to_string(),
        );
    }
    let source_arg = legacy_store.to_string_lossy().into_owned();
    let workspace_arg = workspace.to_string_lossy().into_owned();
    let imported = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        [
            "storage",
            "import-legacy",
            "--source",
            &source_arg,
            "--workspace",
            &workspace_arg,
            "--output",
            "json",
        ],
    )?;
    let imported = parse_single_json(&imported, "explicit CLI legacy import")?;
    if imported["sessionsImported"] != 1 || imported["imported"] != true {
        return Err(format!("explicit CLI import failed: {imported}"));
    }
    let retried = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        [
            "storage",
            "import-legacy",
            "--source",
            &source_arg,
            "--workspace",
            &workspace_arg,
            "--output",
            "json",
        ],
    )?;
    let retried = parse_single_json(&retried, "idempotent CLI legacy import")?;
    if retried["sessionsImported"] != 0 || retried["imported"] != false {
        return Err(format!("CLI import retry was not idempotent: {retried}"));
    }

    let cli_seed = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        [
            "run",
            "--prompt",
            "cli seed",
            "--new-session",
            "--output",
            "json",
        ],
    )?;
    let cli_seed = parse_single_json(&cli_seed, "CLI seed run")?;
    let session_id = required_json_string(&cli_seed, "sessionId")?;
    let cli_run_id = required_json_string(&cli_seed, "runId")?;
    if cli_seed["status"] != "completed" {
        return Err(format!("CLI seed did not complete: {cli_seed}"));
    }

    let rpc_legacy_seed = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &rpc_legacy_store,
        [
            "run",
            "--prompt",
            "rpc import seed",
            "--new-session",
            "--output",
            "json",
        ],
    )?;
    let rpc_legacy_seed = parse_single_json(&rpc_legacy_seed, "RPC legacy seed")?;
    let rpc_legacy_session_id = required_json_string(&rpc_legacy_seed, "sessionId")?;

    let mut host = RpcHost::spawn(rpc, &workspace, &global, &store)?;
    host.initialize()?;
    let rpc_source_arg = rpc_legacy_store.to_string_lossy().into_owned();
    let imported_by_rpc = host.request(
        2,
        "storage.importLegacy",
        json!({"sourcePath": rpc_source_arg, "workspace": workspace_arg}),
    )?;
    assert_no_rpc_error(&imported_by_rpc)?;
    if imported_by_rpc["result"]["sessionsImported"] != 1
        || imported_by_rpc["result"]["imported"] != true
    {
        return Err(format!("typed RPC import failed: {imported_by_rpc}"));
    }
    let retried_by_rpc = host.request(
        3,
        "storage.importLegacy",
        json!({"sourcePath": rpc_source_arg, "workspace": workspace_arg}),
    )?;
    assert_no_rpc_error(&retried_by_rpc)?;
    if retried_by_rpc["result"]["sessionsImported"] != 0
        || retried_by_rpc["result"]["imported"] != false
    {
        return Err(format!(
            "typed RPC import retry was not idempotent: {retried_by_rpc}"
        ));
    }
    let listed = host.request(4, "session.list", json!({"limit": 50}))?;
    assert_no_rpc_error(&listed)?;
    if !listed["result"]["sessions"]
        .as_array()
        .is_some_and(|sessions| {
            [legacy_session_id.as_str(), rpc_legacy_session_id.as_str()]
                .iter()
                .all(|expected| {
                    sessions
                        .iter()
                        .any(|session| session["session_id"] == *expected)
                })
        })
    {
        return Err(format!(
            "RPC could not read explicitly imported legacy sessions: {listed}"
        ));
    }
    let loaded = host.request(
        5,
        "session.get",
        json!({"sessionId": session_id, "runs": 20}),
    )?;
    assert_no_rpc_error(&loaded)?;
    if loaded["result"]["session"]["session_id"] != session_id {
        return Err(format!("RPC could not read CLI session: {loaded}"));
    }
    let replayed = host.request(
        6,
        "stream.replay",
        json!({"sessionId": session_id, "runId": cli_run_id}),
    )?;
    assert_no_rpc_error(&replayed)?;
    if replayed["result"]["messages"]
        .as_array()
        .is_none_or(Vec::is_empty)
    {
        return Err(format!("RPC could not replay CLI evidence: {replayed}"));
    }
    let started = host.request(
        7,
        "run.start",
        json!({
            "sessionId": session_id,
            "restoreFromRunId": cli_run_id,
            "profile": "local",
            "continuationMode": "switch",
            "prompt": "rpc continues cli",
            "idempotencyKey": "rpc-interop-e2e-cli-to-rpc"
        }),
    )?;
    assert_no_rpc_error(&started)?;
    let rpc_run_id = required_json_string(&started["result"], "runId")?;
    let awaited = host.request(
        8,
        "run.await",
        json!({"sessionId": session_id, "runId": rpc_run_id, "timeoutMs": 10_000}),
    )?;
    assert_no_rpc_error(&awaited)?;
    if awaited["result"]["status"]["status"] != "completed" {
        return Err(format!("RPC continuation did not complete: {awaited}"));
    }
    host.shutdown()?;

    let cli_show = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        ["session", "show", &session_id, "--output", "json"],
    )?;
    let cli_show = parse_single_json(&cli_show, "CLI session show")?;
    if !cli_show["runs"]
        .as_array()
        .is_some_and(|runs| runs.iter().any(|run| run["run_id"] == rpc_run_id))
    {
        return Err(format!("CLI could not read RPC run: {cli_show}"));
    }
    let cli_replay = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        [
            "session",
            "replay",
            &session_id,
            "--run",
            &rpc_run_id,
            "--output",
            "json",
        ],
    )?;
    let cli_replay = parse_single_json(&cli_replay, "CLI RPC replay")?;
    if cli_replay["messages"].as_array().is_none_or(Vec::is_empty) {
        return Err(format!("CLI could not replay RPC evidence: {cli_replay}"));
    }
    let cli_continued = cli_capture(
        cli,
        &workspace,
        &global,
        &project,
        &store,
        [
            "run",
            "--prompt",
            "cli continues rpc",
            "--session",
            &session_id,
            "--run",
            &rpc_run_id,
            "--continuation-mode",
            "switch",
            "--output",
            "json",
        ],
    )?;
    let cli_continued = parse_single_json(&cli_continued, "CLI continuation")?;
    if cli_continued["status"] != "completed" {
        return Err(format!(
            "CLI continuation did not complete: {cli_continued}"
        ));
    }
    Ok(())
}

fn cli_capture<const N: usize>(
    cli: &Path,
    workspace: &Path,
    global: &Path,
    project: &Path,
    store: &Path,
    args: [&str; N],
) -> Result<String, String> {
    run_capture(
        Command::new(cli)
            .current_dir(workspace)
            .env("STARWEAVER_CONFIG_DIR", global)
            .env("STARWEAVER_PROJECT_DIR", project)
            .arg("--store")
            .arg(store)
            .args(args),
    )
}

struct RpcHost {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RpcHost {
    fn spawn(rpc: &Path, workspace: &Path, global: &Path, store: &Path) -> Result<Self, String> {
        let mut command = Command::new(rpc);
        command
            .current_dir(workspace)
            .env("STARWEAVER_CONFIG_DIR", global)
            .arg("--store")
            .arg(store)
            .arg("stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        Self::spawn_command(&mut command)
    }

    fn spawn_command(command: &mut Command) -> Result<Self, String> {
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("RPC stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("RPC stdout unavailable")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        let response = self.request(1, "initialize", json!({}))?;
        assert_no_rpc_error(&response)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").map_err(|error| error.to_string())?;
        self.stdin.flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if line.is_empty() {
            return Err(format!("RPC exited before responding to {method}"));
        }
        serde_json::from_str(line.trim()).map_err(|error| format!("invalid RPC response: {error}"))
    }

    fn shutdown(mut self) -> Result<(), String> {
        let response = self.request(99, "shutdown", json!({}))?;
        assert_no_rpc_error(&response)?;
        let status = self.child.wait().map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("RPC exited with {status}"))
        }
    }
}

impl Drop for RpcHost {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn assert_no_rpc_error(response: &Value) -> Result<(), String> {
    response
        .get("error")
        .map_or(Ok(()), |error| Err(format!("RPC error response: {error}")))
}

fn parse_single_json(text: &str, context: &str) -> Result<Value, String> {
    serde_json::from_str(text.trim())
        .map_err(|error| format!("invalid {context} JSON: {error}: {text}"))
}

fn required_json_string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("missing {key} in {value}"))
}
