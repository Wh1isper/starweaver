#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use starweaver_agent::{
    AgentCapability, AgentContext, AgentRuntimeBuilder, AgentSession, EnvironmentContextCapability,
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse, ScrapeRequest, ScrapeResponse,
    SearchRequest, SearchResponse, SearchResultItem, ToolContext, ToolRegistry, ToolResult,
    attach_environment, context_tools, dynamic_tool_proxy, filesystem_tools, host_io_tools,
    json_tool, namespaced_toolset, shell_tools, task_tools,
};
use starweaver_context::{AgentContextHandle, DependencyStore, ToolConfig};
use starweaver_core::{CancellationToken, ConversationId, Metadata, RunId};
use starweaver_environment::{
    EnvironmentPolicy, EnvironmentProvider, FilePolicy, LocalEnvironmentProvider,
    ProcessShellProvider, ShellOutput, ShellPolicy, ShellProcessStatus, VirtualEnvironmentProvider,
};
use starweaver_model::{
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_METADATA, ContentPart, ModelMessage,
    ModelRequest, ModelRequestPart, ModelResponse, TestModel, tool_call_response,
};
use starweaver_usage::Usage;

#[tokio::test]
async fn environment_context_capability_injects_provider_context_as_user_prompt() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file("README.md", "hello")
            .with_file("src/lib.rs", "pub fn hello() {}\n"),
    );
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider);
    let request = ModelRequest::user_text("inspect workspace");
    let mut state =
        starweaver_agent::AgentRunState::new(RunId::default(), ConversationId::default());

    let messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![starweaver_model::ModelMessage::Request(request)],
        )
        .await
        .unwrap();
    let starweaver_model::ModelMessage::Request(request) = messages.last().unwrap() else {
        panic!("expected request");
    };

    assert!(matches!(
        request.parts.first(),
        Some(ModelRequestPart::UserPrompt { content, metadata, .. })
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT))
                && matches!(&content[0], ContentPart::Text { text } if text.contains("<environment-context>"))
    ));
    assert!(matches!(
        request.parts.get(1),
        Some(ModelRequestPart::UserPrompt { content, metadata, .. })
            if metadata.get(CONTEXT_ORIGIN_METADATA).is_none()
                && matches!(&content[0], ContentPart::Text { text } if text == "inspect workspace")
    ));
}

#[tokio::test]
async fn environment_context_capability_skips_unchanged_context_on_later_turn() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file("README.md", "hello")
            .with_file("src/lib.rs", "pub fn hello() {}\n"),
    );
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider);
    let mut state =
        starweaver_agent::AgentRunState::new(RunId::default(), ConversationId::default());

    let mut messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![starweaver_model::ModelMessage::Request(
                ModelRequest::user_text("inspect workspace"),
            )],
        )
        .await
        .unwrap();
    messages.push(starweaver_model::ModelMessage::Response(
        ModelResponse::text("ok"),
    ));
    messages.push(starweaver_model::ModelMessage::Request(
        ModelRequest::user_text("continue"),
    ));

    let messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(&mut state, &mut context, messages)
        .await
        .unwrap();

    assert_eq!(environment_context_part_count(&messages), 1);
    let starweaver_model::ModelMessage::Request(request) = messages.last().unwrap() else {
        panic!("expected latest request");
    };
    assert!(matches!(
        request.parts.first(),
        Some(ModelRequestPart::UserPrompt { content, metadata, .. })
            if metadata.get(CONTEXT_ORIGIN_METADATA).is_none()
                && matches!(&content[0], ContentPart::Text { text } if text == "continue")
    ));
}

#[tokio::test]
async fn environment_context_capability_force_reinjects_unchanged_context() {
    let provider =
        Arc::new(VirtualEnvironmentProvider::new("test").with_file("README.md", "hello"));
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider);
    let mut state =
        starweaver_agent::AgentRunState::new(RunId::default(), ConversationId::default());

    let mut messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![starweaver_model::ModelMessage::Request(
                ModelRequest::user_text("inspect workspace"),
            )],
        )
        .await
        .unwrap();
    context.force_inject_context = true;
    messages.push(starweaver_model::ModelMessage::Response(
        ModelResponse::text("ok"),
    ));
    messages.push(starweaver_model::ModelMessage::Request(
        ModelRequest::user_text("continue"),
    ));

    let messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(&mut state, &mut context, messages)
        .await
        .unwrap();

    assert_eq!(environment_context_part_count(&messages), 2);
}

#[tokio::test]
async fn environment_context_capability_keeps_initial_context_on_later_turn_even_if_environment_changes()
 {
    let provider =
        Arc::new(VirtualEnvironmentProvider::new("test").with_file("README.md", "hello"));
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider.clone());
    let mut state =
        starweaver_agent::AgentRunState::new(RunId::default(), ConversationId::default());

    let mut messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![starweaver_model::ModelMessage::Request(
                ModelRequest::user_text("inspect workspace"),
            )],
        )
        .await
        .unwrap();
    provider
        .write_text("src/lib.rs", "pub fn hello() {}\n")
        .await
        .unwrap();
    messages.push(starweaver_model::ModelMessage::Response(
        ModelResponse::text("ok"),
    ));
    messages.push(starweaver_model::ModelMessage::Request(
        ModelRequest::user_text("continue"),
    ));

    let messages = EnvironmentContextCapability
        .prepare_model_messages_with_context(&mut state, &mut context, messages)
        .await
        .unwrap();

    assert_eq!(environment_context_part_count(&messages), 1);
    let starweaver_model::ModelMessage::Request(request) = messages.last().unwrap() else {
        panic!("expected latest request");
    };
    assert!(matches!(
        request.parts.first(),
        Some(ModelRequestPart::UserPrompt { content, metadata, .. })
            if metadata.get(CONTEXT_ORIGIN_METADATA).is_none()
                && matches!(&content[0], ContentPart::Text { text } if text == "continue")
    ));
}

fn request_text_parts(message: &ModelMessage) -> Vec<String> {
    match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .flat_map(|part| match part {
                ModelRequestPart::UserPrompt { content, .. } => content
                    .iter()
                    .filter_map(|content| match content {
                        ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                ModelRequestPart::ToolReturn(tool_return) => vec![tool_return.content.to_string()],
                ModelRequestPart::SystemPrompt { text, .. }
                | ModelRequestPart::RetryPrompt { text, .. }
                | ModelRequestPart::Instruction { text, .. } => vec![text.clone()],
            })
            .collect(),
        ModelMessage::Response(response) => response
            .parts
            .iter()
            .filter_map(|part| match part {
                starweaver_model::ModelResponsePart::Text { text }
                | starweaver_model::ModelResponsePart::Thinking { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect(),
    }
}

fn environment_context_part_count(messages: &[starweaver_model::ModelMessage]) -> usize {
    messages
        .iter()
        .filter_map(|message| match message {
            starweaver_model::ModelMessage::Request(request) => Some(request),
            starweaver_model::ModelMessage::Response(_) => None,
        })
        .flat_map(|request| request.parts.iter())
        .filter(|part| {
            matches!(
                part,
                ModelRequestPart::UserPrompt { metadata, .. }
                    if metadata.get(CONTEXT_ORIGIN_METADATA)
                        == Some(&serde_json::json!(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT))
            )
        })
        .count()
}

async fn execute_tool_call(
    registry: &ToolRegistry,
    context: ToolContext,
    id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> starweaver_model::ToolReturnPart {
    registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.into(),
            },
        )
        .await
}

fn filesystem_shell_test_provider() -> Arc<VirtualEnvironmentProvider> {
    Arc::new(
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
            )
            .with_shell_output(
                "env-check",
                ShellOutput {
                    status: 0,
                    stdout: "ctx\noverride\n".to_string(),
                    stderr: String::new(),
                    metadata: Metadata::default(),
                },
            ),
    )
}

fn filesystem_shell_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    registry.insert_toolset(&shell_tools());
    registry
}

fn filesystem_shell_tool_context(provider: Arc<VirtualEnvironmentProvider>) -> ToolContext {
    let mut agent_context = AgentContext::default();
    agent_context
        .shell_env
        .insert("STARWEAVER_CONTEXT_ENV".to_string(), "ctx".to_string());
    agent_context
        .shell_env
        .insert("STARWEAVER_OVERRIDE_ENV".to_string(), "ctx".to_string());
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context.clone());
    ToolContext::new(RunId::default(), ConversationId::default(), 0).with_dependencies(dependencies)
}

