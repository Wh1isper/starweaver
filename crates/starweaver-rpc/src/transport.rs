use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead as _, Read as _, Write as _},
    net::{IpAddr, TcpListener, TcpStream},
    str::FromStr as _,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    RpcConfig, RpcHostError, RpcHostResult, RpcService,
    auth::{RpcHttpCredential, RpcHttpScope, required_scope},
};

const DEFAULT_HTTP_PATH: &str = "/rpc";
const MAX_HTTP_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_CONNECTIONS: usize = 64;
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const STDIO_OUTBOUND_CAPACITY: usize = 256;
const STDIO_RESPONSE_DEADLINE: Duration = Duration::from_secs(5);
const STDIO_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(12);
const STDIO_QUEUE_RETRY: Duration = Duration::from_millis(5);

struct StdioOutput {
    value: Value,
    flushed: Option<mpsc::SyncSender<()>>,
}

struct StdioThread {
    handle: thread::JoinHandle<()>,
    completed: mpsc::Receiver<()>,
}

fn spawn_stdio_thread(task: impl FnOnce() + Send + 'static) -> StdioThread {
    let (completed_sender, completed) = mpsc::sync_channel(1);
    let handle = thread::spawn(move || {
        task();
        let _ = completed_sender.send(());
    });
    StdioThread { handle, completed }
}

fn join_stdio_thread(thread: StdioThread, deadline: Instant, label: &str) -> RpcHostResult<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match thread.completed.recv_timeout(remaining) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => thread
            .handle
            .join()
            .map_err(|_| RpcHostError::Runtime(format!("stdio {label} panicked"))),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Rust cannot safely kill a thread blocked in an arbitrary `Write` implementation.
            // Detaching the handle bounds host shutdown; standalone process exit terminates it.
            drop(thread.handle);
            Err(RpcHostError::Runtime(format!(
                "stdio {label} did not stop before the shutdown deadline; detached blocked thread"
            )))
        }
    }
}

fn send_stdio_output(
    sender: &mpsc::SyncSender<StdioOutput>,
    mut frame: StdioOutput,
    deadline: Instant,
) -> RpcHostResult<()> {
    loop {
        match sender.try_send(frame) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(RpcHostError::Runtime(
                    "stdio response writer disconnected".to_string(),
                ));
            }
            Err(mpsc::TrySendError::Full(returned)) => {
                if Instant::now() >= deadline {
                    return Err(RpcHostError::Runtime(
                        "stdio outbound queue remained full until the response deadline"
                            .to_string(),
                    ));
                }
                frame = returned;
                thread::sleep(
                    STDIO_QUEUE_RETRY.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
}

/// Run the JSON-RPC stdio server until stdin closes or `shutdown` is requested.
pub fn run_stdio(config: &RpcConfig) -> RpcHostResult<()> {
    let (outbound_sender, outbound_receiver) =
        mpsc::sync_channel::<StdioOutput>(STDIO_OUTBOUND_CAPACITY);
    let writer = spawn_stdio_thread(move || {
        let mut stdout = io::stdout();
        while let Ok(frame) = outbound_receiver.recv() {
            if serde_json::to_writer(&mut stdout, &frame.value).is_err() {
                break;
            }
            if stdout.write_all(b"\n").is_err() || stdout.flush().is_err() {
                break;
            }
            if let Some(flushed) = frame.flushed {
                let _ = flushed.send(());
            }
        }
    });
    let (notification_sender, mut notification_receiver) =
        tokio::sync::mpsc::channel::<Value>(STDIO_OUTBOUND_CAPACITY);
    let notification_output = outbound_sender.clone();
    let notification_forwarder = spawn_stdio_thread(move || {
        while let Some(value) = notification_receiver.blocking_recv() {
            if send_stdio_output(
                &notification_output,
                StdioOutput {
                    value,
                    flushed: None,
                },
                Instant::now() + STDIO_RESPONSE_DEADLINE,
            )
            .is_err()
            {
                break;
            }
        }
    });
    let service = RpcService::live(config.clone())?;
    let connection = service.live_connection(notification_sender.clone());
    let stdin = io::stdin();
    let serve_result = (|| -> RpcHostResult<()> {
        for line in stdin.lock().lines() {
            let line = line.map_err(RpcHostError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            let outcome = connection.handle_text(&line);
            if let Some(response) = outcome.response {
                let (flushed_sender, flushed_receiver) = mpsc::sync_channel(0);
                let response_deadline = Instant::now() + STDIO_RESPONSE_DEADLINE;
                send_stdio_output(
                    &outbound_sender,
                    StdioOutput {
                        value: response,
                        flushed: Some(flushed_sender),
                    },
                    response_deadline,
                )?;
                flushed_receiver
                    .recv_timeout(response_deadline.saturating_duration_since(Instant::now()))
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => RpcHostError::Runtime(
                            "stdio response was not flushed before its deadline".to_string(),
                        ),
                        mpsc::RecvTimeoutError::Disconnected => RpcHostError::Runtime(
                            "stdio response writer disconnected before flush".to_string(),
                        ),
                    })?;
                connection.activate_pending_subscriptions();
            }
            if outcome.shutdown {
                break;
            }
        }
        Ok(())
    })();
    drop(connection);
    drop(notification_sender);
    let shutdown_deadline = Instant::now() + STDIO_SHUTDOWN_DEADLINE;
    let forwarder_result = join_stdio_thread(
        notification_forwarder,
        shutdown_deadline,
        "notification forwarder",
    );
    let shutdown_result = service.shutdown_owned_runtime(
        shutdown_deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(10)),
    );
    drop(service);
    drop(outbound_sender);
    let writer_result = join_stdio_thread(writer, shutdown_deadline, "response writer");

    let mut result = serve_result;
    for cleanup in [forwarder_result, shutdown_result, writer_result] {
        if result.is_ok() {
            result = cleanup;
        }
    }
    result
}

