//! CLI `AgentSpec` profile resolution.

use std::{fs, path::PathBuf, sync::Arc};

use serde_json::json;
use starweaver_agent::{
    environment_toolsets, string_tool, AgentSpec, AgentSpecRegistry, FunctionModel, StaticToolset,
    ToolError, ToolResult,
};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
};

use crate::{config::CliConfig, error::io_error, CliError, CliResult};

/// Resolved CLI profile ready to build an agent session.
pub struct ResolvedProfile {
    /// Profile name.
    pub name: String,
    /// Source path when loaded from YAML.
    pub source: Option<PathBuf>,
    /// Parsed spec.
    pub spec: AgentSpec,
    /// Registry with CLI-provided models and toolsets.
    pub registry: AgentSpecRegistry,
}

impl ResolvedProfile {
    /// Build the runtime agent from the profile.
    pub fn build_agent(&self) -> CliResult<starweaver_runtime::Agent> {
        self.spec
            .builder(&self.registry)
            .map_err(|error| CliError::Config(error.to_string()))
            .map(starweaver_agent::AgentBuilder::build)
    }
}

/// Resolve an `AgentSpec` by name or YAML path.
pub fn resolve_profile(config: &CliConfig, requested: Option<&str>) -> CliResult<ResolvedProfile> {
    let requested = requested.unwrap_or(&config.default_profile);
    let (spec, source) = if requested == "general" || requested == "default" {
        (default_spec(requested), None)
    } else if requested == "approval_model" {
        (scripted_spec("approval_model", "approval_model"), None)
    } else if requested == "deferred_model" {
        (scripted_spec("deferred_model", "deferred_model"), None)
    } else if let Some(path) = find_profile_path(config, requested) {
        let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
        let spec =
            AgentSpec::from_yaml(&content).map_err(|error| CliError::Config(error.to_string()))?;
        (spec, Some(path))
    } else {
        return Err(CliError::NotFound(format!("profile {requested}")));
    };
    let name = spec.name.clone();
    Ok(ResolvedProfile {
        name,
        source,
        spec,
        registry: default_registry(),
    })
}

fn find_profile_path(config: &CliConfig, requested: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(requested);
    if direct.exists() {
        return Some(direct);
    }
    config.profile_search_paths.iter().find_map(|root| {
        [
            root.join(requested),
            root.join(format!("{requested}.yaml")),
            root.join(format!("{requested}.yml")),
        ]
        .into_iter()
        .find(|path| path.exists())
    })
}

fn default_spec(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec!["You are Starweaver CLI, a deterministic local assistant.".to_string()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "local_echo".to_string(),
            settings_preset: None,
            settings: None,
        }),
        toolsets: vec!["cli_control_flow".to_string(), "environment".to_string()],
        ..AgentSpec::default()
    }
}

fn scripted_spec(name: &str, model_id: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec!["Exercise CLI control-flow handling deterministically.".to_string()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: model_id.to_string(),
            settings_preset: None,
            settings: None,
        }),
        toolsets: vec!["cli_control_flow".to_string()],
        ..AgentSpec::default()
    }
}

fn default_registry() -> AgentSpecRegistry {
    let mut registry = AgentSpecRegistry::new()
        .with_model("local_echo", Arc::new(local_echo_model()))
        .with_model(
            "approval_model",
            Arc::new(scripted_tool_model("approval_probe")),
        )
        .with_model(
            "deferred_model",
            Arc::new(scripted_tool_model("deferred_probe")),
        )
        .with_toolset_alias("cli_control_flow", Arc::new(control_flow_toolset()));
    for toolset in environment_toolsets() {
        registry = registry.with_toolset_alias("environment", toolset);
    }
    registry
}

fn local_echo_model() -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
        let prompt = latest_user_prompt(&messages).unwrap_or_default();
        Ok(ModelResponse::text(format!("local echo: {prompt}")))
    })
}

fn scripted_tool_model(tool_name: &'static str) -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
        if messages.iter().any(message_has_tool_return) {
            return Ok(ModelResponse::text(format!("{tool_name} handled")));
        }
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: format!("{tool_name}_call"),
                name: tool_name.to_string(),
                arguments: json!({"action": tool_name}),
            })],
            usage: starweaver_core::Usage::default(),
            model_name: Some(tool_name.to_string()),
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::default(),
        })
    })
}

fn latest_user_prompt(messages: &[ModelMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().rev().find_map(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => {
                content.iter().find_map(|part| match part {
                    starweaver_model::ContentPart::Text { text } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        }),
        ModelMessage::Response(_) => None,
    })
}

fn message_has_tool_return(message: &ModelMessage) -> bool {
    match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_))),
        ModelMessage::Response(_) => false,
    }
}

fn control_flow_toolset() -> StaticToolset {
    let approval_tool = string_tool(
        "approval_probe",
        Some("Deterministic approval probe".to_string()),
        json!({"type": "object"}),
        |_context, arguments| async move {
            Err(ToolError::ApprovalRequired {
                tool: "approval_probe".to_string(),
                metadata: json!({"arguments": arguments, "reason": "cli approval probe"}),
            })
        },
    );
    let deferred_tool = string_tool(
        "deferred_probe",
        Some("Deterministic deferred-call probe".to_string()),
        json!({"type": "object"}),
        |_context, arguments| async move {
            Err(ToolError::CallDeferred {
                tool: "deferred_probe".to_string(),
                metadata: json!({"arguments": arguments, "reason": "cli deferred probe"}),
            })
        },
    );
    StaticToolset::new("cli_control_flow")
        .with_id("cli_control_flow")
        .with_tool(Arc::new(approval_tool))
        .with_tool(Arc::new(deferred_tool))
}

#[allow(dead_code)]
fn ok_tool_result(value: serde_json::Value) -> ToolResult {
    ToolResult::new(value)
}