fn process_tool_context(provider: Arc<VirtualEnvironmentProvider>) -> ToolContext {
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    ToolContext::new(RunId::default(), ConversationId::default(), 0).with_dependencies(dependencies)
}

struct FilesystemShellResults {
    read: starweaver_model::ToolReturnPart,
    write: starweaver_model::ToolReturnPart,
    glob: starweaver_model::ToolReturnPart,
    grep: starweaver_model::ToolReturnPart,
    resource: starweaver_model::ToolReturnPart,
    ignored_ls: starweaver_model::ToolReturnPart,
    default_ls: starweaver_model::ToolReturnPart,
    invalid_write_mode: starweaver_model::ToolReturnPart,
    edit_existing_create: starweaver_model::ToolReturnPart,
    multi_edit_create_then_replace: starweaver_model::ToolReturnPart,
    multi_edit_empty_later: starweaver_model::ToolReturnPart,
    shell: starweaver_model::ToolReturnPart,
    shell_with_env: starweaver_model::ToolReturnPart,
    empty_shell: starweaver_model::ToolReturnPart,
    invalid_grep_context: starweaver_model::ToolReturnPart,
    invalid_glob_multi_root: starweaver_model::ToolReturnPart,
    invalid_grep_multi_root: starweaver_model::ToolReturnPart,
}

async fn execute_filesystem_shell_calls(
    registry: &ToolRegistry,
    context: ToolContext,
) -> FilesystemShellResults {
    macro_rules! call {
        ($id:literal, $name:literal, $args:expr) => {
            execute_tool_call(registry, context.clone(), $id, $name, $args).await
        };
    }

    FilesystemShellResults {
        read: call!("read", "view", serde_json::json!({"path": "README.md"})),
        write: call!(
            "write",
            "write",
            serde_json::json!({"path": "docs/output.txt", "content": "pub fn ok() {}"})
        ),
        glob: call!(
            "glob",
            "glob",
            serde_json::json!({"path": "", "pattern": "*.rs"})
        ),
        grep: call!(
            "grep",
            "grep",
            serde_json::json!({"path": "", "pattern": "hello", "include": "**/*.rs"})
        ),
        resource: call!(
            "resource",
            "resource_ref",
            serde_json::json!({"path": "README.md"})
        ),
        ignored_ls: call!(
            "ignored-ls",
            "ls",
            serde_json::json!({"path": "", "ignore": ["src/main.rs"]})
        ),
        default_ls: call!("default-ls", "ls", serde_json::json!({})),
        invalid_write_mode: call!(
            "invalid-write-mode",
            "write",
            serde_json::json!({"path": "docs/output.txt", "content": "x", "mode": "bad"})
        ),
        edit_existing_create: call!(
            "edit-existing-create",
            "edit",
            serde_json::json!({"file_path": "README.md", "old_string": "", "new_string": "overwrite"})
        ),
        multi_edit_create_then_replace: call!(
            "multi-edit-create-then-replace",
            "multi_edit",
            serde_json::json!({"file_path": "created.txt", "edits": [
                {"old_string": "", "new_string": "Hello World"},
                {"old_string": "World", "new_string": "Universe"}
            ]})
        ),
        multi_edit_empty_later: call!(
            "multi-edit-empty-later",
            "multi_edit",
            serde_json::json!({"file_path": "README.md", "edits": [
                {"old_string": "hello", "new_string": "hi"},
                {"old_string": "", "new_string": "boom", "replace_all": true}
            ]})
        ),
        shell: call!(
            "shell",
            "shell_exec",
            serde_json::json!({"command": "echo ok"})
        ),
        shell_with_env: call!(
            "shell-env",
            "shell_exec",
            serde_json::json!({
                "command": "env-check",
                "environment": {"STARWEAVER_OVERRIDE_ENV": "override"},
            })
        ),
        empty_shell: call!(
            "empty-shell",
            "shell_exec",
            serde_json::json!({"command": "   "})
        ),
        invalid_grep_context: call!(
            "invalid-grep-context",
            "grep",
            serde_json::json!({"path": "", "pattern": "hello", "context_lines": -1})
        ),
        invalid_glob_multi_root: call!(
            "invalid-glob-multi-root",
            "glob",
            serde_json::json!({"root": "src\ndocs", "pattern": "*.rs"})
        ),
        invalid_grep_multi_root: call!(
            "invalid-grep-multi-root",
            "grep",
            serde_json::json!({"root": "src\ndocs", "pattern": "hello", "include": "**/*.rs"})
        ),
    }
}

