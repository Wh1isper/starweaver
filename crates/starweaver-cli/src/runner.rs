//! CLI run execution and display augmentation.

use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_agent::{
    attach_process_shell, attach_shell_review_handle, AgentSession, AgentStreamRecord,
    ResumableState,
};
use starweaver_context::{AgentContext, BusMessage};
use starweaver_environment::{DynEnvironmentProvider, DynProcessShellProvider};
use starweaver_model::{
    ContentPart, FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelSettings, StreamDelta, INSTRUCTION_DYNAMIC_METADATA,
    INSTRUCTION_ORIGIN_METADATA,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, AgentStreamEvent, CapabilityResult, ModelResponseStreamEvent,
};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus,
    RunRecord, RunStatus,
};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, RealtimeCompactionBuffer, ReplayScope, ReplaySnapshot,
};

use crate::{
    args::HitlPolicy, local_store::RunArtifacts, profiles::ResolvedProfile,
    prompt_input::PromptInput, CliError, CliResult,
};

mod projection;

pub use projection::failed_display_message;
use projection::{
    cancelled_display_projection, failed_display_projection, interrupted_partial_response,
    project_display_messages,
};

/// CLI run policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliRunPolicy {
    /// Headless human-in-the-loop behavior.
    pub hitl: HitlPolicy,
}

/// Steering message sent from the interactive UI into the running agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliSteeringMessage {
    /// UI-generated stable steering id used to correlate runtime acknowledgements.
    pub id: String,
    /// User steering text.
    pub text: String,
}

/// CLI execution output and durable artifacts.
pub struct CliRunExecution {
    /// Final output preview.
    pub output: String,
    /// Durable artifacts.
    pub artifacts: RunArtifacts,
}

/// Execute a resolved profile through `AgentSession`.
pub fn execute_agent_session(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_stream_sender(
        input,
        run,
        profile,
        environment,
        process_environment,
        restore_state,
        policy,
        None,
    )
}

/// Execute a resolved profile and forward live stream records to a caller-owned channel.
#[allow(clippy::too_many_arguments)]
pub fn execute_agent_session_with_stream_sender(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
    stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_channels(
        input,
        run,
        profile,
        environment,
        process_environment,
        restore_state,
        policy,
        stream_sender,
        None,
        None,
    )
}

