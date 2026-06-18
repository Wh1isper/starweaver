#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    parse_skill_markdown, skill_tools, SkillRegistry, SkillReloadBinding, SkillReloadChangeKind,
    SkillReloadReason, SkillReloadSchedule, SkillScanDiagnosticKind, SkillSourceScope,
    SKILL_ACTIVATION_EVENT_KIND, SKILL_RELOAD_EVENT_KIND, SKILL_SCAN_EVENT_KIND,
};
use starweaver_agent::{AgentBuilder, AgentContext, AgentStreamEvent, FunctionModel, SkillPackage};
use starweaver_environment::{EnvironmentProvider, VirtualEnvironmentProvider};
use starweaver_model::ModelResponse;

#[tokio::test]
async fn skill_registry_scans_summaries_and_activates_bodies() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("skills")
            .with_file(
                "skills/research/SKILL.md",
                r"---
name: research
description: Gather sources
category: web
---
Use search and cite sources.
",
            )
            .with_file(
                ".agents/skills/debug/SKILL.md",
                r"---
name: debug
description: Diagnose failures
---
Inspect errors.
",
            ),
    );
    let registry = SkillRegistry::scan(provider.clone(), &[SkillSourceScope::new("")])
        .await
        .unwrap();
    let active = SkillRegistry::activate(provider, "skills/research/SKILL.md")
        .await
        .unwrap();

    assert_eq!(registry.packages().len(), 2);
    assert!(registry.get("research").unwrap().body.is_none());
    assert_eq!(active.body.unwrap(), "Use search and cite sources.");
    assert_eq!(active.metadata["category"], "web");
    assert!(parse_skill_markdown("bad/SKILL.md", "plain").is_err());
    assert_eq!(skill_tools(registry.packages()).get_instructions().len(), 1);
}

#[tokio::test]
async fn skill_activation_can_publish_context_event() {
    let provider = Arc::new(VirtualEnvironmentProvider::new("skills").with_file(
        "skills/research/SKILL.md",
        r"---
name: research
description: Gather sources
category: web
---
Use search and cite sources.
",
    ));
    let mut context = AgentContext::default();

    let active =
        SkillRegistry::activate_with_context(provider, "skills/research/SKILL.md", &mut context)
            .await
            .unwrap();

    assert_eq!(active.name, "research");
    let event = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == SKILL_ACTIVATION_EVENT_KIND)
        .unwrap();
    assert_eq!(event.payload["name"], "research");
    assert_eq!(event.payload["path"], "skills/research/SKILL.md");
    assert_eq!(event.payload["body_bytes"], 28);
    assert!(event.payload.get("body").is_none());
}

#[tokio::test]
async fn skill_registry_reports_duplicates_and_invalid_files() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("skills")
            .with_file(
                ".agents/skills/research/SKILL.md",
                r"---
name: research
description: Shared research
---
Shared instructions.
",
            )
            .with_file(
                "skills/research/SKILL.md",
                r"---
name: research
description: Tool-specific research
---
Tool-specific instructions.
",
            )
            .with_file(
                "skills/broken/SKILL.md",
                r"---
name: broken
---
Missing a description.
",
            ),
    );

    let report = SkillRegistry::scan_with_report(provider, &[SkillSourceScope::new("")])
        .await
        .unwrap();

    let registry = report.registry();
    let research = registry.get("research").unwrap();
    assert_eq!(research.description, "Tool-specific research");
    assert_eq!(research.path, "skills/research/SKILL.md");
    assert!(registry.get("broken").is_none());
    assert!(report.diagnostics().iter().any(|diagnostic| {
        diagnostic.kind == SkillScanDiagnosticKind::DuplicateOverridden
            && diagnostic.name.as_deref() == Some("research")
            && diagnostic.path == "skills/research/SKILL.md"
            && diagnostic.previous_path.as_deref() == Some(".agents/skills/research/SKILL.md")
    }));
    assert!(report.diagnostics().iter().any(|diagnostic| {
        diagnostic.kind == SkillScanDiagnosticKind::InvalidSkill
            && diagnostic.path == "skills/broken/SKILL.md"
    }));
}