async fn assert_filesystem_shell_results(
    provider: &VirtualEnvironmentProvider,
    results: &FilesystemShellResults,
) {
    assert_eq!(results.read.content, serde_json::json!("hello"));
    assert_eq!(results.write.content["written"], true);
    assert_eq!(results.glob.content["matches"].as_array().unwrap().len(), 2);
    assert_eq!(results.grep.content["matches"].as_array().unwrap().len(), 2);
    assert_eq!(results.resource.content["uri"], "env://test/README.md");
    assert_eq!(
        results.ignored_ls.content["entries"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(
        results.ignored_ls.content["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.as_str() != Some("src/main.rs"))
    );
    assert_eq!(
        results.default_ls.content["entries"],
        serde_json::json!(["README.md", "docs/output.txt", "src/lib.rs", "src/main.rs"])
    );
    assert!(!results.invalid_write_mode.is_error);
    assert_eq!(results.invalid_write_mode.content["kind"], "feedback");
    assert!(
        results.invalid_write_mode.content["message"]
            .as_str()
            .unwrap()
            .contains("unsupported write mode")
    );
    assert!(
        results.invalid_write_mode.content["how_to_fix"]
            .as_str()
            .unwrap()
            .contains("next step")
    );
    assert!(!results.edit_existing_create.is_error);
    assert_eq!(results.edit_existing_create.content["kind"], "feedback");
    assert!(
        results.edit_existing_create.content["message"]
            .as_str()
            .unwrap()
            .contains("file already exists")
    );
    assert!(
        results.edit_existing_create.content["how_to_fix"]
            .as_str()
            .unwrap()
            .contains("next step")
    );
    assert!(!results.multi_edit_create_then_replace.is_error);
    assert_eq!(
        provider.read_text("created.txt").await.unwrap(),
        "Hello Universe"
    );
    assert!(!results.multi_edit_empty_later.is_error);
    assert_eq!(results.multi_edit_empty_later.content["kind"], "feedback");
    assert!(
        results.multi_edit_empty_later.content["message"]
            .as_str()
            .unwrap()
            .contains("old_string must be non-empty")
    );
    assert_shell_results(results);
    assert_invalid_search_roots(results);
}

fn assert_shell_results(results: &FilesystemShellResults) {
    assert_eq!(results.shell.content["stdout"], "ok\n");
    assert_eq!(
        results.shell_with_env.content["environment"]["STARWEAVER_CONTEXT_ENV"],
        "ctx"
    );
    assert_eq!(
        results.shell_with_env.content["environment"]["STARWEAVER_OVERRIDE_ENV"],
        "override"
    );
    assert_eq!(results.shell_with_env.content["stdout"], "ctx\noverride\n");
    assert!(!results.empty_shell.is_error);
    assert_eq!(results.empty_shell.content["kind"], "feedback");
    assert!(
        results.empty_shell.content["message"]
            .as_str()
            .unwrap()
            .contains("must not be empty")
    );
    assert!(results.empty_shell.content["retryable"].as_bool().unwrap());
    assert!(!results.invalid_grep_context.is_error);
    assert_eq!(results.invalid_grep_context.content["kind"], "feedback");
    assert!(
        results.invalid_grep_context.content["message"]
            .as_str()
            .unwrap()
            .contains("context_lines must be greater than or equal to 0")
    );
}

fn assert_invalid_search_roots(results: &FilesystemShellResults) {
    assert!(!results.invalid_glob_multi_root.is_error);
    assert_eq!(results.invalid_glob_multi_root.content["kind"], "feedback");
    let invalid_glob_root_message = results.invalid_glob_multi_root.content["message"]
        .as_str()
        .unwrap();
    assert!(invalid_glob_root_message.contains("root must be a single directory path"));
    assert!(invalid_glob_root_message.contains("parallel tool calls"));
    assert!(!invalid_glob_root_message.contains("src\ndocs"));
    assert!(!results.invalid_grep_multi_root.is_error);
    assert_eq!(results.invalid_grep_multi_root.content["kind"], "feedback");
    let invalid_grep_root_message = results.invalid_grep_multi_root.content["message"]
        .as_str()
        .unwrap();
    assert!(invalid_grep_root_message.contains("root must be a single directory path"));
    assert!(invalid_grep_root_message.contains("parallel tool calls"));
    assert!(!invalid_grep_root_message.contains("src\ndocs"));
}

async fn assert_background_shell(
    registry: &ToolRegistry,
    provider: Arc<VirtualEnvironmentProvider>,
) {
    let background_shell = execute_tool_call(
        registry,
        process_tool_context(provider),
        "background-shell",
        "shell_exec",
        serde_json::json!({
            "command": "sleep 1",
            "background": true,
            "cwd": "src",
            "timeout_seconds": 42,
            "environment": {"STARWEAVER_BACKGROUND": "yes"},
        }),
    )
    .await;
    assert_eq!(background_shell.content["command"], "sleep 1");
    assert_eq!(background_shell.content["metadata"]["cwd"], "src");
    assert_eq!(background_shell.content["metadata"]["timeout_seconds"], 42);
    assert_eq!(
        background_shell.content["metadata"]["environment"]["STARWEAVER_BACKGROUND"],
        "yes"
    );
}

#[tokio::test]
async fn filesystem_and_shell_bundles_execute_against_virtual_environment() {
    let provider = filesystem_shell_test_provider();
    let registry = filesystem_shell_registry();
    let context = filesystem_shell_tool_context(provider.clone());

    let results = execute_filesystem_shell_calls(&registry, context).await;
    assert_filesystem_shell_results(&provider, &results).await;
    assert_background_shell(&registry, provider).await;
}

#[tokio::test]
async fn filesystem_mutation_tools_execute_through_environment_provider() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file("README.md", "hello")
            .with_file("src/lib.rs", "pub fn hello() {}\n"),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider.clone());
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let mkdir = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "mkdir".to_string(),
                name: "mkdir".to_string(),
                arguments: serde_json::json!({"paths": ["docs/generated"], "parents": true}).into(),
            },
        )
        .await;
    assert!(!mkdir.is_error, "mkdir failed: {:?}", mkdir.content);
    assert!(provider.stat("docs/generated").await.unwrap().is_dir);

    let copy = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "copy".to_string(),
                name: "copy".to_string(),
                arguments: serde_json::json!({
                    "pairs": [{"src": "README.md", "dst": "docs/generated/readme-copy.md"}],
                })
                .into(),
            },
        )
        .await;
    assert!(!copy.is_error, "copy failed: {:?}", copy.content);
    assert_eq!(
        provider
            .read_text("docs/generated/readme-copy.md")
            .await
            .unwrap(),
        "hello"
    );

    let move_result = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "move".to_string(),
                name: "move".to_string(),
                arguments: serde_json::json!({
                    "pairs": [{"src": "docs/generated/readme-copy.md", "dst": "docs/generated/readme-moved.md"}],
                })
                .into(),
            },
        )
        .await;
    assert!(
        !move_result.is_error,
        "move failed: {:?}",
        move_result.content
    );
    assert!(
        provider
            .read_text("docs/generated/readme-copy.md")
            .await
            .is_err()
    );
    assert_eq!(
        provider
            .read_text("docs/generated/readme-moved.md")
            .await
            .unwrap(),
        "hello"
    );

    let delete = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "delete".to_string(),
                name: "delete".to_string(),
                arguments: serde_json::json!({"paths": ["docs"], "recursive": true}).into(),
            },
        )
        .await;
    assert!(!delete.is_error, "delete failed: {:?}", delete.content);
    assert!(provider.stat("docs").await.is_err());
}

#[tokio::test]
async fn filesystem_view_handles_text_metadata_binary_and_local_media() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file("paged.txt", "line 1\nline 2\n")
            .with_file("long.txt", "abcdef\n")
            .with_bytes("binary.dat", vec![b'a', 0, b'b'])
            .with_bytes("image.png", b"\x89PNG\r\n\x1a\nsmall".to_vec()),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider.clone());
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context.clone());
    dependencies.insert(HostMediaUnderstandingClientHandle::new(Arc::new(
        FakeMediaUnderstandingClient,
    )));
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies.clone());

    let paged = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "paged".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "paged.txt", "line_limit": 1}).into(),
            },
        )
        .await;
    assert_eq!(paged.content["content"], "line 1\n");
    assert_eq!(
        paged.content["metadata"]["current_segment"]["has_more_content"],
        true
    );

    let long = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "long".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "long.txt", "max_line_length": 3}).into(),
            },
        )
        .await;
    assert!(
        long.content["content"]
            .as_str()
            .unwrap()
            .contains("... (line truncated)")
    );
    assert_eq!(
        long.content["metadata"]["truncation_info"]["lines_truncated"],
        true
    );

    let binary = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "binary".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "binary.dat"}).into(),
            },
        )
        .await;
    assert!(!binary.is_error);
    assert_eq!(binary.content["kind"], "feedback");
    assert!(
        binary.content["message"]
            .as_str()
            .unwrap()
            .contains("appears to be a binary file")
    );
    assert!(
        binary.content["message"]
            .as_str()
            .unwrap()
            .contains("file-specific tool")
    );

    let image = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "image".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "image.png", "instructions": "describe it"})
                    .into(),
            },
        )
        .await;
    assert_eq!(image.content["content"], "image analysis");
    assert!(
        image.content["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
}

#[tokio::test]
async fn filesystem_view_uses_relaxed_text_limits_for_configured_paths() {
    let mut many_lines = String::new();
    for line in 1..=350 {
        many_lines.push_str("line ");
        many_lines.push_str(&line.to_string());
        many_lines.push('\n');
    }
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file("AGENTS.md", many_lines.clone())
            .with_file("nested/AGENTS.md", many_lines.clone())
            .with_bytes("binary.md", vec![b'a', 0, b'b']),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    let mut agent_context = AgentContext::default();
    agent_context.tool_config.view_relaxed_text_patterns =
        vec!["/AGENTS.md".to_string(), "re:^binary\\.md$".to_string()];
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let relaxed = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "relaxed".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "AGENTS.md"}).into(),
            },
        )
        .await;
    assert!(
        !relaxed.is_error,
        "relaxed read failed: {:?}",
        relaxed.content
    );
    assert!(relaxed.content.as_str().unwrap().contains("line 350"));

    let explicit_limit = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "explicit".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "AGENTS.md", "line_limit": 10}).into(),
            },
        )
        .await;
    assert_eq!(
        explicit_limit.content["metadata"]["reading_parameters"]["line_limit"],
        10
    );
    assert_eq!(
        explicit_limit.content["metadata"]["current_segment"]["end_line"],
        10
    );

    let nested_normal = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "nested".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "nested/AGENTS.md"}).into(),
            },
        )
        .await;
    assert_eq!(
        nested_normal.content["metadata"]["current_segment"]["end_line"],
        300
    );

    let binary = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "binary".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "binary.md"}).into(),
            },
        )
        .await;
    assert!(!binary.is_error);
    assert_eq!(binary.content["kind"], "feedback");
    assert!(
        binary.content["message"]
            .as_str()
            .unwrap()
            .contains("appears to be a binary file")
    );
}

