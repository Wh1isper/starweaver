//! Live MCP client backed by the official `rmcp` SDK.

use std::{
    collections::{BTreeMap, HashMap},
    process::Stdio,
};

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    model::{CallToolRequestParams, RawContent, TaskSupport},
    service::RunningService,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
        TokioChildProcess,
    },
    RoleClient, ServiceExt,
};
use serde_json::{Map, Value};
use starweaver_tools::{
    McpPromptSpec, McpResourceSpec, McpToolSpec, McpTransport, ToolContext, ToolResult,
};
use tokio::{process::Command, sync::Mutex};

use crate::{LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot};

type RmcpRunningService = RunningService<RoleClient, ()>;

/// Official `rmcp` SDK client adapter for live MCP servers.
#[derive(Default)]
pub struct RmcpLiveMcpClient {
    clients: Mutex<BTreeMap<String, RmcpRunningService>>,
}

impl RmcpLiveMcpClient {
    /// Create an empty `rmcp` live MCP client.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LiveMcpClient for RmcpLiveMcpClient {
    async fn discover(
        &self,
        id: &str,
        transport: &McpTransport,
    ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
        let client = connect_rmcp_client(transport).await?;
        let instructions = client
            .peer_info()
            .and_then(|info| info.instructions.clone());
        let tools = client
            .list_all_tools()
            .await
            .map_err(|error| rmcp_service_error(&error))?
            .into_iter()
            .map(mcp_tool_spec_from_rmcp)
            .collect();
        let resources = client
            .list_all_resources()
            .await
            .map_err(|error| rmcp_service_error(&error))?
            .into_iter()
            .map(|resource| mcp_resource_spec_from_rmcp(&resource))
            .collect();
        let prompts = client
            .list_all_prompts()
            .await
            .map_err(|error| rmcp_service_error(&error))?
            .into_iter()
            .map(mcp_prompt_spec_from_rmcp)
            .collect();
        let mut snapshot = LiveMcpServerSnapshot::new(id);
        snapshot.instructions = instructions;
        snapshot.tools = tools;
        snapshot.resources = resources;
        snapshot.prompts = prompts;

        let previous = {
            let mut clients = self.clients.lock().await;
            clients.remove(id)
        };
        if let Some(mut previous) = previous {
            let _ = previous.close().await;
        }
        {
            let mut clients = self.clients.lock().await;
            clients.insert(id.to_string(), client);
        }
        Ok(snapshot)
    }

    async fn call_tool(
        &self,
        _context: ToolContext,
        id: &str,
        _transport: &McpTransport,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolResult, LiveMcpError> {
        let arguments = json_object_arguments(arguments)?;
        let client = {
            let mut clients = self.clients.lock().await;
            clients.remove(id).ok_or_else(|| {
                LiveMcpError::Adapter(format!("rmcp client for server {id} was not discovered"))
            })?
        };
        let result = client
            .call_tool(CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments))
            .await
            .map_err(|error| rmcp_service_error(&error));
        {
            let mut clients = self.clients.lock().await;
            clients.insert(id.to_string(), client);
        }
        let result = result?;
        if result.is_error == Some(true) {
            return Err(LiveMcpError::Adapter(format!(
                "rmcp tool {id}/{tool_name} returned an error: {}",
                rmcp_content_text(&result.content)
            )));
        }
        let mut tool_result = ToolResult::new(
            result
                .structured_content
                .unwrap_or_else(|| Value::String(rmcp_content_text(&result.content))),
        );
        tool_result
            .metadata
            .insert("rmcp_live".to_string(), Value::Bool(true));
        Ok(tool_result)
    }

    async fn close(&self, id: &str, _transport: &McpTransport) -> Result<(), LiveMcpError> {
        let client = {
            let mut clients = self.clients.lock().await;
            clients.remove(id)
        };
        if let Some(mut client) = client {
            client.close().await.map_err(|error| {
                LiveMcpError::Adapter(format!("rmcp client close failed: {error}"))
            })?;
        }
        Ok(())
    }
}

async fn connect_rmcp_client(transport: &McpTransport) -> Result<RmcpRunningService, LiveMcpError> {
    match transport {
        McpTransport::Stdio { .. } => connect_stdio_client(transport).await,
        McpTransport::StreamableHttp { url, headers } => {
            connect_streamable_http_client(url, headers).await
        }
        McpTransport::Sse { .. } => Err(LiveMcpError::Adapter(
            "rmcp 1.7 does not expose the removed standalone SSE transport; use streamable_http"
                .to_string(),
        )),
    }
}