/// Run the JSON-RPC HTTP server until `shutdown` is requested or the listener fails.
pub fn run_http(config: &RpcConfig, host: &str, port: u16) -> RpcHostResult<()> {
    validate_http_host(host)?;
    let credential = config.http_auth.load_credential(&config.state_dir)?;
    let listener = TcpListener::bind((host, port)).map_err(|error| {
        RpcHostError::Runtime(format!(
            "failed to bind RPC HTTP listener at {host}:{port}: {error}"
        ))
    })?;
    let local_address = listener
        .local_addr()
        .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
    if !local_address.ip().is_loopback() {
        return Err(RpcHostError::Runtime(format!(
            "RPC HTTP may bind only to a loopback address without TLS or an authenticated reverse proxy, resolved {host} to {}",
            local_address.ip()
        )));
    }
    let security = Arc::new(HttpSecurity::new(
        config,
        host,
        local_address.port(),
        credential,
    ));
    eprintln!("starweaver rpc http listening on http://{local_address}{DEFAULT_HTTP_PATH}");
    let service = Arc::new(RpcService::replay_only(config.clone())?);
    let served = serve_http(&listener, &service, &security);
    let shutdown = service.shutdown_owned_runtime(Duration::from_secs(10));
    served.and(shutdown)
}

pub fn validate_http_host(host: &str) -> RpcHostResult<()> {
    let host = host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    let ip_literal = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if IpAddr::from_str(ip_literal).is_ok_and(|address| address.is_loopback()) {
        return Ok(());
    }
    Err(RpcHostError::Runtime(format!(
        "RPC HTTP may bind only to a loopback host without TLS or an authenticated reverse proxy; rejected '{host}'"
    )))
}

fn configure_http_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(HTTP_REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT))
}

struct ConnectionPermit {
    active_connections: Arc<AtomicUsize>,
}

