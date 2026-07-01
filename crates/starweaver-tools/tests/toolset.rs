#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_context::AgentContext;
use starweaver_core::AgentId;
use starweaver_tools::{
    DynToolset, FunctionTool, StaticToolset, TOOLSET_INITIALIZED_EVENT_KIND,
    TOOLSET_UNAVAILABLE_EVENT_KIND, ToolContext, ToolInstruction, ToolRegistry, ToolResult,
    Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy, ToolsetPreparation,
};

#[test]
fn registry_collects_toolsets_and_deduplicates_instructions() {
    let first = FunctionTool::new(
        "first",
        Some("First tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let second = FunctionTool::new(
        "second",
        Some("Second tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let toolset = StaticToolset::new("example")
        .with_tool(Arc::new(first))
        .with_tool(Arc::new(second))
        .with_instruction(ToolInstruction::new("example", "Use example tools."))
        .with_instruction(ToolInstruction::new("example", "Duplicate ignored."));

    let toolset: DynToolset = Arc::new(toolset);
    let registry = ToolRegistry::new().with_toolset(&toolset);

    assert_eq!(registry.definitions().len(), 2);
    assert_eq!(
        registry.get_instructions(),
        vec![
            "<tool-instruction name=\"example\">Use example tools.</tool-instruction>".to_string()
        ]
    );
    let instructions = registry.instructions();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].group, "example");
    assert_eq!(instructions[0].content, "Use example tools.");
}

struct ContextPreparedToolset;

#[async_trait::async_trait]
impl Toolset for ContextPreparedToolset {
    fn name(&self) -> &'static str {
        "context_tools"
    }

    fn id(&self) -> Option<&str> {
        Some("context.tools")
    }

    fn get_tools(&self) -> Vec<starweaver_tools::DynTool> {
        Vec::new()
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        assert_eq!(context.metadata["tenant"], "alpha");
        let tool = FunctionTool::new(
            "context_echo",
            Some("Context echo tool".to_string()),
            serde_json::json!({"type": "object"}),
            |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
        );
        Ok(ToolsetPreparation::initialized(
            self.name(),
            self.id().map(ToOwned::to_owned),
            vec![Arc::new(tool)],
            vec![ToolInstruction::new("context_tools", "Use context tools.")],
        ))
    }
}

#[tokio::test]
async fn registry_can_insert_context_prepared_toolset_and_publish_report() {
    let toolset: DynToolset = Arc::new(ContextPreparedToolset);
    let mut context = AgentContext::new(AgentId::from_string("agent"));
    context
        .metadata
        .insert("tenant".to_string(), serde_json::json!("alpha"));
    let mut registry = ToolRegistry::new();

    let report = registry
        .insert_toolset_with_context(&mut context, &toolset)
        .await
        .unwrap();

    assert_eq!(report.name, "context_tools");
    assert_eq!(report.id.as_deref(), Some("context.tools"));
    assert_eq!(report.tool_count, 1);
    assert!(registry.contains("context_echo"));
    assert_eq!(registry.instructions()[0].group, "context_tools");
    let events = context.events.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, TOOLSET_INITIALIZED_EVENT_KIND);
    assert_eq!(events[0].payload["name"], "context_tools");
    assert_eq!(events[0].payload["state"], "initialized");
    assert_eq!(events[0].payload["tool_count"], 1);
}

struct UnavailableToolset;

#[async_trait::async_trait]
impl Toolset for UnavailableToolset {
    fn name(&self) -> &'static str {
        "remote_tools"
    }

    fn get_tools(&self) -> Vec<starweaver_tools::DynTool> {
        Vec::new()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        ToolsetLifecyclePolicy::default().with_fail_on_unavailable(true)
    }

    async fn prepare_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        Ok(ToolsetPreparation::unavailable(
            self.name(),
            self.id().map(ToOwned::to_owned),
            "remote index offline",
        ))
    }
}

#[tokio::test]
async fn registry_reports_unavailable_context_toolset_before_failing_policy() {
    let toolset: DynToolset = Arc::new(UnavailableToolset);
    let mut context = AgentContext::new(AgentId::from_string("agent"));
    let mut registry = ToolRegistry::new();

    let error = registry
        .insert_toolset_with_context(&mut context, &toolset)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ToolsetLifecycleError::Unavailable {
            ref toolset,
            ref message
        } if toolset == "remote_tools" && message == "remote index offline"
    ));
    assert!(registry.is_empty());
    let events = context.events.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, TOOLSET_UNAVAILABLE_EVENT_KIND);
    assert_eq!(events[0].payload["state"], "unavailable");
    assert_eq!(events[0].payload["message"], "remote index offline");
}
