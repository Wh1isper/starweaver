//! `EnvD` JSON-RPC client.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use starweaver_envd_core::{
    CleanupIdleRequest, CommandRunRequest, CommandRunResult, EnvdError, EnvdErrorCode, EnvdResult,
    EnvdService, EnvironmentContextRequest, EnvironmentContextResult, EnvironmentDescriptor,
    EnvironmentRequest, EnvironmentStateSnapshot, FileCopyRequest, FileCreateDirRequest,
    FileDeleteRequest, FileGlobMatch, FileGlobRequest, FileGrepMatch, FileGrepRequest,
    FileListRequest, FileListResult, FileMoveRequest, FileReadRequest, FileReadResult, FileStat,
    FileStatRequest, FileWriteRequest, FileWriteResult, FileWriteTmpRequest, FileWriteTmpResult,
    InitializeEnvdRequest, InitializeEnvdResult, MutationResult, OpenEnvironmentRequest,
    ProcessInputRequest, ProcessKillRequest, ProcessListResult, ProcessSignalRequest,
    ProcessSnapshot, ProcessStartRequest, ProcessWaitRequest, ShellReviewContextRequest,
    ShellReviewContextResult, envd_protocol_identity, validate_envd_protocol,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

/// Client construction error.
#[derive(Debug, Error)]
pub enum EnvdClientError {
    /// I/O error while creating or using the transport.
    #[error("{0}")]
    Io(String),
    /// The requested transport endpoint is malformed.
    #[error("{0}")]
    InvalidEndpoint(String),
}

/// `EnvD` JSON-RPC client implementing the shared service trait.
#[derive(Clone)]
pub struct EnvdRpcClient {
    inner: Arc<EnvdRpcClientInner>,
}

struct EnvdRpcClientInner {
    transport: EnvdRpcTransport,
    next_id: AtomicU64,
}

enum EnvdRpcTransport {
    Stdio(Box<Mutex<StdioClientState>>),
    Http(HttpEndpoint),
}

struct StdioClientState {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Clone)]
struct HttpEndpoint {
    authority: String,
    path: String,
    auth_token: Option<String>,
}