#[tokio::test]
async fn skill_registry_registers_markdown_relaxed_view_patterns() {
    let mut skill_readme = String::new();
    for line in 1..=350 {
        skill_readme.push_str("readme ");
        skill_readme.push_str(&line.to_string());
        skill_readme.push('\n');
    }
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_file(
                "skills/research/SKILL.md",
                "---\nname: research\ndescription: Research workflow\n---\nbody",
            )
            .with_file("skills/research/README.md", skill_readme),
    );
    let mut skills = starweaver_agent::SkillRegistry::new();
    skills.insert(starweaver_agent::SkillPackage {
        name: "research".to_string(),
        description: "Research workflow".to_string(),
        path: "skills/research/SKILL.md".to_string(),
        body: None,
        metadata: Metadata::default(),
    });
    let mut agent_context = AgentContext::default();
    skills.register_relaxed_view_patterns(&mut agent_context);
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context.clone());
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());

    let view = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "skill-readme".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "skills/research/README.md"}).into(),
            },
        )
        .await;
    assert!(view.content.as_str().unwrap().contains("readme 350"));
}

#[tokio::test]
async fn filesystem_view_native_media_returns_provider_backed_content_parts() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_bytes("image.png", b"\x89PNG\r\n\x1a\nsmall".to_vec()),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    dependencies.insert(HostMediaCapabilities {
        model_id: Some("vision-model".to_string()),
        supports_image_url: true,
        supports_video_url: false,
        supports_audio_url: false,
        supports_document_url: false,
        supports_youtube_url: false,
    });
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let image = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "image".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "image.png", "instructions": "describe it"})
                    .into(),
            },
        )
        .await;

    assert_eq!(image.content["native_supported"], true);
    let parts = image.private_metadata["starweaver_tool_return_content_parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["kind"], "data_url");
    assert_eq!(parts[0]["media_type"], "image/png");
    assert!(
        image.private_metadata["starweaver_tool_return_prompt"]
            .as_str()
            .unwrap()
            .contains("describe it")
    );
}

#[tokio::test]
async fn glob_grep_ls_and_shell_large_outputs_are_bounded_or_saved_to_tmp_files() {
    let mut provider_value = VirtualEnvironmentProvider::new("test").with_shell_output(
        "big output",
        ShellOutput {
            status: 0,
            stdout: "o".repeat(25_000),
            stderr: "e".repeat(25_000),
            metadata: Metadata::default(),
        },
    );
    for index in 0..900 {
        provider_value = provider_value.with_file(
            format!("src/generated/very_long_file_name_{index:04}.rs"),
            format!("needle line {index}\n"),
        );
    }
    let provider = Arc::new(provider_value);
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    registry.insert_toolset(&shell_tools());
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider.clone());
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let ls = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "ls".to_string(),
                name: "ls".to_string(),
                arguments: serde_json::json!({"path": "", "ignore": ["very_long_file_name_0000"]})
                    .into(),
            },
        )
        .await;
    assert_eq!(ls.content["entries"].as_array().unwrap().len(), 500);
    assert_eq!(ls.content["truncated"], true);
    assert_eq!(ls.content["total_entries"], 899);
    assert_eq!(ls.content["showing"], 500);
    assert!(
        ls.content["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| !entry.as_str().unwrap().contains("very_long_file_name_0000"))
    );

    let glob = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "glob".to_string(),
                name: "glob".to_string(),
                arguments: serde_json::json!({"path": "", "pattern": "*.rs", "max_results": -1})
                    .into(),
            },
        )
        .await;
    let glob_path = glob.content["output_file_path"].as_str().unwrap();
    assert!(glob_path.starts_with(".starweaver/tmp/glob-"));
    assert!(
        provider
            .read_text(glob_path)
            .await
            .unwrap()
            .contains("very_long_file_name_0000")
    );

    let grep = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "grep".to_string(),
                name: "grep".to_string(),
                arguments: serde_json::json!({
                    "path": "",
                    "pattern": "needle",
                    "include": "**/*.rs",
                    "max_results": -1,
                    "max_matches_per_file": 1,
                    "max_files": -1,
                })
                .into(),
            },
        )
        .await;
    let grep_path = grep.content["output_file_path"].as_str().unwrap();
    assert!(grep_path.starts_with(".starweaver/tmp/grep-"));
    assert!(
        provider
            .read_text(grep_path)
            .await
            .unwrap()
            .contains("needle line")
    );

    let shell = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "shell".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "big output"}).into(),
            },
        )
        .await;
    let stdout_path = shell.content["stdout_file_path"].as_str().unwrap();
    let stderr_path = shell.content["stderr_file_path"].as_str().unwrap();
    assert!(stdout_path.starts_with(".starweaver/tmp/stdout-"));
    assert!(stderr_path.starts_with(".starweaver/tmp/stderr-"));
    assert_eq!(provider.read_text(stdout_path).await.unwrap().len(), 25_000);
    assert_eq!(provider.read_text(stderr_path).await.unwrap().len(), 25_000);
    assert!(
        shell.content["stdout"]
            .as_str()
            .unwrap()
            .contains("truncated")
    );
    assert!(
        shell.content["stderr"]
            .as_str()
            .unwrap()
            .contains("truncated")
    );
    let shell_result_path = shell.content["output_file_path"].as_str().unwrap();
    assert!(shell_result_path.starts_with(".starweaver/tmp/shell-exec-"));
    let full_shell_result = provider.read_text(shell_result_path).await.unwrap();
    assert!(full_shell_result.contains(stdout_path));
    assert!(full_shell_result.contains(stderr_path));
    assert!(serde_json::to_string(&shell.content).unwrap().len() <= 20_000);
}

#[tokio::test]
async fn host_io_large_outputs_are_bounded_and_saved_to_tmp_files() {
    let provider = Arc::new(VirtualEnvironmentProvider::new("test"));
    let toolset = host_io_tools();
    let search_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "search")
        .unwrap();
    let scrape_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "scrape")
        .unwrap();
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider.clone());
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    dependencies.insert(HostSearchClientHandle::new(Arc::new(LargeSearchClient)));
    dependencies.insert(HostScrapeClientHandle::new(Arc::new(LargeScrapeClient)));
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let search = search_tool
        .call(
            context.clone(),
            serde_json::json!({"query": "large", "num": 10}),
        )
        .await
        .unwrap();
    assert_eq!(search.content["truncated"], true);
    assert_eq!(search.content["results_total"], 8);
    assert!(search.content["results_showing"].as_u64().unwrap() < 8);
    let search_path = search.content["output_file_path"].as_str().unwrap();
    assert!(search_path.starts_with(".starweaver/tmp/search-"));
    assert!(
        provider
            .read_text(search_path)
            .await
            .unwrap()
            .contains("large snippet 7")
    );
    assert!(serde_json::to_string(&search.content).unwrap().len() <= 20_000);

    let scrape = scrape_tool
        .call(
            context,
            serde_json::json!({"url": "https://example.com/large"}),
        )
        .await
        .unwrap();
    assert_eq!(scrape.content["truncated"], true);
    let scrape_path = scrape.content["output_file_path"].as_str().unwrap();
    assert!(scrape_path.starts_with(".starweaver/tmp/scrape-"));
    assert_eq!(
        provider.read_text(scrape_path).await.unwrap(),
        large_markdown()
    );
    assert!(
        scrape.content["markdown_content"]
            .as_str()
            .unwrap()
            .contains("output_file_path")
    );
    assert!(serde_json::to_string(&scrape.content).unwrap().len() <= 20_000);
}

