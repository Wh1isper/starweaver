#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    attach_environment, dynamic_tool_proxy, filesystem_tools, host_operation_tools, json_tool,
    namespaced_toolset, shell_tools, task_tools, AgentCapability, AgentContext, AgentSession,
    EnvironmentContextCapability, HostMediaCapabilities, HostMediaUnderstandingClient,
    HostMediaUnderstandingClientHandle, HostScrapeClient, HostScrapeClientHandle, HostSearchClient,
    HostSearchClientHandle, MediaUnderstandingRequest, MediaUnderstandingResponse, ScrapeRequest,
    ScrapeResponse, SearchRequest, SearchResponse, SearchResultItem, ToolContext, ToolRegistry,
    ToolResult,
};
use starweaver_context::ToolConfig;
use starweaver_core::{ConversationId, Metadata, RunId, Usage};
use starweaver_environment::{
    EnvironmentPolicy, EnvironmentProvider, FilePolicy, LocalEnvironmentProvider, ShellOutput,
    ShellPolicy, VirtualEnvironmentProvider,
};
use starweaver_model::{
    tool_call_response, ContentPart, ModelProfile, ModelRequest, ModelRequestPart, ModelResponse,
    ProtocolFamily, TestModel, INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT, INSTRUCTION_ORIGIN_METADATA,
};

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
        .prepare_provider_messages_with_context(
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
            if metadata.get(INSTRUCTION_ORIGIN_METADATA)
                == Some(&serde_json::json!(INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT))
                && matches!(&content[0], ContentPart::Text { text } if text.contains("<environment-context>"))
    ));
}

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
    attach_environment(&mut agent_context, provider.clone());
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
                arguments: serde_json::json!({"path": "README.md"}).into(),
            },
        )
        .await;
    let write = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "write".to_string(),
                name: "write".to_string(),
                arguments:
                    serde_json::json!({"path": "docs/output.txt", "content": "pub fn ok() {}"})
                        .into(),
            },
        )
        .await;
    let glob = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "glob".to_string(),
                name: "glob".to_string(),
                arguments: serde_json::json!({"path": "", "pattern": "*.rs"}).into(),
            },
        )
        .await;
    let grep =
        registry
            .execute_call(
                context.clone(),
                &starweaver_model::ToolCallPart {
                    id: "grep".to_string(),
                    name: "grep".to_string(),
                    arguments:
                        serde_json::json!({"path": "", "pattern": "hello", "include": "**/*.rs"})
                            .into(),
                },
            )
            .await;
    let resource = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "resource".to_string(),
                name: "resource_ref".to_string(),
                arguments: serde_json::json!({"path": "README.md"}).into(),
            },
        )
        .await;
    let ignored_ls = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "ignored-ls".to_string(),
                name: "ls".to_string(),
                arguments: serde_json::json!({"path": "", "ignore": ["src/main.rs"]}).into(),
            },
        )
        .await;
    let invalid_write_mode = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "invalid-write-mode".to_string(),
                name: "write".to_string(),
                arguments:
                    serde_json::json!({"path": "docs/output.txt", "content": "x", "mode": "bad"})
                        .into(),
            },
        )
        .await;
    let edit_existing_create = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "edit-existing-create".to_string(),
                name: "edit".to_string(),
                arguments: serde_json::json!({"file_path": "README.md", "old_string": "", "new_string": "overwrite"}).into(),
            },
        )
        .await;
    let multi_edit_create_then_replace = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "multi-edit-create-then-replace".to_string(),
                name: "multi_edit".to_string(),
                arguments: serde_json::json!({"file_path": "created.txt", "edits": [
                    {"old_string": "", "new_string": "Hello World"},
                    {"old_string": "World", "new_string": "Universe"}
                ]})
                .into(),
            },
        )
        .await;
    let multi_edit_empty_later = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "multi-edit-empty-later".to_string(),
                name: "multi_edit".to_string(),
                arguments: serde_json::json!({"file_path": "README.md", "edits": [
                    {"old_string": "hello", "new_string": "hi"},
                    {"old_string": "", "new_string": "boom", "replace_all": true}
                ]})
                .into(),
            },
        )
        .await;
    let shell = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "shell".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "echo ok"}).into(),
            },
        )
        .await;
    let empty_shell = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "empty-shell".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "   "}).into(),
            },
        )
        .await;
    let invalid_grep_context = registry
        .execute_call(
            context,
            &starweaver_model::ToolCallPart {
                id: "invalid-grep-context".to_string(),
                name: "grep".to_string(),
                arguments: serde_json::json!({
                    "path": "",
                    "pattern": "hello",
                    "context_lines": -1,
                })
                .into(),
            },
        )
        .await;

    assert_eq!(read.content, serde_json::json!("hello"));
    assert_eq!(write.content["written"], true);
    assert_eq!(glob.content["matches"].as_array().unwrap().len(), 2);
    assert_eq!(grep.content["matches"].as_array().unwrap().len(), 2);
    assert_eq!(resource.content["uri"], "env://test/README.md");
    assert_eq!(ignored_ls.content["entries"].as_array().unwrap().len(), 3);
    assert!(ignored_ls.content["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry.as_str() != Some("src/main.rs")));
    assert!(invalid_write_mode.is_error);
    assert!(invalid_write_mode.content["error"]
        .as_str()
        .unwrap()
        .contains("unsupported write mode"));
    assert!(edit_existing_create.is_error);
    assert!(edit_existing_create.content["error"]
        .as_str()
        .unwrap()
        .contains("file already exists"));
    assert!(!multi_edit_create_then_replace.is_error);
    assert_eq!(
        provider.read_text("created.txt").await.unwrap(),
        "Hello Universe"
    );
    assert!(multi_edit_empty_later.is_error);
    assert!(multi_edit_empty_later.content["error"]
        .as_str()
        .unwrap()
        .contains("old_string must be non-empty"));
    assert_eq!(shell.content["stdout"], "ok\n");
    assert_eq!(empty_shell.content["return_code"], 1);
    assert_eq!(empty_shell.content["stdout"], "");
    assert_eq!(empty_shell.content["stderr"], "");
    assert!(empty_shell.content["error"]
        .as_str()
        .unwrap()
        .contains("must not be empty"));

    let mut process_agent_context = AgentContext::default();
    attach_environment(&mut process_agent_context, provider.clone());
    let mut process_dependencies = process_agent_context.dependencies.clone();
    process_dependencies.insert(process_agent_context);
    let process_context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(process_dependencies);
    let background_shell = registry
        .execute_call(
            process_context,
            &starweaver_model::ToolCallPart {
                id: "background-shell".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({
                    "command": "sleep 1",
                    "background": true,
                    "cwd": "src",
                    "timeout_seconds": 42,
                    "environment": {"STARWEAVER_BACKGROUND": "yes"},
                })
                .into(),
            },
        )
        .await;
    assert_eq!(background_shell.content["command"], "sleep 1");
    assert_eq!(background_shell.content["metadata"]["cwd"], "src");
    assert_eq!(background_shell.content["metadata"]["timeout_seconds"], 42);
    assert_eq!(
        background_shell.content["metadata"]["environment"]["STARWEAVER_BACKGROUND"],
        "yes"
    );

    assert!(invalid_grep_context.is_error);
    assert!(invalid_grep_context.content["error"]
        .as_str()
        .unwrap()
        .contains("context_lines must be greater than or equal to 0"));
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
    assert!(provider
        .read_text("docs/generated/readme-copy.md")
        .await
        .is_err());
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
    assert!(long.content["content"]
        .as_str()
        .unwrap()
        .contains("... (line truncated)"));
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
    assert!(binary
        .content
        .as_str()
        .unwrap()
        .contains("appears to be a binary file"));

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
    assert!(image.content["url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
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
    assert!(binary
        .content
        .as_str()
        .unwrap()
        .contains("appears to be a binary file"));
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
    assert!(image.private_metadata["starweaver_tool_return_prompt"]
        .as_str()
        .unwrap()
        .contains("describe it"));
}

#[tokio::test]
async fn glob_grep_and_shell_large_outputs_are_saved_to_environment_tmp_files() {
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
    assert!(provider
        .read_text(glob_path)
        .await
        .unwrap()
        .contains("very_long_file_name_0000"));

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
    assert!(provider
        .read_text(grep_path)
        .await
        .unwrap()
        .contains("needle line"));

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
    assert!(shell.content["stdout"]
        .as_str()
        .unwrap()
        .contains("truncated"));
    assert!(shell.content["stderr"]
        .as_str()
        .unwrap()
        .contains("truncated"));
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
                arguments: serde_json::json!({"command": "printf '0123456789abcdef'"}).into(),
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
    assert_eq!(viewed.content.as_str().unwrap(), "0123456789abcdef");

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
async fn agent_builder_attaches_model_media_capabilities_to_host_tools() {
    let responses = vec![
        tool_call_response(
            "media",
            "load_media_url",
            serde_json::json!({"url": "https://example.com/video.mp4"}),
        ),
        ModelResponse::text("done"),
    ];
    let model = TestModel::with_responses(responses).with_profile(ModelProfile::for_protocol(
        ProtocolFamily::GeminiGenerateContent,
    ));
    let mut session = AgentSession::new(
        starweaver_agent::AgentBuilder::new(Arc::new(model))
            .toolset(&host_operation_tools())
            .build(),
    );

    let result = session.run("load media").await.unwrap();
    let tool_return = result
        .state
        .message_history
        .iter()
        .flat_map(|message| match message {
            starweaver_model::ModelMessage::Request(request) => request.parts.iter().collect(),
            starweaver_model::ModelMessage::Response(_) => Vec::new(),
        })
        .find_map(|part| match part {
            starweaver_model::ModelRequestPart::ToolReturn(tool_return) => Some(tool_return),
            _ => None,
        })
        .unwrap();

    assert_eq!(tool_return.content["category"], "video");
    assert_eq!(tool_return.content["native_supported"], true);
    assert_eq!(tool_return.content["model_id"], "test");
    assert_eq!(tool_return.content["provider_ready"]["type"], "media_url");
}

#[tokio::test]
async fn agent_builder_filters_media_tools_by_model_capabilities() {
    let model = TestModel::with_text("done").with_profile(ModelProfile::for_protocol(
        ProtocolFamily::GeminiGenerateContent,
    ));
    let model_handle = Arc::new(model.clone());
    let mut session = AgentSession::new(
        starweaver_agent::AgentBuilder::new(model_handle)
            .toolset(&host_operation_tools())
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
    assert!(tool_names.contains(&"load_media_url"));
    assert!(!tool_names.contains(&"read_image"));
    assert!(!tool_names.contains(&"read_video"));
    assert!(!tool_names.contains(&"read_audio"));

    let text_model = TestModel::with_text("done").with_profile(ModelProfile::for_protocol(
        ProtocolFamily::OpenAiChatCompletions,
    ));
    let text_model_handle = Arc::new(text_model.clone());
    let mut text_session = AgentSession::new(
        starweaver_agent::AgentBuilder::new(text_model_handle)
            .toolset(&host_operation_tools())
            .build(),
    );

    let result = text_session.run("inspect tools").await.unwrap();

    assert_eq!(result.output, "done");
    let text_params = text_model.captured_params();
    let text_tool_names = text_params[0]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(text_tool_names.contains(&"load_media_url"));
    assert!(!text_tool_names.contains(&"read_image"));
    assert!(text_tool_names.contains(&"read_video"));
    assert!(text_tool_names.contains(&"read_audio"));
}

#[tokio::test]
async fn host_operations_use_injected_clients_and_capabilities() {
    let toolset = host_operation_tools();
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
    let load_media_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "load_media_url")
        .unwrap();
    let read_image_tool = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "read_image")
        .unwrap();
    let mut dependencies = starweaver_context::DependencyStore::new();
    dependencies.insert(HostSearchClientHandle::new(Arc::new(FakeSearchClient)));
    dependencies.insert(HostScrapeClientHandle::new(Arc::new(FakeScrapeClient)));
    dependencies.insert(HostMediaCapabilities {
        model_id: Some("test-vision".to_string()),
        supports_image_url: true,
        supports_video_url: false,
        supports_audio_url: false,
        supports_document_url: false,
    });
    dependencies.insert(HostMediaUnderstandingClientHandle::new(Arc::new(
        FakeMediaUnderstandingClient,
    )));
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
        .call(
            context.clone(),
            serde_json::json!({"url": "https://example.com"}),
        )
        .await
        .unwrap();
    let media = load_media_tool
        .call(
            context.clone(),
            serde_json::json!({"url": "https://example.com/image.png"}),
        )
        .await
        .unwrap();
    let image = read_image_tool
        .call(
            context,
            serde_json::json!({"url": "https://example.com/image.png"}),
        )
        .await
        .unwrap();

    assert_eq!(search.content["provider"], "fake");
    assert_eq!(search.content["results"][0]["title"], "Rust SDK");
    assert_eq!(scrape.content["adapter"], "fake_scrape");
    assert_eq!(scrape.content["markdown_content"], "# Example");
    assert_eq!(media.content["category"], "image");
    assert_eq!(media.content["native_supported"], true);
    assert_eq!(media.content["provider_ready"]["type"], "media_url");
    assert_eq!(image.content["content"], "image analysis");
    assert_eq!(image.content["model_id"], "fake-media-model");
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
            "fetch",
            "scrape",
            "download",
            "read_image",
            "read_video",
            "read_audio",
            "load_media_url",
            "summarize",
            "note",
            "note_get",
            "thinking",
        ],
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
    let note_metadata = host
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "note")
        .unwrap()
        .definition()
        .metadata;

    assert_eq!(task_metadata["bundle"], "task");
    assert_eq!(task_metadata["auto_inherit"], true);
    assert_eq!(shell_metadata["bundle"], "shell");
    assert_eq!(shell_metadata["approval_required"], true);
    assert_eq!(note_metadata["bundle"], "host_operations");
    assert_eq!(note_metadata["auto_inherit"], true);

    let filesystem_instructions = filesystem.get_instructions();
    assert_eq!(filesystem_instructions.len(), 13);
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "view"
            && instruction
                .content
                .contains("pass `instructions` when you need focused analysis")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "edit"
            && instruction
                .content
                .contains("Use multi_edit instead of multiple edit calls")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "multi_edit"
            && instruction
                .content
                .contains("do not issue concurrent edit calls")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "glob"
            && instruction.content.contains("ripgrep-style glob semantics")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "grep"
            && instruction.content.contains("ripgrep-backed regex")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "mkdir"
            && instruction.content.contains("parents=true")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "delete"
            && instruction
                .content
                .contains("Verify broad recursive targets")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "move"
            && instruction.content.contains("overwrite=true")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "copy"
            && instruction.content.contains("multiple copies")));
    assert!(filesystem_instructions
        .iter()
        .any(|instruction| instruction.group == "resource_ref"
            && instruction.content.contains("durable reference")));
    assert_eq!(shell.get_instructions().len(), 1);
    assert!(shell.get_instructions()[0]
        .content
        .contains("Set background=true for long-running commands"));
    assert_eq!(task.get_instructions().len(), 1);
    assert!(task.get_instructions()[0]
        .content
        .contains("Task management tools track multi-step work"));
    assert_eq!(host.get_instructions().len(), 9);
    assert!(host
        .get_instructions()
        .iter()
        .any(|instruction| instruction.group == "load_media_url"
            && instruction.content.contains("native media/document URL")));
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
                ("ls", vec!["path", "ignore"]),
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
            host_operation_tools(),
            vec![
                ("load_media_url", vec!["url"]),
                ("summarize", vec!["content", "auto_load_files"]),
                ("note", vec!["key", "value"]),
                ("note_get", vec!["key"]),
                ("thinking", vec!["thought"]),
                ("read_audio", vec!["url"]),
                ("read_image", vec!["url"]),
                ("read_video", vec!["url"]),
                ("download", vec!["urls", "save_dir"]),
                ("fetch", vec!["url", "head_only"]),
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
            assert!(schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("pattern")));
        }
        "task_update" => {
            assert!(schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("task_id")));
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
