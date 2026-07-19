use std::{
    collections::BTreeSet,
    env, fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::common::{root, run_command};

const CORPUS: &str = "crates/starweaver-rpc-core/tests/fixtures/contracts/rpc-wire-v1.json";
const CATALOG: &str =
    "crates/starweaver-rpc-core/tests/fixtures/contracts/rpc-contract-catalog-v1.json";
const SCHEMA: &str = "crates/starweaver-rpc-core/schemas/rpc-wire-v1.schema.json";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Corpus {
    schema: String,
    major: u32,
    revision: String,
    methods: Vec<Method>,
    notifications: Vec<Value>,
    invalid_notifications: Vec<Value>,
    errors: Vec<ErrorFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Method {
    #[serde(rename = "method")]
    name: String,
    canonical_params: Value,
    canonical_result: Value,
    invalid_params: Vec<Value>,
    invalid_results: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ErrorFixture {
    name: String,
    code: i64,
    response: Value,
    invalid_responses: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    schema: String,
    major: u32,
    revision: String,
    methods: Vec<CatalogMethod>,
    notifications: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogMethod {
    #[serde(rename = "method")]
    name: String,
    params_type: String,
    result_type: String,
}

pub fn generate(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("generate-rpc-contracts takes no arguments".to_string());
    }
    let corpus = load_corpus()?;
    let catalog = load_catalog()?;
    validate_corpus(&corpus, &catalog)?;
    write_generated(SCHEMA, &render_schema(&corpus)?)?;
    Ok(())
}

pub fn check(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("check-rpc-contracts takes no arguments".to_string());
    }
    let corpus = load_corpus()?;
    let catalog = load_catalog()?;
    validate_corpus(&corpus, &catalog)?;
    check_generated(SCHEMA, &render_schema(&corpus)?)?;
    check_in_process_corpus()?;
    check_wire_runtime(&corpus)?;
    Ok(())
}

fn load_corpus() -> Result<Corpus, String> {
    let source = fs::read_to_string(CORPUS).map_err(|error| format!("read {CORPUS}: {error}"))?;
    serde_json::from_str(&source).map_err(|error| format!("parse {CORPUS}: {error}"))
}

fn load_catalog() -> Result<Catalog, String> {
    let source = fs::read_to_string(CATALOG).map_err(|error| format!("read {CATALOG}: {error}"))?;
    serde_json::from_str(&source).map_err(|error| format!("parse {CATALOG}: {error}"))
}

fn validate_corpus(corpus: &Corpus, catalog: &Catalog) -> Result<(), String> {
    if corpus.schema != "starweaver.host.wire-corpus" {
        return Err(format!("unexpected corpus schema: {}", corpus.schema));
    }
    if corpus.methods.is_empty()
        || corpus.notifications.is_empty()
        || corpus.invalid_notifications.is_empty()
        || corpus.errors.is_empty()
    {
        return Err(
            "RPC corpus methods, notifications, invalid notifications, and errors must be non-empty"
                .to_string(),
        );
    }
    if catalog.schema != "starweaver.host.contract-catalog"
        || catalog.major != corpus.major
        || catalog.revision.is_empty()
    {
        return Err("RPC catalog identity does not match the wire corpus major".to_string());
    }
    if catalog.methods.len() != corpus.methods.len()
        || catalog.notifications.len() != corpus.notifications.len()
    {
        return Err("RPC catalog and wire corpus cardinalities differ".to_string());
    }
    let mut methods = BTreeSet::new();
    for method in &corpus.methods {
        if !methods.insert(&method.name) {
            return Err(format!("duplicate RPC method: {}", method.name));
        }
        if !method.canonical_params.is_object()
            || method.invalid_params.is_empty()
            || method.invalid_results.is_empty()
        {
            return Err(format!(
                "{} requires object canonical params plus invalid params and result vectors",
                method.name
            ));
        }
        let catalog_method = &catalog.methods[methods.len() - 1];
        if catalog_method.name != method.name
            || catalog_method.params_type.is_empty()
            || catalog_method.result_type.is_empty()
        {
            return Err(format!(
                "RPC catalog does not align with wire method {}",
                method.name
            ));
        }
    }
    if corpus.invalid_notifications.len() != corpus.notifications.len() {
        return Err("every RPC notification requires one invalid vector".to_string());
    }
    let mut notifications = BTreeSet::new();
    for (index, notification) in corpus.notifications.iter().enumerate() {
        let method = notification
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| "notification is missing method".to_string())?;
        if !notifications.insert(method) {
            return Err(format!("duplicate RPC notification: {method}"));
        }
        if catalog.notifications[index] != method {
            return Err(format!(
                "RPC catalog does not align with notification {method}"
            ));
        }
        if corpus.invalid_notifications[index]
            .get("method")
            .and_then(Value::as_str)
            != Some(method)
        {
            return Err(format!(
                "invalid notification vector does not align with {method}"
            ));
        }
    }
    let mut errors = BTreeSet::new();
    for fixture in &corpus.errors {
        if !errors.insert((&fixture.name, fixture.code)) {
            return Err(format!("duplicate RPC error: {}", fixture.name));
        }
        if fixture.invalid_responses.is_empty() {
            return Err(format!(
                "{} requires at least one invalid error response",
                fixture.name
            ));
        }
        if fixture
            .response
            .pointer("/error/code")
            .and_then(Value::as_i64)
            != Some(fixture.code)
        {
            return Err(format!(
                "{} response code does not match catalog",
                fixture.name
            ));
        }
    }
    Ok(())
}

fn render_schema(corpus: &Corpus) -> Result<String, String> {
    let methods = corpus
        .methods
        .iter()
        .map(|method| {
            object_schema(&Map::from_iter([
                ("method".to_string(), json!({"const": method.name})),
                (
                    "canonicalParams".to_string(),
                    schema_for_value(&method.canonical_params),
                ),
                (
                    "canonicalResult".to_string(),
                    schema_for_value(&method.canonical_result),
                ),
                (
                    "invalidParams".to_string(),
                    json!({"type": "array", "minItems": 1, "items": {}}),
                ),
                (
                    "invalidResults".to_string(),
                    json!({"type": "array", "minItems": 1, "items": {}}),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let notifications = corpus
        .notifications
        .iter()
        .map(schema_for_value)
        .collect::<Vec<_>>();
    let errors = corpus
        .errors
        .iter()
        .map(|fixture| {
            object_schema(&Map::from_iter([
                ("name".to_string(), json!({"const": fixture.name})),
                ("code".to_string(), json!({"const": fixture.code})),
                ("response".to_string(), schema_for_value(&fixture.response)),
                (
                    "invalidResponses".to_string(),
                    json!({"type": "array", "minItems": 1, "items": {}}),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://starweaver.dev/schema/rpc-wire-v1.schema.json",
        "title": "Starweaver RPC host wire conformance corpus v1",
        "description": "Generated deterministic schema for the concrete canonical/invalid RPC host wire corpus. Rust DTO deserialization remains the acceptance authority.",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema", "major", "revision", "methods", "notifications", "invalidNotifications", "errors"],
        "properties": {
            "schema": {"const": corpus.schema},
            "major": {"const": corpus.major},
            "revision": {"const": corpus.revision},
            "methods": {"type": "array", "minItems": methods.len(), "maxItems": methods.len(), "items": {"oneOf": methods}},
            "notifications": {"type": "array", "minItems": notifications.len(), "maxItems": notifications.len(), "items": {"oneOf": notifications}},
            "invalidNotifications": {"type": "array", "minItems": corpus.invalid_notifications.len(), "maxItems": corpus.invalid_notifications.len(), "items": {}},
            "errors": {"type": "array", "minItems": errors.len(), "maxItems": errors.len(), "items": {"oneOf": errors}}
        }
    });
    serde_json::to_string_pretty(&schema)
        .map(|text| format!("{text}\n"))
        .map_err(|error| format!("serialize RPC schema: {error}"))
}

fn object_schema(properties: &Map<String, Value>) -> Value {
    let required = properties.keys().cloned().collect::<Vec<_>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}

fn schema_for_value(value: &Value) -> Value {
    match value {
        Value::Null => json!({"type": "null"}),
        Value::Bool(_) => json!({"type": "boolean"}),
        Value::Number(number) if number.is_i64() || number.is_u64() => json!({"type": "integer"}),
        Value::Number(_) => json!({"type": "number"}),
        Value::String(_) => json!({"type": "string"}),
        Value::Array(values) => {
            let items = values.first().map_or_else(|| json!({}), schema_for_value);
            json!({"type": "array", "items": items})
        }
        Value::Object(properties) => object_schema(
            &properties
                .iter()
                .map(|(key, value)| (key.clone(), schema_for_value(value)))
                .collect(),
        ),
    }
}

fn check_in_process_corpus() -> Result<(), String> {
    let repository = root()?;
    run_command(Command::new("cargo").current_dir(repository).args([
        "test",
        "-p",
        "starweaver-rpc-core",
        "--test",
        "rpc_wire_conformance",
        "--locked",
    ]))
}

fn check_wire_runtime(corpus: &Corpus) -> Result<(), String> {
    let repository = root()?;
    run_command(Command::new("cargo").current_dir(&repository).args([
        "build",
        "-p",
        "starweaver-rpc",
        "--bin",
        "starweaver-rpc",
        "--locked",
    ]))?;
    let rpc = target_dir(&repository)
        .join("debug")
        .join(binary_name("starweaver-rpc"));
    if !rpc.is_file() {
        return Err(format!("missing RPC contract binary: {}", rpc.display()));
    }

    let temp = env::temp_dir().join(format!("starweaver-rpc-contracts-{}", std::process::id()));
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
        check_stdio_corpus(
            corpus,
            &rpc,
            &workspace,
            &config,
            &temp.join("stdio.sqlite"),
        )?;
        check_http_corpus(corpus, &rpc, &workspace, &config, &temp.join("http.sqlite"))
    })();
    let _ = fs::remove_dir_all(&temp);
    result?;
    println!("RPC v1 corpus validated through in-process, stdio, and loopback HTTP gates");
    Ok(())
}

fn exercise_corpus(
    corpus: &Corpus,
    transport: &str,
    mut send: impl FnMut(&str, &Value) -> Result<Value, String>,
) -> Result<(), String> {
    let initialize = corpus
        .methods
        .iter()
        .find(|fixture| fixture.name == "initialize")
        .ok_or_else(|| "wire corpus is missing initialize".to_string())?;
    exercise_canonical(initialize, transport, &mut send)?;
    exercise_invalid(initialize, transport, &mut send)?;

    for fixture in corpus
        .methods
        .iter()
        .filter(|fixture| !matches!(fixture.name.as_str(), "initialize" | "shutdown"))
    {
        exercise_canonical(fixture, transport, &mut send)?;
        exercise_invalid(fixture, transport, &mut send)?;
    }

    let shutdown = corpus
        .methods
        .iter()
        .find(|fixture| fixture.name == "shutdown")
        .ok_or_else(|| "wire corpus is missing shutdown".to_string())?;
    exercise_invalid(shutdown, transport, &mut send)?;
    exercise_canonical(shutdown, transport, &mut send)
}

fn exercise_canonical(
    fixture: &Method,
    transport: &str,
    send: &mut impl FnMut(&str, &Value) -> Result<Value, String>,
) -> Result<(), String> {
    let response = send(&fixture.name, &fixture.canonical_params)?;
    validate_transport_envelope(&response, &fixture.name, transport)?;
    if typed_invalid_params(&response, &fixture.name) {
        return Err(format!(
            "{transport} rejected canonical {} params at the typed boundary: {response}",
            fixture.name
        ));
    }
    if typed_invalid_result(&response, &fixture.name) {
        return Err(format!(
            "{transport} emitted a nonconformant {} result: {response}",
            fixture.name
        ));
    }
    Ok(())
}

fn exercise_invalid(
    fixture: &Method,
    transport: &str,
    send: &mut impl FnMut(&str, &Value) -> Result<Value, String>,
) -> Result<(), String> {
    for invalid in &fixture.invalid_params {
        let response = send(&fixture.name, invalid)?;
        validate_transport_envelope(&response, &fixture.name, transport)?;
        if !typed_invalid_params(&response, &fixture.name) {
            return Err(format!(
                "{transport} accepted invalid {} params {invalid}: {response}",
                fixture.name
            ));
        }
    }
    Ok(())
}

fn typed_invalid_params(response: &Value, method: &str) -> bool {
    response.pointer("/error/code").and_then(Value::as_i64) == Some(-32_602)
        && response
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.starts_with(&format!("invalid {method} params:")))
}

fn typed_invalid_result(response: &Value, method: &str) -> bool {
    response.pointer("/error/code").and_then(Value::as_i64) == Some(-32_000)
        && response
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.starts_with(&format!("invalid {method} result:")))
}

fn validate_transport_envelope(
    response: &Value,
    method: &str,
    transport: &str,
) -> Result<(), String> {
    if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || response.get("id").is_none()
        || (response.get("result").is_none() == response.get("error").is_none())
    {
        return Err(format!(
            "{transport} returned an invalid JSON-RPC envelope for {method}: {response}"
        ));
    }
    Ok(())
}

fn check_stdio_corpus(
    corpus: &Corpus,
    rpc: &Path,
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
    let result = exercise_corpus(corpus, "stdio", |method, params| {
        host.request(method, params)
    });
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
    next_id: u64,
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
            next_id: 1,
        })
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").map_err(|error| error.to_string())?;
        self.stdin.flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if line.is_empty() {
            return Err(format!(
                "RPC stdio host exited before responding to {method}"
            ));
        }
        serde_json::from_str(line.trim())
            .map_err(|error| format!("invalid RPC stdio response for {method}: {error}"))
    }
}

fn check_http_corpus(
    corpus: &Corpus,
    rpc: &Path,
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
        match http_rpc_request(port, TOKEN, 0, "initialize", &json!({}), false) {
            Ok(response) if response.get("result").is_some() => break,
            Ok(response) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "RPC HTTP initialize failed during readiness probe: {response}"
                ));
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

    let mut next_id = 1_u64;
    let result = exercise_corpus(corpus, "http", |method, params| {
        let id = next_id;
        next_id += 1;
        http_rpc_request(port, TOKEN, id, method, params, method != "initialize")
    });
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

fn http_rpc_request(
    port: u16,
    token: &str,
    id: u64,
    method: &str,
    params: &Value,
    include_protocol: bool,
) -> Result<Value, String> {
    let mut request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    if include_protocol {
        request["protocol"] = json!({
            "name": "starweaver.host",
            "major": 1,
            "revision": "rpc-contract-gate",
        });
    }
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
        return Err(format!("RPC HTTP request for {method} failed: {response}"));
    }
    serde_json::from_str(body)
        .map_err(|error| format!("invalid RPC HTTP JSON response for {method}: {error}: {body}"))
}

fn target_dir(repository: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(
        || repository.join("target"),
        |path| {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                repository.join(path)
            }
        },
    )
}

fn binary_name(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

fn write_generated(path: &str, contents: &str) -> Result<(), String> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|error| format!("write {path}: {error}"))
}

fn check_generated(path: &str, expected: &str) -> Result<(), String> {
    let actual = fs::read_to_string(path).map_err(|error| {
        format!("read {path}: {error}; run `cargo run -p xtask -- generate-rpc-contracts`")
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{path} is stale; run `cargo run -p xtask -- generate-rpc-contracts`"
        ))
    }
}