#[tokio::test]
async fn skill_source_precedence_is_deterministic_across_scopes() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("skills")
            .with_file(
                "user/skills/research/SKILL.md",
                r"---
name: research
description: User tool skill
---
User instructions.
",
            )
            .with_file(
                "workspace/.agents/skills/research/SKILL.md",
                r"---
name: research
description: Workspace shared skill
---
Workspace instructions.
",
            ),
    );

    let report = SkillRegistry::scan_with_report(
        provider,
        &[
            SkillSourceScope::workspace_shared("workspace"),
            SkillSourceScope::user_tool("user"),
        ],
    )
    .await
    .unwrap();

    let research = report.registry().get("research").unwrap();
    assert_eq!(research.description, "Workspace shared skill");
    assert_eq!(research.path, "workspace/.agents/skills/research/SKILL.md");
    assert!(report.diagnostics().iter().any(|diagnostic| {
        diagnostic.kind == SkillScanDiagnosticKind::DuplicateOverridden
            && diagnostic.previous_path.as_deref() == Some("user/skills/research/SKILL.md")
    }));
}

#[tokio::test]
async fn skill_registry_reload_reports_added_removed_and_modified_skills() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("skills")
            .with_file(
                "skills/research/SKILL.md",
                r"---
name: research
description: Gather sources
---
Use search and cite sources.
",
            )
            .with_file(
                "skills/debug/SKILL.md",
                r"---
name: debug
description: Diagnose failures
---
Inspect errors.
",
            ),
    );
    let registry = SkillRegistry::scan(provider.clone(), &[SkillSourceScope::new("")])
        .await
        .unwrap();

    provider
        .write_text(
            "skills/research/SKILL.md",
            r"---
name: research
description: Gather sources with citations
---
Use search and cite sources.
",
        )
        .await
        .unwrap();
    provider
        .write_text(
            "skills/plan/SKILL.md",
            r"---
name: plan
description: Build a plan
---
Plan the work.
",
        )
        .await
        .unwrap();
    provider
        .delete_path("skills/debug/SKILL.md", false)
        .await
        .unwrap();

    let mut context = AgentContext::default();
    let report = registry
        .reload_with_context(provider, &[SkillSourceScope::new("")], &mut context)
        .await
        .unwrap();

    assert_eq!(report.registry().packages().len(), 2);
    assert_eq!(
        report.registry().get("research").unwrap().description,
        "Gather sources with citations"
    );
    assert!(report.registry().get("debug").is_none());
    assert!(report.registry().get("plan").is_some());
    assert!(report.changes().iter().any(|change| {
        change.kind == SkillReloadChangeKind::Added
            && change.name == "plan"
            && change.path.as_deref() == Some("skills/plan/SKILL.md")
    }));
    assert!(report.changes().iter().any(|change| {
        change.kind == SkillReloadChangeKind::Modified
            && change.name == "research"
            && change.previous_path.as_deref() == Some("skills/research/SKILL.md")
            && change.path.as_deref() == Some("skills/research/SKILL.md")
    }));
    assert!(report.changes().iter().any(|change| {
        change.kind == SkillReloadChangeKind::Removed
            && change.name == "debug"
            && change.previous_path.as_deref() == Some("skills/debug/SKILL.md")
            && change.path.is_none()
    }));

    let event = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == SKILL_RELOAD_EVENT_KIND)
        .unwrap();
    assert_eq!(event.payload["package_count"], 2);
    assert_eq!(event.payload["changes"].as_array().unwrap().len(), 3);
    assert!(event.payload.get("body").is_none());
}