#[tokio::test]
async fn fetch_large_text_output_is_bounded_and_saved_to_tmp_file() {
    let large_text = format!("fetch-large-{}", "f".repeat(30_000));
    let expected = large_text.clone();
    let app = axum::Router::new().route(
        "/large.txt",
        axum::routing::get(move || {
            let large_text = large_text.clone();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/plain")],
                    large_text,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider = Arc::new(VirtualEnvironmentProvider::new("test"));
    let toolset = host_io_tools();
    let fetch_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "fetch")
        .unwrap();
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider.clone());
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let fetch = fetch_tool
        .call(
            context,
            serde_json::json!({"url": format!("http://{addr}/large.txt")}),
        )
        .await
        .unwrap();

    assert_eq!(fetch.content["truncated"], true);
    let fetch_path = fetch.content["output_file_path"].as_str().unwrap();
    assert!(fetch_path.starts_with(".starweaver/tmp/fetch-"));
    assert_eq!(provider.read_text(fetch_path).await.unwrap(), expected);
    assert!(
        fetch.content["content"]
            .as_str()
            .unwrap()
            .contains("output_file_path")
    );
    assert!(serde_json::to_string(&fetch.content).unwrap().len() <= 20_000);
}

#[tokio::test]
async fn read_media_native_image_url_returns_provider_backed_content_parts() {
    let app = axum::Router::new().route(
        "/image.png",
        axum::routing::get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "image/png")],
                b"\x89PNG\r\n\x1a\nsmall".to_vec(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let tool = host_io_tools()
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "read_media")
        .unwrap();
    let agent_context = AgentContext::default();
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    dependencies.insert(HostMediaCapabilities {
        model_id: Some("vision-model".to_string()),
        supports_image_url: true,
        ..HostMediaCapabilities::default()
    });
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let result = tool
        .call(
            context,
            serde_json::json!({
                "url": format!("http://{addr}/image.png"),
                "instructions": "describe the image"
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.content["success"], true);
    assert_eq!(result.content["media_kind"], "image");
    assert_eq!(result.content["media_type"], "image/png");
    assert_eq!(result.content["native_supported"], true);
    assert_eq!(
        result.model_content,
        Some(serde_json::json!(
            "The image is attached in the user message."
        ))
    );
    let parts = result.private_metadata["starweaver_tool_return_content_parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["kind"], "data_url");
    assert_eq!(parts[0]["media_type"], "image/png");
    assert!(
        parts[0]["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert!(
        result.private_metadata["starweaver_tool_return_prompt"]
            .as_str()
            .unwrap()
            .contains("describe the image")
    );
}

#[tokio::test]
async fn read_media_uses_fallback_adapter_when_native_media_is_unavailable() {
    let app = axum::Router::new().route(
        "/clip.mp4",
        axum::routing::get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "video/mp4")],
                b"\0\0\0\x18ftypmp42small".to_vec(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let tool = host_io_tools()
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "read_media")
        .unwrap();
    let mut dependencies = DependencyStore::new();
    dependencies.insert(AgentContext::default());
    dependencies.insert(HostMediaUnderstandingClientHandle::new(Arc::new(
        FakeMediaUnderstandingClient,
    )));
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let result = tool
        .call(
            context,
            serde_json::json!({
                "url": format!("http://{addr}/clip.mp4"),
                "instructions": "summarize the clip"
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.content["success"], true);
    assert_eq!(result.content["media_kind"], "video");
    assert_eq!(result.content["model_id"], "fake-media-model");
    assert!(
        result.content["url"]
            .as_str()
            .unwrap()
            .starts_with("data:video/mp4;base64,")
    );
}

#[tokio::test]
async fn read_media_youtube_url_uses_native_file_url_when_supported() {
    let tool = host_io_tools()
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "read_media")
        .unwrap();
    let mut dependencies = DependencyStore::new();
    dependencies.insert(HostMediaCapabilities {
        model_id: Some("gemini-model".to_string()),
        supports_video_url: true,
        supports_youtube_url: true,
        ..HostMediaCapabilities::default()
    });
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let result = tool
        .call(
            context,
            serde_json::json!({
                "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "instructions": "focus on the chorus"
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.content["success"], true);
    assert_eq!(result.content["youtube_url"], true);
    assert_eq!(result.content["media_kind"], "video");
    let parts = result.private_metadata["starweaver_tool_return_content_parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["kind"], "file_url");
    assert_eq!(parts[0]["media_type"], "video/mp4");
    assert_eq!(
        parts[0]["url"],
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
    );
    assert!(
        result.private_metadata["starweaver_tool_return_prompt"]
            .as_str()
            .unwrap()
            .contains("focus on the chorus")
    );
}

#[tokio::test]
async fn read_media_rejects_non_media_http_url_with_actionable_retry() {
    let app = axum::Router::new().route(
        "/page",
        axum::routing::get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "text/html")],
                "<html><body>not media</body></html>",
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&host_io_tools());
    let mut dependencies = DependencyStore::new();
    dependencies.insert(AgentContext::default());
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let result = execute_tool_call(
        &registry,
        context,
        "read-media-page",
        "read_media",
        serde_json::json!({"url": format!("http://{addr}/page")}),
    )
    .await;

    assert!(!result.is_error);
    assert_eq!(result.content["kind"], "feedback");
    assert!(
        result.content["message"]
            .as_str()
            .unwrap()
            .contains("Use `scrape`")
    );
}

#[tokio::test]
async fn local_shell_exec_foreground_cancels_running_process_quickly() {
    let root = unique_agent_test_dir();
    let provider = Arc::new(
        LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::allow_all(),
        }),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&shell_tools());
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider.clone());
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let cancellation_token = CancellationToken::new();
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies)
        .with_cancellation_token(cancellation_token.clone());
    let registry = Arc::new(registry);
    let command = "exec sleep 5";
    let started_at = Instant::now();
    let handle = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move {
            execute_tool_call(
                &registry,
                context,
                "cancel-shell",
                "shell_exec",
                serde_json::json!({"command": command, "timeout_seconds": 30}),
            )
            .await
        }
    });

    assert!(
        wait_for_local_process_status(
            provider.as_ref(),
            command,
            ShellProcessStatus::Running,
            Duration::from_secs(2),
        )
        .await,
        "foreground shell process should start"
    );
    cancellation_token.cancel();
    let result = match tokio::time::timeout(Duration::from_secs(2), handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => panic!("shell task should not panic: {error}"),
        Err(error) => panic!("cancelled shell tool should return quickly: {error}"),
    };

    assert!(
        started_at.elapsed() < Duration::from_secs(2),
        "foreground shell cancellation took {:?}",
        started_at.elapsed()
    );
    assert!(result.is_error, "expected cancellation error: {result:?}");
    assert_eq!(result.content["kind"], "cancelled");
    assert_eq!(result.metadata["error_kind"], "cancelled");
    assert!(
        wait_for_local_process_status(
            provider.as_ref(),
            command,
            ShellProcessStatus::Killed,
            Duration::from_secs(2),
        )
        .await,
        "foreground shell process should be killed after cancellation: {:?}",
        provider.list_processes().await.unwrap()
    );

    std::fs::remove_dir_all(root).unwrap();
}

