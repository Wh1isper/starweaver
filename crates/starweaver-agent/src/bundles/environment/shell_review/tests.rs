#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequestPart, ModelResponse, OutputMode, TestModel, ToolCallPart,
};
use starweaver_tools::{ToolContext, ToolError};

use super::*;

fn review_tool_context(handle: &ShellReviewHandle, tool_call_id: &str) -> ToolContext {
    let mut agent_context = AgentContext::default();
    attach_shell_review_handle(&mut agent_context, handle.clone());
    let dependencies = agent_context.tool_dependency_store();
    let mut context = ToolContext::new(
        RunId::from_string("run_shell_review"),
        ConversationId::from_string("conversation_shell_review"),
        0,
    )
    .with_dependencies(dependencies);
    context
        .metadata
        .insert("tool_call_id".to_string(), serde_json::json!(tool_call_id));
    context
}

fn review_snapshot() -> ShellReviewContextSnapshot {
    ShellReviewContextSnapshot {
        timeout_seconds: Some(30),
        tool_call_id: None,
        tool_call_approved: false,
        default_cwd: Some("/workspace".to_string()),
        allowed_paths: vec!["/workspace".to_string()],
        shell_platform: Some("linux".to_string()),
        shell_executable: Some("/bin/bash".to_string()),
    }
}