#[tokio::test]
async fn skill_registry_reload_binding_runs_after_watcher_debounce() {
    let provider = Arc::new(VirtualEnvironmentProvider::new("skills").with_file(
        "skills/research/SKILL.md",
        r"---
name: research
description: Gather sources
---
Use search.
",
    ));
    let registry = SkillRegistry::scan(provider.clone(), &[SkillSourceScope::new("")])
        .await
        .unwrap();
    provider
        .write_text(
            "skills/research/SKILL.md",
            r"---
name: research
description: Gather sources with citations
---
Use search and cite sources.
",
        )
        .await
        .unwrap();
    let mut context = AgentContext::default();
    let mut binding = SkillReloadBinding::new(SkillReloadSchedule::new().debounce(50));

    binding.observe_inventory_version("skills-v2", 100);
    let waiting = registry
        .reload_with_context_if_due(
            provider.clone(),
            &[SkillSourceScope::new("")],
            &mut context,
            &mut binding,
            120,
        )
        .await
        .unwrap();

    assert!(!waiting.reloaded());
    assert_eq!(waiting.inventory_version.as_deref(), Some("skills-v2"));
    assert!(context.events.events().is_empty());

    let reloaded = registry
        .reload_with_context_if_due(
            provider,
            &[SkillSourceScope::new("")],
            &mut context,
            &mut binding,
            150,
        )
        .await
        .unwrap();

    assert!(reloaded.reloaded());
    assert_eq!(
        reloaded.decision.reason,
        Some(SkillReloadReason::InventoryChanged)
    );
    assert_eq!(
        binding.state.last_inventory_version.as_deref(),
        Some("skills-v2")
    );
    let report = reloaded.reload.unwrap();
    assert_eq!(
        report.registry().get("research").unwrap().description,
        "Gather sources with citations"
    );
    let event = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == SKILL_RELOAD_EVENT_KIND)
        .unwrap();
    assert_eq!(event.payload["reload_scheduled"], true);
    assert_eq!(event.payload["reload_reason"], "inventory_changed");
    assert_eq!(event.payload["inventory_version"], "skills-v2");
    assert_eq!(event.payload["reload_ms"], 150);
    assert!(event.payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| { change["kind"] == "modified" && change["name"] == "research" }));
}

#[tokio::test]
async fn builder_skills_installs_instructions_and_context_discovery() {
    let mut registry = SkillRegistry::new();
    registry.insert(SkillPackage {
        name: "research".to_string(),
        description: "Gather sources".to_string(),
        path: "skills/research/SKILL.md".to_string(),
        body: None,
        metadata: serde_json::Map::new(),
    });
    let model = FunctionModel::new(|messages, _settings, _info| {
        let debug = format!("{messages:?}");
        assert!(debug.contains("Available fileops-loaded skills"));
        assert!(debug.contains("research"));
        Ok(ModelResponse::text("ok"))
    });
    let mut context = AgentContext::default();

    let result = AgentBuilder::new(Arc::new(model))
        .skills(registry)
        .build()
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let relaxed_patterns = context
        .tool_config
        .view_relaxed_text_dynamic_patterns
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    assert!(relaxed_patterns
        .iter()
        .any(|pattern| pattern.contains("skills/research")));
}

#[tokio::test]
async fn builder_skills_report_emits_scan_and_activation_events() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("skills")
            .with_file(
                "skills/research/SKILL.md",
                r"---
name: research
description: Gather sources
---
Use search and cite sources.
",
            )
            .with_file(
                "skills/broken/SKILL.md",
                r"---
name: broken
---
Missing a description.
",
            ),
    );
    let mut report =
        SkillRegistry::scan_with_report(provider.clone(), &[SkillSourceScope::new("")])
            .await
            .unwrap();
    let active = SkillRegistry::activate(provider, "skills/research/SKILL.md")
        .await
        .unwrap();
    report.registry.insert(active);
    let model = FunctionModel::new(|_messages, _settings, _info| Ok(ModelResponse::text("ok")));

    let stream = AgentBuilder::new(Arc::new(model))
        .skills_report(report)
        .build()
        .run_stream("hello")
        .await
        .unwrap();

    let custom_events = stream
        .events()
        .iter()
        .filter_map(|record| match &record.event {
            AgentStreamEvent::Custom { event } => Some(event),
            _ => None,
        })
        .collect::<Vec<_>>();
    let scan = custom_events
        .iter()
        .find(|event| event.kind == SKILL_SCAN_EVENT_KIND)
        .unwrap();
    assert_eq!(scan.payload["package_count"], 1);
    assert_eq!(scan.payload["packages"][0]["activated"], true);
    assert_eq!(scan.payload["diagnostics"][0]["kind"], "invalid_skill");

    let activation = custom_events
        .iter()
        .find(|event| event.kind == SKILL_ACTIVATION_EVENT_KIND)
        .unwrap();
    assert_eq!(activation.payload["name"], "research");
    assert_eq!(activation.payload["body_bytes"], 28);
    assert!(activation.payload.get("body").is_none());
}
