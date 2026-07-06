#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_context::{AgentContext, AgentContextHandle, DependencyStore};
use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    FunctionTool, StaticToolset, TOOL_SEARCH_FAILED_EVENT_KIND, TOOL_SEARCH_INVALIDATED_EVENT_KIND,
    TOOL_SEARCH_NO_MATCH_EVENT_KIND, TOOL_SEARCH_REFRESHED_EVENT_KIND, ToolContext, ToolError,
    ToolInstruction, ToolProxyToolset, ToolRegistry, ToolResult, ToolSearchNamespaceStatus,
    ToolSearchRefreshBinding, ToolSearchRefreshReason, ToolSearchRefreshSchedule,
    ToolSearchRefreshScheduleState, ToolSearchToolset, Toolset, dynamic_tool_proxy,
    dynamic_tool_search, json_tool,
};

fn context() -> ToolContext {
    ToolContext::new(RunId::from_string("run_test"), ConversationId::new(), 0)
}

fn context_with_handle(handle: &AgentContextHandle) -> ToolContext {
    let mut dependencies = DependencyStore::new();
    dependencies.insert(handle.clone());
    context().with_dependencies(dependencies)
}

fn result_content(result: &ToolResult) -> String {
    result.content["content"].as_str().unwrap().to_string()
}

#[test]
fn tool_search_refresh_schedule_debounces_inventory_changes() {
    let schedule = ToolSearchRefreshSchedule::new().every(1_000).debounce(50);
    let mut state = ToolSearchRefreshScheduleState::default();

    state.observe_inventory_version("v1", 100);

    let waiting = schedule.evaluate(&state, 120);
    assert!(!waiting.due);
    assert_eq!(waiting.next_check_after_ms, Some(30));

    let due = schedule.evaluate(&state, 150);
    assert!(due.due);
    assert_eq!(due.reason, Some(ToolSearchRefreshReason::InventoryChanged));

    state.mark_refreshed(None, 150);
    assert_eq!(state.last_inventory_version.as_deref(), Some("v1"));
    assert!(state.pending_inventory_version.is_none());

    state.observe_inventory_version("v1", 200);
    assert!(state.pending_inventory_version.is_none());
}

#[test]
fn tool_search_refresh_schedule_marks_interval_due() {
    let schedule = ToolSearchRefreshSchedule::new().every(100);
    let mut state = ToolSearchRefreshScheduleState::default();

    assert_eq!(
        schedule.evaluate(&state, 0).reason,
        Some(ToolSearchRefreshReason::IntervalElapsed)
    );

    state.mark_refreshed(Some("v1".to_string()), 25);
    let waiting = schedule.evaluate(&state, 75);
    assert!(!waiting.due);
    assert_eq!(waiting.next_check_after_ms, Some(50));

    let due = schedule.evaluate(&state, 125);
    assert!(due.due);
    assert_eq!(due.reason, Some(ToolSearchRefreshReason::IntervalElapsed));
}