fn user_prompt_text(messages: &[ModelMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().rev().find_map(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => {
                content.iter().find_map(|part| match part {
                    ContentPart::Text { text } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        }),
        ModelMessage::Response(_) => None,
    })
}

#[test]
fn prompt_includes_context_and_previous_reviews_without_tool_call_id() {
    let request = ShellReviewRequest {
        command: "cargo test -p starweaver-agent".to_string(),
        cwd: Some("crates/starweaver-agent".to_string()),
        background: true,
        environment_keys: vec!["RUST_LOG".to_string(), "CI".to_string()],
        context_snapshot: Some(ShellReviewContextSnapshot {
            timeout_seconds: Some(120),
            tool_call_id: Some("call-secret".to_string()),
            tool_call_approved: true,
            default_cwd: Some("/workspace".to_string()),
            allowed_paths: vec!["/workspace".to_string(), "/tmp/shared".to_string()],
            shell_platform: Some("linux".to_string()),
            shell_executable: Some("/bin/bash".to_string()),
        }),
        previous_reviews: vec![ShellReviewPreviousDecision {
            approved: true,
            risk_level: ShellReviewRiskLevel::Medium,
            reason: "bounded workspace-local write".to_string(),
            command: Some("cargo test".to_string()),
            cwd: Some("crates/starweaver-agent".to_string()),
        }],
    };

    let prompt = request.to_prompt();

    assert!(prompt.contains("<command>\ncargo test -p starweaver-agent\n</command>"));
    assert!(prompt.contains("cwd: crates/starweaver-agent"));
    assert!(prompt.contains("background: True"));
    assert!(prompt.contains("environment_keys: ['RUST_LOG', 'CI']"));
    assert!(prompt.contains("timeout_seconds: 120"));
    assert!(prompt.contains("tool_call_approved: True"));
    assert!(prompt.contains("default_cwd: /workspace"));
    assert!(prompt.contains("allowed_paths: ['/workspace', '/tmp/shared']"));
    assert!(prompt.contains("shell_platform: linux"));
    assert!(prompt.contains("shell_executable: /bin/bash"));
    assert!(prompt.contains("<previous_shell_reviews>"));
    assert!(prompt.contains("approved: True"));
    assert!(prompt.contains("risk_level: medium"));
    assert!(!prompt.contains("tool_call_id:"));
    assert!(!prompt.contains("call-secret"));

    let metadata = request.to_approval_metadata(&ShellReviewDecision {
        risk_level: ShellReviewRiskLevel::High,
        reason: "needs approval".to_string(),
    });
    assert_eq!(metadata["reviewer"], "shell_command_reviewer");
    assert_eq!(metadata["command"], "cargo test -p starweaver-agent");
    assert_eq!(metadata["context"]["tool_call_id"], "call-secret");
    assert_eq!(metadata["context"]["tool_call_approved"], true);
    assert_eq!(metadata["previous_shell_reviews"][0]["approved"], true);
}

#[test]
fn review_decision_threshold_respects_defer_and_deny_actions() {
    let high = ShellReviewDecision {
        risk_level: ShellReviewRiskLevel::High,
        reason: "broad change".to_string(),
    };
    let medium = ShellReviewDecision {
        risk_level: ShellReviewRiskLevel::Medium,
        reason: "bounded change".to_string(),
    };
    let defer_config = ShellReviewConfig::disabled()
        .with_action(ShellReviewAction::Defer)
        .with_risk_threshold(ShellReviewRiskLevel::High);
    let defer_config = ShellReviewConfig {
        enabled: true,
        ..defer_config
    };
    let deny_config = defer_config.clone().with_action(ShellReviewAction::Deny);

    assert!(high.requires_approval(&defer_config));
    assert!(high.requires_defer(&defer_config));
    assert!(!high.requires_deny(&defer_config));
    assert!(high.requires_deny(&deny_config));
    assert!(!medium.requires_approval(&defer_config));
}

#[tokio::test]
async fn review_defer_records_previous_reviews_and_metadata() {
    let review_model = TestModel::with_responses(vec![
        ModelResponse::text(r#"{"risk_level":"low","reason":"read-only verification"}"#),
        ModelResponse::text(
            "```json\n{\"risk_level\":\"high\",\"reason\":\"writes outside workspace\"}\n```",
        ),
    ]);
    let handle = ShellReviewHandle::new(ShellReviewConfig::enabled(Arc::new(review_model.clone())));

    let first = review_shell_command_or_block(
        &review_tool_context(&handle, "call-1"),
        "cargo test -p starweaver-agent",
        Some("crates/starweaver-agent"),
        false,
        vec!["RUST_LOG".to_string()],
        30,
        review_snapshot(),
    )
    .await
    .unwrap();
    assert!(first.is_none());
    assert_eq!(handle.records().len(), 1);
    assert!(handle.records()[0].approved);

    let error = review_shell_command_or_block(
        &review_tool_context(&handle, "call-2"),
        "cargo test -p starweaver-agent",
        Some("crates/starweaver-agent"),
        false,
        vec!["RUST_LOG".to_string()],
        30,
        review_snapshot(),
    )
    .await
    .unwrap_err();

    let ToolError::ApprovalRequired { tool, metadata } = error else {
        panic!("expected approval required error");
    };
    assert_eq!(tool, "shell_exec");
    assert_eq!(metadata["reviewer"], "shell_command_reviewer");
    assert_eq!(metadata["risk_level"], "high");
    assert_eq!(metadata["context"]["tool_call_id"], "call-2");
    assert_eq!(metadata["context"]["timeout_seconds"], 30);
    assert_eq!(metadata["previous_shell_reviews"][0]["approved"], true);
    assert_eq!(metadata["previous_shell_reviews"][0]["risk_level"], "low");

    let captured_messages = review_model.captured_messages();
    assert_eq!(captured_messages.len(), 2);
    let prompt = user_prompt_text(&captured_messages[1]).unwrap();
    assert!(prompt.contains("<previous_shell_reviews>"));
    assert!(prompt.contains("approved: True"));
    assert!(prompt.contains("risk_level: low"));
    assert!(!prompt.contains("tool_call_id:"));
    assert!(!prompt.contains("call-2"));

    let captured_params = review_model.captured_params();
    assert_eq!(captured_params.len(), 2);
    assert!(captured_params[1].output_schema.is_some());
    assert_eq!(captured_params[1].output_mode, Some(OutputMode::Prompted));
}

#[tokio::test]
async fn review_uses_streaming_model_request() {
    let review_model = TestModel::with_stream_events(vec![vec![
        starweaver_model::ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text(
            r#"{"risk_level":"low","reason":"streamed decision"}"#,
        ))),
    ]]);
    let handle = ShellReviewHandle::new(ShellReviewConfig::enabled(Arc::new(review_model.clone())));

    let result = review_shell_command_or_block(
        &review_tool_context(&handle, "call-stream-review"),
        "git status --short",
        Some("/workspace"),
        false,
        Vec::new(),
        30,
        review_snapshot(),
    )
    .await
    .unwrap();

    assert!(result.is_none());
    assert_eq!(handle.records().len(), 1);
    assert!(handle.records()[0].approved);
    assert_eq!(handle.records()[0].decision.reason, "streamed decision");
    assert_eq!(review_model.captured_messages().len(), 1);
}