impl ConnectionPermit {
    fn try_acquire(active_connections: &Arc<AtomicUsize>) -> Option<Self> {
        active_connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_HTTP_CONNECTIONS).then_some(active + 1)
            })
            .ok()?;
        Some(Self {
            active_connections: Arc::clone(active_connections),
        })
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve_http(
    listener: &TcpListener,
    service: &Arc<RpcService>,
    security: &Arc<HttpSecurity>,
) -> RpcHostResult<()> {
    listener
        .set_nonblocking(true)
        .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let active_connections = Arc::new(AtomicUsize::new(0));
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _address)) => {
                configure_http_stream(&stream).map_err(RpcHostError::Io)?;
                let Some(permit) = ConnectionPermit::try_acquire(&active_connections) else {
                    let _ = write_http_text(
                        &mut stream,
                        "503 Service Unavailable",
                        "too many concurrent connections",
                    );
                    continue;
                };
                let service = Arc::clone(service);
                let security = Arc::clone(security);
                let shutdown = Arc::clone(&shutdown);
                thread::spawn(move || {
                    let _permit = permit;
                    if let Err(error) =
                        handle_http_connection(stream, &service, &security, &shutdown)
                    {
                        eprintln!("rpc http connection error: {error}");
                    }
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(RpcHostError::Io(error)),
        }
    }
    Ok(())
}

fn handle_http_connection(
    mut stream: TcpStream,
    service: &RpcService,
    security: &HttpSecurity,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    configure_http_stream(&stream)?;
    let request = match read_http_request(&mut stream)? {
        Ok(request) => request,
        Err(response) => return write_http_response(&mut stream, &response),
    };
    if let Err(response) = security.validate_host_and_origin(&request) {
        return write_http_response(&mut stream, &response);
    }
    let supplied_token = match bearer_token(&request) {
        Ok(token) => token,
        Err(response) => return write_http_response(&mut stream, &response),
    };
    if !security.credential.authenticates(supplied_token) {
        return write_http_response(&mut stream, &unauthorized_response());
    }
    if request.method == "GET" && matches!(request.path.as_str(), "/health" | "/healthz") {
        if !security
            .credential
            .authorizes(supplied_token, RpcHttpScope::Read)
        {
            return write_http_text(&mut stream, "403 Forbidden", "insufficient RPC scope");
        }
        return write_http_json(
            &mut stream,
            "200 OK",
            &json!({"status": "ok", "protocol": "json-rpc"}),
        );
    }
    if request.method != "POST" {
        return write_http_text(&mut stream, "405 Method Not Allowed", "method not allowed");
    }
    if !matches!(request.path.as_str(), "/" | DEFAULT_HTTP_PATH) {
        return write_http_text(&mut stream, "404 Not Found", "not found");
    }
    if !request
        .header("content-type")
        .is_some_and(valid_json_content_type)
    {
        return write_http_text(
            &mut stream,
            "415 Unsupported Media Type",
            "Content-Type must be application/json",
        );
    }
    let Some(method) = request_method(&request.body) else {
        return write_http_text(
            &mut stream,
            "400 Bad Request",
            "JSON-RPC method is required",
        );
    };
    // Unknown methods retain JSON-RPC's method-not-found response for an
    // administrative caller, but never inherit read authority. A newly added
    // handler that is missing from the scope registry is therefore fail-closed
    // for read/run/approval credentials.
    let scope = required_scope(&method).unwrap_or(RpcHttpScope::Admin);
    if !security.credential.authorizes(supplied_token, scope) {
        return write_http_text(&mut stream, "403 Forbidden", "insufficient RPC scope");
    }
    let outcome = service.handle_text(&request.body);
    if outcome.shutdown {
        shutdown.store(true, Ordering::SeqCst);
    }
    if let Some(response) = outcome.response {
        write_http_json(&mut stream, "200 OK", &response)
    } else {
        let response = HttpResponse {
            status: "204 No Content",
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: Vec::new(),
        };
        write_http_response(&mut stream, &response)
    }
}

struct HttpSecurity {
    credential: RpcHttpCredential,
    allowed_origins: BTreeSet<String>,
    allowed_hosts: BTreeSet<String>,
}

