#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentContext, ConversationId, ProcessShellHandle, RunId, ShellProcessSnapshot,
    ShellProcessStatus, ToolContext, ToolRegistry, attach_environment,
};
use starweaver_core::Metadata;
use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};

#[tokio::test]
async fn process_shell_tools_use_process_capable_provider() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("process").with_shell_output(
            "echo ok",
            ShellOutput {
                status: 0,
                stdout: "ok\n".to_string(),
                stderr: String::new(),
                metadata: Metadata::default(),
            },
        ),
    );
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider);
    assert!(
        agent_context
            .dependencies
            .get::<ProcessShellHandle>()
            .is_some()
    );
    let dependencies = agent_context.tool_dependency_store();
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);
    let mut tools = ToolRegistry::new();
    tools.insert_toolset(&starweaver_agent::shell_tools());

    let started = tools
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "start".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "sleep 1", "background": true}).into(),
            },
        )
        .await;
    let waited = tools
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "wait".to_string(),
                name: "shell_wait".to_string(),
                arguments: serde_json::json!({"process_id": started.content["process_id"].as_str().unwrap(), "timeout_seconds": 0}).into(),
            },
        )
        .await;

    assert_eq!(started.content["status"], "running");
    assert_eq!(waited.content["process_id"], started.content["process_id"]);
}

#[test]
fn process_shell_snapshot_status_serializes_as_snake_case() {
    let snapshot = ShellProcessSnapshot {
        process_id: "process_1".to_string(),
        command: "sleep 1".to_string(),
        status: ShellProcessStatus::Running,
        stdout: String::new(),
        stderr: String::new(),
        return_code: None,
        metadata: Metadata::default(),
    };

    assert_eq!(serde_json::to_value(snapshot).unwrap()["status"], "running");
}