async fn wait_for_local_process_status(
    provider: &LocalEnvironmentProvider,
    command: &str,
    status: ShellProcessStatus,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let processes = provider.list_processes().await.unwrap();
        if processes
            .iter()
            .any(|process| process.command == command && process.status == status)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn local_shell_tmp_output_path_can_be_viewed() {
    let root = unique_agent_test_dir();
    let provider = Arc::new(
        LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::allow_all(),
        }),
    );
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&filesystem_tools());
    registry.insert_toolset(&shell_tools());
    let mut agent_context = AgentContext {
        tool_config: ToolConfig {
            shell_output_truncate_limit: 8,
            ..ToolConfig::default()
        },
        ..AgentContext::default()
    };
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let shell = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "shell".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "echo 0123456789abcdef"}).into(),
            },
        )
        .await;
    let stdout_path = shell.content["stdout_file_path"].as_str().unwrap();
    assert!(std::path::Path::new(stdout_path).is_absolute());

    let viewed = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "view".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": stdout_path}).into(),
            },
        )
        .await;
    let viewed_content = viewed
        .content
        .as_str()
        .or_else(|| viewed.content["content"].as_str())
        .unwrap_or_else(|| panic!("unexpected view result: {}", viewed.content));
    assert_eq!(
        viewed_content.trim_end_matches(['\r', '\n']),
        "0123456789abcdef"
    );

    std::fs::remove_dir_all(root).unwrap();
}

fn unique_agent_test_dir() -> std::path::PathBuf {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "starweaver-agent-test-{}-{:?}-{suffix}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap_or(path)
}

#[tokio::test]
async fn agent_builder_registers_unified_read_media_url_tool() {
    let model = TestModel::with_text("done");
    let model_handle = Arc::new(model.clone());
    let mut session = AgentSession::new(
        starweaver_agent::AgentBuilder::new(model_handle)
            .toolset(&host_io_tools())
            .build(),
    );

    let result = session.run("inspect tools").await.unwrap();

    assert_eq!(result.output, "done");
    let params = model.captured_params();
    let tool_names = params[0]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"search"));
    assert!(tool_names.contains(&"fetch"));
    assert!(tool_names.contains(&"scrape"));
    assert!(tool_names.contains(&"download"));
    assert!(tool_names.contains(&"read_media"));
    assert!(!tool_names.contains(&"load_media_url"));
    assert!(!tool_names.contains(&"read_image"));
    assert!(!tool_names.contains(&"read_video"));
    assert!(!tool_names.contains(&"read_audio"));
}

#[tokio::test]
async fn host_io_tools_use_injected_web_clients() {
    let toolset = host_io_tools();
    let search_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "search")
        .unwrap();
    let scrape_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "scrape")
        .unwrap();
    let mut dependencies = starweaver_context::DependencyStore::new();
    dependencies.insert(HostSearchClientHandle::new(Arc::new(FakeSearchClient)));
    dependencies.insert(HostScrapeClientHandle::new(Arc::new(FakeScrapeClient)));
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let search = search_tool
        .call(
            context.clone(),
            serde_json::json!({"query": "rust sdk", "num": 3}),
        )
        .await
        .unwrap();
    let scrape = scrape_tool
        .call(context, serde_json::json!({"url": "https://example.com"}))
        .await
        .unwrap();

    assert_eq!(search.content["provider"], "fake");
    assert_eq!(search.content["results"][0]["title"], "Rust SDK");
    assert_eq!(scrape.content["adapter"], "fake_scrape");
    assert_eq!(scrape.content["markdown_content"], "# Example");
}

#[tokio::test]
async fn host_io_and_view_media_failures_return_actionable_tool_errors() {
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&host_io_tools());
    registry.insert_toolset(&filesystem_tools());

    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test")
            .with_bytes("image.png", b"\x89PNG\r\n\x1a\nsmall".to_vec()),
    );
    let mut agent_context = AgentContext::default();
    attach_environment(&mut agent_context, provider);
    let mut dependencies = agent_context.dependencies.clone();
    dependencies.insert(agent_context);
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let empty_search = execute_tool_call(
        &registry,
        context.clone(),
        "empty-search",
        "search",
        serde_json::json!({"query": "   "}),
    )
    .await;
    assert!(!empty_search.is_error);
    assert_eq!(empty_search.content["kind"], "feedback");
    assert!(
        empty_search.content["message"]
            .as_str()
            .unwrap()
            .contains("query must not be empty")
    );
    assert!(
        empty_search.content["how_to_fix"]
            .as_str()
            .unwrap()
            .contains("next step")
    );

    let local_media_without_adapter = execute_tool_call(
        &registry,
        context,
        "local-media-without-adapter",
        "view",
        serde_json::json!({"path": "image.png"}),
    )
    .await;
    assert!(!local_media_without_adapter.is_error);
    assert_eq!(local_media_without_adapter.content["kind"], "feedback");
    assert!(
        local_media_without_adapter.content["message"]
            .as_str()
            .unwrap()
            .contains("media-capable model")
    );

    let mut failing_dependencies = starweaver_context::DependencyStore::new();
    failing_dependencies.insert(HostSearchClientHandle::new(Arc::new(FailingSearchClient)));
    let failing_context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(failing_dependencies);
    let failing_search = execute_tool_call(
        &registry,
        failing_context,
        "failing-search",
        "search",
        serde_json::json!({"query": "rust sdk"}),
    )
    .await;
    assert!(failing_search.is_error);
    assert_eq!(failing_search.content["kind"], "execution");
    assert!(
        failing_search.content["message"]
            .as_str()
            .unwrap()
            .contains("adapter unavailable")
    );
    assert!(
        failing_search.content["how_to_fix"]
            .as_str()
            .unwrap()
            .contains("tool/runtime failure")
    );
}

#[tokio::test]
async fn summarize_tool_handoff_replaces_history_on_next_request() {
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "call_summarize",
            "summarize",
            serde_json::json!({
                "content": "## Current State\nSummarized prior work.\n\n## Next Step\nContinue implementation.",
                "auto_load_files": []
            }),
        ),
        ModelResponse::text("handoff complete"),
    ]);
    let model_handle = Arc::new(model.clone());
    let mut session = AgentSession::new(
        starweaver_agent::AgentBuilder::new(model_handle)
            .toolset(&context_tools())
            .build(),
    );

    let result = session.run("summarize now").await.unwrap();

    assert_eq!(result.output, "handoff complete");
    let captured = model.captured_messages();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[1].len(), 1);
    let restored_text = captured[1]
        .iter()
        .flat_map(request_text_parts)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(restored_text.contains("<context-restored>"));
    assert!(restored_text.contains("# Context Summary"));
    assert!(restored_text.contains("Continue implementation."));
    assert!(!restored_text.contains("tool_call_id"));
    assert!(session.context().handoff_message.is_none());
    assert!(session.context().force_inject_context);
}

