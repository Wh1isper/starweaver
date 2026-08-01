#![allow(clippy::unwrap_used)]

use super::*;
use starweaver_agent::{SubagentRegistry, SubagentTask};

use crate::config::ConfigResolver;

fn test_config() -> CliConfig {
    let temp = tempfile::tempdir().unwrap();
    let cli =
        crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();
    ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .unwrap()
}

#[test]
fn shell_review_adjusted_approval_removes_shell_entries_only_when_enabled() {
    let mut config = test_config();
    let approval = vec![
        "shell".to_string(),
        "shell_exec".to_string(),
        "write".to_string(),
        "*".to_string(),
    ];

    assert_eq!(shell_review_adjusted_approval(&config, &approval), approval);

    config.shell_review.enabled = true;
    assert_eq!(
        shell_review_adjusted_approval(&config, &approval),
        vec!["write".to_string(), "*".to_string()]
    );
}

#[test]
fn default_registry_uses_scoped_context_and_host_io_toolset_names() {
    let config = test_config();
    let registry = default_registry(&config, &AgentSpec::default(), None).unwrap();

    assert!(registry.resolve_toolset("context").is_some());
    assert!(registry.resolve_toolset("host_io").is_some());
    assert!(registry.resolve_toolset("codeact").is_some());
    assert!(registry.resolve_toolset("recipes").is_some());
    assert!(registry.resolve_toolset("tools").is_none());
}

#[test]
fn codeact_is_default_enabled_and_explicit_disable_removes_the_toolset() {
    let mut config = test_config();
    let enabled_names = default_toolsets(&config, None)
        .unwrap()
        .into_iter()
        .flat_map(|toolset| toolset.get_tools())
        .map(|tool| tool.name().to_string())
        .collect::<BTreeSet<_>>();
    assert!(enabled_names.contains(starweaver_agent::RUN_CODE_TOOL_NAME));

    config.set_codeact(&crate::config::CliCodeActConfig { enabled: false });
    let disabled_registry = default_registry(&config, &AgentSpec::default(), None).unwrap();
    assert!(disabled_registry.resolve_toolset("codeact").is_none());
    assert!(disabled_registry.resolve_toolset("recipes").is_none());
    let disabled_names = default_toolsets(&config, None)
        .unwrap()
        .into_iter()
        .flat_map(|toolset| toolset.get_tools())
        .map(|tool| tool.name().to_string())
        .collect::<BTreeSet<_>>();
    assert!(!disabled_names.contains(starweaver_agent::RUN_CODE_TOOL_NAME));

    let mut selective = AgentSpec::default();
    inject_codeact_toolset(&mut selective, true);
    inject_codeact_toolset(&mut selective, true);
    assert_eq!(selective.toolsets, ["codeact", "recipes"]);
    inject_codeact_toolset(&mut selective, false);
    assert!(selective.toolsets.is_empty());

    selective.toolsets = vec![
        starweaver_agent::CODEACT_TOOLSET_ID.to_string(),
        starweaver_agent::RECIPE_TOOLSET_ID.to_string(),
    ];
    inject_codeact_toolset(&mut selective, false);
    assert!(selective.toolsets.is_empty());
}

#[test]
fn configured_mcp_tools_are_namespaced_directly_by_default() {
    let mut config = test_config();
    config.mcp_config = json!({
        "servers": {
            "docs": {
                "transport": "stdio",
                "command": "docs-mcp",
                "tools": [{"name": "lookup", "parameters": {"type": "object"}}]
            }
        }
    });

    let names = default_toolsets(&config, None)
        .unwrap()
        .into_iter()
        .flat_map(|toolset| toolset.get_tools())
        .map(|tool| tool.name().to_string())
        .collect::<BTreeSet<_>>();

    assert!(names.contains("docs_lookup"));
    assert!(!names.contains("lookup"));
    assert!(!names.contains("mcp_search_tool"));
    assert!(!names.contains("mcp_call_tool"));
}

#[test]
fn configured_mcp_proxy_mode_keeps_fixed_tool_surface() {
    let mut config = test_config();
    config.tools_config = json!({"tools": {"mcp_mode": "proxy"}});
    config.mcp_config = json!({
        "servers": {
            "docs": {
                "transport": "stdio",
                "command": "docs-mcp",
                "tools": [{"name": "lookup", "parameters": {"type": "object"}}]
            }
        }
    });

    let names = default_toolsets(&config, None)
        .unwrap()
        .into_iter()
        .flat_map(|toolset| toolset.get_tools())
        .map(|tool| tool.name().to_string())
        .collect::<BTreeSet<_>>();

    assert!(names.contains("mcp_search_tool"));
    assert!(names.contains("mcp_call_tool"));
    assert!(!names.contains("docs_lookup"));
}