impl EnvdRpcClient {
    /// Spawn a persistent stdio envd child process.
    ///
    /// # Errors
    ///
    /// Returns an error when the process cannot be spawned or stdio pipes are unavailable.
    pub fn spawn_stdio(
        program: impl AsRef<Path>,
        args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
    ) -> Result<Self, EnvdClientError> {
        let mut command = Command::new(program.as_ref());
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| EnvdClientError::Io(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EnvdClientError::Io("envd child stdin is unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EnvdClientError::Io("envd child stdout is unavailable".to_string()))?;
        Ok(Self {
            inner: Arc::new(EnvdRpcClientInner {
                transport: EnvdRpcTransport::Stdio(Box::new(Mutex::new(StdioClientState {
                    child,
                    stdin,
                    stdout: BufReader::new(stdout),
                }))),
                next_id: AtomicU64::new(1),
            }),
        })
    }

    /// Connect to an envd HTTP endpoint such as `http://127.0.0.1:8766/rpc`.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint is not a supported HTTP URL.
    pub fn http(endpoint: impl AsRef<str>) -> Result<Self, EnvdClientError> {
        Self::http_with_optional_token(endpoint, None)
    }

    /// Connect to a local envd endpoint reference used by host products.
    ///
    /// HTTP endpoint refs must target loopback and carry a non-empty bearer token. Stdio endpoint
    /// refs use `stdio://<percent-encoded-program>?arg=<percent-encoded-argument>`.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint ref violates the local transport security profile or
    /// when a stdio child cannot be spawned.
    pub fn from_local_endpoint_ref(
        endpoint_ref: &str,
        auth_token: Option<&str>,
    ) -> Result<Self, EnvdClientError> {
        validate_local_endpoint_ref(endpoint_ref, auth_token)?;
        if endpoint_ref.starts_with("http://") {
            let auth_token = auth_token.ok_or_else(|| {
                EnvdClientError::InvalidEndpoint(
                    "envd HTTP attachment requires authToken".to_string(),
                )
            })?;
            return Self::http_with_token(endpoint_ref, auth_token);
        }
        let (program, args) = parse_stdio_endpoint_ref(endpoint_ref)?;
        Self::spawn_stdio(program, args)
    }

    /// Connect to an authenticated envd HTTP endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint or bearer token is not supported.
    pub fn http_with_token(
        endpoint: impl AsRef<str>,
        auth_token: impl Into<String>,
    ) -> Result<Self, EnvdClientError> {
        Self::http_with_optional_token(endpoint, Some(auth_token.into()))
    }

    fn http_with_optional_token(
        endpoint: impl AsRef<str>,
        auth_token: Option<String>,
    ) -> Result<Self, EnvdClientError> {
        let mut endpoint = parse_http_endpoint(endpoint.as_ref())?;
        endpoint.auth_token = auth_token.map(validate_http_auth_token).transpose()?;
        Ok(Self {
            inner: Arc::new(EnvdRpcClientInner {
                transport: EnvdRpcTransport::Http(endpoint),
                next_id: AtomicU64::new(1),
            }),
        })
    }

    /// Request graceful shutdown from the remote envd service.
    ///
    /// An owned stdio child is also awaited and reaped before this method returns.
    ///
    /// # Errors
    ///
    /// Returns an envd error when the transport fails, the remote service rejects shutdown, or an
    /// owned stdio child exits unsuccessfully.
    pub async fn shutdown(&self) -> EnvdResult<Value> {
        let result = self.request("shutdown", &json!({})).await?;
        if let EnvdRpcTransport::Stdio(state) = &self.inner.transport {
            let status = state
                .lock()
                .await
                .child
                .wait()
                .await
                .map_err(envd_provider_error)?;
            if !status.success() {
                return Err(EnvdError::provider(format!(
                    "envd stdio child exited with {status}"
                )));
            }
        }
        Ok(result)
    }

    async fn request<Request, Response>(
        &self,
        method: &str,
        params: &Request,
    ) -> EnvdResult<Response>
    where
        Request: Serialize + Sync,
        Response: DeserializeOwned,
    {
        let params = serde_json::to_value(params).map_err(envd_provider_error)?;
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let response = match &self.inner.transport {
            EnvdRpcTransport::Stdio(state) => request_stdio(state, &frame).await,
            EnvdRpcTransport::Http(endpoint) => request_http(endpoint, &frame).await,
        }?;
        decode_response(&response)
    }
}

#[async_trait]
impl EnvdService for EnvdRpcClient {
    async fn initialize(
        &self,
        mut request: InitializeEnvdRequest,
    ) -> EnvdResult<InitializeEnvdResult> {
        request.protocol.get_or_insert_with(envd_protocol_identity);
        let result: InitializeEnvdResult = self.request("initialize", &request).await?;
        validate_envd_protocol(&result.protocol)?;
        Ok(result)
    }

    async fn open_environment(
        &self,
        request: OpenEnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor> {
        self.request("environment.open", &request).await
    }

    async fn environment_state(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentStateSnapshot> {
        self.request("environment.state", &request).await
    }

    async fn prepare_environment(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor> {
        self.request("environment.prepare", &request).await
    }

    async fn stop_environment(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor> {
        self.request("environment.stop", &request).await
    }

    async fn cleanup_idle(&self, request: CleanupIdleRequest) -> EnvdResult<EnvironmentDescriptor> {
        self.request("environment.cleanup_idle", &request).await
    }

    async fn file_read(&self, request: FileReadRequest) -> EnvdResult<FileReadResult> {
        self.request("file.read", &request).await
    }

    async fn file_write(&self, request: FileWriteRequest) -> EnvdResult<FileWriteResult> {
        self.request("file.write", &request).await
    }

    async fn file_create_dir(&self, request: FileCreateDirRequest) -> EnvdResult<MutationResult> {
        self.request("file.create_dir", &request).await
    }

    async fn file_delete(&self, request: FileDeleteRequest) -> EnvdResult<MutationResult> {
        self.request("file.delete", &request).await
    }

    async fn file_move(&self, request: FileMoveRequest) -> EnvdResult<MutationResult> {
        self.request("file.move", &request).await
    }

    async fn file_copy(&self, request: FileCopyRequest) -> EnvdResult<MutationResult> {
        self.request("file.copy", &request).await
    }

    async fn file_write_tmp(&self, request: FileWriteTmpRequest) -> EnvdResult<FileWriteTmpResult> {
        self.request("file.write_tmp", &request).await
    }

    async fn file_stat(&self, request: FileStatRequest) -> EnvdResult<FileStat> {
        self.request("file.stat", &request).await
    }

    async fn file_list(&self, request: FileListRequest) -> EnvdResult<FileListResult> {
        self.request("file.list", &request).await
    }

    async fn file_glob(&self, request: FileGlobRequest) -> EnvdResult<Vec<FileGlobMatch>> {
        self.request("file.glob", &request).await
    }

    async fn file_grep(&self, request: FileGrepRequest) -> EnvdResult<Vec<FileGrepMatch>> {
        self.request("file.grep", &request).await
    }

    async fn command_run(&self, request: CommandRunRequest) -> EnvdResult<CommandRunResult> {
        self.request("command.run", &request).await
    }

    async fn process_start(&self, request: ProcessStartRequest) -> EnvdResult<ProcessSnapshot> {
        self.request("process.start", &request).await
    }

    async fn process_wait(&self, request: ProcessWaitRequest) -> EnvdResult<ProcessSnapshot> {
        self.request("process.wait", &request).await
    }

    async fn process_list(&self, request: EnvironmentRequest) -> EnvdResult<ProcessListResult> {
        self.request("process.list", &request).await
    }

    async fn process_input(&self, request: ProcessInputRequest) -> EnvdResult<ProcessSnapshot> {
        self.request("process.input", &request).await
    }

    async fn process_signal(&self, request: ProcessSignalRequest) -> EnvdResult<ProcessSnapshot> {
        self.request("process.signal", &request).await
    }

    async fn process_kill(&self, request: ProcessKillRequest) -> EnvdResult<ProcessSnapshot> {
        self.request("process.kill", &request).await
    }

    async fn render_environment_context(
        &self,
        request: EnvironmentContextRequest,
    ) -> EnvdResult<EnvironmentContextResult> {
        self.request("context.render", &request).await
    }

    async fn shell_review_context(
        &self,
        request: ShellReviewContextRequest,
    ) -> EnvdResult<ShellReviewContextResult> {
        self.request("shell.review_context", &request).await
    }

    async fn export_snapshot(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentStateSnapshot> {
        self.request("snapshot.export", &request).await
    }
}

impl Drop for StdioClientState {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.start_kill();
        }
    }
}

async fn request_stdio(state: &Mutex<StdioClientState>, frame: &Value) -> EnvdResult<Value> {
    let mut state = state.lock().await;
    state
        .stdin
        .write_all(frame.to_string().as_bytes())
        .await
        .map_err(envd_provider_error)?;
    state
        .stdin
        .write_all(b"\n")
        .await
        .map_err(envd_provider_error)?;
    state.stdin.flush().await.map_err(envd_provider_error)?;
    let mut line = String::new();
    state
        .stdout
        .read_line(&mut line)
        .await
        .map_err(envd_provider_error)?;
    drop(state);
    if line.trim().is_empty() {
        return Err(EnvdError::provider("envd stdio closed without a response"));
    }
    serde_json::from_str(line.trim()).map_err(envd_provider_error)
}

async fn request_http(endpoint: &HttpEndpoint, frame: &Value) -> EnvdResult<Value> {
    let body = frame.to_string();
    let mut stream = TcpStream::connect(&endpoint.authority)
        .await
        .map_err(envd_provider_error)?;
    let auth_header = endpoint
        .auth_token
        .as_deref()
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        endpoint.path,
        endpoint.authority,
        auth_header,
        body.len(),
        body
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(envd_provider_error)?;
    stream.flush().await.map_err(envd_provider_error)?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .await
        .map_err(envd_provider_error)?;
    let text = String::from_utf8(bytes).map_err(envd_provider_error)?;
    let (header, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| EnvdError::provider("invalid HTTP response from envd"))?;
    let status_line = header
        .lines()
        .next()
        .ok_or_else(|| EnvdError::provider("invalid HTTP response from envd"))?;
    if !status_line.contains(" 200 ") {
        let status = status_line
            .strip_prefix("HTTP/1.1 ")
            .unwrap_or(status_line)
            .to_string();
        return Err(EnvdError::provider(format!(
            "envd HTTP request failed: {status}"
        )));
    }
    serde_json::from_str(body).map_err(envd_provider_error)
}

fn decode_response<Response>(response: &Value) -> EnvdResult<Response>
where
    Response: DeserializeOwned,
{
    if let Some(error) = response.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32_000);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("envd rpc error")
            .to_string();
        return Err(rpc_error_to_envd(code, message));
    }
    let result = response
        .get("result")
        .cloned()
        .ok_or_else(|| EnvdError::provider("missing JSON-RPC result"))?;
    serde_json::from_value(result).map_err(envd_provider_error)
}

fn rpc_error_to_envd(code: i64, message: String) -> EnvdError {
    let kind = match code {
        -32_602 => EnvdErrorCode::InvalidRequest,
        -32_010 => EnvdErrorCode::NotFound,
        -32_011 => EnvdErrorCode::AccessDenied,
        -32_002 => EnvdErrorCode::Unsupported,
        _ => EnvdErrorCode::Provider,
    };
    EnvdError::new(kind, message)
}

/// Validate an envd endpoint ref against the local host transport security profile.
///
/// # Errors
///
/// Returns an error for unsupported schemes, non-loopback HTTP endpoints, unsafe HTTP URL
/// components, missing or malformed HTTP auth tokens, and malformed stdio endpoint refs.
pub fn validate_local_endpoint_ref(
    endpoint_ref: &str,
    auth_token: Option<&str>,
) -> Result<(), EnvdClientError> {
    if endpoint_ref.starts_with("http://") {
        validate_local_http_endpoint(endpoint_ref)?;
        let auth_token = auth_token.ok_or_else(|| {
            EnvdClientError::InvalidEndpoint("envd HTTP attachment requires authToken".to_string())
        })?;
        validate_http_auth_token(auth_token.to_string())?;
        return Ok(());
    }
    if endpoint_ref.starts_with("stdio://") {
        parse_stdio_endpoint_ref(endpoint_ref)?;
        return Ok(());
    }
    Err(EnvdClientError::InvalidEndpoint(
        "envd attachment supports http:// and stdio:// endpoint refs".to_string(),
    ))
}

/// Return a safe endpoint ref suitable for diagnostics and lease projections.
#[must_use]
pub fn redacted_endpoint_ref(endpoint_ref: &str) -> Option<String> {
    if endpoint_ref.starts_with("stdio://") {
        return Some("stdio://<redacted>".to_string());
    }
    endpoint_ref
        .starts_with("http://")
        .then(|| endpoint_ref.to_string())
}

fn validate_local_http_endpoint(endpoint: &str) -> Result<(), EnvdClientError> {
    let rest = endpoint.strip_prefix("http://").ok_or_else(|| {
        EnvdClientError::InvalidEndpoint("envd HTTP endpoint must start with http://".to_string())
    })?;
    if rest.is_empty() {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP endpoint host cannot be empty".to_string(),
        ));
    }
    if endpoint.contains('?') || endpoint.contains('#') {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP endpoint cannot contain query strings or fragments".to_string(),
        ));
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.contains('@') {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP endpoint cannot contain userinfo".to_string(),
        ));
    }
    let host = http_authority_host(authority)?;
    if is_loopback_http_host(host) {
        Ok(())
    } else {
        Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP endpoint must be loopback unless configured by a future host policy"
                .to_string(),
        ))
    }
}