#[test]
fn direct_tool_search_refresh_binding_runs_after_watcher_debounce() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"docs": true})))
        },
    );
    let admin = FunctionTool::new(
        "admin_docs",
        Some("Read admin-only documentation".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"admin": true})))
        },
    )
    .with_availability(|context| {
        context
            .metadata
            .get("allow_admin_docs")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(admin));
    let search = ToolSearchToolset::new(vec![Arc::new(toolset)]);
    let mut context = AgentContext::default();
    context.record_tool_search_loaded(["lookup_docs", "admin_docs"], ["docs_ns"]);
    let mut binding = ToolSearchRefreshBinding::new(ToolSearchRefreshSchedule::new().debounce(50));

    binding.observe_inventory_version("catalog-v2", 100);
    let waiting = search.refresh_loaded_state_if_due(&mut context, &mut binding, 120);

    assert!(!waiting.refreshed());
    assert_eq!(waiting.inventory_version.as_deref(), Some("catalog-v2"));
    assert_eq!(
        context.tool_search_state().loaded_tools,
        vec!["lookup_docs".to_string(), "admin_docs".to_string()]
    );

    let refreshed = search.refresh_loaded_state_if_due(&mut context, &mut binding, 150);

    assert!(refreshed.refreshed());
    assert_eq!(
        refreshed.decision.reason,
        Some(ToolSearchRefreshReason::InventoryChanged)
    );
    assert_eq!(
        binding.state.last_inventory_version.as_deref(),
        Some("catalog-v2")
    );
    assert_eq!(
        context.tool_search_state().loaded_tools,
        vec!["lookup_docs".to_string()]
    );
    assert!(context.events.events().iter().any(|event| {
        event.kind == TOOL_SEARCH_REFRESHED_EVENT_KIND
            && event.payload["refresh_scheduled"] == json!(true)
            && event.payload["refresh_reason"] == json!("inventory_changed")
            && event.payload["inventory_version"] == json!("catalog-v2")
            && event.payload["removed_loaded_tools"] == json!(["admin_docs"])
            && event.payload["retained_loaded_tools"] == json!(["lookup_docs"])
    }));
}

#[tokio::test]
async fn proxy_searches_namespaces_and_calls_tools() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let hidden_proxy_name = FunctionTool::new(
        "search_tools",
        Some("wrapped proxy helper name should be skipped".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move { Ok(ToolResult::new(json!(null))) },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(hidden_proxy_name))
        .with_instruction(ToolInstruction::new("docs", "Documentation tools."));
    let proxy = dynamic_tool_proxy(vec![Arc::new(toolset)]);
    let tools = proxy.get_tools();
    let handle = AgentContextHandle::new(AgentContext::default());
    assert_eq!(tools.len(), 2);
    assert_eq!(proxy.max_retries(), Some(3));
    assert!(
        proxy
            .get_instructions()
            .into_iter()
            .any(|instruction| instruction.content.contains("docs_ns"))
    );

    let search = tools
        .iter()
        .find(|tool| tool.name() == "search_tools")
        .unwrap();
    let search_output = result_content(
        &search
            .call(context_with_handle(&handle), json!({"query":"docs"}))
            .await
            .unwrap(),
    );
    assert!(search_output.contains("lookup_docs"));
    assert!(search_output.contains("namespace=\"docs_ns\""));
    assert!(!search_output.contains("wrapped proxy helper name"));
    let search_state = handle.snapshot().tool_search_state();
    assert_eq!(search_state.loaded_tools, vec!["lookup_docs".to_string()]);
    assert_eq!(search_state.loaded_namespaces, vec!["docs_ns".to_string()]);

    let call = tools
        .iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();
    let called = call
        .call(
            context_with_handle(&handle),
            json!({"name":"lookup_docs","arguments":{"topic":"agents"}}),
        )
        .await
        .unwrap();
    assert_eq!(called.content["looked_up"], "agents");
    let call_state = handle.snapshot().tool_search_state();
    assert_eq!(call_state.loaded_tools, vec!["lookup_docs".to_string()]);
    assert_eq!(call_state.loaded_namespaces, vec!["docs_ns".to_string()]);
}

