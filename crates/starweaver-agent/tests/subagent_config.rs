#![allow(missing_docs, clippy::unwrap_used)]

use std::{fs, sync::Arc};

use starweaver_agent::{
    load_subagent_from_file, load_subagents_from_dir, parse_subagent_markdown,
    project_subagent_spec, AgentBuilder, AgentContext, AgentSpecRegistry, SubagentConfig,
    SubagentConfigError, SubagentRegistry, SubagentSpec, TestModel,
};

#[test]
fn parses_subagent_markdown_frontmatter() {
    let spec = parse_subagent_markdown(
        r"
---
name: debugger
description: Debug code issues
instruction: Use this agent for debugging tasks.
tools:
  - grep
  - view
optional_tools: edit, shell
denied_tools:
  - shell_kill
inherit_hooks: true
inherit_capabilities: true
denied_capabilities:
  - unsafe-trace
model: anthropic:claude-sonnet-4
model_settings:
  temperature: 0.2
model_cfg: claude_200k
metadata:
  tier: builtin
---
You are a debugging expert.
",
    )
    .unwrap();

    assert_eq!(spec.name, "debugger");
    assert_eq!(spec.description, "Debug code issues");
    assert_eq!(
        spec.instruction.as_deref(),
        Some("Use this agent for debugging tasks.")
    );
    assert_eq!(spec.system_prompt, "You are a debugging expert.");
    assert_eq!(spec.tools, vec!["grep", "view"]);
    assert_eq!(spec.optional_tools, vec!["edit", "shell"]);
    assert_eq!(
        spec.metadata["denied_tools"],
        serde_json::json!(["shell_kill"])
    );
    assert_eq!(spec.metadata["inherit_hooks"], true);
    assert_eq!(spec.metadata["inherit_capabilities"], true);
    assert_eq!(
        spec.metadata["denied_capabilities"],
        serde_json::json!(["unsafe-trace"])
    );
    assert_eq!(spec.model.as_deref(), Some("anthropic:claude-sonnet-4"));
    assert_eq!(spec.model_settings.unwrap()["temperature"], 0.2);
    assert_eq!(spec.model_config.unwrap(), "claude_200k");
    assert_eq!(spec.metadata["tier"], "builtin");
}

#[test]
fn parses_comma_separated_required_tools() {
    let spec = parse_subagent_markdown(
        r"
---
name: searcher
description: Search specialist
tools: search, scrape, fetch
---
Find facts and sources.
",
    )
    .unwrap();

    assert_eq!(spec.tools, vec!["search", "scrape", "fetch"]);
    assert!(spec.optional_tools.is_empty());
}

#[test]
fn projects_subagent_spec_into_agent_spec_and_inheritance_policy() {
    let spec = parse_subagent_markdown(
        r"
---
name: debugger
description: Debug code issues
instruction: Use this agent for debugging tasks.
tools:
  - grep
optional_tools: view, edit
denied_tools:
  - shell
inherit_hooks: true
inherit_capabilities: true
denied_capabilities:
  - pii-audit
model: inherit
model_settings:
  temperature: 0.1
model_config: claude
---
Debug carefully.
",
    )
    .unwrap();

    let projection = project_subagent_spec(&spec, Some("parent-model")).unwrap();

    assert_eq!(projection.agent_spec.name, "debugger");
    assert_eq!(
        projection.agent_spec.description.as_deref(),
        Some("Debug code issues")
    );
    assert_eq!(projection.agent_spec.instructions, vec!["Debug carefully."]);
    let model = projection.agent_spec.model.as_ref().unwrap();
    assert_eq!(model.model_id, "parent-model");
    assert_eq!(model.settings.as_ref().unwrap().temperature, Some(0.1));
    assert_eq!(model.config_preset.as_deref(), Some("claude"));
    assert_eq!(
        projection.agent_spec.metadata["subagent_instruction"],
        "Use this agent for debugging tasks."
    );
    assert_eq!(
        projection.tool_inheritance.required_tools,
        vec!["grep".to_string()]
    );
    assert_eq!(
        projection.tool_inheritance.optional_tools,
        vec!["view".to_string(), "edit".to_string()]
    );
    assert_eq!(
        projection.tool_inheritance.denied_tools,
        vec!["shell".to_string()]
    );
    assert!(!projection.tool_inheritance.inherit_all_when_empty);
    assert!(projection.capability_inheritance.hooks);
    assert!(projection.capability_inheritance.capability_bundles);
    assert_eq!(
        projection.capability_inheritance.denied_capabilities,
        vec!["pii-audit".to_string()]
    );
}