fn http_authority_host(authority: &str) -> Result<&str, EnvdClientError> {
    if authority.is_empty() {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP endpoint host cannot be empty".to_string(),
        ));
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, _)) = rest.split_once(']') else {
            return Err(EnvdClientError::InvalidEndpoint(
                "envd HTTP endpoint has invalid IPv6 host".to_string(),
            ));
        };
        return Ok(host);
    }
    Ok(authority
        .split_once(':')
        .map_or(authority, |(host, _)| host))
}

fn is_loopback_http_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

fn parse_stdio_endpoint_ref(
    endpoint_ref: &str,
) -> Result<(PathBuf, Vec<OsString>), EnvdClientError> {
    let rest = endpoint_ref.strip_prefix("stdio://").ok_or_else(|| {
        EnvdClientError::InvalidEndpoint("envd stdio endpoint must start with stdio://".to_string())
    })?;
    let (program, query) = rest.split_once('?').unwrap_or((rest, ""));
    if program.trim().is_empty() {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd stdio endpoint program cannot be empty".to_string(),
        ));
    }
    let mut args = Vec::new();
    if !query.is_empty() {
        for part in query.split('&').filter(|part| !part.is_empty()) {
            let Some(value) = part.strip_prefix("arg=") else {
                return Err(EnvdClientError::InvalidEndpoint(
                    "envd stdio endpoint query supports only repeated arg= values".to_string(),
                ));
            };
            args.push(OsString::from(percent_decode_component(value)?));
        }
    }
    Ok((PathBuf::from(percent_decode_component(program)?), args))
}

