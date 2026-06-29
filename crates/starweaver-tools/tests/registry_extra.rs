#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use serde_json::json;
use starweaver_context::AgentContext;
use starweaver_core::{CancellationToken, ConversationId, RunId};
use starweaver_model::ToolCallPart;
use starweaver_tools::{
    json_tool, DynTool, DynToolset, FunctionTool, StaticToolset, ToolContext, ToolError,
    ToolExecutionHook, ToolExecutionOutcome, ToolInstruction, ToolRegistry, ToolResult, Toolset,
};
use tokio::time::{sleep, Duration};

fn context() -> ToolContext {
    ToolContext::new(RunId::from_string("run_registry"), ConversationId::new(), 0)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn registry_dispatch_selects_removes_and_auto_inherits_tools() {
    let mut metadata = serde_json::Map::new();
    metadata.insert("auto_inherit".to_string(), json!(true));
    let inherited = FunctionTool::new(
        "inherited",
        Some("Auto inherited tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata)
    .with_max_retries(4)
    .with_timeout_ms(9);
    let failing = FunctionTool::new(
        "failing",
        Some("Failing tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::ModelRetry {
                tool: "failing".to_string(),
                message: "retry please".to_string(),
            })
        },
    );
    let tools: Vec<DynTool> = vec![Arc::new(inherited), Arc::new(failing)];
    let toolset = StaticToolset::new("registry_extra")
        .with_id("registry_extra")
        .with_max_retries(2)
        .with_timeout_ms(20)
        .with_tools(tools)
        .with_instructions(vec![
            ToolInstruction::new("a", "Use a."),
            ToolInstruction::new("b", "Use b."),
        ]);
    assert_eq!(toolset.id(), Some("registry_extra"));
    assert_eq!(toolset.max_retries(), Some(2));
    let toolset: DynToolset = Arc::new(toolset);
    let mut registry = ToolRegistry::new()
        .with_max_retries(7)
        .with_timeout_ms(30)
        .with_toolset(&toolset);
    assert_eq!(registry.max_retries(), Some(7));
    assert!(!registry.is_empty());
    assert_eq!(registry.names(), vec!["failing", "inherited"]);
    assert_eq!(registry.tools().len(), 2);
    assert_eq!(registry.definitions().len(), 2);
    assert_eq!(registry.max_retries_for("inherited"), 4);
    assert_eq!(registry.max_retries_for("failing"), 2);
    assert_eq!(registry.max_retries_for("missing"), 7);
    assert_eq!(registry.timeout_ms_for("inherited"), Some(9));
    assert_eq!(registry.timeout_ms_for("failing"), Some(20));
    assert_eq!(registry.timeout_ms_for("missing"), None);
    assert_eq!(
        registry
            .definitions()
            .iter()
            .find(|definition| definition.name == "inherited")
            .unwrap()
            .metadata["timeout_ms"],
        json!(9)
    );
    assert_eq!(
        registry.get_instructions(),
        vec![
            "<tool-instruction name=\"a\">Use a.</tool-instruction>",
            "<tool-instruction name=\"b\">Use b.</tool-instruction>",
        ]
    );
    let instructions = registry.instructions();
    assert_eq!(instructions.len(), 2);
    assert_eq!(instructions[0].content, "Use a.");
    assert_eq!(instructions[1].content, "Use b.");

    let call = ToolCallPart {
        id: "call_1".to_string(),
        name: "inherited".to_string(),
        arguments: json!({"ok": true}).into(),
    };
    let returned = registry.execute_call(context(), &call).await;
    assert!(!returned.is_error);
    assert_eq!(returned.content["ok"], true);

    let error_call = ToolCallPart {
        id: "call_2".to_string(),
        name: "failing".to_string(),
        arguments: json!({}).into(),
    };
    let error = registry.execute_call(context(), &error_call).await;
    assert!(error.is_error);
    assert_eq!(error.metadata["error_kind"], "model_retry");
    assert_eq!(error.content["kind"], "model_retry");
    assert_eq!(error.content["tool"], "failing");
    assert_eq!(error.content["message"], "retry please");
    assert!(error.content["how_to_fix"]
        .as_str()
        .unwrap()
        .contains("adjusted arguments"));
    assert!(error.content["retryable"].as_bool().unwrap());
    assert!(error.content["retry_requires_corrected_input"]
        .as_bool()
        .unwrap());

    let missing_call = ToolCallPart {
        id: "call_3".to_string(),
        name: "missing".to_string(),
        arguments: json!({}).into(),
    };
    let missing = registry.execute_call(context(), &missing_call).await;
    assert!(missing.is_error);
    assert_eq!(missing.content["kind"], "not_found");
    assert_eq!(missing.content["tool"], "missing");
    assert!(missing.content["how_to_fix"]
        .as_str()
        .unwrap()
        .contains("advertised in the current tool list"));

    let inherited_only = registry.auto_inherited();
    assert!(inherited_only.contains("inherited"));
    assert!(!inherited_only.contains("failing"));
    assert!(inherited_only.get("inherited").is_some());

    let selected = registry.select(["failing"]);
    assert!(selected.contains("failing"));
    assert!(!selected.contains("inherited"));
    assert_eq!(selected.max_retries_for("failing"), 2);
    assert_eq!(selected.timeout_ms_for("failing"), Some(20));

    assert!(registry.remove("failing").is_some());
    assert!(!registry.contains("failing"));
    assert_eq!(registry.timeout_ms_for("failing"), None);
}

#[test]
fn registry_filters_definitions_by_context_availability() {
    let gated = FunctionTool::new(
        "gated",
        Some("Context-gated tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_availability(|context| {
        context
            .metadata
            .get("enable_gated_tool")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let registry = ToolRegistry::new()
        .with_tool(Arc::new(gated))
        .with_tool(Arc::new(FunctionTool::new(
            "always",
            Some("Always available".to_string()),
            json!({"type":"object"}),
            |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
        )));
    let hidden = AgentContext::default();
    let mut visible = AgentContext::default();
    visible
        .metadata
        .insert("enable_gated_tool".to_string(), json!(true));

    assert_eq!(
        registry
            .definitions_for_context(&hidden)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>(),
        vec!["always".to_string()]
    );
    let hidden_report = registry.availability_report(&hidden);
    assert_eq!(hidden_report.available, vec!["always".to_string()]);
    assert_eq!(hidden_report.unavailable, vec!["gated".to_string()]);
    assert!(!hidden_report.is_all_available());
    let (hidden_definitions, hidden_report_from_definitions) =
        registry.definitions_and_availability_for_context(&hidden);
    assert_eq!(
        hidden_definitions
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>(),
        vec!["always".to_string()]
    );
    assert_eq!(hidden_report_from_definitions, hidden_report);
    assert_eq!(
        registry
            .definitions_for_context(&visible)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>(),
        vec!["always".to_string(), "gated".to_string()]
    );
    assert!(registry.availability_report(&visible).is_all_available());
}

#[test]
fn registry_prepares_tool_definitions_per_context() {
    let prepared = FunctionTool::new(
        "tenant_lookup",
        Some("Lookup tenant data".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_prepare_definition(|context, mut definition| {
        let tenant = context
            .metadata
            .get("tenant")
            .and_then(serde_json::Value::as_str)?;
        definition.description = Some(format!("Lookup data for tenant {tenant}"));
        definition
            .metadata
            .insert("tenant".to_string(), json!(tenant));
        Some(definition)
    });
    let registry = ToolRegistry::new().with_tool(Arc::new(prepared));

    assert_eq!(
        registry.definitions()[0].description.as_deref(),
        Some("Lookup tenant data")
    );
    let hidden = AgentContext::default();
    assert!(registry.definitions_for_context(&hidden).is_empty());
    let hidden_report = registry.availability_report(&hidden);
    assert_eq!(hidden_report.available, Vec::<String>::new());
    assert_eq!(hidden_report.unavailable, vec!["tenant_lookup".to_string()]);

    let mut visible = AgentContext::default();
    visible.metadata.insert("tenant".to_string(), json!("acme"));
    let definitions = registry.definitions_for_context(&visible);

    assert_eq!(definitions.len(), 1);
    assert_eq!(
        definitions[0].description.as_deref(),
        Some("Lookup data for tenant acme")
    );
    assert_eq!(definitions[0].metadata["tenant"], "acme");
    assert!(registry.availability_report(&visible).is_all_available());
}

#[tokio::test]
async fn registry_maps_tool_result_layers_to_tool_return_part() {
    let layered = FunctionTool::new(
        "layered",
        Some("Layered tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            let mut private_metadata = serde_json::Map::new();
            private_metadata.insert("secret".to_string(), json!("host-only"));
            Ok(ToolResult::new(json!({"raw": "app raw"}))
                .with_model_content(json!({"summary": "model visible"}))
                .with_app_value(json!({"app": "application value"}))
                .with_user_content(json!({"ui": "user visible"}))
                .with_private_metadata(private_metadata))
        },
    );
    let registry = ToolRegistry::new().with_tool(Arc::new(layered));
    let call = ToolCallPart {
        id: "call_layered".to_string(),
        name: "layered".to_string(),
        arguments: json!({}).into(),
    };

    let returned = registry.execute_call(context(), &call).await;

    assert!(!returned.is_error);
    assert_eq!(returned.content, json!({"summary": "model visible"}));
    assert_eq!(
        returned.app_value,
        Some(json!({"app": "application value"}))
    );
    assert_eq!(returned.user_content, Some(json!({"ui": "user visible"})));
    assert_eq!(returned.private_metadata["secret"], "host-only");
}

#[tokio::test]
async fn function_tool_argument_validators_run_in_order() {
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let first_order = order.clone();
    let second_order = order.clone();
    let tool = FunctionTool::new(
        "validated",
        Some("Validated tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_argument_validator(move |_context, arguments| {
        first_order.lock().unwrap().push("first".to_string());
        arguments["normalized"] = json!(true);
        Ok(())
    })
    .with_argument_validator(move |_context, arguments| {
        second_order.lock().unwrap().push("second".to_string());
        if arguments
            .get("allow")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            Ok(())
        } else {
            Err(ToolError::InvalidArguments {
                tool: "validated".to_string(),
                message: "allow must be true".to_string(),
            })
        }
    });
    let registry = ToolRegistry::new().with_tool(Arc::new(tool));

    let success = registry
        .execute_call(
            context(),
            &ToolCallPart {
                id: "call_validated".to_string(),
                name: "validated".to_string(),
                arguments: json!({"allow": true}).into(),
            },
        )
        .await;

    assert!(!success.is_error);
    assert_eq!(success.content["normalized"], true);
    assert_eq!(
        *order.lock().unwrap(),
        vec!["first".to_string(), "second".to_string()]
    );

    let rejected = registry
        .execute_call(
            context(),
            &ToolCallPart {
                id: "call_rejected".to_string(),
                name: "validated".to_string(),
                arguments: json!({}).into(),
            },
        )
        .await;

    assert!(rejected.is_error);
    assert_eq!(rejected.metadata["error_kind"], "invalid_arguments");
    assert_eq!(rejected.content["kind"], "invalid_arguments");
    assert_eq!(rejected.content["message"], "allow must be true");
    assert!(rejected.content["how_to_fix"]
        .as_str()
        .unwrap()
        .contains("JSON schema"));
}

#[tokio::test]
async fn registry_rejects_invalid_provider_json_before_tool_execution() {
    let observed = Arc::new(Mutex::new(false));
    let observed_for_tool = observed.clone();
    let tool = FunctionTool::new(
        "validated_json",
        Some("Validated JSON".to_string()),
        json!({"type":"object"}),
        move |_ctx: ToolContext, _args: serde_json::Value| {
            let observed_for_tool = observed_for_tool.clone();
            async move {
                *observed_for_tool.lock().unwrap() = true;
                Ok(ToolResult::new(json!({"ok": true})))
            }
        },
    );
    let registry = ToolRegistry::new().with_tool(Arc::new(tool));

    let rejected = registry
        .execute_call(
            context(),
            &ToolCallPart {
                id: "invalid-json".to_string(),
                name: "validated_json".to_string(),
                arguments: starweaver_model::ToolArguments::invalid("{bad", "expected value"),
            },
        )
        .await;

    assert!(!*observed.lock().unwrap());
    assert!(rejected.is_error);
    assert_eq!(rejected.content["kind"], "invalid_arguments");
    assert!(rejected.content["message"]
        .as_str()
        .unwrap()
        .contains("valid JSON before execution"));
    assert!(rejected.content["retry_requires_corrected_input"]
        .as_bool()
        .unwrap());
}

struct ArgumentAndResultHook {
    label: &'static str,
    order: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl ToolExecutionHook for ArgumentAndResultHook {
    async fn before_tool_call(
        &self,
        _context: &mut ToolContext,
        _call: &ToolCallPart,
        arguments: &mut serde_json::Value,
    ) -> Result<(), ToolError> {
        self.order
            .lock()
            .unwrap()
            .push(format!("{}_before", self.label));
        arguments[self.label] = json!(true);
        Ok(())
    }

    async fn after_tool_call(
        &self,
        _context: &ToolContext,
        _call: &ToolCallPart,
        outcome: &mut ToolExecutionOutcome,
    ) -> Result<(), ToolError> {
        self.order
            .lock()
            .unwrap()
            .push(format!("{}_after", self.label));
        if let ToolExecutionOutcome::Success(result) = outcome {
            result
                .content
                .as_object_mut()
                .unwrap()
                .insert(format!("{}_after", self.label), json!(true));
        }
        Ok(())
    }
}

#[tokio::test]
async fn registry_execution_hooks_wrap_tool_call_in_stable_order() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let tool = FunctionTool::new(
        "hooked",
        Some("Hooked tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let registry = ToolRegistry::new()
        .with_tool(Arc::new(tool))
        .with_global_execution_hook(Arc::new(ArgumentAndResultHook {
            label: "global",
            order: order.clone(),
        }))
        .with_tool_execution_hook(
            "hooked",
            Arc::new(ArgumentAndResultHook {
                label: "tool",
                order: order.clone(),
            }),
        );
    let call = ToolCallPart {
        id: "call_hooked".to_string(),
        name: "hooked".to_string(),
        arguments: json!({"initial": true}).into(),
    };

    let returned = registry.execute_call(context(), &call).await;

    assert!(!returned.is_error);
    assert_eq!(returned.content["initial"], true);
    assert_eq!(returned.content["global"], true);
    assert_eq!(returned.content["tool"], true);
    assert_eq!(returned.content["tool_after"], true);
    assert_eq!(returned.content["global_after"], true);
    assert_eq!(
        *order.lock().unwrap(),
        vec![
            "global_before".to_string(),
            "tool_before".to_string(),
            "tool_after".to_string(),
            "global_after".to_string(),
        ]
    );
}

struct ReplacingPostHook {
    observed: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl ToolExecutionHook for ReplacingPostHook {
    async fn after_tool_call(
        &self,
        _context: &ToolContext,
        _call: &ToolCallPart,
        outcome: &mut ToolExecutionOutcome,
    ) -> Result<(), ToolError> {
        *self.observed.lock().unwrap() = true;
        *outcome = ToolExecutionOutcome::Success(ToolResult::new(json!({"replaced": true})));
        Ok(())
    }
}

#[tokio::test]
async fn registry_execution_hooks_observe_but_do_not_replace_control_flow() {
    let observed = Arc::new(Mutex::new(false));
    let approval_tool = FunctionTool::new(
        "approval",
        Some("Approval tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::ApprovalRequired {
                tool: "approval".to_string(),
                metadata: json!({"reason": "sensitive"}),
            })
        },
    );
    let registry = ToolRegistry::new()
        .with_tool(Arc::new(approval_tool))
        .with_tool_execution_hook(
            "approval",
            Arc::new(ReplacingPostHook {
                observed: observed.clone(),
            }),
        );
    let call = ToolCallPart {
        id: "call_approval".to_string(),
        name: "approval".to_string(),
        arguments: json!({}).into(),
    };

    let returned = registry.execute_call(context(), &call).await;

    assert!(*observed.lock().unwrap());
    assert!(returned.is_error);
    assert_eq!(returned.metadata["error_kind"], "approval_required");
    assert_eq!(returned.metadata["control_flow"], "approval_required");
    assert_eq!(returned.metadata["approval"]["reason"], "sensitive");
}

#[test]
fn registry_insert_registry_carries_retry_and_instructions() {
    let tool = json_tool(
        "plain",
        Some("Plain tool".to_string()),
        json!({"type":"object"}),
        |_ctx, args| async move { Ok(ToolResult::new(args)) },
    );
    let source_toolset = StaticToolset::new("source")
        .with_max_retries(3)
        .with_tool(Arc::new(tool))
        .with_instruction(ToolInstruction::new("source", "Use source."));
    let source_toolset: DynToolset = Arc::new(source_toolset);
    let source = ToolRegistry::new()
        .with_max_retries(5)
        .with_toolset(&source_toolset);
    let mut target = ToolRegistry::new();
    target.set_max_retries(1);
    target.insert_registry(&source);
    assert_eq!(target.max_retries(), Some(5));
    assert_eq!(target.max_retries_for("plain"), 3);
    assert_eq!(target.timeout_ms_for("plain"), None);
    assert_eq!(
        target.get_instructions(),
        vec!["<tool-instruction name=\"source\">Use source.</tool-instruction>"]
    );
}

#[tokio::test]
async fn registry_enforces_tool_timeouts() {
    let slow = FunctionTool::new(
        "slow",
        Some("Slow tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            sleep(Duration::from_millis(50)).await;
            Ok(ToolResult::new(json!({"done": true})))
        },
    )
    .with_timeout_ms(1);
    let registry = ToolRegistry::new().with_tool(Arc::new(slow));
    let call = ToolCallPart {
        id: "call_slow".to_string(),
        name: "slow".to_string(),
        arguments: json!({}).into(),
    };

    let returned = registry.execute_call(context(), &call).await;

    assert!(returned.is_error);
    assert_eq!(returned.metadata["error_kind"], "timeout");
    assert_eq!(returned.metadata["timeout_ms"], json!(1));
    assert_eq!(returned.content["kind"], "timeout");
    assert!(returned.content["how_to_fix"]
        .as_str()
        .unwrap()
        .contains("larger timeout"));
}

#[tokio::test]
async fn registry_cancels_running_tool_when_context_token_is_cancelled() {
    let token = CancellationToken::new();
    let observed_token = Arc::new(Mutex::new(None));
    let observed_token_for_tool = observed_token.clone();
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::channel(1);
    let slow = FunctionTool::new(
        "slow",
        Some("Slow tool".to_string()),
        json!({"type":"object"}),
        move |ctx: ToolContext, _args: serde_json::Value| {
            let started_sender = started_sender.clone();
            let observed_token_for_tool = observed_token_for_tool.clone();
            async move {
                *observed_token_for_tool.lock().unwrap() = Some(ctx.cancellation_token());
                let _ = started_sender.send(()).await;
                ctx.cancellation_token.cancelled().await;
                Ok(ToolResult::new(json!({"done": true})))
            }
        },
    );
    let registry = ToolRegistry::new().with_tool(Arc::new(slow));
    let call = ToolCallPart {
        id: "call_slow".to_string(),
        name: "slow".to_string(),
        arguments: json!({}).into(),
    };
    let context = context().with_cancellation_token(token.clone());
    let task = tokio::spawn(async move { registry.execute_call(context, &call).await });

    started_receiver.recv().await.unwrap();
    let Some(tool_token) = observed_token.lock().unwrap().clone() else {
        panic!("tool should observe the cancellation token");
    };
    assert!(!tool_token.is_cancelled());

    token.cancel();
    let returned = task.await.unwrap();

    assert!(tool_token.is_cancelled());
    assert!(returned.is_error);
    assert_eq!(returned.metadata["error_kind"], "cancelled");
    assert_eq!(returned.content["kind"], "cancelled");
    assert!(!returned.content["retryable"].as_bool().unwrap());
    assert!(returned.content["how_to_fix"]
        .as_str()
        .unwrap()
        .contains("new run"));
}
