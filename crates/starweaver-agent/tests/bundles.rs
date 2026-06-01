#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    attach_environment, filesystem_tools, host_operation_tools, namespaced_toolset, shell_tools,
    string_tool, task_tools, tool_proxy_toolset, AgentContext, AgentSession, HostMediaCapabilities,
    HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle, HostScrapeClient,
    HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle, MediaUnderstandingRequest,
    MediaUnderstandingResponse, ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse,
    SearchResultItem, ToolContext, ToolRegistry, ToolResult,
};
use starweaver_core::{ConversationId, Metadata, RunId, Usage};
use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};
use starweaver_model::{tool_call_response, ModelProfile, ModelResponse, ProtocolFamily};

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
    let ignored_ls = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "ignored-ls".to_string(),
                name: "ls".to_string(),
                arguments: serde_json::json!({"path": "", "ignore": ["src/main.rs"]}),
            },
        )
        .await;
    let invalid_write_mode = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "invalid-write-mode".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"path": "docs/output.txt", "content": "x", "mode": "bad"}),
            },
        )
        .await;
    let edit_existing_create = registry
        .execute_call(
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "edit-existing-create".to_string(),
                name: "edit".to_string(),
                arguments: serde_json::json!({"file_path": "README.md", "old_string": "", "new_string": "overwrite"}),
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
                ]}),
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
            context.clone(),
            &starweaver_model::ToolCallPart {
                id: "background".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "sleep 1", "background": true}),
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
                }),
            },
        )
        .await;

    assert_eq!(read.content["content"], "hello");
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
    assert!(multi_edit_empty_later.is_error);
    assert!(multi_edit_empty_later.content["error"]
        .as_str()
        .unwrap()
        .contains("old_string must be non-empty"));
    assert_eq!(shell.content["stdout"], "ok\n");
    assert!(background.is_error);
    assert!(background.content["error"]
        .as_str()
        .unwrap()
        .contains("background shell execution requires a durable shell provider"));
    assert!(invalid_grep_context.is_error);
    assert!(invalid_grep_context.content["error"]
        .as_str()
        .unwrap()
        .contains("context_lines must be greater than or equal to 0"));
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
    let model = starweaver_agent::TestModel::with_responses(responses).with_profile(
        ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
    );
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

    assert_eq!(filesystem.get_instructions().len(), 1);
    assert_eq!(shell.get_instructions().len(), 1);
    assert_eq!(task.get_instructions().len(), 1);
    assert_eq!(host.get_instructions().len(), 1);
}

#[test]
#[allow(clippy::too_many_lines)]
fn first_party_tool_arg_schemas_match_ya_agent_sdk_and_describe_args() {
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

    let proxy = tool_proxy_toolset(vec![]);
    assert_tool_schema_fields(proxy.as_ref(), "search_tools", &["query"]);
    assert_tool_schema_fields(proxy.as_ref(), "call_tool", &["name", "arguments"]);
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

#[tokio::test]
async fn tool_proxy_escapes_xml_attributes_and_text() {
    let quoted = Arc::new(string_tool(
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
    let proxy = tool_proxy_toolset(vec![toolset]);
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