#[test]
fn configured_mcp_tools_reject_exposed_name_collisions() {
    let mut config = test_config();
    config.mcp_config = json!({
        "servers": {
            "a": {
                "transport": "stdio",
                "command": "a-mcp",
                "tools": [{"name": "b_c", "parameters": {"type": "object"}}]
            },
            "a_b": {
                "transport": "stdio",
                "command": "a-b-mcp",
                "tools": [{"name": "c", "parameters": {"type": "object"}}]
            }
        }
    });

    let Err(error) = default_toolsets(&config, None) else {
        panic!("colliding MCP tools must fail closed");
    };

    assert!(
        error
            .to_string()
            .contains("configured MCP tool name \"a_b_c\" conflicts")
    );
}

#[test]
fn computer_use_injection_is_idempotent_for_name_stable_id_and_all_toolsets() {
    let mut named = AgentSpec {
        toolsets: vec!["computer_use".to_string()],
        ..AgentSpec::default()
    };
    inject_computer_use_toolset(&mut named, true);
    assert_eq!(named.toolsets, ["computer_use"]);

    let mut stable_id = AgentSpec {
        toolsets: vec![starweaver_computer_use::COMPUTER_USE_TOOLSET_ID.to_string()],
        ..AgentSpec::default()
    };
    inject_computer_use_toolset(&mut stable_id, true);
    assert_eq!(stable_id.toolsets.len(), 1);

    let mut all = AgentSpec {
        all_toolsets: true,
        ..AgentSpec::default()
    };
    inject_computer_use_toolset(&mut all, true);
    assert!(all.toolsets.is_empty());
}

#[test]
#[allow(clippy::too_many_lines)]
fn computer_use_is_default_denied_and_enabled_config_auto_injects_full_tools() {
    let mut config = test_config();
    assert!(
        crate::computer_use::CliComputerUseCoordinator::from_config(&config.computer_use())
            .unwrap()
            .is_none()
    );
    let disabled =
        resolve_profile_with_computer_use(&config, Some("approval_model"), None).unwrap();
    assert!(
        !disabled
            .spec
            .toolsets
            .iter()
            .any(|name| name == "computer_use")
    );

    config.set_computer_use(&crate::config::CliComputerUseConfig {
        enabled: true,
        ..crate::config::CliComputerUseConfig::default()
    });
    let coordinator =
        crate::computer_use::CliComputerUseCoordinator::from_config(&config.computer_use())
            .unwrap()
            .unwrap();
    let registry = default_registry(&config, &AgentSpec::default(), Some(&coordinator)).unwrap();
    let toolset = registry.resolve_toolset("computer_use").unwrap();
    let tool_names = toolset
        .get_tools()
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect::<Vec<_>>();
    if cfg!(target_os = "macos") {
        assert_eq!(
            tool_names,
            [
                "computer_status",
                "computer_observe",
                "computer_click",
                "computer_move_pointer",
                "computer_drag",
                "computer_scroll",
                "computer_type_text",
                "computer_press_keys",
            ]
        );
    } else {
        assert!(tool_names.is_empty());
    }

    let profile =
        resolve_profile_with_computer_use(&config, Some("approval_model"), Some(coordinator))
            .unwrap();
    assert!(
        profile
            .spec
            .toolsets
            .iter()
            .any(|name| name == "computer_use")
    );
    let materialization = profile
        .spec
        .resolved_materialization(&profile.registry, "test-policy", "test-environment")
        .unwrap();
    assert!(
        materialization
            .toolset_ids
            .iter()
            .any(|id| id == starweaver_computer_use::COMPUTER_USE_TOOLSET_ID)
    );
    let agent = profile.build_agent().unwrap();
    assert_eq!(
        agent.tools().contains("computer_status"),
        cfg!(target_os = "macos")
    );
    assert_eq!(
        agent.tools().contains("computer_observe"),
        cfg!(target_os = "macos")
    );
    let mut context = AgentContext::default();
    profile.configure_context(&mut context);
    assert_eq!(
        context
            .named_dependency::<starweaver_agent::ComputerObserveHandle>(
                starweaver_agent::COMPUTER_OBSERVE_CAPABILITY,
            )
            .is_some(),
        cfg!(target_os = "macos")
    );
    assert_eq!(
        context
            .named_dependency::<starweaver_agent::ComputerPointerHandle>(
                starweaver_agent::COMPUTER_POINTER_CAPABILITY,
            )
            .is_some(),
        cfg!(target_os = "macos")
    );
    assert_eq!(
        context
            .named_dependency::<starweaver_agent::ComputerKeyboardHandle>(
                starweaver_agent::COMPUTER_KEYBOARD_CAPABILITY,
            )
            .is_some(),
        cfg!(target_os = "macos")
    );
}

