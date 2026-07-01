//! `EnvD` stdio and HTTP JSON-RPC transports.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::{Value, json};
use starweaver_envd_core::EnvdService;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

use crate::EnvdRpcService;

const DEFAULT_HTTP_PATH: &str = "/rpc";
const MAX_HTTP_REQUEST_BYTES: usize = 8 * 1024 * 1024;

/// Run the envd JSON-RPC stdio server until stdin closes or shutdown is requested.
///
/// # Errors
///
/// Returns I/O errors from stdin/stdout handling.
pub async fn run_stdio(service: Arc<dyn EnvdService>) -> io::Result<()> {
    let rpc = EnvdRpcService::new(service);
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let (response, shutdown) = rpc.handle_text(&line).await;
        if let Some(response) = response {
            let bytes = serde_json::to_vec(&response).map_err(json_io_error)?;
            stdout.write_all(&bytes).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        if shutdown {
            break;
        }
    }
    Ok(())
}

/// Run the envd JSON-RPC HTTP server until shutdown is requested.
///
/// # Errors
///
/// Returns listener bind, accept, or response I/O errors.
pub async fn run_http(
    service: Arc<dyn EnvdService>,
    host: &str,
    port: u16,
    auth_token: impl Into<String>,
) -> io::Result<()> {
    let auth_token = validate_auth_token(auth_token.into())?;
    let address = format!("{host}:{port}");
    let listener = TcpListener::bind(&address).await?;
    let local_address = listener.local_addr()?;
    eprintln!("starweaver envd http listening on http://{local_address}{DEFAULT_HTTP_PATH}");
    let rpc = Arc::new(EnvdRpcService::new(service));
    serve_http(&listener, &rpc, Arc::from(auth_token)).await
}

async fn serve_http(
    listener: &TcpListener,
    rpc: &Arc<EnvdRpcService>,
    auth_token: Arc<str>,
) -> io::Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_notify = Arc::new(Notify::new());
    while !shutdown.load(Ordering::SeqCst) {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _address) = accepted?;
                let rpc = Arc::clone(rpc);
                let auth_token = Arc::clone(&auth_token);
                let shutdown = Arc::clone(&shutdown);
                let shutdown_notify = Arc::clone(&shutdown_notify);
                tokio::spawn(async move {
                    if let Err(error) = handle_http_connection(stream, &rpc, &auth_token, &shutdown, &shutdown_notify).await {
                        eprintln!("envd http connection error: {error}");
                    }
                });
            }
            () = shutdown_notify.notified() => {}
        }
    }
    Ok(())
}

async fn handle_http_connection(
    mut stream: TcpStream,
    rpc: &EnvdRpcService,
    auth_token: &str,
    shutdown: &AtomicBool,
    shutdown_notify: &Notify,
) -> io::Result<()> {
    let request = match read_http_request(&mut stream).await? {
        Ok(request) => request,
        Err(response) => return write_http_response(&mut stream, &response).await,
    };
    if !request_is_authorized(&request, auth_token) {
        return write_http_text(&mut stream, "401 Unauthorized", "unauthorized").await;
    }
    if request.method == "GET" && matches!(request.path.as_str(), "/health" | "/healthz") {
        return write_http_json(
            &mut stream,
            "200 OK",
            &json!({"status": "ok", "protocol": "starweaver.envd"}),
        )
        .await;
    }
    if request.method != "POST" {
        return write_http_text(&mut stream, "405 Method Not Allowed", "method not allowed").await;
    }
    if !matches!(request.path.as_str(), "/" | DEFAULT_HTTP_PATH) {
        return write_http_text(&mut stream, "404 Not Found", "not found").await;
    }
    let (response, should_shutdown) = rpc.handle_text(&request.body).await;
    if should_shutdown {
        shutdown.store(true, Ordering::SeqCst);
        shutdown_notify.notify_waiters();
    }
    if let Some(response) = response {
        write_http_json(&mut stream, "200 OK", &response).await
    } else {
        write_http_response(
            &mut stream,
            &HttpResponse {
                status: "204 No Content",
                content_type: "text/plain; charset=utf-8",
                body: Vec::new(),
            },
        )
        .await
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

struct ParsedHttpHeader {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    content_length: usize,
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> io::Result<Result<HttpRequest, HttpResponse>> {
    let mut buffer = Vec::new();
    let header_end = loop {
        if buffer.len() > MAX_HTTP_REQUEST_BYTES {
            return Ok(Err(http_text_response(
                "413 Payload Too Large",
                "request too large",
            )));
        }
        if let Some(header_end) = http_header_end(&buffer) {
            break header_end;
        }
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await?;
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
    let Some(parsed) = parse_http_header(header) else {
        return Ok(Err(http_text_response(
            "400 Bad Request",
            "invalid http request",
        )));
    };
    if header_end + parsed.content_length > MAX_HTTP_REQUEST_BYTES {
        return Ok(Err(http_text_response(
            "413 Payload Too Large",
            "request too large",
        )));
    }
    while buffer.len() < header_end + parsed.content_length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(Err(http_text_response(
                "400 Bad Request",
                "incomplete http body",
            )));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body_bytes = &buffer[header_end..header_end + parsed.content_length];
    let body = std::str::from_utf8(body_bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
        .to_string();
    Ok(Ok(HttpRequest {
        method: parsed.method,
        path: parsed.path,
        headers: parsed.headers,
        body,
    }))
}

fn http_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn parse_http_header(header: &str) -> Option<ParsedHttpHeader> {
    let mut lines = header.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let _version = parts.next()?;
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().ok()?;
        }
        headers.push((name, value));
    }
    Some(ParsedHttpHeader {
        method,
        path,
        headers,
        content_length,
    })
}

fn request_is_authorized(request: &HttpRequest, auth_token: &str) -> bool {
    request
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .any(|(_, value)| value == &format!("Bearer {auth_token}"))
}

fn validate_auth_token(token: String) -> io::Result<String> {
    if token.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "envd HTTP auth token cannot be empty",
        ));
    }
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "envd HTTP auth token cannot contain newlines",
        ));
    }
    Ok(token)
}

fn http_text_response(status: &'static str, body: &'static str) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/plain; charset=utf-8",
        body: body.as_bytes().to_vec(),
    }
}

async fn write_http_text(
    stream: &mut TcpStream,
    status: &'static str,
    body: &'static str,
) -> io::Result<()> {
    write_http_response(stream, &http_text_response(status, body)).await
}

async fn write_http_json(
    stream: &mut TcpStream,
    status: &'static str,
    body: &Value,
) -> io::Result<()> {
    let body = serde_json::to_vec(body).map_err(json_io_error)?;
    write_http_response(
        stream,
        &HttpResponse {
            status,
            content_type: "application/json",
            body,
        },
    )
    .await
}

async fn write_http_response(stream: &mut TcpStream, response: &HttpResponse) -> io::Result<()> {
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await
}

fn json_io_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
