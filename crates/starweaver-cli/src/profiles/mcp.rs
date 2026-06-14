use std::{collections::BTreeMap, sync::Arc};

use serde_json::json;
use starweaver_agent::{
    DynToolset, McpServerSpec, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport,
    ToolProxyToolset,
};

use crate::config::{mcp_servers, CliConfig};

pub(super) fn configured_mcp_server_specs(config: &CliConfig) -> Vec<McpServerSpec> {
    mcp_servers(config)
        .into_iter()
        .filter_map(|(name, value)| {
            let transport = parse_mcp_transport(&value)?;
            let metadata = value.as_object().cloned().unwrap_or_default();
            Some(McpServerSpec {
                name,
                transport: transport.kind().to_string(),
                metadata,
            })
        })
        .collect()
}

pub(super) fn configured_mcp_proxy_toolset(config: &CliConfig) -> Option<DynToolset> {
    let mut descriptions = BTreeMap::new();
    let toolsets = configured_mcp_toolsets(config, &mut descriptions);
    if toolsets.is_empty() {
        return None;
    }
    let proxy = ToolProxyToolset::new(toolsets)
        .try_with_name_prefix("mcp")
        .ok()?
        .with_namespace_descriptions(descriptions);
    Some(Arc::new(proxy))
}

fn configured_mcp_toolsets(
    config: &CliConfig,
    descriptions: &mut BTreeMap<String, String>,
) -> Vec<DynToolset> {
    mcp_servers(config)
        .into_iter()
        .filter_map(|(name, value)| {
            let transport = parse_mcp_transport(&value)?;
            let toolset_id = format!("mcp_{name}");
            let mut toolset_config = McpToolsetConfig::new(toolset_id.clone(), transport);
            if let Some(prefix) = value.get("tool_prefix").and_then(serde_json::Value::as_str) {
                toolset_config = toolset_config.with_tool_prefix(prefix);
            }
            if value
                .get("include_instructions")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                toolset_config = toolset_config.with_include_instructions(true);
            }
            if let Some(instructions) = value
                .get("instructions")
                .and_then(serde_json::Value::as_str)
            {
                toolset_config = toolset_config.with_instructions(instructions);
            }
            let description = mcp_namespace_description(&value);
            for tool in parse_mcp_tools(&value) {
                toolset_config = toolset_config.with_tool(tool);
            }
            if let Some(description) = description {
                descriptions.insert(toolset_id, description);
            }
            Some(Arc::new(McpToolset::new(toolset_config)) as DynToolset)
        })
        .collect()
}

fn mcp_namespace_description(value: &serde_json::Value) -> Option<String> {
    value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("instructions")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
}

pub(super) fn mcp_transport_error(value: &serde_json::Value) -> Option<String> {
    let transport = value
        .get("transport")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio");
    match transport {
        "stdio" => value
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| command.trim().is_empty())
            .map_or_else(
                || Some("stdio transport requires command".to_string()),
                |empty| empty.then(|| "stdio transport requires command".to_string()),
            ),
        "streamable_http" | "http" | "sse" => value
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(|url| url.trim().is_empty())
            .map_or_else(
                || Some(format!("{transport} transport requires url")),
                |empty| empty.then(|| format!("{transport} transport requires url")),
            ),
        other => Some(format!("unknown MCP transport {other}")),
    }
}

fn parse_mcp_transport(value: &serde_json::Value) -> Option<McpTransport> {
    let transport = value
        .get("transport")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio");
    match transport {
        "stdio" => {
            let command = value.get("command").and_then(serde_json::Value::as_str)?;
            let args = value
                .get("args")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            let mut parsed = McpTransport::stdio(command).with_args(args);
            if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
                parsed = parsed.with_cwd(cwd);
            }
            if let Some(env) = value.get("env").and_then(serde_json::Value::as_object) {
                parsed = parsed.with_env(env.clone());
            }
            Some(parsed)
        }
        "streamable_http" | "http" => {
            let url = value.get("url").and_then(serde_json::Value::as_str)?;
            let mut parsed = McpTransport::streamable_http(url);
            if let Some(headers) = value.get("headers").and_then(serde_json::Value::as_object) {
                parsed = parsed.with_headers(headers.clone());
            }
            Some(parsed)
        }
        "sse" => {
            let url = value.get("url").and_then(serde_json::Value::as_str)?;
            let mut parsed = McpTransport::sse(url);
            if let Some(headers) = value.get("headers").and_then(serde_json::Value::as_object) {
                parsed = parsed.with_headers(headers.clone());
            }
            Some(parsed)
        }
        _ => None,
    }
}

fn parse_mcp_tools(value: &serde_json::Value) -> Vec<McpToolSpec> {
    value
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(serde_json::Value::as_str)?;
            let parameters = tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            let mut spec = McpToolSpec::new(name, parameters);
            if let Some(description) = tool.get("description").and_then(serde_json::Value::as_str) {
                spec = spec.with_description(description);
            }
            if let Some(task) = tool.get("task").and_then(serde_json::Value::as_bool) {
                spec = spec.with_task(task);
            }
            if let Some(metadata) = tool.get("metadata").and_then(serde_json::Value::as_object) {
                spec = spec.with_metadata(metadata.clone());
            }
            Some(spec)
        })
        .collect()
}
