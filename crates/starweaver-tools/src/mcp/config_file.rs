use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use super::{McpToolSpec, McpToolsetConfig, McpTransport};

/// Product-neutral MCP configuration document shared by host products.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfigDocument {
    /// MCP servers keyed by stable configuration name.
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerConfig>,
}

/// One MCP server entry from a standalone configuration file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// `stdio`, `streamable_http` (`http` alias), or `sse`.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Executable used by the stdio transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments passed to the stdio executable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Optional stdio working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// String-valued stdio subprocess environment.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub env: Map<String, Value>,
    /// Streamable HTTP or SSE endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// String-valued HTTP request headers.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub headers: Map<String, Value>,
    /// Optional prefix applied to discovered tool names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_prefix: Option<String>,
    /// Whether server instructions are added to model instructions.
    #[serde(default)]
    pub include_instructions: bool,
    /// Host-provided instructions used in preference to discovered instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Per-tool MCP request timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_ms: Option<u64>,
    /// Initial connection and discovery timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_timeout_ms: Option<u64>,
    /// Exit and transport cleanup timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_timeout_ms: Option<u64>,
    /// Optional static annotations. Live discovery remains authoritative for schemas, while `task`
    /// and metadata are merged by tool name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<McpToolSpec>,
}

impl McpConfigDocument {
    /// Read and validate one MCP JSON file.
    ///
    /// # Errors
    ///
    /// Returns an I/O, JSON, or semantic configuration error.
    pub fn from_path(path: &Path) -> Result<Self, McpConfigFileError> {
        let bytes = fs::read(path).map_err(|source| McpConfigFileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_slice(&bytes)
    }

    /// Parse and validate an MCP configuration document.
    ///
    /// # Errors
    ///
    /// Returns a JSON or semantic configuration error.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, McpConfigFileError> {
        let document = serde_json::from_slice::<Self>(bytes)?;
        document.validate()?;
        Ok(document)
    }

    /// Validate stable server names and transport-specific fields.
    ///
    /// # Errors
    ///
    /// Returns the first invalid server entry.
    pub fn validate(&self) -> Result<(), McpConfigFileError> {
        for (name, server) in &self.servers {
            if !valid_identifier(name) {
                return Err(McpConfigFileError::InvalidServer {
                    server: name.clone(),
                    message:
                        "server name must contain only ASCII letters, digits, '_', '-', or '.'"
                            .to_string(),
                });
            }
            server.to_toolset_config(name)?;
        }
        Ok(())
    }
}

impl McpServerConfig {
    /// Resolve this file entry into the provider-neutral MCP toolset configuration.
    ///
    /// # Errors
    ///
    /// Returns an error for missing or malformed transport fields.
    pub fn to_toolset_config(&self, name: &str) -> Result<McpToolsetConfig, McpConfigFileError> {
        let transport = match self.transport.trim().to_ascii_lowercase().as_str() {
            "stdio" => {
                if self.url.is_some() || !self.headers.is_empty() {
                    return invalid_server(name, "stdio transport does not accept url or headers");
                }
                let command = required_non_empty(name, "command", self.command.as_deref())?;
                validate_string_map(name, "env", &self.env)?;
                let mut transport = McpTransport::stdio(command).with_args(self.args.clone());
                if let Some(cwd) = self.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
                    transport = transport.with_cwd(cwd);
                }
                transport.with_env(self.env.clone())
            }
            "streamable_http" | "http" => {
                reject_stdio_fields(name, self)?;
                let url = required_non_empty(name, "url", self.url.as_deref())?;
                validate_http_url(name, url)?;
                validate_string_map(name, "headers", &self.headers)?;
                McpTransport::streamable_http(url).with_headers(self.headers.clone())
            }
            "sse" => {
                reject_stdio_fields(name, self)?;
                let url = required_non_empty(name, "url", self.url.as_deref())?;
                validate_http_url(name, url)?;
                validate_string_map(name, "headers", &self.headers)?;
                McpTransport::sse(url).with_headers(self.headers.clone())
            }
            other => {
                return Err(McpConfigFileError::InvalidServer {
                    server: name.to_string(),
                    message: format!("unsupported transport: {other}"),
                });
            }
        };
        let mut config = McpToolsetConfig::new(name, transport)
            .with_include_instructions(self.include_instructions);
        if let Some(prefix) = self
            .tool_prefix
            .as_deref()
            .filter(|prefix| !prefix.trim().is_empty())
        {
            if !valid_identifier(prefix) {
                return Err(McpConfigFileError::InvalidServer {
                    server: name.to_string(),
                    message: "tool_prefix contains unsupported characters".to_string(),
                });
            }
            config = config.with_tool_prefix(prefix);
        }
        if let Some(instructions) = self
            .instructions
            .as_deref()
            .filter(|instructions| !instructions.trim().is_empty())
        {
            config = config.with_instructions(instructions);
        }
        if let Some(timeout) = self.read_timeout_ms {
            if timeout == 0 {
                return invalid_timeout(name, "read_timeout_ms");
            }
            config = config.with_read_timeout_ms(timeout);
        }
        if let Some(timeout) = self.init_timeout_ms {
            if timeout == 0 {
                return invalid_timeout(name, "init_timeout_ms");
            }
            config = config.with_init_timeout_ms(timeout);
        }
        if let Some(timeout) = self.exit_timeout_ms {
            if timeout == 0 {
                return invalid_timeout(name, "exit_timeout_ms");
            }
            config = config.with_exit_timeout_ms(timeout);
        }
        for tool in &self.tools {
            if !valid_identifier(&tool.name) {
                return Err(McpConfigFileError::InvalidServer {
                    server: name.to_string(),
                    message: format!("invalid static tool name: {}", tool.name),
                });
            }
            config = config.with_tool(tool.clone());
        }
        Ok(config)
    }
}

