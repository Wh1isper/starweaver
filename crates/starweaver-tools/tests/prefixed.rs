#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use starweaver_context::{AgentContext, ToolCapabilityGrant};
use starweaver_core::{ConversationId, Metadata, RunId};
use starweaver_tools::{
    DynToolset, FunctionTool, PrefixedTool, PrefixedToolset, RenamedToolset, StaticToolset,
    TOOL_METADATA_DEPENDENCIES_KEY, Tool, ToolContext, ToolDependencyRequirements, ToolInstruction,
    ToolRegistry, ToolResult,
};

#[tokio::test]
async fn prefixed_tool_exposes_prefixed_name_and_delegates_execution() {
    let called = Arc::new(Mutex::new(false));
    let called_clone = called.clone();
    let inner = FunctionTool::new(
        "lookup",
        Some("Lookup".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, args: serde_json::Value| {
            let called = called_clone.clone();
            async move {
                *called.lock().unwrap() = true;
                Ok(ToolResult::new(serde_json::json!({"value": args["query"]})))
            }
        },
    );
    let prefixed = PrefixedTool::new("weather", Arc::new(inner));

    assert_eq!(prefixed.name(), "weather_lookup");
    assert_eq!(prefixed.description(), Some("Lookup"));
    let result = prefixed
        .call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            serde_json::json!({"query": "Paris"}),
        )
        .await
        .unwrap();

    assert!(*called.lock().unwrap());
    assert_eq!(result.content["value"], "Paris");
}

#[tokio::test]
async fn prefixed_toolset_prefixes_tools_and_instruction_groups() {
    let inner_tool = FunctionTool::new(
        "conditions",
        Some("Conditions".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    );
    let toolset: DynToolset = Arc::new(
        StaticToolset::new("weather")
            .with_tool(Arc::new(inner_tool))
            .with_instruction(ToolInstruction::new(
                "weather",
                "Prefer canonical weather data.",
            )),
    );
    let prefixed: DynToolset = Arc::new(PrefixedToolset::new("api", toolset));
    let registry = ToolRegistry::new().with_toolset(&prefixed);

    let definitions = registry.definitions();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "api_conditions");
    assert_eq!(
        registry.get_instructions(),
        vec![
            "<tool-instruction name=\"api_weather\">Prefer canonical weather data.</tool-instruction>"
        ]
    );
    let instructions = registry.instructions();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].group, "api_weather");
    assert_eq!(instructions[0].content, "Prefer canonical weather data.");
    let call = starweaver_model::ToolCallPart {
        id: "call_1".to_string(),
        name: "api_conditions".to_string(),
        arguments: serde_json::json!({"city": "Paris"}).into(),
    };
    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &call,
        )
        .await;

    assert_eq!(result.name, "api_conditions");
    assert_eq!(result.content["city"], "Paris");
    assert!(!result.is_error);
}

#[test]
fn prefixed_and_renamed_tools_preserve_the_exact_capability_grant_identity() {
    let metadata = Metadata::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(["weather.read"], Vec::<String>::new(), false)
            .to_metadata_value(),
    )]);
    let inner_tool = FunctionTool::new(
        "lookup",
        Some("Lookup weather".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata)
    .with_codeact(true);
    let inner: DynToolset = Arc::new(StaticToolset::new("weather").with_tool(Arc::new(inner_tool)));
    let prefixed: DynToolset = Arc::new(PrefixedToolset::new("api", inner));
    let renamed: DynToolset = Arc::new(RenamedToolset::new(
        prefixed,
        [("api_lookup".to_string(), "weather_search".to_string())],
    ));
    let registry = ToolRegistry::new().with_toolset(&renamed);
    let denied = BTreeSet::new();

    assert_eq!(
        registry.capability_grant_name_for("weather_search"),
        Some("lookup")
    );

    let mut wrong_grant_context = AgentContext::default();
    wrong_grant_context.grant_tool_capabilities(
        "weather_search",
        ToolCapabilityGrant::new().with_host_capabilities(["weather.read"]),
    );
    assert!(
        registry
            .codeact_definitions_for_context(&wrong_grant_context, &denied)
            .is_empty()
    );

    let mut exact_grant_context = AgentContext::default();
    exact_grant_context.grant_tool_capabilities(
        "lookup",
        ToolCapabilityGrant::new().with_host_capabilities(["weather.read"]),
    );
    assert_eq!(
        registry
            .codeact_definitions_for_context(&exact_grant_context, &denied)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>(),
        vec!["weather_search".to_string()]
    );
}
