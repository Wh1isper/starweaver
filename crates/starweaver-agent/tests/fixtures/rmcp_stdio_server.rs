#![allow(missing_docs, clippy::expect_used)]

use std::future::Future;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, Implementation, ListPromptsResult,
        ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument,
        RawResource, ServerCapabilities, ServerInfo, TaskSupport, Tool, ToolExecution,
    },
    service::{MaybeSendFuture, RequestContext},
    transport::stdio,
};

struct FixtureMcpServer;

impl ServerHandler for FixtureMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_instructions("Use fixture MCP tools.")
        .with_server_info(Implementation::new("starweaver-fixture", "0.0.1"))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![
            Tool::new("lookup", "Look up fixture data.", object_schema("query")),
            Tool::new(
                "dangerous",
                "Execute an approval-gated fixture action.",
                object_schema("path"),
            ),
            Tool::new(
                "slow",
                "Start a deferred fixture task.",
                object_schema("job"),
            )
            .with_execution(ToolExecution::from_raw(Some(TaskSupport::Required))),
        ])))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(vec![
            RawResource::new("resource://fixture/docs", "Fixture Docs")
                .with_description("Fixture documentation.")
                .with_mime_type("text/markdown")
                .no_annotation(),
        ])))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "summarize",
            Some("Summarize fixture docs."),
            Some(vec![
                PromptArgument::new("topic")
                    .with_description("Topic to summarize.")
                    .with_required(false),
            ]),
        )])))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + MaybeSendFuture + '_ {
        let structured = match request.name.as_ref() {
            "dangerous" => serde_json::json!({
                "executed": true,
                "path": argument(&request, "path"),
            }),
            "slow" => serde_json::json!({
                "answer": "slow task should be deferred",
                "job": argument(&request, "job"),
            }),
            _ => serde_json::json!({
                "answer": "fixture result",
                "query": argument(&request, "query"),
            }),
        };
        std::future::ready(Ok(CallToolResult::structured(structured)))
    }
}

fn object_schema(required_property: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "type": "object",
        "properties": {required_property: {"type": "string"}},
        "required": [required_property],
    })
    .as_object()
    .expect("fixture schema is an object")
    .clone()
}

fn argument(request: &CallToolRequestParams, key: &str) -> serde_json::Value {
    request
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.get(key))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    FixtureMcpServer.serve(stdio()).await?.waiting().await?;
    Ok(())
}
