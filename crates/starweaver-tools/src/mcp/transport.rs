//! MCP transport configuration.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// MCP client transport kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum McpTransport {
    /// Streamable HTTP transport.
    StreamableHttp {
        /// MCP endpoint URL.
        url: String,
        /// Optional HTTP headers.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        headers: Map<String, Value>,
    },
    /// Server-Sent Events transport.
    Sse {
        /// MCP endpoint URL.
        url: String,
        /// Optional HTTP headers.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        headers: Map<String, Value>,
    },
    /// Stdio subprocess transport.
    Stdio {
        /// Command to run.
        command: String,
        /// Command arguments.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Optional working directory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Optional subprocess environment.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        env: Map<String, Value>,
    },
}

impl McpTransport {
    /// Build a streamable HTTP transport.
    #[must_use]
    pub fn streamable_http(url: impl Into<String>) -> Self {
        Self::StreamableHttp {
            url: url.into(),
            headers: Map::new(),
        }
    }

    /// Build an SSE transport.
    #[must_use]
    pub fn sse(url: impl Into<String>) -> Self {
        Self::Sse {
            url: url.into(),
            headers: Map::new(),
        }
    }

    /// Build a stdio transport.
    #[must_use]
    pub fn stdio(command: impl Into<String>) -> Self {
        Self::Stdio {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: Map::new(),
        }
    }

    /// Attach transport headers to an HTTP transport.
    #[must_use]
    pub fn with_headers(mut self, headers: Map<String, Value>) -> Self {
        match &mut self {
            Self::StreamableHttp {
                headers: target, ..
            }
            | Self::Sse {
                headers: target, ..
            } => {
                *target = headers;
            }
            Self::Stdio { .. } => {}
        }
        self
    }

    /// Attach stdio command arguments.
    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        if let Self::Stdio { args: target, .. } = &mut self {
            *target = args;
        }
        self
    }

    /// Attach a stdio working directory.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        if let Self::Stdio { cwd: target, .. } = &mut self {
            *target = Some(cwd.into());
        }
        self
    }

    /// Attach a stdio environment map.
    #[must_use]
    pub fn with_env(mut self, env: Map<String, Value>) -> Self {
        if let Self::Stdio { env: target, .. } = &mut self {
            *target = env;
        }
        self
    }

    /// Transport name used in metadata.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::StreamableHttp { .. } => "streamable_http",
            Self::Sse { .. } => "sse",
            Self::Stdio { .. } => "stdio",
        }
    }
}