/// Execute a resolved profile, forward live stream records, and poll caller-owned steering messages.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn execute_agent_session_with_channels(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
    stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
    steering_receiver: Option<mpsc::Receiver<CliSteeringMessage>>,
    cancel_receiver: Option<mpsc::Receiver<()>>,
) -> CliResult<CliRunExecution> {
    let mut agent = profile.build_agent()?;
    let pending_steering = steering_receiver.map(start_steering_collector);
    let observed_records = Arc::new(Mutex::new(Vec::new()));
    agent = agent.with_stream_observer(Arc::new(CliStreamObserver {
        sender: stream_sender,
        records: Arc::clone(&observed_records),
    }));
    let prompt_text = input.text.clone();
    let guidance_text_parts = input.guidance_text_parts.clone();
    if input.has_content_parts() {
        agent = agent.with_capability(Arc::new(CliPromptContentAdapter {
            content_parts: input.into_content_parts(),
        }));
    }
    if !guidance_text_parts.is_empty() {
        agent = agent.with_capability(Arc::new(CliGuidanceAdapter {
            guidance_text_parts,
        }));
    }
    if let Some(pending) = pending_steering {
        agent = agent.with_capability(Arc::new(CliSteeringAdapter { pending }));
    }
    let mut session = restore_state.map_or_else(
        || AgentSession::new(agent.clone()),
        |state| AgentSession::from_state(agent.clone(), state),
    );
    session.set_environment(environment.clone());
    if let Some(process_environment) = process_environment {
        attach_process_shell(session.context_mut(), process_environment.clone());
    }
    profile.configure_context(session.context_mut());
    if let Some(shell_review) = profile.shell_review.as_ref() {
        attach_shell_review_handle(session.context_mut(), shell_review.clone());
    }
    session.set_metadata("cli.profile", json!(profile.name));
    session.set_metadata("cli.profile_source", json!(profile.source.kind()));
    if let Some(path) = profile.source.path() {
        session.set_metadata("cli.profile_path", json!(path));
    }
    session.set_metadata("cli.run_id", json!(run.run_id.as_str()));
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let run_outcome = run_session_stream(&runtime, &mut session, prompt_text, cancel_receiver)?;
    let environment_state = runtime
        .block_on(environment.export_state())
        .map_err(|error| CliError::Run(error.to_string()))?;
    let (output, raw_records, projection, saved_interrupted_partial, failure_error) =
        match run_outcome {
            SessionRunOutcome::Completed(stream) => {
                let projection = project_display_messages(run, &stream.events, policy, &runtime);
                (stream.result.output, stream.events, projection, false, None)
            }
            SessionRunOutcome::Cancelled => {
                let raw_records = observed_records
                    .lock()
                    .map_or_else(|_| Vec::new(), |records| records.clone());
                let partial = interrupted_partial_response(&raw_records, session.context());
                let saved_partial = partial.is_some();
                if let Some(partial) = partial {
                    session
                        .context_mut()
                        .message_history
                        .push(ModelMessage::Response(partial));
                }
                let projection = cancelled_display_projection(run, &raw_records, policy, &runtime);
                (
                    "cancelled".to_string(),
                    raw_records,
                    projection,
                    saved_partial,
                    None,
                )
            }
            SessionRunOutcome::Failed(error) => {
                let raw_records = observed_records
                    .lock()
                    .map_or_else(|_| Vec::new(), |records| records.clone());
                let partial = interrupted_partial_response(&raw_records, session.context());
                let saved_partial = partial.is_some();
                if let Some(partial) = partial {
                    session
                        .context_mut()
                        .message_history
                        .push(ModelMessage::Response(partial));
                }
                let projection =
                    failed_display_projection(run, &raw_records, &error, policy, &runtime);
                (
                    error.clone(),
                    raw_records,
                    projection,
                    saved_partial,
                    Some(error),
                )
            }
        };
    let mut state = session.export_state();
    state
        .metadata
        .insert("cli.run_id".to_string(), json!(run.run_id.as_str()));
    state
        .metadata
        .insert("cli.session_id".to_string(), json!(run.session_id.as_str()));
    if projection.status == RunStatus::Cancelled {
        state
            .metadata
            .insert("cli.interrupted".to_string(), json!(true));
        state.metadata.insert(
            "cli.interrupted.reason".to_string(),
            json!("cancelled_by_user"),
        );
        state.metadata.insert(
            "cli.interrupted.saved_partial".to_string(),
            json!(saved_interrupted_partial),
        );
    }
    if let Some(error) = failure_error.as_ref() {
        state.metadata.insert("cli.failed".to_string(), json!(true));
        state
            .metadata
            .insert("cli.failed.error".to_string(), json!(error));
        state.metadata.insert(
            "cli.failed.saved_partial".to_string(),
            json!(saved_interrupted_partial),
        );
    }
    let artifacts = RunArtifacts {
        state,
        environment_state: Some(environment_state),
        raw_records,
        display_messages: projection.messages,
        display_snapshot: projection.snapshot,
        approvals: projection.approvals,
        deferred_tools: projection.deferred_tools,
        status: projection.status,
    };
    Ok(CliRunExecution { output, artifacts })
}

enum SessionRunOutcome {
    Completed(Box<starweaver_agent::AgentStreamResult>),
    Cancelled,
    Failed(String),
}