#[tokio::test]
async fn direct_tool_search_loads_tools_for_next_turn_and_restores_state() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_instruction(ToolInstruction::new("docs", "Documentation tools."));
    let search = dynamic_tool_search(vec![Arc::new(toolset)]);
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&search);
    let handle = AgentContextHandle::new(AgentContext::default());

    let initial_defs = registry
        .definitions_for_context(&handle.snapshot())
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(initial_defs, vec!["tool_search".to_string()]);

    let tools = search.get_tools();
    let direct_lookup = tools
        .iter()
        .find(|tool| tool.name() == "lookup_docs")
        .unwrap();
    let unloaded = direct_lookup
        .call(
            context_with_handle(&handle),
            json!({"topic":"before-search"}),
        )
        .await;
    assert!(matches!(unloaded, Err(ToolError::UserError { .. })));

    let search_tool = tools
        .iter()
        .find(|tool| tool.name() == "tool_search")
        .unwrap();
    let search_result = search_tool
        .call(context_with_handle(&handle), json!({"query":"docs"}))
        .await
        .unwrap();
    assert_eq!(
        search_result.content["loaded_tools"][0]["name"],
        "lookup_docs"
    );
    assert_eq!(search_result.content["loaded_namespaces"][0], "docs_ns");

    let loaded_context = handle.snapshot();
    let loaded_defs = registry
        .definitions_for_context(&loaded_context)
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(
        loaded_defs,
        vec!["lookup_docs".to_string(), "tool_search".to_string()]
    );
    assert!(loaded_context.events.events().iter().any(|event| {
        event.kind == "tool_search_loaded"
            && event.payload["loaded_tools"][0] == "lookup_docs"
            && event.payload["loaded_namespaces"][0] == "docs_ns"
    }));

    let called = direct_lookup
        .call(context_with_handle(&handle), json!({"topic":"agents"}))
        .await
        .unwrap();
    assert_eq!(called.content["looked_up"], "agents");

    let restored = AgentContext::from_state(loaded_context.export_full_state());
    let restored_defs = registry
        .definitions_for_context(&restored)
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(restored_defs, loaded_defs);
}

#[tokio::test]
async fn direct_tool_search_publishes_query_error_events() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup));
    let search = dynamic_tool_search(vec![Arc::new(toolset)]);
    let handle = AgentContextHandle::new(AgentContext::default());
    let search_tool = search
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "tool_search")
        .unwrap();

    let empty = search_tool
        .call(context_with_handle(&handle), json!({"query":""}))
        .await
        .unwrap();
    assert_eq!(empty.content["error"], "Parameter 'query' is required.");
    let no_match = search_tool
        .call(context_with_handle(&handle), json!({"query":"missing"}))
        .await
        .unwrap();
    assert!(
        no_match.content["loaded_tools"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let snapshot = handle.snapshot();
    assert!(snapshot.events.events().iter().any(|event| {
        event.kind == TOOL_SEARCH_FAILED_EVENT_KIND
            && event.payload["search_tool_name"] == "tool_search"
            && event.payload["error_kind"] == "empty_query"
    }));
    assert!(snapshot.events.events().iter().any(|event| {
        event.kind == TOOL_SEARCH_NO_MATCH_EVENT_KIND
            && event.payload["query"] == "missing"
            && event.payload["error_kind"] == "no_match"
    }));
    assert!(!snapshot.events.events().iter().any(|event| {
        event.kind == "tool_search_loaded"
            && event.payload["loaded_tools"]
                .as_array()
                .is_some_and(Vec::is_empty)
    }));
}

#[test]
fn direct_tool_search_can_preload_namespace_from_host_code() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup));
    let search = ToolSearchToolset::new(vec![Arc::new(toolset)]);
    let mut registry = ToolRegistry::new();
    let search_toolset = Arc::new(search.clone()) as starweaver_tools::DynToolset;
    registry.insert_toolset(&search_toolset);
    let mut context = AgentContext::default();

    assert_eq!(
        registry.definitions_for_context(&context)[0].name,
        "tool_search"
    );
    assert!(search.preload_namespace(&mut context, "missing").is_empty());
    let loaded = search.preload_namespace(&mut context, "docs_ns");

    assert_eq!(loaded.loaded_tools, vec!["lookup_docs".to_string()]);
    assert_eq!(loaded.loaded_namespaces, vec!["docs_ns".to_string()]);
    let names = registry
        .definitions_for_context(&context)
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["lookup_docs".to_string(), "tool_search".to_string()]
    );
    assert!(context.events.events().iter().any(|event| {
        event.kind == "tool_search_loaded"
            && event.payload["loaded_tools"][0] == "lookup_docs"
            && event.payload["loaded_namespaces"][0] == "docs_ns"
    }));
}