#[tokio::test]
async fn review_deny_returns_structured_shell_result() {
    let review_model = TestModel::with_json(&serde_json::json!({
        "risk_level": "medium",
        "reason": "bounded generated files"
    }));
    let handle = ShellReviewHandle::new(
        ShellReviewConfig::enabled(Arc::new(review_model))
            .with_action(ShellReviewAction::Deny)
            .with_risk_threshold(ShellReviewRiskLevel::Medium),
    );

    let blocked = review_shell_command_or_block(
        &review_tool_context(&handle, "call-deny"),
        "python -m compileall .",
        None,
        false,
        Vec::new(),
        30,
        ShellReviewContextSnapshot::default(),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(blocked.content["return_code"], 1);
    assert_eq!(blocked.content["stdout"], "");
    assert_eq!(blocked.content["stderr"], "");
    assert!(
        blocked.content["error"]
            .as_str()
            .unwrap()
            .contains("Shell command blocked by review")
    );
    assert_eq!(blocked.content["shell_review"]["risk_level"], "medium");
}

#[tokio::test]
async fn shell_exec_runs_review_before_provider_execution() {
    use starweaver_core::Metadata;
    use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};
    use starweaver_tools::ToolRegistry;

    let provider = Arc::new(VirtualEnvironmentProvider::new("test").with_shell_output(
        "echo ok",
        ShellOutput {
            status: 0,
            stdout: "ok\n".to_string(),
            stderr: String::new(),
            metadata: Metadata::default(),
        },
    ));
    let review_model = TestModel::with_responses(vec![
        ModelResponse::text(r#"{"risk_level":"high","reason":"dangerous"}"#),
        ModelResponse::text(r#"{"risk_level":"low","reason":"read-only"}"#),
    ]);
    let handle = ShellReviewHandle::new(
        ShellReviewConfig::enabled(Arc::new(review_model))
            .with_action(ShellReviewAction::Deny)
            .with_risk_threshold(ShellReviewRiskLevel::High),
    );
    let mut agent_context = AgentContext::default();
    crate::bundles::environment::attach_environment(&mut agent_context, provider);
    attach_shell_review_handle(&mut agent_context, handle);
    let dependencies = agent_context.tool_dependency_store();
    let context = ToolContext::new(
        RunId::from_string("run_shell_exec_review"),
        ConversationId::from_string("conversation_shell_exec_review"),
        0,
    )
    .with_dependencies(dependencies);
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&crate::bundles::environment::shell_tools());

    let denied = registry
        .execute_call(
            context.clone(),
            &ToolCallPart {
                id: "call-denied".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "echo missing"}).into(),
            },
        )
        .await;
    assert!(!denied.is_error);
    assert_eq!(denied.content["return_code"], 1);
    assert!(
        denied.content["error"]
            .as_str()
            .unwrap()
            .contains("Shell command blocked by review")
    );

    let allowed = registry
        .execute_call(
            context,
            &ToolCallPart {
                id: "call-allowed".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "echo ok"}).into(),
            },
        )
        .await;
    assert!(!allowed.is_error);
    assert_eq!(allowed.content["stdout"], "ok\n");
}

#[tokio::test]
async fn approved_tool_call_bypasses_reviewer_and_marks_history_approved() {
    let review_model = TestModel::with_json(&serde_json::json!({
        "risk_level": "high",
        "reason": "needs approval"
    }));
    let handle = ShellReviewHandle::new(ShellReviewConfig::enabled(Arc::new(review_model.clone())));
    let context = review_tool_context(&handle, "call-approved");

    let error = review_shell_command_or_block(
        &context,
        "rm -rf target",
        None,
        false,
        Vec::new(),
        30,
        review_snapshot(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ToolError::ApprovalRequired { .. }));
    assert_eq!(review_model.captured_messages().len(), 1);
    assert!(!handle.records()[0].approved);

    let mut approved_context = review_tool_context(&handle, "call-approved");
    approved_context.approve();
    let allowed = review_shell_command_or_block(
        &approved_context,
        "rm -rf target",
        None,
        false,
        Vec::new(),
        30,
        review_snapshot(),
    )
    .await
    .unwrap();

    assert!(allowed.is_none());
    assert_eq!(review_model.captured_messages().len(), 1);
    assert!(handle.records()[0].approved);
}
