use std::{
    env, fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::common::{binary_name, root, run_command, target_dir};

const INITIALIZE_EXAMPLE: &str = "protocol/host/examples/initialize.request.json";

pub fn check(args: &[String]) -> Result<(), String> {
    ensure_no_args(args, "check-rpc-contracts")?;
    check_generated_boundaries()?;
    let repository = root()?;
    let rpc = build_rpc_binary(&repository)?;
    check_transports_with_rpc(&rpc)?;
    println!(
        "generated starweaver.host contract validated through typed Rust, stdio, and loopback HTTP gates"
    );
    Ok(())
}

pub fn check_transports(args: &[String]) -> Result<(), String> {
    ensure_no_args(args, "check-rpc-transports")?;
    let repository = root()?;
    let rpc = build_rpc_binary(&repository)?;
    check_transports_with_rpc(&rpc)
}

pub fn check_transports_with_rpc(rpc: &Path) -> Result<(), String> {
    if !rpc.is_file() {
        return Err(format!("missing RPC contract binary: {}", rpc.display()));
    }
    let repository = root()?;
    let initialize = canonical_initialize_request(&repository)?;
    check_transport_runtime(rpc, &initialize)?;
    println!("generated starweaver.host identity validated through stdio and loopback HTTP");
    Ok(())
}

fn ensure_no_args(args: &[String], command: &str) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("{command} takes no arguments"))
    }
}

fn check_generated_boundaries() -> Result<(), String> {
    let repository = root()?;
    run_command(Command::new("cargo").current_dir(&repository).args([
        "test",
        "-p",
        "starweaver-rpc-core",
        "--test",
        "generated_protocol",
        "--locked",
    ]))?;
    run_command(Command::new("cargo").current_dir(repository).args([
        "test",
        "-p",
        "starweaver-rpc",
        "--lib",
        "service::generated_service_tests",
        "--locked",
    ]))
}

fn build_rpc_binary(repository: &Path) -> Result<PathBuf, String> {
    run_command(Command::new("cargo").current_dir(repository).args([
        "build",
        "-p",
        "starweaver-rpc",
        "--bin",
        "starweaver-rpc",
        "--locked",
    ]))?;
    let rpc = target_dir(repository)
        .join("debug")
        .join(binary_name("starweaver-rpc"));
    if !rpc.is_file() {
        return Err(format!("missing RPC contract binary: {}", rpc.display()));
    }
    Ok(rpc)
}

fn canonical_initialize_request(repository: &Path) -> Result<Value, String> {
    let path = repository.join(INITIALIZE_EXAMPLE);
    let source =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let request: Value = serde_json::from_str(&source)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || request.get("method").and_then(Value::as_str) != Some("initialize")
        || !request.get("id").is_some_and(Value::is_string)
        || !request.get("params").is_some_and(Value::is_object)
    {
        return Err(format!(
            "{INITIALIZE_EXAMPLE} is not a canonical generated initialize request"
        ));
    }
    Ok(request)
}

fn check_transport_runtime(rpc: &Path, initialize: &Value) -> Result<(), String> {
    let temp = env::temp_dir().join(format!(
        "starweaver-generated-host-contracts-{}",
        std::process::id()
    ));
    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
    }
    let config = temp.join("config");
    let workspace = temp.join("workspace");
    fs::create_dir_all(&config).map_err(|error| error.to_string())?;
    fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let workspace_toml = toml::Value::String(workspace.to_string_lossy().into_owned()).to_string();
    fs::write(
        config.join("rpc.toml"),
        format!(
            "[server]\nworkspace_root = {workspace_toml}\ndefault_profile = \"general\"\n\n[server.http_auth]\ntoken_env = \"STARWEAVER_RPC_CONTRACT_TOKEN\"\nscopes = [\"read\", \"run\", \"approval\", \"admin\", \"shutdown\"]\n\n[profiles.general]\nmodel_id = \"local_echo\"\n"
        ),
    )
    .map_err(|error| error.to_string())?;

    let result = (|| {
        check_stdio(
            rpc,
            initialize,
            &workspace,
            &config,
            &temp.join("stdio.sqlite"),
        )?;
        check_http(
            rpc,
            initialize,
            &workspace,
            &config,
            &temp.join("http.sqlite"),
        )
    })();
    let _ = fs::remove_dir_all(&temp);
    result
}