#[tokio::test]
async fn agent_runtime_builder_runs_host_search_adapter() {
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "call_search",
            "search",
            serde_json::json!({"query": "rust sdk", "num": 1}),
        ),
        ModelResponse::text("searched"),
    ]);
    let mut context = AgentContext::default();
    context
        .dependencies
        .insert(HostSearchClientHandle::new(Arc::new(FakeSearchClient)));
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(model))
        .context(context)
        .toolset(&host_io_tools())
        .build();

    let result = runtime.run("search docs").await.unwrap();

    assert_eq!(result.output, "searched");
    let tool_return = result
        .state
        .message_history
        .iter()
        .flat_map(|message| match message {
            starweaver_model::ModelMessage::Request(request) => request.parts.iter().collect(),
            starweaver_model::ModelMessage::Response(_) => Vec::new(),
        })
        .find_map(|part| match part {
            starweaver_model::ModelRequestPart::ToolReturn(tool_return)
                if tool_return.name == "search" =>
            {
                Some(tool_return)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(tool_return.content["provider"], "fake");
    assert_eq!(tool_return.content["results"][0]["title"], "Rust SDK");
    assert_eq!(
        tool_return.content["results"][0]["citation"]["provider"],
        "fake"
    );
}

#[tokio::test]
async fn summarize_sets_context_handoff_and_auto_load_files() {
    let toolset = context_tools();
    let summarize_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "summarize")
        .unwrap();
    let mut agent_context = AgentContext {
        auto_load_files: vec!["AGENTS.md".to_string()],
        ..AgentContext::default()
    };
    let handle = AgentContextHandle::new(agent_context.clone());
    let mut dependencies = DependencyStore::new();
    dependencies.insert(handle.clone());
    let tool_context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let result = summarize_tool
        .call(
            tool_context,
            serde_json::json!({
                "content": "## Current State\nImplemented handoff.",
                "auto_load_files": ["AGENTS.md", "crates/starweaver-agent/src/bundles/context_tools/context.rs"]
            }),
        )
        .await
        .unwrap();
    agent_context = handle.snapshot();

    assert_eq!(result.content["operation"], "summarize");
    assert_eq!(
        result.content["payload"]["rendered"],
        "# Context Summary\n\n## Current State\nImplemented handoff."
    );
    assert_eq!(
        agent_context.handoff_message.as_deref(),
        Some("# Context Summary\n\n## Current State\nImplemented handoff.")
    );
    assert_eq!(
        agent_context.auto_load_files,
        vec![
            "AGENTS.md".to_string(),
            "crates/starweaver-agent/src/bundles/context_tools/context.rs".to_string(),
        ]
    );
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
    assert_eq!(task.content["payload"]["task"]["subject"], "ship");
    assert_eq!(
        task.content["payload"]["task"]["description"],
        "Ship the release"
    );
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
#[allow(clippy::too_many_lines)]
fn bundle_toolsets_export_stable_tool_names_and_instructions() {
    let filesystem = filesystem_tools();
    let shell = shell_tools();
    let task = task_tools();
    let context = context_tools();
    let host_io = host_io_tools();

    assert_eq!(filesystem.name(), "filesystem");
    assert_eq!(shell.name(), "shell");
    assert_eq!(task.name(), "task");
    assert_eq!(context.name(), "context");
    assert_eq!(host_io.name(), "host_io");

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
    assert_tool_names(&context, &["summarize", "note", "note_get", "thinking"]);
    assert_tool_names(
        &host_io,
        &["search", "fetch", "scrape", "download", "read_media"],
    );

    let task_metadata = task
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_create")
        .unwrap()
        .definition()
        .metadata;
    let shell_metadata = shell
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "shell_exec")
        .unwrap()
        .definition()
        .metadata;
    let note_metadata = context
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "note")
        .unwrap()
        .definition()
        .metadata;
    let summarize_metadata = context
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "summarize")
        .unwrap()
        .definition()
        .metadata;

    assert_eq!(task_metadata["bundle"], "task");
    assert_eq!(task_metadata["auto_inherit"], true);
    assert_eq!(shell_metadata["bundle"], "shell");
    assert_eq!(shell_metadata["approval_required"], true);
    assert_eq!(note_metadata["bundle"], "context");
    assert_eq!(note_metadata["auto_inherit"], true);
    assert_eq!(summarize_metadata["bundle"], "context");
    assert_eq!(summarize_metadata["starweaver_context_management"], true);

    let filesystem_instructions = filesystem.get_instructions();
    assert_eq!(filesystem_instructions.len(), 9);
    assert!(filesystem_instructions.iter().any(|instruction| {
        instruction.group == "view"
            && instruction
                .content
                .contains("pass `instructions` when you need focused analysis")
    }));
    assert!(filesystem_instructions.iter().any(|instruction| {
        instruction.group == "edit"
            && instruction
                .content
                .contains("Use multi_edit instead of multiple edit calls")
    }));
    assert!(filesystem_instructions.iter().any(|instruction| {
        instruction.group == "multi_edit"
            && instruction
                .content
                .contains("do not issue concurrent edit calls")
    }));
    assert!(
        filesystem_instructions
            .iter()
            .any(|instruction| instruction.group == "glob"
                && instruction.content.contains("Use specific patterns"))
    );
    assert!(filesystem_instructions.iter().any(|instruction| {
        instruction.group == "grep"
            && instruction
                .content
                .contains("Use a specific `include` pattern")
    }));
    assert!(filesystem_instructions.iter().any(|instruction| {
        instruction.group == "delete"
            && instruction
                .content
                .contains("Verify broad recursive targets")
    }));
    assert!(filesystem_instructions.iter().any(|instruction| {
        instruction.group == "resource_ref"
            && instruction
                .content
                .contains("durable provider-scoped handle")
    }));
    assert_eq!(shell.get_instructions().len(), 1);
    assert!(
        shell.get_instructions()[0]
            .content
            .contains("Set background=true for long-running commands")
    );
    assert_eq!(task.get_instructions().len(), 1);
    assert!(
        task.get_instructions()[0]
            .content
            .contains("multiple meaningful steps")
    );
    assert!(
        task.get_instructions()[0]
            .content
            .contains("single direct action")
    );
    assert!(
        task.get_instructions()[0]
            .content
            .contains("user's language")
    );
    assert!(
        task.get_instructions()[0]
            .content
            .contains("specific enough to execute and verify")
    );
    assert!(
        task.get_instructions()[0]
            .content
            .contains("delegate tool's own execution model")
    );
    assert_eq!(context.get_instructions().len(), 3);
    assert_eq!(host_io.get_instructions().len(), 5);
}

#[test]
#[allow(clippy::too_many_lines)]
fn first_party_tool_arg_schemas_match_starweaver_sdk_and_describe_args() {
    let expected = [
        (
            filesystem_tools(),
            vec![
                (
                    "view",
                    vec![
                        "file_path",
                        "line_offset",
                        "line_limit",
                        "max_line_length",
                        "instructions",
                    ],
                ),
                ("ls", vec!["path", "ignore", "max_entries"]),
                ("write", vec!["file_path", "content", "mode"]),
                (
                    "edit",
                    vec!["file_path", "old_string", "new_string", "replace_all"],
                ),
                ("multi_edit", vec!["file_path", "edits"]),
                (
                    "glob",
                    vec![
                        "pattern",
                        "root",
                        "include_hidden",
                        "include_ignored",
                        "max_results",
                    ],
                ),
                (
                    "grep",
                    vec![
                        "pattern",
                        "include",
                        "root",
                        "context_lines",
                        "max_results",
                        "max_matches_per_file",
                        "max_files",
                        "include_hidden",
                        "include_ignored",
                    ],
                ),
                ("mkdir", vec!["paths", "parents"]),
                ("delete", vec!["paths", "recursive", "force"]),
                ("move", vec!["pairs", "overwrite"]),
                ("copy", vec!["pairs", "overwrite"]),
                ("resource_ref", vec!["file_path"]),
            ],
        ),
        (
            shell_tools(),
            vec![
                (
                    "shell_exec",
                    vec![
                        "command",
                        "timeout_seconds",
                        "environment",
                        "cwd",
                        "background",
                    ],
                ),
                ("shell_wait", vec!["process_id", "timeout_seconds"]),
                ("shell_status", vec![]),
                ("shell_input", vec!["process_id", "text", "close_stdin"]),
                ("shell_signal", vec!["process_id", "signal"]),
                ("shell_kill", vec!["process_id"]),
            ],
        ),
        (
            task_tools(),
            vec![
                (
                    "task_create",
                    vec!["subject", "description", "active_form", "metadata"],
                ),
                ("task_get", vec!["task_id"]),
                (
                    "task_update",
                    vec![
                        "task_id",
                        "status",
                        "subject",
                        "description",
                        "active_form",
                        "owner",
                        "add_blocks",
                        "add_blocked_by",
                        "metadata",
                    ],
                ),
                ("task_list", vec![]),
            ],
        ),
        (
            context_tools(),
            vec![
                ("summarize", vec!["content", "auto_load_files"]),
                ("note", vec!["key", "value"]),
                ("note_get", vec!["key"]),
                ("thinking", vec!["thought"]),
            ],
        ),
        (
            host_io_tools(),
            vec![
                ("download", vec!["urls", "save_dir"]),
                ("fetch", vec!["url", "head_only"]),
                ("read_media", vec!["url", "instructions"]),
                ("scrape", vec!["url"]),
                ("search", vec!["query", "num"]),
            ],
        ),
    ];

    for (toolset, tools) in expected {
        for (tool_name, fields) in tools {
            assert_tool_schema_fields(toolset.as_ref(), tool_name, &fields);
        }
    }

    let proxy = dynamic_tool_proxy(vec![]);
    assert_tool_schema_fields(proxy.as_ref(), "search_tools", &["query"]);
    assert_tool_schema_fields(proxy.as_ref(), "call_tool", &["name", "arguments"]);
}