fn percent_decode_component(value: &str) -> Result<String, EnvdClientError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1])?;
                let low = hex_value(bytes[index + 2])?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            b'%' => {
                return Err(EnvdClientError::InvalidEndpoint(
                    "envd stdio endpoint has incomplete percent escape".to_string(),
                ));
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|error| EnvdClientError::InvalidEndpoint(error.to_string()))
}

fn hex_value(byte: u8) -> Result<u8, EnvdClientError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(EnvdClientError::InvalidEndpoint(
            "envd stdio endpoint has invalid percent escape".to_string(),
        )),
    }
}

fn parse_http_endpoint(endpoint: &str) -> Result<HttpEndpoint, EnvdClientError> {
    let rest = endpoint.strip_prefix("http://").ok_or_else(|| {
        EnvdClientError::InvalidEndpoint("envd HTTP endpoint must start with http://".to_string())
    })?;
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, "/rpc"), |(authority, path)| {
            (authority, if path.is_empty() { "/rpc" } else { path })
        });
    if authority.is_empty() {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP endpoint host is empty".to_string(),
        ));
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Ok(HttpEndpoint {
        authority: authority.to_string(),
        path,
        auth_token: None,
    })
}

fn validate_http_auth_token(token: String) -> Result<String, EnvdClientError> {
    if token.trim().is_empty() {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP auth token cannot be empty".to_string(),
        ));
    }
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(EnvdClientError::InvalidEndpoint(
            "envd HTTP auth token cannot contain newlines".to_string(),
        ));
    }
    Ok(token)
}

