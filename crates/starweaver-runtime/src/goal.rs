//! Runtime goal-mode output validator capability.

use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use starweaver_context::{AgentContext, AgentEvent};

use crate::{
    capability::{AgentCapability, CapabilityError, CapabilityResult, CapabilitySpec},
    run::AgentRunState,
};

/// Stable capability id for runtime goal mode.
pub const GOAL_CAPABILITY_ID: &str = "starweaver.goal";
/// Marker the model must place on its own line when the objective is complete.
pub const GOAL_COMPLETE_MARKER: &str = "[GOAL_COMPLETE]";
/// Context event emitted before another goal retry.
pub const GOAL_ITERATION_EVENT_KIND: &str = "goal_iteration";
/// Context event emitted when goal mode stops.
pub const GOAL_COMPLETE_EVENT_KIND: &str = "goal_complete";

/// Goal-mode runtime options.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoalRunOptions {
    objective: String,
    max_iterations: usize,
}

impl GoalRunOptions {
    /// Create goal-mode options.
    #[must_use]
    pub fn new(objective: impl Into<String>, max_iterations: usize) -> Self {
        Self {
            objective: objective.into(),
            max_iterations: max_iterations.max(1),
        }
    }

    /// Return the objective text.
    #[must_use]
    pub fn objective(&self) -> &str {
        &self.objective
    }

    /// Return the maximum retry iterations.
    #[must_use]
    pub const fn max_iterations(&self) -> usize {
        self.max_iterations
    }
}

/// Reason goal mode stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalCompleteReason {
    /// The model emitted the completion marker and no post-handoff audit is pending.
    Verified,
    /// The retry iteration budget was exhausted.
    MaxIterations,
    /// The host cancelled the run.
    Cancelled,
    /// The run failed while goal mode was active.
    Error,
    /// The host stopped goal mode without a verified marker.
    UnverifiedStop,
}

impl GoalCompleteReason {
    /// Stable snake-case reason string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::MaxIterations => "max_iterations",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
            Self::UnverifiedStop => "unverified_stop",
        }
    }
}

/// Runtime capability that keeps `/goal` inside the agent output retry loop.
#[derive(Debug)]
pub struct GoalCapability {
    options: GoalRunOptions,
    state: Mutex<GoalRuntimeState>,
}

impl GoalCapability {
    /// Create a goal-mode capability.
    #[must_use]
    pub fn new(options: GoalRunOptions) -> Self {
        Self {
            options,
            state: Mutex::new(GoalRuntimeState::default()),
        }
    }

    fn continue_or_stop_action(
        &self,
        state: &mut GoalRuntimeState,
        prompt: String,
    ) -> GoalValidationAction {
        let next_iteration = state.iteration.saturating_add(1);
        if next_iteration > self.options.max_iterations {
            state.completed = true;
            return GoalValidationAction::Complete {
                iteration: state.iteration,
                reason: GoalCompleteReason::MaxIterations,
            };
        }

        state.iteration = next_iteration;
        GoalValidationAction::Retry {
            iteration: state.iteration,
            prompt,
        }
    }

    fn sync_context_handoff_state(context: &AgentContext, state: &mut GoalRuntimeState) {
        let events = context.events.events();
        let start = state.observed_event_cursor.min(events.len());
        for event in &events[start..] {
            if let Some(source) = context_handoff_source(&event.kind) {
                state.needs_post_restore_audit = true;
                state.last_context_handoff_source = Some(source.to_string());
            }
        }
        state.observed_event_cursor = events.len();
    }
}

