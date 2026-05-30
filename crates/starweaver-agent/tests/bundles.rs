#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    attach_environment, filesystem_tools, host_operation_tools, namespaced_toolset, shell_tools,
    task_tools, tool_proxy_toolset, AgentContext, AgentSession, ToolContext, ToolRegistry,
};
use starweaver_core::{ConversationId, Metadata, RunId, Usage};
use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};
use starweaver_model::{tool_call_response, ModelResponse};

#[tokio::test]
async fn filesystem_and_shell_bundles_execute_against_virtual_environment() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file("README.md", "hello")
            .with_file("src/lib.rs", "pub fn hello() {}\n")
            .with_file("src/main.rs", "fn main() { hello(); }\n")
            .with_shell_output(
                "echo ok",
                ShellOutput {
                    status: 0,
                    stdout: "ok\n".to_string(),
                    stderr: String::new(),
                    metadata: Metadata::default(),
                },
            ),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    registry.insert_toolset(&shell_tools());
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context.clone());
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let read = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "read".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "README.md"}),
            },
        )
        .await;
    let write = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "write".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"path": "docs/output.txt", "content": "pub fn ok() {}"}),
            },
        )
        .await;
    let glob = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "glob".to_string(),
                name: "glob".to_string(),
                arguments: serde_json::json!({"path": "", "pattern": "*.rs"}),
            },
        )
        .await;
    let grep = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "grep".to_string(),
                name: "grep".to_string(),
                arguments: serde_json::json!({"path": "", "pattern": "hello", "include": "**/*.rs"}),
            },
        )
        .await;
    let resource = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "resource".to_string(),
                name: "resource_ref".to_string(),
                arguments: serde_json::json!({"path": "README.md"}),
            },
        )
        .await;
    let shell = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "shell".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "echo ok"}),
            },
        )
        .await;
    let background = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "background".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "sleep 1", "background": true}),
            },
        )
        .await;

    assert_eq!(read.content["content"], "hello");
    assert_eq!(write.content["written"], true);
    assert_eq!(glob.content["matches"].as_array().unwrap().len(), 2);
    assert_eq!(grep.content["matches"].as_array().unwrap().len(), 2);
    assert_eq!(resource.content["uri"], "env://test/README.md");
    assert_eq!(shell.content["stdout"], "ok\n");
    assert_eq!(background.content["status"], "pending");
}

#[tokio::test]
async fn task_bundle_creates_operation_envelopes() {
    let toolset = task_tools();
    let task = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_create")
        .unwrap()
        .call(
            ToolContext::new(RunId::default(), ConversationId::default(), 7),
            serde_json::json!({"subject": "ship", "description": "Ship the release"}),
        )
        .await
        .unwrap();

    assert_eq!(task.content["operation"], "task_create");
    assert_eq!(task.content["payload"]["subject"], "ship");
    assert_eq!(task.content["payload"]["description"], "Ship the release");
}

#[tokio::test]
async fn first_party_bundles_can_be_registered_on_agent_builder() {
    let provider =
        Arc::new(VirtualEnvironmentProvider::new("test").with_file("README.md", "bundle result"));
    let responses = vec![
        tool_call_response("call-1", "view", serde_json::json!({"path": "README.md"})),
        ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text("done")
        },
    ];
    let mut session = AgentSession::new(
        starweaver_agent::AgentBuilder::new(Arc::new(starweaver_agent::TestModel::with_responses(
            responses,
        )))
        .toolset(&filesystem_tools())
        .build(),
    )
    .with_environment(provider);

    let result = session.run("read file").await.unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(session.context().usage.tool_calls, 1);
}

#[test]
fn bundle_toolsets_export_stable_tool_names_and_instructions() {
    let filesystem = filesystem_tools();
    let shell = shell_tools();
    let task = task_tools();
    let host = host_operation_tools();

    assert_eq!(filesystem.name(), "filesystem");
    assert_eq!(shell.name(), "shell");
    assert_eq!(task.name(), "task");
    assert_eq!(host.name(), "host_operations");

    assert_tool_names(
        &filesystem,
        &[
            "view",
            "ls",
            "write",
            "edit",
            "multi_edit",
            "glob",
            "grep",
            "mkdir",
            "delete",
            "move",
            "copy",
            "resource_ref",
        ],
    );
    assert_tool_names(
        &shell,
        &[
            "shell_exec",
            "shell_wait",
            "shell_status",
            "shell_input",
            "shell_signal",
            "shell_kill",
        ],
    );
    assert_tool_names(
        &task,
        &["task_create", "task_get", "task_update", "task_list"],
    );
    assert_tool_names(
        &host,
        &[
            "search",
            "search_stock_image",
            "search_image",
            "fetch",
            "scrape",
            "download",
            "pdf_convert",
            "office_to_markdown",
            "read_image",
            "read_video",
            "read_audio",
            "load_media_url",
            "summarize",
            "note",
            "note_get",
            "thinking",
            "to_do_read",
            "to_do_write",
        ],
    );

    assert_eq!(filesystem.get_instructions().len(), 1);
    assert_eq!(shell.get_instructions().len(), 1);
    assert_eq!(task.get_instructions().len(), 1);
    assert_eq!(host.get_instructions().len(), 1);
}

#[tokio::test]
async fn tool_proxy_searches_and_calls_namespaced_toolsets() {
    let provider =
        Arc::new(VirtualEnvironmentProvider::new("test").with_file("README.md", "proxied content"));
    let filesystem = filesystem_tools();
    let namespaced = namespaced_toolset("workspace", filesystem.clone());
    let proxy = tool_proxy_toolset(vec![namespaced.clone()]);
    let proxy_tools = proxy.get_tools();

    assert_eq!(proxy.name(), "tool_proxy");
    assert_tool_names(&proxy, &["search_tools", "call_tool"]);

    let prefixed_proxy = namespaced_toolset("remote", proxy.clone());
    assert_tool_names(
        &prefixed_proxy,
        &["remote_search_tools", "remote_call_tool"],
    );

    assert_eq!(proxy.get_instructions()[0].group, "tool-proxy");
    assert_eq!(namespaced.name(), "workspace_filesystem");
    assert!(namespaced
        .get_tools()
        .iter()
        .any(|tool| tool.name() == "workspace_view"));

    let search_tools = proxy_tools
        .iter()
        .find(|tool| tool.name() == "search_tools")
        .unwrap();
    let search_result = search_tools
        .call(
            ToolContext::new(RunId::default(), ConversationId::default(), 0),
            serde_json::json!({"query": "view"}),
        )
        .await
        .unwrap();
    let search_xml = search_result.content["content"].as_str().unwrap();

    assert!(search_xml.contains("<search-results"));
    assert!(search_xml.contains("workspace_view"));
    assert!(search_xml.contains("<parameters>"));

    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let call_tool = proxy_tools
        .iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();
    let call_result = call_tool
        .call(
            ToolContext::new(RunId::default(), ConversationId::default(), 0)
                .with_dependencies(dependencies),
            serde_json::json!({
                "name": "workspace_view",
                "arguments": {"path": "README.md"},
            }),
        )
        .await
        .unwrap();

    assert_eq!(call_result.content["content"], "proxied content");
}

fn assert_tool_names(toolset: &starweaver_tools::DynToolset, expected: &[&str]) {
    let actual = toolset
        .get_tools()
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect::<Vec<_>>();
    for name in expected {
        assert!(
            actual.iter().any(|actual| actual == name),
            "missing tool {name}"
        );
    }
}