#[tokio::test]
async fn tool_proxy_searches_and_calls_namespaced_toolsets() {
    let provider =
        Arc::new(VirtualEnvironmentProvider::new("test").with_file("README.md", "proxied content"));
    let filesystem = filesystem_tools();
    let namespaced = namespaced_toolset("workspace", filesystem.clone());
    let proxy = dynamic_tool_proxy(vec![namespaced.clone()]);
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
    assert!(
        namespaced
            .get_tools()
            .iter()
            .any(|tool| tool.name() == "workspace_view")
    );

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

    assert_eq!(call_result.content, serde_json::json!("proxied content"));
}

#[tokio::test]
async fn tool_proxy_escapes_xml_attributes_and_text() {
    let quoted = Arc::new(json_tool(
        "quote\"tool",
        Some("Handle <quoted> & special text".to_string()),
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": {"type": "string", "description": "Quoted value"}
            }
        }),
        |_context: ToolContext, arguments: serde_json::Value| async move {
            Ok(ToolResult::new(arguments))
        },
    ));
    let toolset = Arc::new(starweaver_tools::StaticToolset::new("quoted\"set").with_tool(quoted));
    let proxy = dynamic_tool_proxy(vec![toolset]);
    let search_tools = proxy
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "search_tools")
        .unwrap();

    let result = search_tools
        .call(
            ToolContext::new(RunId::default(), ConversationId::default(), 0),
            serde_json::json!({"query": "quote\" <tag>"}),
        )
        .await
        .unwrap();
    let xml = result.content["content"].as_str().unwrap();

    assert!(xml.contains("query=\"quote&quot; &lt;tag&gt;\""));
    assert!(xml.contains("name=\"quote&quot;tool\""));
    assert!(xml.contains("toolset=\"quoted&quot;set\""));
    assert!(xml.contains("Handle &lt;quoted&gt; &amp; special text"));
}

fn assert_tool_schema_fields(
    toolset: &dyn starweaver_tools::Toolset,
    tool_name: &str,
    expected: &[&str],
) {
    let tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == tool_name)
        .unwrap_or_else(|| panic!("missing tool {tool_name}"));
    let schema = tool.parameters_schema();
    let properties = schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("{tool_name} schema has no properties object: {schema}"));
    assert_eq!(
        properties.len(),
        expected.len(),
        "{tool_name} argument count drifted: {schema}"
    );
    for field in expected {
        assert!(
            properties.contains_key(*field),
            "{tool_name} is missing argument {field}: {schema}"
        );
    }
    for field in expected {
        let description = properties[*field]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool_name}.{field} is missing description: {schema}"));
        assert!(
            !description.trim().is_empty(),
            "{tool_name}.{field} has an empty description"
        );
    }
    match tool_name {
        "multi_edit" => {
            let edit_item = &schema["$defs"]["EditItemArgs"];
            assert_eq!(edit_item["type"], "object");
            for field in ["old_string", "new_string", "replace_all"] {
                assert!(
                    edit_item["properties"][field]["description"].is_string(),
                    "multi_edit nested edit item is missing {field} description: {schema}"
                );
            }
        }
        "grep" => {
            assert_eq!(properties["max_files"]["type"], "integer");
            assert_eq!(properties["context_lines"]["type"], "integer");
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("pattern"))
            );
        }
        "task_update" => {
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("task_id"))
            );
            assert!(properties["metadata"].is_object());
        }
        _ => {}
    }
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

struct FailingSearchClient;

#[async_trait]
impl HostSearchClient for FailingSearchClient {
    async fn search(&self, _request: SearchRequest) -> Result<SearchResponse, String> {
        Err("adapter unavailable".to_string())
    }
}

struct FakeSearchClient;

#[async_trait]
impl HostSearchClient for FakeSearchClient {
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, String> {
        Ok(SearchResponse {
            success: true,
            query: request.query,
            results: vec![SearchResultItem {
                title: "Rust SDK".to_string(),
                url: "https://example.com/rust".to_string(),
                description: "SDK result".to_string(),
                provider: "fake".to_string(),
                rank: 1,
                content_type: Some("text/html".to_string()),
                published_at: None,
                citation: Some(serde_json::json!({"provider": "fake"})),
            }],
            errors: Vec::new(),
            truncated: false,
            provider: "fake".to_string(),
        })
    }
}

struct FakeScrapeClient;

#[async_trait]
impl HostScrapeClient for FakeScrapeClient {
    async fn scrape(&self, request: ScrapeRequest) -> Result<ScrapeResponse, String> {
        Ok(ScrapeResponse {
            success: true,
            url: request.url.clone(),
            final_url: request.url,
            title: Some("Example".to_string()),
            markdown_content: "# Example".to_string(),
            adapter: "fake_scrape".to_string(),
            truncated: false,
            total_length: 9,
            content_type: Some("text/html".to_string()),
            citation: None,
            handoff: None,
        })
    }
}

struct LargeSearchClient;

#[async_trait]
impl HostSearchClient for LargeSearchClient {
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, String> {
        Ok(SearchResponse {
            success: true,
            query: request.query,
            results: (0..8)
                .map(|index| SearchResultItem {
                    title: format!("Large result {index}"),
                    url: format!("https://example.com/{index}"),
                    description: format!("large snippet {index} {}", "x".repeat(5_000)),
                    provider: "large_fake".to_string(),
                    rank: index + 1,
                    content_type: Some("text/html".to_string()),
                    published_at: None,
                    citation: Some(
                        serde_json::json!({"provider": "large_fake", "rank": index + 1}),
                    ),
                })
                .collect(),
            errors: Vec::new(),
            truncated: false,
            provider: "large_fake".to_string(),
        })
    }
}

struct LargeScrapeClient;

#[async_trait]
impl HostScrapeClient for LargeScrapeClient {
    async fn scrape(&self, request: ScrapeRequest) -> Result<ScrapeResponse, String> {
        let markdown = large_markdown();
        Ok(ScrapeResponse {
            success: true,
            url: request.url.clone(),
            final_url: request.url,
            title: Some("Large Example".to_string()),
            markdown_content: markdown.clone(),
            adapter: "large_fake_scrape".to_string(),
            truncated: false,
            total_length: markdown.chars().count(),
            content_type: Some("text/html".to_string()),
            citation: None,
            handoff: None,
        })
    }
}

fn large_markdown() -> String {
    format!("# Large\n\n{}", "m".repeat(30_000))
}

struct FakeMediaUnderstandingClient;

#[async_trait]
impl HostMediaUnderstandingClient for FakeMediaUnderstandingClient {
    async fn understand(
        &self,
        request: MediaUnderstandingRequest,
    ) -> Result<MediaUnderstandingResponse, String> {
        Ok(MediaUnderstandingResponse {
            success: true,
            media_kind: request.media_kind,
            url: request.url,
            model_id: "fake-media-model".to_string(),
            content: "image analysis".to_string(),
            truncated: false,
            metadata: serde_json::Map::new(),
        })
    }
}
