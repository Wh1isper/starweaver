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