#[test]
fn direct_tool_search_reports_initialization_status_and_events() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let gated = FunctionTool::new(
        "admin_docs",
        Some("Read admin-only documentation".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"admin": true})))
        },
    )
    .with_availability(|context| {
        context
            .metadata
            .get("allow_admin_docs")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let loose = FunctionTool::new(
        "calculate",
        Some("Run a small calculation".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"value": 1})))
        },
    );
    let namespaced = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(gated));
    let loose = StaticToolset::new("loose").with_tool(Arc::new(loose));
    let search =
        ToolSearchToolset::new(vec![Arc::new(namespaced), Arc::new(loose)]).with_max_results(7);
    let no_context_report = search.initialization_report(None);

    assert!(!no_context_report.availability_checked);
    assert_eq!(no_context_report.total_tools, 3);
    assert_eq!(no_context_report.total_namespaces, 1);
    assert_eq!(no_context_report.loose_tools, vec!["calculate".to_string()]);
    assert_eq!(no_context_report.max_results, 7);
    assert_eq!(
        no_context_report.namespaces[0].status,
        ToolSearchNamespaceStatus::Connected
    );
    assert_eq!(no_context_report.namespaces[0].available_tools, 0);

    let mut context = AgentContext::default();
    let report = search.publish_initialization_report(&mut context);

    assert!(report.availability_checked);
    assert_eq!(report.available_tools, 2);
    assert_eq!(report.unavailable_tools, 1);
    assert_eq!(report.namespaces[0].namespace, "docs_ns");
    assert_eq!(report.namespaces[0].total_tools, 2);
    assert_eq!(report.namespaces[0].available_tools, 1);
    assert_eq!(report.namespaces[0].unavailable_tools, 1);
    assert_eq!(
        report.namespaces[0].status,
        ToolSearchNamespaceStatus::Connected
    );
    assert!(context.events.events().iter().any(|event| {
        event.kind == "tool_search_initialized"
            && event.payload["toolset_name"] == "tool_search"
            && event.payload["search_tool_name"] == "tool_search"
            && event.payload["total_tools"] == 3
            && event.payload["namespaces"][0]["namespace"] == "docs_ns"
            && event.payload["namespaces"][0]["unavailable_tools"] == 1
    }));

    context
        .metadata
        .insert("allow_admin_docs".to_string(), json!(true));
    let refreshed = search.publish_refresh_report(&mut context);

    assert_eq!(refreshed.available_tools, 3);
    assert_eq!(refreshed.unavailable_tools, 0);
    assert!(context.events.events().iter().any(|event| {
        event.kind == "tool_search_refreshed"
            && event.payload["available_tools"] == 3
            && event.payload["unavailable_tools"] == 0
    }));
}

#[test]
fn direct_tool_search_refresh_prunes_stale_loaded_state() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let gated = FunctionTool::new(
        "admin_docs",
        Some("Read admin-only documentation".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"admin": true})))
        },
    )
    .with_availability(|context| {
        context
            .metadata
            .get("allow_admin_docs")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(gated));
    let search = ToolSearchToolset::new(vec![Arc::new(toolset)]);
    let mut context = AgentContext::default();
    context.record_tool_search_loaded(
        ["lookup_docs", "admin_docs", "missing_tool"],
        ["docs_ns", "missing_ns"],
    );

    let refresh = search.refresh_loaded_state(&mut context);

    assert_eq!(refresh.report.total_tools, 2);
    assert_eq!(
        refresh.removed_loaded_tools,
        vec!["admin_docs".to_string(), "missing_tool".to_string()]
    );
    assert_eq!(
        refresh.removed_loaded_namespaces,
        vec!["missing_ns".to_string()]
    );
    assert_eq!(
        refresh.retained_loaded_tools,
        vec!["lookup_docs".to_string()]
    );
    assert_eq!(
        refresh.retained_loaded_namespaces,
        vec!["docs_ns".to_string()]
    );
    assert_eq!(
        context.tool_search_state().loaded_tools,
        vec!["lookup_docs".to_string()]
    );
    assert!(context.events.events().iter().any(|event| {
        event.kind == TOOL_SEARCH_REFRESHED_EVENT_KIND
            && event.payload["removed_loaded_tools"] == json!(["admin_docs", "missing_tool"])
            && event.payload["removed_loaded_namespaces"] == json!(["missing_ns"])
            && event.payload["retained_loaded_tools"] == json!(["lookup_docs"])
    }));
}