fn run_session_stream(
    runtime: &tokio::runtime::Runtime,
    session: &mut AgentSession,
    prompt: String,
    cancel_receiver: Option<mpsc::Receiver<()>>,
) -> CliResult<SessionRunOutcome> {
    let run_future = session.run_stream(prompt);
    if let Some(cancel_receiver) = cancel_receiver {
        runtime.block_on(async move {
            tokio::select! {
                result = run_future => Ok(match result {
                    Ok(stream) => SessionRunOutcome::Completed(Box::new(stream)),
                    Err(error) => SessionRunOutcome::Failed(error.to_string()),
                }),
                () = wait_for_cancel(cancel_receiver) => Ok(SessionRunOutcome::Cancelled),
            }
        })
    } else {
        Ok(match runtime.block_on(run_future) {
            Ok(stream) => SessionRunOutcome::Completed(Box::new(stream)),
            Err(error) => SessionRunOutcome::Failed(error.to_string()),
        })
    }
}

async fn wait_for_cancel(cancel_receiver: mpsc::Receiver<()>) {
    loop {
        match cancel_receiver.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }
}

fn start_steering_collector(receiver: mpsc::Receiver<CliSteeringMessage>) -> Arc<PendingSteering> {
    let pending = Arc::new(PendingSteering::default());
    let thread_pending = Arc::clone(&pending);
    thread::spawn(move || {
        while let Ok(message) = receiver.recv() {
            if let Ok(mut messages) = thread_pending.messages.lock() {
                messages.push_back(message);
            }
        }
    });
    pending
}

struct CliStreamObserver {
    sender: Option<mpsc::Sender<AgentStreamRecord>>,
    records: Arc<Mutex<Vec<AgentStreamRecord>>>,
}

#[async_trait]
impl AgentCapability for CliStreamObserver {
    async fn on_stream_event(
        &self,
        _state: &AgentRunState,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        if let Ok(mut records) = self.records.lock() {
            records.push(event.clone());
        }
        if let Some(sender) = &self.sender {
            let _ = sender.send(event.clone());
        }
        Ok(())
    }
}

struct CliPromptContentAdapter {
    content_parts: Vec<ContentPart>,
}

struct CliGuidanceAdapter {
    guidance_text_parts: Vec<String>,
}

#[async_trait]
impl AgentCapability for CliPromptContentAdapter {
    async fn before_model_request(
        &self,
        state: &mut AgentRunState,
        request: &mut ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        if state.run_step != 0 || self.content_parts.is_empty() {
            return Ok(());
        }
        if let Some(ModelRequestPart::UserPrompt {
            content, metadata, ..
        }) = request
            .parts
            .iter_mut()
            .find(|part| matches!(part, ModelRequestPart::UserPrompt { .. }))
        {
            content.clone_from(&self.content_parts);
            metadata.insert(
                "starweaver.cli.attachments".to_string(),
                json!(content
                    .iter()
                    .filter(|part| matches!(part, ContentPart::Binary { .. }))
                    .count()),
            );
        }
        Ok(())
    }
}

#[async_trait]
impl AgentCapability for CliGuidanceAdapter {
    // This capability may run after dynamic/environment context injection. It
    // intentionally inserts at the static instruction boundary so CLI guidance
    // remains cacheable and independent of capability ordering.
    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        if self.guidance_text_parts.is_empty() {
            return Ok(messages);
        }
        let Some(ModelMessage::Request(request)) = messages
            .iter_mut()
            .rev()
            .find(|message| matches!(message, ModelMessage::Request(_)))
        else {
            return Ok(messages);
        };
        let parts = self
            .guidance_text_parts
            .iter()
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    INSTRUCTION_ORIGIN_METADATA.to_string(),
                    json!("cli_guidance"),
                );
                metadata.insert(INSTRUCTION_DYNAMIC_METADATA.to_string(), json!(false));
                ModelRequestPart::Instruction {
                    text: text.clone(),
                    metadata,
                }
            })
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return Ok(messages);
        }
        let insert_at = static_instruction_insert_index(request);
        request.parts.splice(insert_at..insert_at, parts);
        Ok(messages)
    }
}