#[async_trait]
impl AgentCapability for GoalCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(GOAL_CAPABILITY_ID)
    }

    async fn on_run_start_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        {
            let mut state = self.state.lock().map_err(|_| {
                CapabilityError::Failed("goal capability state lock poisoned".to_string())
            })?;
            *state = GoalRuntimeState {
                observed_event_cursor: context.events.len(),
                ..GoalRuntimeState::default()
            };
        }
        Ok(())
    }

    async fn validate_output_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> CapabilityResult<()> {
        let action = {
            let mut state = self.state.lock().map_err(|_| {
                CapabilityError::Failed("goal capability state lock poisoned".to_string())
            })?;
            let action = if state.completed {
                GoalValidationAction::Done
            } else {
                Self::sync_context_handoff_state(context, &mut state);

                if has_completion_marker(output) {
                    if state.needs_post_restore_audit {
                        state.needs_post_restore_audit = false;
                        let prompt = build_post_restore_goal_audit_prompt(
                            self.options.objective(),
                            state
                                .last_context_handoff_source
                                .as_deref()
                                .unwrap_or("context"),
                        );
                        self.continue_or_stop_action(&mut state, prompt)
                    } else {
                        state.completed = true;
                        GoalValidationAction::Complete {
                            iteration: state.iteration,
                            reason: GoalCompleteReason::Verified,
                        }
                    }
                } else {
                    let next_iteration = state.iteration.saturating_add(1);
                    self.continue_or_stop_action(
                        &mut state,
                        build_goal_check_prompt(
                            self.options.objective(),
                            next_iteration,
                            self.options.max_iterations(),
                        ),
                    )
                }
            };
            drop(state);
            action
        };

        match action {
            GoalValidationAction::Done => Ok(()),
            GoalValidationAction::Retry { iteration, prompt } => {
                publish_goal_iteration(context, &self.options, iteration);
                Err(CapabilityError::ModelRetry(prompt))
            }
            GoalValidationAction::Complete { iteration, reason } => {
                publish_goal_complete(context, &self.options, iteration, reason);
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
enum GoalValidationAction {
    Done,
    Retry {
        iteration: usize,
        prompt: String,
    },
    Complete {
        iteration: usize,
        reason: GoalCompleteReason,
    },
}

#[derive(Debug, Default)]
struct GoalRuntimeState {
    iteration: usize,
    completed: bool,
    needs_post_restore_audit: bool,
    last_context_handoff_source: Option<String>,
    observed_event_cursor: usize,
}

fn publish_goal_iteration(context: &mut AgentContext, options: &GoalRunOptions, iteration: usize) {
    context.publish_event(AgentEvent::new(
        GOAL_ITERATION_EVENT_KIND,
        json!({
            "iteration": iteration,
            "max_iterations": options.max_iterations(),
            "task": options.objective(),
        }),
    ));
}

fn publish_goal_complete(
    context: &mut AgentContext,
    options: &GoalRunOptions,
    iteration: usize,
    reason: GoalCompleteReason,
) {
    context.publish_event(AgentEvent::new(
        GOAL_COMPLETE_EVENT_KIND,
        json!({
            "iteration": iteration,
            "max_iterations": options.max_iterations(),
            "reason": reason.as_str(),
            "task": options.objective(),
        }),
    ));
}

/// Return whether output includes the completion marker on its own line.
#[must_use]
pub fn has_completion_marker(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim() == GOAL_COMPLETE_MARKER)
}

/// Build the standard continuation prompt for goal-mode retry.
#[must_use]
pub fn build_goal_check_prompt(objective: &str, iteration: usize, max_iterations: usize) -> String {
    format!(
        "Continue working toward the active goal.\n\n<objective>\n{}\n</objective>\n\n<goal-check>\nCurrent iteration: {}/{}.\nIf the goal is fully complete, include {} on its own line.\nOtherwise, make concrete progress and continue.\n</goal-check>",
        escape_xml_text(objective),
        iteration,
        max_iterations.max(1),
        GOAL_COMPLETE_MARKER,
    )
}

/// Build the verification prompt used after context handoff or compaction.
#[must_use]
pub fn build_post_restore_goal_audit_prompt(objective: &str, source: &str) -> String {
    format!(
        "The previous response claimed the active goal was complete after a {} handoff. Re-audit the restored context before stopping.\n\n<objective>\n{}\n</objective>\n\n<goal-check>\nUse fresh evidence from the current context. If the goal is still fully complete, include {} on its own line. Otherwise, continue with the next concrete step.\n</goal-check>",
        escape_xml_text(source),
        escape_xml_text(objective),
        GOAL_COMPLETE_MARKER,
    )
}

fn context_handoff_source(kind: &str) -> Option<&'static str> {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    if matches!(
        normalized.as_str(),
        "compact_complete" | "compact_completed" | "compaction_complete" | "compaction_completed"
    ) || normalized.ends_with("_compact_complete")
        || normalized.ends_with("_compact_completed")
        || normalized.ends_with("_compaction_complete")
        || normalized.ends_with("_compaction_completed")
    {
        return Some("context compaction");
    }
    if matches!(
        normalized.as_str(),
        "handoff_complete" | "handoff_completed" | "summary_complete" | "summary_completed"
    ) || normalized.ends_with("_handoff_complete")
        || normalized.ends_with("_handoff_completed")
        || normalized.ends_with("_summary_complete")
        || normalized.ends_with("_summary_completed")
    {
        return Some("summary");
    }
    None
}