impl HttpSecurity {
    fn new(config: &RpcConfig, bind_host: &str, port: u16, credential: RpcHttpCredential) -> Self {
        let mut allowed_hosts = config
            .http_auth
            .allowed_hosts
            .iter()
            .map(|host| host.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let bind_host = bind_host.trim().trim_matches(['[', ']']);
        for host in [bind_host, "localhost", "127.0.0.1", "[::1]"] {
            allowed_hosts.insert(format_host_header(host, port));
        }
        Self {
            credential,
            allowed_origins: config.http_auth.allowed_origins.clone(),
            allowed_hosts,
        }
    }

    fn validate_host_and_origin(&self, request: &HttpRequest) -> Result<(), HttpResponse> {
        let Some(host) = request.header("host") else {
            return Err(http_text_response(
                "400 Bad Request",
                "Host header is required",
            ));
        };
        if !self
            .allowed_hosts
            .contains(&host.trim().to_ascii_lowercase())
        {
            return Err(http_text_response(
                "421 Misdirected Request",
                "Host is not allowed",
            ));
        }
        if let Some(origin) = request.header("origin")
            && !self.allowed_origins.contains(origin.trim())
        {
            return Err(http_text_response("403 Forbidden", "Origin is not allowed"));
        }
        Ok(())
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

fn read_with_deadline(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<usize> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "HTTP request timed out"))?;
    stream.set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;
    stream.read(buffer)
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<Result<HttpRequest, HttpResponse>> {
    let deadline = Instant::now() + HTTP_REQUEST_TIMEOUT;
    let mut buffer = Vec::new();
    let header_end = loop {
        if buffer.len() > MAX_HTTP_HEADER_BYTES {
            return Ok(Err(http_text_response(
                "413 Payload Too Large",
                "request too large",
            )));
        }
        if let Some(header_end) = http_header_end(&buffer) {
            break header_end;
        }
        let mut chunk = [0_u8; 4096];
        let read = read_with_deadline(stream, &mut chunk, deadline)?;
        if read == 0 {
            return Ok(Err(http_text_response(
                "400 Bad Request",
                "incomplete http request",
            )));
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let Ok(header) = std::str::from_utf8(&buffer[..header_end]) else {
        return Ok(Err(http_text_response(
            "400 Bad Request",
            "invalid http headers",
        )));
    };
    let Some((method, path, headers, content_length)) = parse_http_header(header) else {
        return Ok(Err(http_text_response(
            "400 Bad Request",
            "invalid http request",
        )));
    };
    let Some(request_end) = header_end.checked_add(content_length) else {
        return Ok(Err(http_text_response(
            "413 Payload Too Large",
            "request too large",
        )));
    };
    if request_end > MAX_HTTP_REQUEST_BYTES {
        return Ok(Err(http_text_response(
            "413 Payload Too Large",
            "request too large",
        )));
    }
    while buffer.len() < request_end {
        let mut chunk = [0_u8; 4096];
        let read = read_with_deadline(stream, &mut chunk, deadline)?;
        if read == 0 {
            return Ok(Err(http_text_response(
                "400 Bad Request",
                "incomplete http body",
            )));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body_bytes = &buffer[header_end..request_end];
    let body = match std::str::from_utf8(body_bytes) {
        Ok(body) => body.to_string(),
        Err(_) => {
            return Ok(Err(http_text_response(
                "400 Bad Request",
                "request body must be utf-8",
            )));
        }
    };
    Ok(Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    }))
}

fn http_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn parse_http_header(header: &str) -> Option<(String, String, BTreeMap<String, String>, usize)> {
    let mut lines = header.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let version = parts.next()?;
    if parts.next().is_some()
        || version != "HTTP/1.1"
        || !path.starts_with('/')
        || path.contains('#')
    {
        return None;
    }
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || headers.insert(name, value.trim().to_string()).is_some()
        {
            return None;
        }
    }
    let content_length = headers
        .get("content-length")
        .map_or(Some(0), |value| value.parse().ok())?;
    if headers.contains_key("transfer-encoding") {
        return None;
    }
    Some((method, path, headers, content_length))
}

fn http_text_response(status: &'static str, body: &'static str) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/plain; charset=utf-8",
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn unauthorized_response() -> HttpResponse {
    HttpResponse {
        status: "401 Unauthorized",
        content_type: "text/plain; charset=utf-8",
        headers: vec![("WWW-Authenticate", "Bearer realm=\"starweaver-rpc\"")],
        body: b"valid bearer token required".to_vec(),
    }
}

fn bearer_token(request: &HttpRequest) -> Result<&str, HttpResponse> {
    let Some(value) = request.header("authorization") else {
        return Err(unauthorized_response());
    };
    let Some((scheme, token)) = value.split_once(' ') else {
        return Err(unauthorized_response());
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(unauthorized_response());
    }
    Ok(token)
}

fn request_method(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("method")?
        .as_str()
        .map(ToString::to_string)
}

fn valid_json_content_type(value: &str) -> bool {
    let mut parts = value.split(';').map(str::trim);
    if !parts
        .next()
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case("application/json"))
    {
        return false;
    }
    parts.all(|parameter| parameter.eq_ignore_ascii_case("charset=utf-8"))
}

fn format_host_header(host: &str, port: u16) -> String {
    let host = host.trim().to_ascii_lowercase();
    if host.starts_with('[') || !host.contains(':') {
        format!("{host}:{port}")
    } else {
        format!("[{host}]:{port}")
    }
}

fn write_http_text(
    stream: &mut TcpStream,
    status: &'static str,
    body: &'static str,
) -> io::Result<()> {
    let response = http_text_response(status, body);
    write_http_response(stream, &response)
}

fn write_http_json(stream: &mut TcpStream, status: &'static str, body: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(body).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to encode JSON-RPC response: {error}"),
        )
    })?;
    let response = HttpResponse {
        status,
        content_type: "application/json",
        headers: Vec::new(),
        body,
    };
    write_http_response(stream, &response)
}