/// MCP configuration file failure.
#[derive(Debug, Error)]
pub enum McpConfigFileError {
    /// The configured file could not be read.
    #[error("failed to read MCP config {path}: {source}")]
    Io {
        /// File path requested by the host.
        path: std::path::PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The document is not valid strict JSON for the MCP schema.
    #[error("invalid MCP JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// One named server violates transport or field invariants.
    #[error("invalid MCP server {server:?}: {message}")]
    InvalidServer {
        /// Stable server name.
        server: String,
        /// Safe configuration diagnostic.
        message: String,
    },
}

fn default_transport() -> String {
    "stdio".to_string()
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn required_non_empty<'a>(
    server: &str,
    field: &str,
    value: Option<&'a str>,
) -> Result<&'a str, McpConfigFileError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpConfigFileError::InvalidServer {
            server: server.to_string(),
            message: format!("{field} is required"),
        })
}

fn validate_string_map(
    server: &str,
    field: &str,
    values: &Map<String, Value>,
) -> Result<(), McpConfigFileError> {
    if let Some(key) = values
        .iter()
        .find_map(|(key, value)| (!value.is_string()).then_some(key))
    {
        return Err(McpConfigFileError::InvalidServer {
            server: server.to_string(),
            message: format!("{field}.{key} must be a string"),
        });
    }
    Ok(())
}

fn validate_http_url(server: &str, value: &str) -> Result<(), McpConfigFileError> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return Ok(());
    }
    Err(McpConfigFileError::InvalidServer {
        server: server.to_string(),
        message: "url must use http:// or https://".to_string(),
    })
}

fn reject_stdio_fields(server: &str, config: &McpServerConfig) -> Result<(), McpConfigFileError> {
    if config.command.is_some()
        || !config.args.is_empty()
        || config.cwd.is_some()
        || !config.env.is_empty()
    {
        return invalid_server(
            server,
            "HTTP transports do not accept command, args, cwd, or env",
        );
    }
    Ok(())
}

fn invalid_server<T>(server: &str, message: impl Into<String>) -> Result<T, McpConfigFileError> {
    Err(McpConfigFileError::InvalidServer {
        server: server.to_string(),
        message: message.into(),
    })
}

fn invalid_timeout<T>(server: &str, field: &str) -> Result<T, McpConfigFileError> {
    invalid_server(server, format!("{field} must be greater than zero"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_stdio_and_http_servers() {
        let document = McpConfigDocument::from_slice(
            br#"{
              "servers": {
                "docs": {"command":"npx","args":["-y","docs-mcp"]},
                "remote": {"transport":"http","url":"https://example.test/mcp","headers":{"Authorization":"Bearer test"}}
              }
            }"#,
        )
        .unwrap();

        assert_eq!(document.servers.len(), 2);
        assert_eq!(
            document.servers["docs"]
                .to_toolset_config("docs")
                .unwrap()
                .transport
                .kind(),
            "stdio"
        );
        assert_eq!(
            document.servers["remote"]
                .to_toolset_config("remote")
                .unwrap()
                .transport
                .kind(),
            "streamable_http"
        );
    }

    #[test]
    fn accepts_sse_for_product_specific_validation() {
        let document = McpConfigDocument::from_slice(
            br#"{"servers":{"events":{"transport":"sse","url":"https://example.test/sse"}}}"#,
        )
        .unwrap();
        assert_eq!(
            document.servers["events"]
                .to_toolset_config("events")
                .unwrap()
                .transport
                .kind(),
            "sse"
        );
    }

    #[test]
    fn rejects_invalid_or_transport_mismatched_fields() {
        let invalid = [
            br#"{"servers":{"bad":{"transport":"stdio"}}}"#.as_slice(),
            br#"{"servers":{"bad":{"transport":"unknown","command":"x"}}}"#.as_slice(),
            br#"{"servers":{"bad":{"transport":"http","url":"ftp://example.test"}}}"#.as_slice(),
            br#"{"servers":{"bad":{"transport":"http","url":"https://example.test","headers":{"x":1}}}}"#.as_slice(),
            br#"{"servers":{"bad":{"command":"x","url":"https://unexpected.test"}}}"#.as_slice(),
            br#"{"servers":{"bad":{"transport":"http","url":"https://example.test","command":"unexpected"}}}"#.as_slice(),
            br#"{"servers":{"bad":{"command":"x","init_timeout_ms":0}}}"#.as_slice(),
            br#"{"servers":{"bad":{"command":"x","exit_timeout_ms":0}}}"#.as_slice(),
            br#"{"servers":{"bad":{"command":"x","unexpected":true}}}"#.as_slice(),
        ];
        for document in invalid {
            assert!(
                McpConfigDocument::from_slice(document).is_err(),
                "accepted invalid MCP document: {}",
                String::from_utf8_lossy(document)
            );
        }
    }
}
