#![allow(missing_docs, clippy::unwrap_used)]

use starweaver_context::{
    AgentContext, AgentId, ModelCapability, ModelConfig, Ratio, ResumableState, ToolConfig,
};

#[test]
fn agent_context_has_default_model_and_tool_config() {
    let context = AgentContext::default();

    assert_eq!(
        context.model_config.compact_threshold.parts_per_thousand(),
        900
    );
    assert_eq!(context.model_config.max_images, 20);
    assert_eq!(
        context.tool_config.view_max_text_file_size,
        10 * 1024 * 1024
    );
    assert_eq!(context.tool_config.view_relaxed_line_limit, 5000);
    assert_eq!(context.tool_config.cold_start_tool_return_limit, 500);
}

#[test]
fn config_round_trips_through_resumable_state() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.model_config.context_window = Some(1_000_000);
    context
        .model_config
        .capabilities
        .insert(ModelCapability::Vision);
    context.tool_config.view_relaxed_text_patterns = vec!["AGENTS.md".to_string()];
    context.tool_config.view_relaxed_line_limit = 6000;

    let restored = AgentContext::from_state(context.export_state());

    assert_eq!(restored.model_config.context_window, Some(1_000_000));
    assert!(restored.model_config.has_vision());
    assert_eq!(
        restored.tool_config.view_relaxed_text_patterns,
        vec!["AGENTS.md"]
    );
    assert_eq!(restored.tool_config.view_relaxed_line_limit, 6000);
}

#[test]
fn dynamic_relaxed_view_patterns_are_runtime_only() {
    let mut context = AgentContext::default();
    context.tool_config.register_view_relaxed_text_patterns(
        "skills:test",
        vec!["re:^skills/demo/.*\\.md$".to_string()],
    );
    assert_eq!(context.tool_config.view_relaxed_text_patterns().len(), 1);

    let encoded = serde_json::to_string(&context.export_state()).unwrap();
    assert!(!encoded.contains("skills:test"));
    assert!(!encoded.contains("view_relaxed_text_dynamic_patterns"));

    let restored =
        AgentContext::from_state(serde_json::from_str::<ResumableState>(&encoded).unwrap());
    assert!(restored.tool_config.view_relaxed_text_patterns().is_empty());
}

#[test]
fn subagent_context_inherits_model_tool_and_security_config() {
    let mut context = AgentContext::new(AgentId::from_string("parent"));
    context.model_config.context_window = Some(270_000);
    context.tool_config.view_relaxed_text_patterns = vec!["memory/**/*.md".to_string()];
    context.tool_config.view_relaxed_text_file_size = 60 * 1024 * 1024;

    let child = context.subagent_context("child");

    assert_eq!(child.model_config.context_window, Some(270_000));
    assert_eq!(
        child.tool_config.view_relaxed_text_patterns,
        vec!["memory/**/*.md"]
    );
    assert_eq!(
        child.tool_config.view_relaxed_text_file_size,
        60 * 1024 * 1024
    );
}

#[test]
fn merge_model_and_tool_config_updates_context_defaults() {
    let mut context = AgentContext::default();
    let model_config = ModelConfig {
        context_window: Some(123_000),
        compact_threshold: Ratio::from_parts_per_thousand(875),
        ..ModelConfig::default()
    };
    let tool_config = ToolConfig {
        download_max_concurrency: 0,
        view_relaxed_text_patterns: vec!["AGENTS.md".to_string()],
        ..ToolConfig::default()
    };

    context.merge_model_config(model_config);
    context.merge_tool_config(tool_config);

    assert_eq!(context.model_config.context_window, Some(123_000));
    assert_eq!(
        context.model_config.compact_threshold.parts_per_thousand(),
        875
    );
    assert_eq!(context.tool_config.download_max_concurrency, 1);
    assert_eq!(
        context.tool_config.view_relaxed_text_patterns,
        vec!["AGENTS.md"]
    );
}