#[test]
fn subagent_spec_projection_requires_inherited_model_for_inherit() {
    let spec = SubagentSpec::new("child", "Child", "Help");

    let error = project_subagent_spec(&spec, None).unwrap_err();

    assert!(matches!(error, SubagentConfigError::MissingInheritedModel));
}

#[test]
fn subagent_spec_projection_inherits_all_tools_when_tool_lists_are_empty() {
    let mut spec = SubagentSpec::new("child", "Child", "Help");
    spec.model = Some("child-model".to_string());

    let projection = project_subagent_spec(&spec, None).unwrap();

    assert!(projection.tool_inheritance.inherit_all_when_empty);
    assert!(!projection.capability_inheritance.hooks);
    assert!(!projection.capability_inheritance.capability_bundles);
}

#[tokio::test]
async fn projected_subagent_spec_materializes_runnable_subagent_config() {
    let spec = parse_subagent_markdown(
        r"
---
name: researcher
description: Research specialist
model: child-model
---
Gather facts.
",
    )
    .unwrap();
    let projection = project_subagent_spec(&spec, None).unwrap();
    let registry = AgentSpecRegistry::new().with_model(
        "child-model",
        Arc::new(TestModel::with_text("child output")),
    );
    let config = SubagentConfig::from_agent_spec(
        &projection.agent_spec,
        &registry,
        projection.tool_inheritance,
    )
    .unwrap();
    let subagents = SubagentRegistry::new().with_subagent(config);
    let mut context = AgentContext::default();

    let result = subagents
        .delegate("researcher", "collect facts", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child output");
    assert_eq!(context.subagent_history.len(), 1);
}

#[test]
fn reports_missing_frontmatter_and_required_fields() {
    let missing_frontmatter = parse_subagent_markdown("plain markdown").unwrap_err();
    assert!(matches!(
        missing_frontmatter,
        SubagentConfigError::MissingFrontmatter
    ));

    let missing_name = parse_subagent_markdown(
        r"
---
description: Missing name
---
Body
",
    )
    .unwrap_err();
    assert!(matches!(
        missing_name,
        SubagentConfigError::MissingField("name")
    ));
}

#[test]
fn loads_subagent_from_file_and_directory() {
    let dir = std::env::temp_dir().join(format!(
        "starweaver-subagents-{}",
        starweaver_agent::TaskId::new().as_str()
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("debugger.md"),
        r"
---
name: debugger
description: Debug code issues
---
Debug carefully.
",
    )
    .unwrap();
    fs::write(dir.join("notes.txt"), "ignored").unwrap();

    let spec = load_subagent_from_file(dir.join("debugger.md")).unwrap();
    let specs = load_subagents_from_dir(&dir).unwrap();

    assert_eq!(spec.name, "debugger");
    assert_eq!(specs, vec![spec]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn serializable_subagent_spec_stays_runtime_free() {
    let spec = SubagentSpec::new("researcher", "Research specialist", "Gather facts")
        .with_tools(vec!["search".to_string()])
        .with_optional_tools(vec!["fetch".to_string()]);

    let encoded = serde_json::to_value(&spec).unwrap();
    let decoded: SubagentSpec = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, spec);
    assert!(encoded.get("agent").is_none());
}

#[test]
fn runtime_subagent_config_keeps_agent_handle_programmatic() {
    let agent = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let config =
        starweaver_agent::SubagentConfig::new("child", agent).with_description("Child helper");

    assert_eq!(config.name, "child");
    assert_eq!(config.description.as_deref(), Some("Child helper"));
}