async fn connect_stdio_client(
    transport: &McpTransport,
) -> Result<RmcpRunningService, LiveMcpError> {
    let McpTransport::Stdio {
        command,
        args,
        cwd,
        env,
    } = transport
    else {
        return Err(LiveMcpError::Adapter(format!(
            "rmcp live client currently supports stdio transports, got {}",
            transport.kind()
        )));
    };

    let mut command = Command::new(command);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in env {
        command.env(key, env_value_string(value));
    }
    let process = TokioChildProcess::new(command).map_err(|error| {
        LiveMcpError::Adapter(format!("failed to spawn rmcp stdio transport: {error}"))
    })?;
    ().serve(process).await.map_err(|error| {
        LiveMcpError::Adapter(format!("rmcp client initialization failed: {error}"))
    })
}

async fn connect_streamable_http_client(
    url: &str,
    headers: &Map<String, Value>,
) -> Result<RmcpRunningService, LiveMcpError> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .custom_headers(http_headers(headers)?)
        .reinit_on_expired_session(true);
    let transport = StreamableHttpClientTransport::from_config(config);
    ().serve(transport).await.map_err(|error| {
        LiveMcpError::Adapter(format!(
            "rmcp streamable HTTP client initialization failed: {error}"
        ))
    })
}

fn mcp_tool_spec_from_rmcp(tool: rmcp::model::Tool) -> McpToolSpec {
    let parameters = Value::Object((*tool.input_schema).clone());
    let mut spec = McpToolSpec::new(tool.name.to_string(), parameters);
    if let Some(description) = tool.description {
        spec = spec.with_description(description.to_string());
    }
    if tool
        .execution
        .is_some_and(|execution| matches!(execution.task_support, Some(TaskSupport::Required)))
    {
        spec = spec.with_task(true);
    }
    spec
}

fn mcp_resource_spec_from_rmcp(resource: &rmcp::model::Resource) -> McpResourceSpec {
    let mut spec = McpResourceSpec::new(resource.uri.clone()).with_name(resource.name.clone());
    if let Some(description) = resource.description.clone() {
        spec = spec.with_description(description);
    }
    if let Some(mime_type) = resource.mime_type.clone() {
        spec = spec.with_mime_type(mime_type);
    }
    spec
}

fn mcp_prompt_spec_from_rmcp(prompt: rmcp::model::Prompt) -> McpPromptSpec {
    let arguments = prompt.arguments.map_or(Value::Null, |arguments| {
        serde_json::to_value(arguments).unwrap_or(Value::Null)
    });
    let mut spec = McpPromptSpec::new(prompt.name, arguments);
    if let Some(description) = prompt.description {
        spec = spec.with_description(description);
    }
    spec
}

fn json_object_arguments(arguments: Value) -> Result<Map<String, Value>, LiveMcpError> {
    match arguments {
        Value::Object(arguments) => Ok(arguments),
        other => Err(LiveMcpError::Adapter(format!(
            "rmcp tool arguments must be a JSON object, got {other}"
        ))),
    }
}

fn rmcp_content_text(content: &[rmcp::model::Content]) -> String {
    let mut text = Vec::new();
    for item in content {
        match &item.raw {
            RawContent::Text(raw) => text.push(raw.text.clone()),
            other => text.push(serde_json::to_string(other).unwrap_or_else(|_| String::new())),
        }
    }
    text.join("\n")
}

fn env_value_string(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

fn http_headers(
    headers: &Map<String, Value>,
) -> Result<HashMap<HeaderName, HeaderValue>, LiveMcpError> {
    let mut converted = HashMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            LiveMcpError::Adapter(format!("invalid MCP HTTP header name {name}: {error}"))
        })?;
        let value = HeaderValue::from_str(&env_value_string(value)).map_err(|error| {
            LiveMcpError::Adapter(format!("invalid MCP HTTP header value for {name}: {error}"))
        })?;
        converted.insert(name, value);
    }
    Ok(converted)
}

fn rmcp_service_error(error: &rmcp::ServiceError) -> LiveMcpError {
    LiveMcpError::Adapter(format!("rmcp service failed: {error}"))
}
