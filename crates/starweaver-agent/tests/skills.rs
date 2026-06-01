#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{parse_skill_markdown, skill_tools, SkillRegistry, SkillSourceScope};
use starweaver_environment::VirtualEnvironmentProvider;

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