fn write_http_response(stream: &mut TcpStream, response: &HttpResponse) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )?;
    for (name, value) in &response.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(&response.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn http_accepts_only_loopback_hosts_without_tls_proxy() {
        for host in ["127.0.0.1", "127.10.20.30", "::1", "[::1]", "localhost"] {
            validate_http_host(host).unwrap();
        }
        for host in ["0.0.0.0", "::", "[::]", "192.168.1.20", "example.test", ""] {
            let error = validate_http_host(host).unwrap_err();
            assert!(error.to_string().contains("loopback"), "{error}");
        }
    }

    #[test]
    fn stdio_queue_and_thread_waits_obey_deadlines() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        sender
            .try_send(StdioOutput {
                value: json!({"first": true}),
                flushed: None,
            })
            .unwrap();
        let started = Instant::now();
        let error = send_stdio_output(
            &sender,
            StdioOutput {
                value: json!({"second": true}),
                flushed: None,
            },
            started + Duration::from_millis(25),
        )
        .unwrap_err();
        assert!(error.to_string().contains("queue remained full"));
        assert!(started.elapsed() < Duration::from_secs(1));

        let (release, blocked) = mpsc::sync_channel(0);
        let worker = spawn_stdio_thread(move || {
            let _ = blocked.recv();
        });
        let started = Instant::now();
        let error = join_stdio_thread(worker, started + Duration::from_millis(25), "test writer")
            .unwrap_err();
        assert!(error.to_string().contains("detached blocked thread"));
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = release.send(());
    }

    #[test]
    fn connection_permits_enforce_and_release_the_limit() {
        let active_connections = Arc::new(AtomicUsize::new(0));
        let permits = (0..MAX_HTTP_CONNECTIONS)
            .map(|_| ConnectionPermit::try_acquire(&active_connections).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            active_connections.load(Ordering::Acquire),
            MAX_HTTP_CONNECTIONS
        );
        assert!(ConnectionPermit::try_acquire(&active_connections).is_none());
        drop(permits);
        assert_eq!(active_connections.load(Ordering::Acquire), 0);
        assert!(ConnectionPermit::try_acquire(&active_connections).is_some());
    }
}
