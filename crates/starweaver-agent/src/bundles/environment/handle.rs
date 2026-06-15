use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_environment::{DynEnvironmentProvider, DynProcessShellProvider};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart,
    INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT, INSTRUCTION_ORIGIN_HANDOFF,
    INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_RUNTIME_CONTEXT,
};
use starweaver_runtime::{AgentCapability, AgentRunState, CapabilityError, CapabilityResult};
use starweaver_tools::{ToolContext, ToolError};

use crate::bundles::helpers::tool_execution_error;

/// `AgentContext` dependency that exposes the active SDK environment.
#[derive(Clone)]
pub struct EnvironmentHandle {
    provider: DynEnvironmentProvider,
}

impl EnvironmentHandle {
    /// Create an environment handle from a provider.
    #[must_use]
    pub fn new(provider: DynEnvironmentProvider) -> Self {
        Self { provider }
    }

    /// Return the underlying provider.
    #[must_use]
    pub fn provider(&self) -> DynEnvironmentProvider {
        self.provider.clone()
    }
}

/// Capability that injects provider-supplied environment context into model requests.
#[derive(Clone, Debug, Default)]
pub struct EnvironmentContextCapability;

#[async_trait]
impl AgentCapability for EnvironmentContextCapability {
    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let Some(request_index) = latest_request_index(&messages) else {
            return Ok(messages);
        };
        let has_control_part = match &messages[request_index] {
            ModelMessage::Request(request) => request_has_tool_return_or_retry(request),
            ModelMessage::Response(_) => false,
        };
        if has_control_part && !force_inject_instructions(state, context) {
            return Ok(messages);
        }

        let Some(environment) = context.dependencies.get::<EnvironmentHandle>() else {
            return Ok(messages);
        };
        let Some(text) = environment
            .provider()
            .get_context_instructions()
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))?
        else {
            return Ok(messages);
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            INSTRUCTION_ORIGIN_METADATA.to_string(),
            serde_json::json!(INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT),
        );
        if let ModelMessage::Request(request) = &mut messages[request_index] {
            insert_context_part_after_control_parts(
                request,
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text { text }],
                    name: None,
                    metadata,
                },
            );
        }
        Ok(messages)
    }
}

fn latest_request_index(messages: &[ModelMessage]) -> Option<usize> {
    messages
        .iter()
        .rposition(|message| matches!(message, ModelMessage::Request(_)))
}

fn request_has_tool_return_or_retry(request: &ModelRequest) -> bool {
    request.parts.iter().any(|part| {
        matches!(
            part,
            ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. }
        )
    })
}

fn insert_context_part_after_control_parts(request: &mut ModelRequest, part: ModelRequestPart) {
    let instruction_end = request_instruction_end_index(request);
    let context_prefix_len = request.parts[instruction_end..]
        .iter()
        .take_while(|part| is_context_user_prompt(part))
        .count();
    request
        .parts
        .insert(instruction_end + context_prefix_len, part);
}

fn request_instruction_end_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count();
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_instruction_prefix_part(part))
            .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => metadata
            .get("starweaver_instruction_origin")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|origin| origin == "tool_return_media"),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

const fn is_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    matches!(
        part,
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. }
    )
}

fn is_context_user_prompt(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::UserPrompt { metadata, .. } => metadata
            .get(INSTRUCTION_ORIGIN_METADATA)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|origin| {
                matches!(
                    origin,
                    INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT
                        | INSTRUCTION_ORIGIN_RUNTIME_CONTEXT
                        | INSTRUCTION_ORIGIN_HANDOFF
                )
            }),
        ModelRequestPart::SystemPrompt { .. }
        | ModelRequestPart::Instruction { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
}

fn force_inject_instructions(state: &AgentRunState, context: &AgentContext) -> bool {
    metadata_bool(&state.metadata, "starweaver_force_inject_instructions")
        || metadata_bool(&context.metadata, "starweaver_force_inject_instructions")
}

fn metadata_bool(metadata: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Attach the active environment to an `AgentContext`.
///
/// Process-capable environment providers also expose the background shell handle
/// from the same attachment point, so callers do not need a separate injection
/// path for foreground and background shell operations.
pub fn attach_environment(context: &mut AgentContext, provider: DynEnvironmentProvider) {
    context
        .dependencies
        .insert(EnvironmentHandle::new(provider.clone()));
    if let Some(process_provider) = provider.process_shell_provider() {
        super::shell::attach_process_shell(context, process_provider);
    }
}

/// Collect environment-provided toolsets for a provider.
#[must_use]
pub fn environment_toolsets() -> Vec<starweaver_tools::DynToolset> {
    vec![
        super::filesystem::filesystem_tools(),
        super::shell::shell_tools(),
    ]
}

/// Collect resource-backed toolsets for a process-capable provider.
#[must_use]
pub fn process_shell_toolsets(
    _provider: DynProcessShellProvider,
) -> Vec<starweaver_tools::DynToolset> {
    vec![super::shell::shell_tools()]
}

pub(super) fn environment_provider(
    context: &ToolContext,
    tool: &str,
) -> Result<DynEnvironmentProvider, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    let environment = agent_context
        .dependencies
        .get::<EnvironmentHandle>()
        .ok_or_else(|| {
            tool_execution_error(tool, "EnvironmentHandle is missing from AgentContext")
        })?;
    Ok(environment.provider())
}

pub(super) fn maybe_environment_provider(context: &ToolContext) -> Option<DynEnvironmentProvider> {
    let agent_context = context.dependency::<AgentContext>()?;
    let environment = agent_context.dependencies.get::<EnvironmentHandle>()?;
    Some(environment.provider())
}