fn static_instruction_insert_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count();
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_static_instruction_prefix_part(part))
            .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => metadata
            .get(INSTRUCTION_ORIGIN_METADATA)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|origin| origin == "tool_return_media"),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

fn is_static_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::SystemPrompt { .. } => true,
        ModelRequestPart::Instruction { metadata, .. } => !metadata
            .get(INSTRUCTION_DYNAMIC_METADATA)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        ModelRequestPart::UserPrompt { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
}

#[derive(Default)]
struct PendingSteering {
    messages: Mutex<VecDeque<CliSteeringMessage>>,
}

struct CliSteeringAdapter {
    pending: Arc<PendingSteering>,
}

#[async_trait]
impl AgentCapability for CliSteeringAdapter {
    async fn before_model_request_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _request: &mut ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        drain_pending_steering(&self.pending, context);
        Ok(())
    }

    async fn validate_output_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _output: &str,
    ) -> CapabilityResult<()> {
        drain_pending_steering(&self.pending, context);
        Ok(())
    }
}

fn drain_pending_steering(pending: &PendingSteering, context: &mut AgentContext) -> bool {
    let mut drained = false;
    if let Ok(mut messages) = pending.messages.lock() {
        while let Some(message) = messages.pop_front() {
            context.enqueue_message(BusMessage::new(
                "steering",
                json!({"id": message.id, "text": message.text}),
            ));
            drained = true;
        }
    }
    drained
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use std::{
        sync::{mpsc, Arc, Mutex},
        thread,
        time::Duration,
    };

    use starweaver_agent::{AgentBuilder, AgentSession, FunctionModel, FunctionModelInfo};
    use starweaver_context::AgentContext;
    use starweaver_core::{ConversationId, RunId, SessionId};
    use starweaver_model::{
        ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
        ModelResponsePart, ModelResponseStreamEvent, ModelSettings, PartDelta, PartEnd, PartStart,
        INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_METADATA,
    };
    use starweaver_runtime::{AgentCapability, AgentRunState, AgentStreamEvent, AgentStreamRecord};
    use starweaver_session::{RunRecord, RunStatus};
    use starweaver_stream::DisplayMessageKind;

    use super::{
        cancelled_display_projection, interrupted_partial_response, start_steering_collector,
        CliGuidanceAdapter, CliPromptContentAdapter, CliRunPolicy, CliSteeringMessage,
    };
    use crate::{args::HitlPolicy, prompt_input::PromptAttachment};

    #[test]
    fn prompt_content_adapter_replaces_initial_user_prompt_with_multimodal_parts() {
        let attachment = PromptAttachment::image(1, b"image-bytes".to_vec(), "image/png");
        let placeholder = attachment.placeholder.clone();
        let adapter = CliPromptContentAdapter {
            content_parts: crate::prompt_input::PromptInput {
                text: format!("inspect this {placeholder} now"),
                attachments: vec![attachment],
                extra_text_parts: vec!["Extra context for this prompt.".to_string()],
                guidance_text_parts: Vec::new(),
            }
            .into_content_parts(),
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_attach"),
            ConversationId::from_string("conversation_attach"),
        );
        let mut request = ModelRequest::user_text(format!("inspect this {placeholder} now"));
        let mut settings = None::<ModelSettings>;

        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.before_model_request(&mut state, &mut request, &mut settings))
            .expect("adapter should update request");

        let ModelRequestPart::UserPrompt {
            content, metadata, ..
        } = &request.parts[0]
        else {
            panic!("expected user prompt");
        };
        assert_eq!(
            content,
            &vec![
                ContentPart::Text {
                    text: "inspect this  now".to_string(),
                },
                ContentPart::Binary {
                    data: b"image-bytes".to_vec(),
                    media_type: "image/png".to_string(),
                },
                ContentPart::Text {
                    text: "Extra context for this prompt.".to_string(),
                },
            ]
        );
        assert_eq!(metadata["starweaver.cli.attachments"], 1);
    }

    #[test]
    fn prompt_content_adapter_appends_extra_text_parts_to_initial_request() {
        let adapter = CliPromptContentAdapter {
            content_parts: crate::prompt_input::PromptInput {
                text: "implement feature".to_string(),
                attachments: Vec::new(),
                extra_text_parts: vec![
                    "Extra context one.".to_string(),
                    "Extra context two.".to_string(),
                ],
                guidance_text_parts: Vec::new(),
            }
            .into_content_parts(),
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_guidance"),
            ConversationId::from_string("conversation_guidance"),
        );
        let mut request = ModelRequest::user_text("implement feature");
        let mut settings = None::<ModelSettings>;

        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.before_model_request(&mut state, &mut request, &mut settings))
            .expect("adapter should update request");

        let ModelRequestPart::UserPrompt {
            content, metadata, ..
        } = &request.parts[0]
        else {
            panic!("expected user prompt");
        };
        assert_eq!(metadata["starweaver.cli.attachments"], 0);
        assert_eq!(content.len(), 3);
        assert!(matches!(
            &content[0],
            ContentPart::Text { text } if text == "implement feature"
        ));
        assert!(matches!(
            &content[1],
            ContentPart::Text { text } if text == "Extra context one."
        ));
        assert!(matches!(
            &content[2],
            ContentPart::Text { text } if text == "Extra context two."
        ));
    }

    #[test]
    fn cli_guidance_adapter_injects_guidance_as_static_request_instructions() {
        let adapter = CliGuidanceAdapter {
            guidance_text_parts: vec![
                "<project-guidance name=AGENTS.md>\nUse cargo test.\n</project-guidance>"
                    .to_string(),
                "<user-rules location=/home/user/.starweaver/RULES.md>\nPrefer Chinese replies.\n</user-rules>"
                    .to_string(),
            ],
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_guidance"),
            ConversationId::from_string("conversation_guidance"),
        );
        let mut context = AgentContext::default();
        let mut dynamic_metadata = serde_json::Map::new();
        dynamic_metadata.insert(
            INSTRUCTION_DYNAMIC_METADATA.to_string(),
            serde_json::json!(true),
        );
        let request = ModelRequest {
            parts: vec![
                ModelRequestPart::SystemPrompt {
                    text: "stable system".to_string(),
                    metadata: serde_json::Map::new(),
                },
                ModelRequestPart::Instruction {
                    text: "<environment-context>fresh</environment-context>".to_string(),
                    metadata: dynamic_metadata,
                },
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "implement feature".to_string(),
                    }],
                    name: None,
                    metadata: serde_json::Map::new(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        };

        let messages = tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.prepare_model_messages_with_context(
                &mut state,
                &mut context,
                vec![ModelMessage::Request(request)],
            ))
            .expect("adapter should inject guidance");

        let ModelMessage::Request(request) = messages.last().expect("request") else {
            panic!("expected request");
        };
        assert!(matches!(
            &request.parts[0],
            ModelRequestPart::SystemPrompt { text, .. } if text == "stable system"
        ));
        assert!(matches!(
            &request.parts[1],
            ModelRequestPart::Instruction { text, metadata }
                if text.contains("<project-guidance name=AGENTS.md>")
                    && metadata.get(INSTRUCTION_ORIGIN_METADATA) == Some(&serde_json::json!("cli_guidance"))
                    && metadata.get(INSTRUCTION_DYNAMIC_METADATA) == Some(&serde_json::json!(false))
        ));
        assert!(matches!(
            &request.parts[2],
            ModelRequestPart::Instruction { text, metadata }
                if text.contains("<user-rules location=/home/user/.starweaver/RULES.md>")
                    && metadata.get(INSTRUCTION_ORIGIN_METADATA) == Some(&serde_json::json!("cli_guidance"))
                    && metadata.get(INSTRUCTION_DYNAMIC_METADATA) == Some(&serde_json::json!(false))
        ));
        assert!(matches!(
            &request.parts[3],
            ModelRequestPart::Instruction { text, metadata }
                if text.contains("<environment-context>fresh</environment-context>")
                    && metadata.get(INSTRUCTION_DYNAMIC_METADATA) == Some(&serde_json::json!(true))
        ));
        assert!(matches!(
            &request.parts[4],
            ModelRequestPart::UserPrompt { content, .. }
                if matches!(&content[0], ContentPart::Text { text } if text == "implement feature")
        ));
    }

    #[test]
    fn cli_guidance_is_model_facing_but_not_persisted_in_session_history() {
        let captured_messages = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
        let model_captured = Arc::clone(&captured_messages);
        let model = FunctionModel::new(
            move |messages: Vec<ModelMessage>,
                  _settings: Option<ModelSettings>,
                  _info: FunctionModelInfo| {
                model_captured.lock().unwrap().push(messages);
                Ok(ModelResponse::text("ok"))
            },
        );
        let guidance =
            "<project-guidance name=AGENTS.md>\nUse cargo test.\n</project-guidance>".to_string();
        let agent = AgentBuilder::new(Arc::new(model))
            .capability(Arc::new(CliGuidanceAdapter {
                guidance_text_parts: vec![guidance.clone()],
            }))
            .build();
        let mut session = AgentSession::new(agent);
        let runtime = tokio::runtime::Runtime::new().expect("runtime should start");

        let first_result = runtime
            .block_on(session.run_stream("implement feature"))
            .expect("first run should succeed");
        assert_eq!(first_result.result.output, "ok");
        let second_result = runtime
            .block_on(session.run_stream("continue feature"))
            .expect("second run should succeed");
        assert_eq!(second_result.result.output, "ok");

        {
            let captured = captured_messages.lock().unwrap();
            assert_eq!(captured.len(), 2);
            assert_eq!(count_guidance_in_messages(&captured[0], &guidance), 1);
            assert_eq!(count_guidance_in_messages(&captured[1], &guidance), 1);
            drop(captured);
        }
        let persisted = serde_json::to_string(&session.export_state().message_history)
            .expect("message history should serialize");
        assert!(!persisted.contains("project-guidance"));
        assert!(!persisted.contains("Use cargo test."));
    }

    fn count_guidance_in_messages(messages: &[ModelMessage], guidance: &str) -> usize {
        messages
            .iter()
            .flat_map(|message| match message {
                ModelMessage::Request(request) => request.parts.iter().collect::<Vec<_>>(),
                ModelMessage::Response(_) => Vec::new(),
            })
            .filter(|part| {
                matches!(
                    part,
                    ModelRequestPart::Instruction { text, metadata }
                        if text == guidance
                            && metadata.get(INSTRUCTION_ORIGIN_METADATA)
                                == Some(&serde_json::json!("cli_guidance"))
                            && metadata.get(INSTRUCTION_DYNAMIC_METADATA)
                                == Some(&serde_json::json!(false))
                )
            })
            .count()
    }

    #[test]
    fn prompt_content_adapter_only_updates_first_model_step() {
        let adapter = CliPromptContentAdapter {
            content_parts: vec![ContentPart::Binary {
                data: vec![1, 2, 3],
                media_type: "image/png".to_string(),
            }],
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_attach_skip"),
            ConversationId::from_string("conversation_attach_skip"),
        );
        state.run_step = 1;
        let mut request = ModelRequest::user_text("retry text");
        let mut settings = None::<ModelSettings>;

        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.before_model_request(&mut state, &mut request, &mut settings))
            .expect("adapter should skip request");

        let ModelRequestPart::UserPrompt { content, .. } = &request.parts[0] else {
            panic!("expected user prompt");
        };
        assert_eq!(
            content,
            &vec![ContentPart::Text {
                text: "retry text".to_string(),
            }]
        );
    }

    #[test]
    fn steering_collector_buffers_messages_without_runtime_ack() {
        let (steer_sender, steer_receiver) = mpsc::channel::<CliSteeringMessage>();
        let pending = start_steering_collector(steer_receiver);

        assert!(steer_sender
            .send(CliSteeringMessage {
                id: "steer_test".to_string(),
                text: "tighten scroll".to_string(),
            })
            .is_ok());

        let mut buffered = None;
        for _ in 0..20 {
            buffered = {
                let mut messages = pending
                    .messages
                    .lock()
                    .expect("pending steering lock should be available");
                messages.pop_front()
            };
            if buffered.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let buffered = buffered.expect("steering message should be buffered");
        assert_eq!(buffered.id, "steer_test");
        assert_eq!(buffered.text, "tighten scroll");
    }

    #[test]
    fn interrupted_partial_response_persists_text_and_completed_thinking_only() {
        let context = AgentContext::default();
        let records = vec![
            AgentStreamRecord::new(
                0,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartStart(PartStart {
                        index: 0,
                        part_kind: "text".to_string(),
                    }),
                },
            ),
            AgentStreamRecord::new(
                1,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "partial")),
                },
            ),
            AgentStreamRecord::new(
                2,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartStart(PartStart {
                        index: 1,
                        part_kind: "thinking".to_string(),
                    }),
                },
            ),
            AgentStreamRecord::new(
                3,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(
                        1,
                        "done reasoning",
                    )),
                },
            ),
            AgentStreamRecord::new(
                4,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartEnd(PartEnd::with_kind(1, "thinking")),
                },
            ),
            AgentStreamRecord::new(
                5,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartStart(PartStart {
                        index: 2,
                        part_kind: "thinking".to_string(),
                    }),
                },
            ),
            AgentStreamRecord::new(
                6,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(
                        2,
                        "unfinished reasoning",
                    )),
                },
            ),
        ];

        let partial = interrupted_partial_response(&records, &context)
            .expect("partial response should be recovered");
        assert_eq!(partial.text_output(), "partial");
        assert_eq!(partial.parts.len(), 2);
        assert_eq!(
            partial.metadata["starweaver.interrupted.partial"],
            serde_json::Value::Bool(true)
        );
        assert!(matches!(
            &partial.parts[1],
            ModelResponsePart::Thinking { text, .. } if text == "done reasoning"
        ));
        assert!(!serde_json::to_string(&partial)
            .unwrap()
            .contains("unfinished reasoning"));
    }

    #[test]
    fn cancelled_projection_preserves_partial_stream_and_terminal_status() {
        let run = RunRecord::new(
            SessionId::from_string("session_cancel"),
            RunId::from_string("run_cancel"),
            ConversationId::from_string("conversation_cancel"),
        );
        let records = vec![AgentStreamRecord::new(
            0,
            AgentStreamEvent::RunStart {
                run_id: RunId::from_string("runtime_run"),
                conversation_id: ConversationId::from_string("conversation_cancel"),
            },
        )];
        let runtime = tokio::runtime::Runtime::new().expect("runtime should start");
        let projection = cancelled_display_projection(
            &run,
            &records,
            CliRunPolicy {
                hitl: HitlPolicy::Deny,
            },
            &runtime,
        );

        assert_eq!(projection.status, RunStatus::Cancelled);
        assert!(projection
            .messages
            .iter()
            .any(|message| message.kind == DisplayMessageKind::RunStarted));
        assert!(projection
            .messages
            .iter()
            .any(|message| message.kind == DisplayMessageKind::RunCancelled));
        assert_eq!(
            projection
                .messages
                .iter()
                .filter(|message| message.kind == DisplayMessageKind::CompactionCompleted)
                .count(),
            1
        );
    }
}
