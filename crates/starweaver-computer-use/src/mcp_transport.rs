//! Bounded newline-delimited JSON transport for the local stdio MCP process.

use std::{io, sync::Arc};

use rmcp::{
    RoleServer,
    model::ErrorData,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use starweaver_core::CancellationToken;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Mutex,
};

/// Maximum accepted bytes in one inbound newline-delimited JSON-RPC frame.
pub const MAX_MCP_INPUT_FRAME_BYTES: usize = 256 * 1024;
/// Maximum accepted object/array nesting in one inbound JSON-RPC frame.
pub const MAX_MCP_JSON_DEPTH: usize = 32;

const READ_BUFFER_BYTES: usize = 8 * 1024;
const OVERSIZE_CODE: &str = "mcp_input_frame_too_large";
const DEPTH_CODE: &str = "mcp_json_depth_exceeded";
const PARSE_CODE: &str = "mcp_json_parse_error";

/// A resource-bounded stdio transport that preserves `rmcp`'s transport API.
pub struct BoundedStdioTransport<R, W> {
    read: BufReader<R>,
    frame: Vec<u8>,
    discarding_oversize: bool,
    write: Arc<Mutex<Option<W>>>,
    shutdown: CancellationToken,
}

impl<R, W> BoundedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    /// Construct a transport. The supplied token is cancelled as soon as EOF or a
    /// terminal transport error is observed, before `rmcp` waits for handlers.
    pub fn new(read: R, write: W, shutdown: CancellationToken) -> Self {
        Self {
            read: BufReader::with_capacity(READ_BUFFER_BYTES, read),
            frame: Vec::with_capacity(READ_BUFFER_BYTES),
            discarding_oversize: false,
            write: Arc::new(Mutex::new(Some(write))),
            shutdown,
        }
    }

    async fn read_frame(&mut self) -> Result<Option<Vec<u8>>, FrameReadError> {
        loop {
            let available = self.read.fill_buf().await.map_err(|_| FrameReadError::Io)?;
            if available.is_empty() {
                if self.discarding_oversize {
                    self.discarding_oversize = false;
                    return Err(FrameReadError::TooLarge);
                }
                if self.frame.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(std::mem::take(&mut self.frame)));
            }

            let newline = available.iter().position(|byte| *byte == b'\n');
            if self.discarding_oversize {
                let consumed = newline.map_or(available.len(), |offset| offset + 1);
                self.read.consume(consumed);
                if newline.is_some() {
                    self.discarding_oversize = false;
                    return Err(FrameReadError::TooLarge);
                }
                continue;
            }

            if let Some(offset) = newline {
                if self.frame.len().saturating_add(offset) > MAX_MCP_INPUT_FRAME_BYTES {
                    self.read.consume(offset + 1);
                    self.frame.clear();
                    return Err(FrameReadError::TooLarge);
                }
                self.frame.extend_from_slice(&available[..offset]);
                self.read.consume(offset + 1);
                if self.frame.last() == Some(&b'\r') {
                    self.frame.pop();
                }
                return Ok(Some(std::mem::take(&mut self.frame)));
            }

            if self.frame.len().saturating_add(available.len()) > MAX_MCP_INPUT_FRAME_BYTES {
                let consumed = available.len();
                self.read.consume(consumed);
                self.frame.clear();
                self.discarding_oversize = true;
                continue;
            }
            let consumed = available.len();
            self.frame.extend_from_slice(available);
            self.read.consume(consumed);
        }
    }
}

impl<R, W> Transport<RoleServer> for BoundedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let write = self.write.clone();
        async move { write_message(&write, item).await }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let frame = match self.read_frame().await {
                Ok(Some(frame)) => frame,
                Ok(None) | Err(FrameReadError::Io) => {
                    self.shutdown.cancel();
                    return None;
                }
                Err(FrameReadError::TooLarge) => {
                    if send_protocol_error(
                        self.write.clone(),
                        OVERSIZE_CODE,
                        "MCP input frame exceeds the byte limit",
                    )
                    .await
                    .is_err()
                    {
                        self.shutdown.cancel();
                        return None;
                    }
                    continue;
                }
            };
            if frame.is_empty() {
                continue;
            }
            if json_depth_exceeds(&frame, MAX_MCP_JSON_DEPTH) {
                if send_protocol_error(
                    self.write.clone(),
                    DEPTH_CODE,
                    "MCP JSON nesting exceeds the depth limit",
                )
                .await
                .is_err()
                {
                    self.shutdown.cancel();
                    return None;
                }
                continue;
            }
            match serde_json::from_slice(&frame) {
                Ok(message) => return Some(message),
                Err(_) => {
                    if send_protocol_error(
                        self.write.clone(),
                        PARSE_CODE,
                        "Invalid MCP JSON-RPC frame",
                    )
                    .await
                    .is_err()
                    {
                        self.shutdown.cancel();
                        return None;
                    }
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.shutdown.cancel();
        let mut write = self.write.lock().await;
        let result = if let Some(mut writer) = write.take() {
            writer.shutdown().await
        } else {
            Ok(())
        };
        drop(write);
        result
    }
}

async fn send_protocol_error<W>(
    write: Arc<Mutex<Option<W>>>,
    code: &'static str,
    message: &'static str,
) -> io::Result<()>
where
    W: AsyncWrite + Send + Unpin,
{
    let response = TxJsonRpcMessage::<RoleServer>::error(
        ErrorData::parse_error(message, Some(serde_json::json!({ "code": code }))),
        None,
    );
    write_message(&write, response).await
}

async fn write_message<W>(
    write: &Arc<Mutex<Option<W>>>,
    item: TxJsonRpcMessage<RoleServer>,
) -> io::Result<()>
where
    W: AsyncWrite + Send + Unpin,
{
    let mut bytes = serde_json::to_vec(&item).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let mut guard = write.lock().await;
    let result = if let Some(writer) = guard.as_mut() {
        match writer.write_all(&bytes).await {
            Ok(()) => writer.flush().await,
            Err(error) => Err(error),
        }
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "MCP transport is closed",
        ))
    };
    drop(guard);
    result
}

fn json_depth_exceeds(input: &[u8], maximum: usize) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in input {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                if depth > maximum {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

#[derive(Debug)]
enum FrameReadError {
    TooLarge,
    Io,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_scan_ignores_delimiters_inside_strings() {
        assert!(!json_depth_exceeds(
            br#"{"value":"[[[{{{","nested":{"ok":true}}}"#,
            2
        ));
        assert!(json_depth_exceeds(br#"{"a":[{"b":true}]}"#, 2));
    }

    #[tokio::test]
    async fn bounded_reader_discards_an_oversize_frame_and_recovers() {
        let mut input = vec![b'x'; MAX_MCP_INPUT_FRAME_BYTES + 1];
        input.extend_from_slice(b"\n{}\n");
        let (reader, mut writer) = tokio::io::duplex(input.len() + 32);
        let (output, _) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            let _ = writer.write_all(&input).await;
        });
        let shutdown = CancellationToken::new();
        let mut transport = BoundedStdioTransport::new(reader, output, shutdown);

        assert!(matches!(
            transport.read_frame().await,
            Err(FrameReadError::TooLarge)
        ));
        assert_eq!(
            transport.read_frame().await.ok().flatten(),
            Some(b"{}".to_vec())
        );
    }
}
