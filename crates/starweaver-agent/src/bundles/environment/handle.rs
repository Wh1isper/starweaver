use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_environment::{DynEnvironmentProvider, DynProcessShellProvider};
use starweaver_model::{
    context_origin_metadata, ContentPart, ModelMessage, ModelRequest, ModelRequestPart,
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_HANDOFF, CONTEXT_ORIGIN_METADATA,
    CONTEXT_ORIGIN_RUNTIME_CONTEXT, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, CONTEXT_TYPE_METADATA,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, CapabilityError, CapabilityOrdering, CapabilityResult,
    CapabilitySpec, RUNTIME_CONTEXT_CAPABILITY_ID,
};
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
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("starweaver.environment.context")
            .with_description(
                "Injects provider-supplied environment context into the initial model request.",
            )
            .with_ordering(CapabilityOrdering::default().before(RUNTIME_CONTEXT_CAPABILITY_ID))
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        strip_context_origin_from_latest_request(&mut messages, CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT);
        inject_environment_context(context, &mut messages).await?;
        Ok(messages)
    }

    async fn prepare_provider_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }
}

async fn inject_environment_context(
    context: &AgentContext,
    messages: &mut Vec<ModelMessage>,
) -> CapabilityResult<()> {
    let force_inject = context.force_inject_context;
    if latest_request(messages).is_some_and(request_has_tool_return_or_retry) && !force_inject {
        return Ok(());
    }
    if has_context_origin(messages, CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT) && !force_inject {
        return Ok(());
    }
    let Some(environment) = context.dependencies.get::<EnvironmentHandle>() else {
        return Ok(());
    };
    let Some(text) = environment
        .provider()
        .render_environment_context()
        .await
        .map_err(|error| CapabilityError::Failed(error.to_string()))?
        .filter(|text| !text.trim().is_empty())
    else {
        return Ok(());
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        CONTEXT_TYPE_METADATA.to_string(),
        serde_json::json!("environment"),
    );
    metadata.insert(
        CONTEXT_ORIGIN_METADATA.to_string(),
        serde_json::json!(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT),
    );
    insert_context_into_latest_request(
        messages,
        ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text { text }],
            name: None,
            metadata,
        },
    );
    Ok(())
}

fn strip_context_origin_from_latest_request(messages: &mut [ModelMessage], origin: &str) {
    let Some(request) = messages.iter_mut().rev().find_map(|message| match message {
        ModelMessage::Request(request) => Some(request),
        ModelMessage::Response(_) => None,
    }) else {
        return;
    };
    request
        .parts
        .retain(|part| part_context_origin(part) != Some(origin));
}

fn has_context_origin(messages: &[ModelMessage], origin: &str) -> bool {
    messages.iter().any(|message| match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .any(|part| part_context_origin(part) == Some(origin)),
        ModelMessage::Response(_) => false,
    })
}

fn part_context_origin(part: &ModelRequestPart) -> Option<&str> {
    match part {
        ModelRequestPart::SystemPrompt { metadata, .. }
        | ModelRequestPart::UserPrompt { metadata, .. }
        | ModelRequestPart::Instruction { metadata, .. } => context_origin_metadata(metadata),
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => None,
    }
}

fn latest_request(messages: &[ModelMessage]) -> Option<&ModelRequest> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => Some(request),
        ModelMessage::Response(_) => None,
    })
}

fn request_has_tool_return_or_retry(request: &ModelRequest) -> bool {
    request.parts.iter().any(|part| {
        matches!(
            part,
            ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. }
        )
    })
}

fn insert_context_into_latest_request(messages: &mut Vec<ModelMessage>, part: ModelRequestPart) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            let insert_at = request_context_insert_index(request);
            request.parts.insert(insert_at, part);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![part],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }));
}

fn request_context_insert_index(request: &ModelRequest) -> usize {
    let instruction_end = request_instruction_end_index(request);
    instruction_end
        + request.parts[instruction_end..]
            .iter()
            .take_while(|part| is_context_user_prompt(part) || is_user_prompt(part))
            .count()
}

fn request_instruction_end_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request_control_prefix_len(request);
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_instruction_prefix_part(part))
            .count()
}

fn request_control_prefix_len(request: &ModelRequest) -> usize {
    request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => context_origin_metadata(metadata)
            .is_some_and(|origin| origin == CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
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
        ModelRequestPart::UserPrompt { metadata, .. } => context_origin_metadata(metadata)
            .is_some_and(|origin| {
                matches!(
                    origin,
                    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT
                        | CONTEXT_ORIGIN_RUNTIME_CONTEXT
                        | CONTEXT_ORIGIN_HANDOFF
                )
            }),
        ModelRequestPart::SystemPrompt { .. }
        | ModelRequestPart::Instruction { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
}

const fn is_user_prompt(part: &ModelRequestPart) -> bool {
    matches!(part, ModelRequestPart::UserPrompt { .. })
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