fn envd_provider_error(error: impl std::error::Error) -> EnvdError {
    EnvdError::provider(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{redacted_endpoint_ref, validate_local_endpoint_ref};

    #[test]
    fn local_endpoint_profile_accepts_loopback_http_and_stdio() {
        assert!(validate_local_endpoint_ref("http://127.0.0.1:8766/rpc", Some("secret")).is_ok());
        assert!(
            validate_local_endpoint_ref(
                "stdio://%2Fusr%2Fbin%2Fenvd?arg=--stdio&arg=value+with+spaces",
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn local_endpoint_profile_rejects_unsafe_http_components() {
        for endpoint in [
            "http://example.com:8766/rpc",
            "http://user:pass@127.0.0.1:8766/rpc",
            "http://127.0.0.1:8766/rpc?token=secret",
            "http://127.0.0.1:8766/rpc#fragment",
        ] {
            assert!(validate_local_endpoint_ref(endpoint, Some("secret")).is_err());
        }
        assert!(validate_local_endpoint_ref("http://127.0.0.1:8766/rpc", None).is_err());
        assert!(
            validate_local_endpoint_ref("http://127.0.0.1:8766/rpc", Some("line\nbreak")).is_err()
        );
    }

    #[test]
    fn endpoint_ref_redaction_hides_stdio_command_and_arguments() {
        assert_eq!(
            redacted_endpoint_ref("stdio:///tmp/envd?arg=--token&arg=secret").as_deref(),
            Some("stdio://<redacted>")
        );
        assert_eq!(
            redacted_endpoint_ref("http://127.0.0.1:8766/rpc").as_deref(),
            Some("http://127.0.0.1:8766/rpc")
        );
        assert_eq!(redacted_endpoint_ref("https://example.com"), None);
    }
}