#[test]
fn direct_tool_search_invalidates_loaded_state() {
    let search = ToolSearchToolset::new(Vec::new());
    let mut context = AgentContext::default();
    context.record_tool_search_loaded(["lookup_docs"], ["docs_ns"]);

    let invalidated = search.invalidate_loaded_state(&mut context, "remote_catalog_changed");

    assert_eq!(invalidated.reason, "remote_catalog_changed");
    assert_eq!(
        invalidated.removed_loaded_tools,
        vec!["lookup_docs".to_string()]
    );
    assert_eq!(
        invalidated.removed_loaded_namespaces,
        vec!["docs_ns".to_string()]
    );
    assert!(context.tool_search_state().is_empty());
    assert!(context.events.events().iter().any(|event| {
        event.kind == TOOL_SEARCH_INVALIDATED_EVENT_KIND
            && event.payload["reason"] == "remote_catalog_changed"
            && event.payload["removed_loaded_tools"] == json!(["lookup_docs"])
            && event.payload["removed_loaded_namespaces"] == json!(["docs_ns"])
    }));
}

#[tokio::test]
async fn proxy_supports_prefixed_visible_tools_and_instructions() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let hidden_unprefixed_proxy_name = FunctionTool::new(
        "search_tools",
        Some("unprefixed proxy helper name remains an ordinary wrapped tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move { Ok(ToolResult::new(json!(null))) },
    );
    let hidden_prefixed_proxy_name = FunctionTool::new(
        "mcp_search_tool",
        Some("prefixed proxy helper name should be skipped".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move { Ok(ToolResult::new(json!(null))) },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(hidden_unprefixed_proxy_name))
        .with_tool(Arc::new(hidden_prefixed_proxy_name));

    let proxy = ToolProxyToolset::new(vec![Arc::new(toolset)])
        .try_with_name_prefix("__mcp__")
        .unwrap();
    assert_eq!(proxy.name(), "tool_proxy");
    assert_eq!(proxy.prefix(), Some("mcp"));
    assert_eq!(proxy.search_tool_name(), "mcp_search_tool");
    assert_eq!(proxy.call_tool_name(), "mcp_call_tool");
    assert!(proxy.get_instructions().into_iter().any(|instruction| {
        instruction.group == "mcp-tool-proxy"
            && instruction.content.contains("mcp_search_tool")
            && instruction.content.contains("mcp_call_tool")
            && !instruction.content.contains("search_tools")
    }));

    let tools = proxy.get_tools();
    assert_eq!(
        tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
        vec!["mcp_search_tool", "mcp_call_tool"]
    );

    let search = tools
        .iter()
        .find(|tool| tool.name() == "mcp_search_tool")
        .unwrap();
    let search_output = result_content(
        &search
            .call(context(), json!({"query":"docs"}))
            .await
            .unwrap(),
    );
    assert!(search_output.contains("lookup_docs"));
    assert!(search_output.contains("search_tools"));
    assert!(!search_output.contains("name=\"mcp_search_tool\""));
    assert!(!search_output.contains("prefixed proxy helper name should be skipped"));

    let call = tools
        .iter()
        .find(|tool| tool.name() == "mcp_call_tool")
        .unwrap();
    let called = call
        .call(
            context(),
            json!({"name":"lookup_docs","arguments":{"topic":"agents"}}),
        )
        .await
        .unwrap();
    assert_eq!(called.content["looked_up"], "agents");

    let missing = result_content(
        &call
            .call(context(), json!({"name":"missing","arguments":{}}))
            .await
            .unwrap(),
    );
    assert!(missing.contains("Use mcp_search_tool to discover available tools"));
}

#[test]
fn proxy_rejects_invalid_prefixes() {
    let Err(error) = ToolProxyToolset::new(vec![]).try_with_name_prefix("mcp-server") else {
        panic!("invalid prefix should be rejected");
    };
    assert_eq!(error.prefix(), "mcp-server");
}

#[tokio::test]
async fn proxy_returns_xml_for_empty_unknown_and_execution_errors() {
    let failing = FunctionTool::new(
        "fail_tool",
        Some("Fails for testing".to_string()),
        json!({"type":"object","properties":{"value":{"type":"string"}}}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::Execution {
                tool: "fail_tool".to_string(),
                message: "bad <input> & quote\"".to_string(),
            })
        },
    );
    let toolset = StaticToolset::new("ops")
        .with_id("ops")
        .with_tool(Arc::new(failing));
    let proxy = dynamic_tool_proxy(vec![Arc::new(toolset)]);
    let tools = proxy.get_tools();
    let search = tools
        .iter()
        .find(|tool| tool.name() == "search_tools")
        .unwrap();
    assert!(
        result_content(&search.call(context(), json!({"query":""})).await.unwrap())
            .contains("Parameter 'query' is required")
    );
    assert!(
        result_content(
            &search
                .call(context(), json!({"query":"missing"}))
                .await
                .unwrap()
        )
        .contains("No tools found")
    );

    let call = tools
        .iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();
    assert!(
        result_content(&call.call(context(), json!({"name":""})).await.unwrap())
            .contains("Parameter 'name' is required")
    );
    assert!(
        result_content(
            &call
                .call(context(), json!({"name":"unknown","arguments":{}}))
                .await
                .unwrap()
        )
        .contains("not found")
    );
    let error = result_content(
        &call
            .call(
                context(),
                json!({"name":"fail_tool","arguments":{"value":"x"}}),
            )
            .await
            .unwrap(),
    );
    assert!(error.contains("tool-call-error"));
    assert!(error.contains("&lt;input&gt;"));
    assert!(error.contains("&amp;"));
}

