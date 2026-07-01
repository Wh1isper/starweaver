use std::{
    io::{self, BufRead as _, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{CliError, CliResult, config::CliConfig, rpc::RpcService};

const DEFAULT_HTTP_PATH: &str = "/rpc";
const MAX_HTTP_REQUEST_BYTES: usize = 8 * 1024 * 1024;

/// Run the JSON-RPC stdio server until stdin closes or `shutdown` is requested.
pub(super) fn run_stdio(config: &CliConfig) -> CliResult<()> {
    let (output_sender, output_receiver) = mpsc::channel::<Value>();
    let writer = thread::spawn(move || {
        let mut stdout = io::stdout();
        while let Ok(response) = output_receiver.recv() {
            if serde_json::to_writer(&mut stdout, &response).is_err() {
                break;
            }
            if stdout.write_all(b"\n").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    });
    let service = RpcService::new(config.clone(), output_sender.clone());
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| CliError::Run(error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let (response, shutdown) = service.handle_line(&line);
        if let Some(response) = response {
            output_sender
                .send(response)
                .map_err(|error| CliError::Run(error.to_string()))?;
        }
        if shutdown {
            break;
        }
    }
    drop(service);
    drop(output_sender);
    let _ = writer.join();
    Ok(())
}

/// Run the JSON-RPC HTTP server until `shutdown` is requested or the listener fails.
pub(super) fn run_http(config: &CliConfig, host: &str, port: u16) -> CliResult<()> {
    let address = format!("{host}:{port}");
    let listener = TcpListener::bind(&address).map_err(|error| {
        CliError::Run(format!(
            "failed to bind RPC HTTP listener at {address}: {error}"
        ))
    })?;
    let local_address = listener
        .local_addr()
        .map_err(|error| CliError::Run(error.to_string()))?;
    eprintln!("starweaver rpc http listening on http://{local_address}{DEFAULT_HTTP_PATH}");
    let service = Arc::new(RpcService::replay_only(
        config.clone(),
        closed_notification_sender(),
    ));
    serve_http(&listener, &service)
}

pub(super) fn closed_notification_sender() -> mpsc::Sender<Value> {
    let (sender, receiver) = mpsc::channel();
    drop(receiver);
    sender
}

fn serve_http(listener: &TcpListener, service: &Arc<RpcService>) -> CliResult<()> {
    listener
        .set_nonblocking(true)
        .map_err(|error| CliError::Run(error.to_string()))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _address)) => {
                let service = Arc::clone(service);
                let shutdown = Arc::clone(&shutdown);
                thread::spawn(move || {
                    if let Err(error) = handle_http_connection(stream, &service, &shutdown) {
                        eprintln!("rpc http connection error: {error}");
                    }
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(CliError::Run(error.to_string())),
        }
    }
    Ok(())
}

pub(super) fn handle_http_connection(
    mut stream: TcpStream,
    service: &RpcService,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let request = match read_http_request(&mut stream)? {
        Ok(request) => request,
        Err(response) => return write_http_response(&mut stream, &response),
    };
    if request.method == "GET" && matches!(request.path.as_str(), "/health" | "/healthz") {
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
    let (response, should_shutdown) = service.handle_text(&request.body);
    if should_shutdown {
        shutdown.store(true, Ordering::SeqCst);
    }
    if let Some(response) = response {
        write_http_json(&mut stream, "200 OK", &response)
    } else {
        let response = HttpResponse {
            status: "204 No Content",
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        };
        write_http_response(&mut stream, &response)
    }
}

struct HttpRequest {
    method: String,
    path: String,
    body: String,
}

struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<Result<HttpRequest, HttpResponse>> {
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
        let read = stream.read(&mut chunk)?;
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
    let Some((method, path, content_length)) = parse_http_header(header) else {
        return Ok(Err(http_text_response(
            "400 Bad Request",
            "invalid http request",
        )));
    };
    if header_end + content_length > MAX_HTTP_REQUEST_BYTES {
        return Ok(Err(http_text_response(
            "413 Payload Too Large",
            "request too large",
        )));
    }
    while buffer.len() < header_end + content_length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(Err(http_text_response(
                "400 Bad Request",
                "incomplete http body",
            )));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body_bytes = &buffer[header_end..header_end + content_length];
    let body = match std::str::from_utf8(body_bytes) {
        Ok(body) => body.to_string(),
        Err(_) => {
            return Ok(Err(http_text_response(
                "400 Bad Request",
                "request body must be utf-8",
            )));
        }
    };
    Ok(Ok(HttpRequest { method, path, body }))
}

fn http_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn parse_http_header(header: &str) -> Option<(String, String, usize)> {
    let mut lines = header.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let _version = parts.next()?;
    let mut content_length = 0_usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().ok()?;
        }
    }
    Some((method, path, content_length))
}

fn http_text_response(status: &'static str, body: &'static str) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/plain; charset=utf-8",
        body: body.as_bytes().to_vec(),
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
        body,
    };
    write_http_response(stream, &response)
}

fn write_http_response(stream: &mut TcpStream, response: &HttpResponse) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)?;
    stream.flush()
}