#[test]
fn resolve_profile_builds_configured_shell_review_handle() {
    let mut config = test_config();
    config.shell_review.enabled = true;
    config.shell_review.model = Some("local_echo".to_string());
    config.shell_review.on_needs_approval = "deny".to_string();
    config.shell_review.risk_threshold = "medium".to_string();
    config.shell_review.system_prompt = Some("Custom shell review prompt".to_string());

    let profile = resolve_profile(&config, Some("general")).unwrap();
    let Some(handle) = profile.shell_review else {
        panic!("shell review handle");
    };

    assert!(handle.config().enabled);
    assert_eq!(handle.config().on_needs_approval, ShellReviewAction::Deny);
    assert_eq!(handle.config().risk_threshold, ShellReviewRiskLevel::Medium);
    assert_eq!(
        handle.config().system_prompt.as_deref(),
        Some("Custom shell review prompt")
    );
    assert!(handle.config().model.is_some());
}

#[test]
fn subagent_model_settings_default_to_parent_settings() {
    let inherited = ModelSettings {
        provider_options: Some(json!({"store": false})),
        temperature: Some(0.2),
        ..ModelSettings::default()
    };

    let inherited_settings = resolve_subagent_model_settings(None, Some(&inherited)).unwrap();
    assert_eq!(inherited_settings, Some(inherited.clone()));

    let explicit_inherit =
        resolve_subagent_model_settings(Some(&json!("inherit")), Some(&inherited)).unwrap();
    assert_eq!(explicit_inherit, Some(inherited));
}

#[test]
fn subagent_model_settings_can_override_with_preset_or_inline_object() {
    let Some(preset) =
        resolve_subagent_model_settings(Some(&json!("openai_responses_high")), None).unwrap()
    else {
        panic!("settings preset");
    };
    assert_eq!(preset.provider_options.unwrap()["store"], false);

    let Some(inline) = resolve_subagent_model_settings(
        Some(&json!({
            "provider_options": {"store": false},
            "temperature": 0.1
        })),
        None,
    )
    .unwrap() else {
        panic!("inline settings");
    };
    assert_eq!(inline.provider_options.unwrap()["store"], false);
    assert_eq!(inline.temperature, Some(0.1));
}

#[test]
fn subagent_model_config_defaults_to_parent_config() {
    let inherited = ModelConfig {
        context_window: Some(123_456),
        ..ModelConfig::default()
    };

    let resolved =
        resolve_subagent_model_config(None, Some(&inherited), Some("claude_200k")).unwrap();
    assert_eq!(resolved.context, Some(inherited));
    assert_eq!(resolved.preset.as_deref(), Some("claude_200k"));
}

#[test]
fn subagent_model_config_can_override_with_preset_or_inline_object() {
    let preset = resolve_subagent_model_config(Some(&json!("claude_200k")), None, None).unwrap();
    assert_eq!(preset.context.unwrap().context_window, Some(200_000));
    assert_eq!(preset.preset.as_deref(), Some("claude_200k"));

    let inline = resolve_subagent_model_config(
        Some(&json!({"context_window": 42_000, "max_images": 3})),
        None,
        None,
    )
    .unwrap();
    let Some(context) = inline.context else {
        panic!("inline config");
    };
    assert_eq!(context.context_window, Some(42_000));
    assert_eq!(context.max_images, 3);
    assert!(inline.preset.is_none());
}

#[test]
fn openai_responses_ws_model_id_prefers_websocket_with_http_fallback() {
    let parsed = ProviderModelId::parse("openai-responses-ws:gpt-5").unwrap();

    assert_eq!(parsed.provider, "openai");
    assert_eq!(parsed.model_name, "gpt-5");
    assert_eq!(parsed.protocol, ProtocolFamily::OpenAiResponses);
    assert_eq!(parsed.gateway_name, None);
    assert_eq!(parsed.oauth_provider, None);
    assert_eq!(parsed.stream_transport, Some(ResponseStreamTransport::Auto));
}

#[test]
fn gateway_openai_responses_ws_model_id_prefers_websocket_with_http_fallback() {
    let parsed = ProviderModelId::parse("homelab@openai-responses-ws:gpt-5").unwrap();

    assert_eq!(parsed.provider, "openai");
    assert_eq!(parsed.model_name, "gpt-5");
    assert_eq!(parsed.protocol, ProtocolFamily::OpenAiResponses);
    assert_eq!(parsed.gateway_name.as_deref(), Some("homelab"));
    assert_eq!(parsed.oauth_provider, None);
    assert_eq!(parsed.stream_transport, Some(ResponseStreamTransport::Auto));
}