fn check_stdio(
    rpc: &Path,
    initialize: &Value,
    workspace: &Path,
    config: &Path,
    store: &Path,
) -> Result<(), String> {
    let mut command = Command::new(rpc);
    command
        .current_dir(workspace)
        .env("STARWEAVER_CONFIG_DIR", config)
        .arg("--store")
        .arg(store)
        .arg("stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut host = ContractStdioHost::spawn(&mut command)?;
    let mut stdio_initialize = initialize.clone();
    let supported = stdio_initialize
        .pointer_mut("/params/supportedFeatures")
        .and_then(Value::as_array_mut)
        .ok_or("canonical initialize supportedFeatures must be an array")?;
    supported.push(json!("host.shutdown"));
    supported.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    let result = (|| {
        let response = host.request(&stdio_initialize)?;
        validate_initialize_response(&stdio_initialize, &response, "stdio")?;
        let shutdown = request(
            "contract-stdio-shutdown",
            "shutdown",
            &json!({"deadlineMs": 2_000}),
        );
        let response = host.request(&shutdown)?;
        validate_response(&shutdown, &response, "stdio")
    })();
    if result.is_err() {
        let _ = host.child.kill();
    }
    let status = host.child.wait().map_err(|error| error.to_string())?;
    result?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("RPC stdio contract host exited with {status}"))
    }
}

struct ContractStdioHost {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ContractStdioHost {
    fn spawn(command: &mut Command) -> Result<Self, String> {
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("RPC contract stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("RPC contract stdout unavailable")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn request(&mut self, request: &Value) -> Result<Value, String> {
        writeln!(self.stdin, "{request}").map_err(|error| error.to_string())?;
        self.stdin.flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if line.is_empty() {
            return Err("RPC stdio host exited before responding".to_string());
        }
        serde_json::from_str(line.trim())
            .map_err(|error| format!("invalid RPC stdio response: {error}"))
    }
}

fn check_http(
    rpc: &Path,
    initialize: &Value,
    workspace: &Path,
    config: &Path,
    store: &Path,
) -> Result<(), String> {
    const TOKEN: &str = "rpc-contract-token-0123456789abcdef0123456789abcdef";
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    drop(listener);

    let mut child = Command::new(rpc)
        .current_dir(workspace)
        .env("STARWEAVER_CONFIG_DIR", config)
        .env("STARWEAVER_RPC_CONTRACT_TOKEN", TOKEN)
        .arg("--store")
        .arg(store)
        .arg("http")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| error.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match http_rpc_request(port, TOKEN, initialize) {
            Ok(response) => {
                validate_initialize_response(initialize, &response, "http readiness")?;
                break;
            }
            Err(error) if Instant::now() < deadline => {
                if let Some(status) = child.try_wait().map_err(|wait| wait.to_string())? {
                    return Err(format!(
                        "RPC HTTP contract host exited with {status} before readiness: {error}"
                    ));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("RPC HTTP contract host was not ready: {error}"));
            }
        }
    }

    let shutdown = request(
        "contract-http-shutdown",
        "shutdown",
        &json!({"deadlineMs": 2_000}),
    );
    let result = http_rpc_request(port, TOKEN, &shutdown)
        .and_then(|response| validate_response(&shutdown, &response, "http"));
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    result?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("RPC HTTP contract host exited with {status}"))
    }
}

fn request(id: &str, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn validate_initialize_response(
    request: &Value,
    response: &Value,
    transport: &str,
) -> Result<(), String> {
    validate_response(request, response, transport)?;
    if response.pointer("/result/protocol") != request.pointer("/params/protocol") {
        return Err(format!(
            "{transport} initialize response did not preserve the exact generated identity: {response}"
        ));
    }
    Ok(())
}

fn validate_response(request: &Value, response: &Value, transport: &str) -> Result<(), String> {
    if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || response.get("id") != request.get("id")
        || (response.get("result").is_none() == response.get("error").is_none())
    {
        return Err(format!(
            "{transport} returned an invalid generated JSON-RPC response: {response}"
        ));
    }
    if let Some(error) = response.get("error") {
        return Err(format!(
            "{transport} rejected canonical {} request: {error}",
            request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ));
    }
    Ok(())
}

fn http_rpc_request(port: u16, token: &str, request: &Value) -> Result<Value, String> {
    let body = request.to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| error.to_string())?;
    let response = String::from_utf8(response)
        .map_err(|error| format!("RPC HTTP response is not UTF-8: {error}"))?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("malformed RPC HTTP response: {response}"))?;
    if !head
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return Err(format!("RPC HTTP request failed: {response}"));
    }
    serde_json::from_str(body)
        .map_err(|error| format!("invalid RPC HTTP JSON response: {error}: {body}"))
}