#[tokio::test]
async fn proxy_propagates_approval_and_deferred_errors() {
    let approval = json_tool(
        "approval_tool",
        Some("Needs approval".to_string()),
        json!({"type":"object"}),
        |_ctx, _args| async move {
            Err(ToolError::ApprovalRequired {
                tool: "approval_tool".to_string(),
                metadata: json!({"reason":"review"}),
            })
        },
    );
    let deferred = json_tool(
        "deferred_tool",
        Some("Defers work".to_string()),
        json!({"type":"object"}),
        |_ctx, _args| async move {
            Err(ToolError::CallDeferred {
                tool: "deferred_tool".to_string(),
                metadata: json!({"worker":"remote"}),
            })
        },
    );
    let toolset = StaticToolset::new("control")
        .with_id("control")
        .with_tool(Arc::new(approval))
        .with_tool(Arc::new(deferred));
    let proxy = dynamic_tool_proxy(vec![Arc::new(toolset)]);
    let call = proxy
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();

    let approval_error = call
        .call(context(), json!({"name":"approval_tool","arguments":{}}))
        .await
        .unwrap_err();
    assert!(matches!(approval_error, ToolError::ApprovalRequired { .. }));

    let deferred_error = call
        .call(context(), json!({"name":"deferred_tool","arguments":{}}))
        .await
        .unwrap_err();
    assert!(matches!(deferred_error, ToolError::CallDeferred { .. }));
}