fn escape_xml_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use starweaver_core::{AgentId, ConversationId, RunId};
    use starweaver_model::{
        FunctionModel, FunctionModelInfo, ModelMessage, ModelResponse, ModelSettings,
    };

    use super::*;
    use crate::{Agent, OutputPolicy, stream::AgentStreamEvent};

    #[test]
    fn marker_must_be_on_its_own_line() {
        assert!(has_completion_marker("done\n[GOAL_COMPLETE]\n"));
        assert!(has_completion_marker("  [GOAL_COMPLETE]  "));
        assert!(!has_completion_marker("done [GOAL_COMPLETE]"));
        assert!(!has_completion_marker("[GOAL_COMPLETE] but more text"));
    }

    #[test]
    fn continuation_prompt_escapes_objective() {
        let prompt = build_goal_check_prompt("ship <a&b>", 1, 3);
        assert!(prompt.contains("ship &lt;a&amp;b&gt;"));
        assert!(prompt.contains("Current iteration: 1/3."));
    }

    #[tokio::test]
    async fn goal_retries_until_marker_or_budget() {
        let capability = GoalCapability::new(GoalRunOptions::new("finish task", 1));
        let mut run_state = AgentRunState::new(RunId::new(), ConversationId::new());
        let mut context = AgentContext::new(AgentId::default());

        capability
            .on_run_start_with_context(&mut run_state, &mut context)
            .await
            .unwrap_or_else(|error| panic!("goal start failed: {error}"));
        let retry = capability
            .validate_output_with_context(&mut run_state, &mut context, "not done")
            .await;
        let Err(retry) = retry else {
            panic!("goal validation should retry without marker");
        };
        assert!(matches!(retry, CapabilityError::ModelRetry(_)));
        assert_eq!(context.events.events()[0].kind, GOAL_ITERATION_EVENT_KIND);

        capability
            .validate_output_with_context(&mut run_state, &mut context, "still not done")
            .await
            .unwrap_or_else(|error| panic!("goal budget completion failed: {error}"));
        let Some(complete) = context.events.events().last() else {
            panic!("goal completion event should be published");
        };
        assert_eq!(complete.kind, GOAL_COMPLETE_EVENT_KIND);
        assert_eq!(complete.payload["reason"], "max_iterations");
    }

    #[tokio::test]
    async fn completion_after_handoff_requires_audit_retry() {
        let capability = GoalCapability::new(GoalRunOptions::new("finish task", 2));
        let mut run_state = AgentRunState::new(RunId::new(), ConversationId::new());
        let mut context = AgentContext::new(AgentId::default());

        capability
            .on_run_start_with_context(&mut run_state, &mut context)
            .await
            .unwrap_or_else(|error| panic!("goal start failed: {error}"));
        context.publish_event(AgentEvent::new("compact_complete", json!({})));
        let retry = capability
            .validate_output_with_context(&mut run_state, &mut context, "done\n[GOAL_COMPLETE]")
            .await;
        let Err(retry) = retry else {
            panic!("goal validation should retry after context handoff");
        };
        assert!(matches!(retry, CapabilityError::ModelRetry(_)));
        assert!(matches!(
            context.events.events().last(),
            Some(event) if event.kind == GOAL_ITERATION_EVENT_KIND
        ));
    }

    #[tokio::test]
    async fn goal_capability_runs_inside_output_retry_loop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let model_calls = Arc::clone(&calls);
        let model = FunctionModel::new(
            move |_messages: Vec<ModelMessage>,
                  _settings: Option<ModelSettings>,
                  _info: FunctionModelInfo| {
                let call = model_calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    Ok(ModelResponse::text("not done yet"))
                } else {
                    Ok(ModelResponse::text("done\n[GOAL_COMPLETE]"))
                }
            },
        );
        let agent = Agent::new(Arc::new(model))
            .with_output_policy(OutputPolicy::new().with_retries(7))
            .with_capability(Arc::new(GoalCapability::new(GoalRunOptions::new(
                "finish task",
                2,
            ))));

        let mut events = Vec::new();
        let result = Box::pin(agent.run_with_stream_events("start", &mut events))
            .await
            .unwrap_or_else(|error| panic!("goal run should succeed: {error}"));

        assert_eq!(result.output, "done\n[GOAL_COMPLETE]");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(events.iter().any(|record| matches!(
            record.event,
            AgentStreamEvent::OutputRetry { retries: 1, .. }
        )));
        assert!(events.iter().any(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::Custom { event } if event.kind == GOAL_ITERATION_EVENT_KIND
            )
        }));
        assert!(events.iter().any(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::Custom { event }
                    if event.kind == GOAL_COMPLETE_EVENT_KIND
                        && event.payload["reason"] == "verified"
            )
        }));
    }
}
