#![allow(missing_docs, clippy::unwrap_used)]

use std::{fs, sync::Arc};

use starweaver_agent::{
    load_subagent_from_file, load_subagents_from_dir, parse_subagent_markdown, AgentBuilder,
    SubagentConfigError, SubagentSpec, TestModel,
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