#[test]
fn openai_responses_ws_transport_default_can_be_overridden_by_settings() {
    let base = openai_responses_stream_transport_settings(ResponseStreamTransport::Auto);
    let overlay = ModelSettings {
        provider_settings: ProviderSettings {
            openai_responses: Some(OpenAiResponsesSettings {
                stream_transport: Some(ResponseStreamTransport::Http),
                ..OpenAiResponsesSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };

    let merged = base.merge(&overlay);

    assert_eq!(
        merged
            .provider_settings
            .openai_responses
            .unwrap()
            .stream_transport,
        Some(ResponseStreamTransport::Http)
    );
}

#[test]
fn anthropic_gateway_endpoint_uses_v1_when_base_url_has_no_sub_path() {
    let mut http_config = anthropic_http_config("test-key");
    let provider_config = ProviderConfig {
        base_url: Some("http://localhost:8090".to_string()),
        ..ProviderConfig::default()
    };

    apply_provider_http_config_overrides(&mut http_config, &provider_config);

    assert_eq!(http_config.base_url, "http://localhost:8090");
    assert_eq!(http_config.endpoint_path, "messages");
    assert_eq!(
        http_config.endpoint_url(),
        "http://localhost:8090/v1/messages"
    );
}

#[test]
fn anthropic_gateway_endpoint_uses_messages_when_base_url_has_sub_path() {
    let mut http_config = anthropic_http_config("test-key");
    let provider_config = ProviderConfig {
        base_url: Some("http://localhost:8090/abc".to_string()),
        ..ProviderConfig::default()
    };

    apply_provider_http_config_overrides(&mut http_config, &provider_config);

    assert_eq!(http_config.endpoint_path, "messages");
    assert_eq!(
        http_config.endpoint_url(),
        "http://localhost:8090/abc/messages"
    );
}

#[test]
fn anthropic_gateway_endpoint_keeps_explicit_endpoint_path() {
    let mut http_config = anthropic_http_config("test-key");
    let provider_config = ProviderConfig {
        base_url: Some("http://localhost:8090".to_string()),
        endpoint_path: Some("custom/messages".to_string()),
        ..ProviderConfig::default()
    };

    apply_provider_http_config_overrides(&mut http_config, &provider_config);

    assert_eq!(http_config.endpoint_path, "custom/messages");
    assert_eq!(
        http_config.endpoint_url(),
        "http://localhost:8090/custom/messages"
    );
}

#[test]
fn anthropic_endpoint_sub_path_detection_handles_root_and_existing_v1() {
    assert_eq!(
        anthropic_http_config("test-key")
            .with_base_url("https://gateway.example")
            .endpoint_url(),
        "https://gateway.example/v1/messages"
    );
    assert_eq!(
        anthropic_http_config("test-key")
            .with_base_url("https://gateway.example/")
            .endpoint_url(),
        "https://gateway.example/v1/messages"
    );
    assert_eq!(
        anthropic_http_config("test-key")
            .with_base_url("https://gateway.example/v1")
            .endpoint_url(),
        "https://gateway.example/v1/messages"
    );
}

#[tokio::test]
async fn configured_subagent_delegate_inherits_parent_model_settings_and_config() {
    let config = test_config();
    let inherited_settings = get_model_settings("openai_responses_high").unwrap();
    let Some(inherited_config) = resolve_inherited_model_config(Some("claude_200k")).unwrap()
    else {
        panic!("parent config");
    };
    let spec =
        starweaver_core::SubagentSpec::new("child", "Child helper", "You are a child helper.")
            .with_tools(Vec::new());
    let child_config = build_subagent_config(
        &config,
        &spec,
        "capture_subagent_inheritance",
        Some(&inherited_settings),
        Some(&inherited_config),
        Some("claude_200k"),
    )
    .unwrap();
    let registry = SubagentRegistry::new().with_subagent(child_config);
    let mut context = AgentContext::default();

    let result = registry
        .delegate_task("child", SubagentTask::new("hello"), &mut context)
        .await
        .unwrap();

    assert_eq!(result.output(), "captured: hello");
    let lifecycle_events = context
        .events
        .events()
        .iter()
        .filter(|event| event.kind == "subagent_started" || event.kind == "subagent_completed")
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle_events,
        vec!["subagent_started", "subagent_completed"]
    );
}

#[test]
fn cli_model_catalog_exposes_query_only_session_tools() {
    let config = test_config();
    let names = list_default_tools(&config)
        .unwrap()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    for name in [
        "list_sessions",
        "get_session",
        "list_session_runs",
        "get_session_run",
        "replay_session_run",
    ] {
        assert!(names.contains(name), "missing CLI query tool {name}");
    }
    assert!(
        names.contains(starweaver_agent::ASK_USER_QUESTION_TOOL_NAME),
        "CLI must explicitly expose the clarifying-question tool"
    );
    for name in [
        "create_session",
        "update_session",
        "delete_session",
        "start_session_run",
        "steer_session_run",
        "interrupt_session_run",
    ] {
        assert!(
            !names.contains(name),
            "CLI must not expose control tool {name}"
        );
    }
}
