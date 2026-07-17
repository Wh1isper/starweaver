use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    env,
    io::IsTerminal as _,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use starweaver_rpc_core::{
    EnvironmentAttachmentAccessMode, EnvironmentAttachmentRef, LOCAL_ENVIRONMENT_ATTACHMENT_ID,
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND,
};
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus, RunStatus,
};

use super::CliService;
use crate::{
    CliError, CliResult,
    args::{GoalCommandOptions, OutputMode, RunCommand, TuiCommand, TuiRenderMode},
    client_state,
    config::{CliConfig, clear_current_session, write_current_session},
    environment::resolve_environment_for_session_with_attachments,
    local_store::{HITL_RESUME_PREFLIGHT_SOURCE_RUN_ID_METADATA_KEY, SessionSummary},
    profiles::{ProfileSummary, list_config_model_profiles, list_profiles},
    prompt_input::PromptInput,
    runner::CliSteeringMessage,
    runtime_coordinator::{CliRuntimeCoordinator, RunStatusItem, RunStreamEvent},
};

#[derive(Clone, Debug)]
struct PromptRunOutcome {
    session_id: String,
    run_id: String,
    status: String,
    error: Option<String>,
}

struct ActiveTuiRun {
    receiver: mpsc::Receiver<TuiRunMessage>,
    coordinator: CliRuntimeCoordinator,
    run_id: String,
    hitl_resume: bool,
    cancelling: bool,
}

struct ActiveTuiShell {
    run: crate::tui::TuiShellRun,
    cancelling: bool,
}

struct QueuedTuiPrompt {
    prompt: PromptInput,
    display_prompt: String,
    goal: Option<GoalCommandOptions>,
    context_generation: u64,
}

struct PendingTuiHitl {
    session_id: String,
    source_run_id: String,
    approvals: VecDeque<ApprovalRecord>,
    unresolved_deferred: usize,
}

enum TuiWaitingContinuation {
    Ready { source_run_id: String },
    Blocked(PendingTuiHitl),
    Busy { active_run_id: String },
}

struct TuiRuntimeShutdownGuard {
    coordinator: Option<CliRuntimeCoordinator>,
}

impl TuiRuntimeShutdownGuard {
    fn shutdown(&mut self) -> CliResult<()> {
        let Some(coordinator) = self.coordinator.take() else {
            return Ok(());
        };
        coordinator.shutdown(TUI_BACKGROUND_SHUTDOWN_TIMEOUT)
    }
}

impl Drop for TuiRuntimeShutdownGuard {
    fn drop(&mut self) {
        if let Some(coordinator) = self.coordinator.as_ref() {
            let _ = coordinator.shutdown(TUI_BACKGROUND_SHUTDOWN_TIMEOUT);
        }
    }
}

enum TuiRunMessage {
    Stream(Box<AgentStreamRecord>),
    Completed(PromptRunOutcome),
    Waiting(PromptRunOutcome),
    Failed(String),
}

const TUI_FRAME_INTERVAL: Duration = Duration::from_millis(33);
const TUI_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(33);
const TUI_MIN_POLL_INTERVAL: Duration = Duration::from_millis(1);
const TUI_BACKGROUND_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const TUI_SHELL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const TUI_STREAM_DRAIN_BUDGET: usize = 256;
const TUI_STREAM_DRAIN_TIME_BUDGET: Duration = Duration::from_millis(4);

impl CliService {
    pub(super) fn tui(&mut self, command: &TuiCommand) -> CliResult<String> {
        if command.interactive
            && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal())
        {
            return Err(CliError::Run(
                "interactive TUI requires stdin and stdout to be TTYs; use --snapshot for redirected output"
                    .to_string(),
            ));
        }
        if should_run_interactive_tui(command) {
            tui_environment_attachments(&self.config)?;
            self.interactive_tui(command)?;
            return Ok(String::new());
        }
        self.tui_snapshot(command)
    }

    fn tui_snapshot(&mut self, command: &TuiCommand) -> CliResult<String> {
        let Some(snapshot) = self.tui_snapshot_state(command)? else {
            return Ok(tui_empty_state(&self.config));
        };
        match command.output {
            OutputMode::Text => Ok(snapshot.render_text()),
            OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => {
                Ok(format!("{}\n", serde_json::to_string(&snapshot)?))
            }
            OutputMode::Silent => Ok(format!(
                "session_id={}\nmessages={}\napprovals={}\ndeferred={}\nstatus=tui\n",
                snapshot.session_id,
                snapshot.messages,
                snapshot.pending_approvals,
                snapshot.pending_deferred
            )),
        }
    }

    fn tui_snapshot_state(
        &mut self,
        command: &TuiCommand,
    ) -> CliResult<Option<crate::tui::TuiSnapshot>> {
        let Some(requested_session) = command.session.as_deref() else {
            return Ok(None);
        };
        let session_id = self.resolve_session_id(Some(requested_session))?;
        self.tui_snapshot_for_session(&session_id, command.run.as_deref(), command.after)
            .map(Some)
    }

    fn tui_snapshot_for_session(
        &mut self,
        session_id: &str,
        run_id: Option<&str>,
        after: Option<usize>,
    ) -> CliResult<crate::tui::TuiSnapshot> {
        let run_parts = {
            let store = self.store()?;
            if let Some(run_id) = run_id {
                let run = store.load_run(session_id, run_id)?;
                let messages = store.replay_display(session_id, Some(run_id), after)?;
                vec![(run_prompt_text(&run), messages)]
            } else {
                let runs = store.list_run_records(session_id)?;
                let mut global_sequence = 0_usize;
                let mut parts = Vec::with_capacity(runs.len());
                for run in runs {
                    let messages = store
                        .replay_display(session_id, Some(run.run_id.as_str()), None)?
                        .into_iter()
                        .filter(|_| {
                            let include = after.is_none_or(|after| global_sequence > after);
                            global_sequence = global_sequence.saturating_add(1);
                            include
                        })
                        .collect::<Vec<_>>();
                    if after.is_none() || !messages.is_empty() {
                        parts.push((run_prompt_text(&run), messages));
                    }
                }
                parts
            }
        };
        let approvals = self.store()?.list_approvals(Some(session_id), run_id)?;
        let deferred = self
            .store()?
            .list_deferred_tools(Some(session_id), run_id)?;
        Ok(crate::tui::TuiSnapshot::from_run_parts(
            session_id.to_string(),
            run_parts,
            &approvals,
            &deferred,
        ))
    }

    pub(super) fn reload_tui_session(
        &mut self,
        state: &mut crate::tui::InteractiveTuiState,
        session_id_or_prefix: &str,
    ) -> CliResult<()> {
        self.restore_tui_session(state, session_id_or_prefix, None, None, true)
    }

    pub(super) fn restore_tui_session(
        &mut self,
        state: &mut crate::tui::InteractiveTuiState,
        session_id_or_prefix: &str,
        run_id: Option<&str>,
        after: Option<usize>,
        announce: bool,
    ) -> CliResult<()> {
        let session_id = self.store()?.resolve_session_prefix(session_id_or_prefix)?;
        let session = self.store()?.load_session(&session_id)?;
        let snapshot = self.tui_snapshot_for_session(&session_id, run_id, after)?;
        state.set_snapshot(&snapshot);
        apply_tui_session_profile(&self.config, state, session.profile.as_deref());
        write_current_session(&self.config, &session_id)?;
        state.set_session_choices(self.tui_session_choices(50)?);
        if announce {
            state.push_transcript_notice(format!(
                "[SYS] Loaded session {session_id}. Next message will continue from loaded history."
            ));
        }
        Ok(())
    }

    fn pending_tui_hitl(&mut self, session_id: &str, run_id: &str) -> CliResult<PendingTuiHitl> {
        let approvals = self
            .store()?
            .list_approvals(Some(session_id), Some(run_id))?;
        let deferred = self
            .store()?
            .list_deferred_tools(Some(session_id), Some(run_id))?;
        Ok(pending_tui_hitl_from_records(
            session_id, run_id, approvals, deferred,
        ))
    }

    fn tui_waiting_continuation(
        &mut self,
        session_id: &str,
    ) -> CliResult<Option<TuiWaitingContinuation>> {
        let session = self.store()?.load_session(session_id)?;
        let runs = self.store()?.list_run_records(session_id)?;
        let source_run_id = if let Some(active_run_id) = session.active_run_id {
            active_run_id.as_str().to_string()
        } else if let Some(active) = runs
            .iter()
            .rev()
            .find(|run| run.status.is_active() && run.status != RunStatus::Waiting)
        {
            return Ok(Some(TuiWaitingContinuation::Busy {
                active_run_id: active.run_id.as_str().to_string(),
            }));
        } else {
            let Some(head_run_id) = session.head_run_id else {
                return Ok(None);
            };
            let Some(head) = runs.iter().find(|run| run.run_id == head_run_id) else {
                return Ok(None);
            };
            if !matches!(head.status, RunStatus::Failed | RunStatus::Cancelled) {
                return Ok(None);
            }
            let Some(source_run_id) = head.restore_from_run_id.as_ref() else {
                return Ok(None);
            };
            if head
                .metadata
                .get(HITL_RESUME_PREFLIGHT_SOURCE_RUN_ID_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                != Some(source_run_id.as_str())
            {
                return Ok(None);
            }
            let Some(source) = runs.iter().find(|run| run.run_id == *source_run_id) else {
                return Ok(None);
            };
            if source.status != RunStatus::Waiting {
                return Ok(None);
            }
            source_run_id.as_str().to_string()
        };
        let source = self.store()?.load_run(session_id, &source_run_id)?;
        if source.status != RunStatus::Waiting {
            return Ok(source
                .status
                .is_active()
                .then_some(TuiWaitingContinuation::Busy {
                    active_run_id: source_run_id,
                }));
        }
        let pending = self.pending_tui_hitl(session_id, &source_run_id)?;
        if pending.approvals.is_empty() && pending.unresolved_deferred == 0 {
            Ok(Some(TuiWaitingContinuation::Ready { source_run_id }))
        } else {
            Ok(Some(TuiWaitingContinuation::Blocked(pending)))
        }
    }

    fn open_tui_session_picker(
        &mut self,
        state: &mut crate::tui::InteractiveTuiState,
    ) -> CliResult<()> {
        state.set_session_choices(self.tui_session_choices(50)?);
        state.open_session_picker();
        Ok(())
    }

    pub(super) fn tui_session_choices(
        &mut self,
        limit: usize,
    ) -> CliResult<Vec<crate::tui::SessionChoice>> {
        self.store()?
            .list_sessions(limit)
            .map(session_choices_from_summaries)
    }

    #[allow(clippy::too_many_lines)]
    fn interactive_tui(&mut self, command: &TuiCommand) -> CliResult<()> {
        let mut state = crate::tui::InteractiveTuiState::welcome(&self.config.tui_state_dir);
        state.set_goal_max_iterations(self.config.max_goal_iterations);
        let initial_render_mode = command
            .render_mode
            .or(read_tui_render_mode(&self.config)?)
            .unwrap_or(self.config.tui_render_mode);
        state.set_render_mode(initial_render_mode);
        state.set_custom_commands(self.config.slash_commands.clone());
        state.set_model_choices(model_choices(&self.config));
        state.set_session_choices(self.tui_session_choices(50)?);
        let choices = state.model_choices().to_vec();
        let selected_profile = read_tui_selected_profile(&self.config)?
            .filter(|profile| {
                state
                    .model_choices()
                    .iter()
                    .any(|choice| choice.profile == *profile)
            })
            .or_else(|| selectable_profile(&choices, &self.config.default_profile))
            .or_else(|| selectable_profile(&choices, "general"))
            .or_else(|| choices.first().map(|choice| choice.profile.clone()))
            .unwrap_or_else(|| self.config.default_profile.clone());
        let selected_choice = choices
            .iter()
            .find(|choice| choice.profile == selected_profile);
        state.set_profile(
            selected_profile.clone(),
            selected_choice.map_or_else(|| selected_profile.clone(), model_choice_label),
        );
        state.set_context_window(selected_choice.and_then(|choice| choice.context_window));
        if let Some(session_id_or_prefix) = command.session.as_deref() {
            self.restore_tui_session(
                &mut state,
                session_id_or_prefix,
                command.run.as_deref(),
                command.after,
                false,
            )?;
        }
        let mut restored_hitl = None;
        if let Some(session_id) = state.session_id.clone() {
            match self.tui_waiting_continuation(&session_id) {
                Ok(Some(TuiWaitingContinuation::Blocked(hitl))) => {
                    project_pending_tui_hitl(&mut state, &hitl);
                    restored_hitl = Some(hitl);
                }
                Ok(Some(TuiWaitingContinuation::Ready { .. })) => {
                    state.require_hitl_reload(session_id);
                    state.push_transcript_notice(
                        "[SYS] Waiting run inputs are resolved. Press Esc to claim and continue it."
                            .to_string(),
                    );
                }
                Ok(Some(TuiWaitingContinuation::Busy { active_run_id })) => {
                    state.require_hitl_reload(session_id);
                    state.push_transcript_notice(format!(
                        "[SYS] Run {active_run_id} is active in another client. Press Esc to refresh."
                    ));
                }
                Ok(None) if state.status == "WAITING" => {
                    state.require_hitl_reload(session_id);
                    state.push_transcript_notice(
                        "[SYS] This historical run is waiting but is not the session's active continuation source."
                            .to_string(),
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    state.require_hitl_reload(session_id);
                    state.push_transcript_notice(format!(
                        "[SYS] Could not load pending HITL records: {error}. Press Esc to retry."
                    ));
                }
            }
        }
        let mut coordinator_config = self.config.clone();
        coordinator_config.oauth_refresh.enabled = false;
        let coordinator = CliRuntimeCoordinator::new(coordinator_config)?;
        let mut shutdown_guard = TuiRuntimeShutdownGuard {
            coordinator: Some(coordinator.clone()),
        };
        // Acquire the terminal last so it is restored before runtime cleanup on unwind.
        let mut tui = crate::tui::InteractiveTui::enter()?;
        let mut active_run: Option<ActiveTuiRun> = None;
        let mut active_shell: Option<ActiveTuiShell> = None;
        let mut pending_hitl = restored_hitl;
        let mut queued_prompt: Option<QueuedTuiPrompt> = None;
        let mut context_generation = 0_u64;
        let mut pending_background_wakes = HashMap::<String, VecDeque<String>>::new();
        let mut suppressed_background_sessions = BTreeSet::<String>::new();
        let mut persisted_profile = state.profile.clone();
        let mut persisted_render_mode = initial_render_mode;
        let mut dirty = true;
        let now = Instant::now();
        let mut last_render = now.checked_sub(TUI_FRAME_INTERVAL).unwrap_or(now);
        loop {
            if let Some(session_id) = state.session_id.as_deref() {
                for completion in coordinator.take_background_completions(session_id) {
                    let attempts = pending_background_wakes
                        .entry(completion.session_id)
                        .or_default();
                    if !attempts.contains(&completion.attempt_id) {
                        attempts.push_back(completion.attempt_id);
                    }
                }
            }
            let drain_started = Instant::now();
            let mut drained_records = 0_usize;
            state.begin_projection_batch();
            while let Some(run) = active_run.as_mut() {
                if drained_records >= TUI_STREAM_DRAIN_BUDGET
                    || drain_started.elapsed() >= TUI_STREAM_DRAIN_TIME_BUDGET
                {
                    break;
                }
                drained_records = drained_records.saturating_add(1);
                match run.receiver.try_recv() {
                    Ok(TuiRunMessage::Stream(record)) => {
                        state.apply_stream_record(&record);
                        dirty = true;
                    }
                    Ok(TuiRunMessage::Waiting(waiting)) => {
                        state.wait_run(Some(waiting.session_id.clone()));
                        pending_hitl = None;
                        match self.pending_tui_hitl(&waiting.session_id, &waiting.run_id) {
                            Ok(hitl) => {
                                project_pending_tui_hitl(&mut state, &hitl);
                                pending_hitl = Some(hitl);
                            }
                            Err(error) => {
                                state.clear_pending_hitl();
                                state.require_hitl_reload(waiting.session_id.clone());
                                state.push_transcript_notice(format!(
                                    "[SYS] Could not load pending HITL records: {error}. Press Esc to retry the session reload."
                                ));
                            }
                        }
                        let waiting_counts = self
                            .tui_snapshot_for_session(
                                &waiting.session_id,
                                Some(&waiting.run_id),
                                None,
                            )
                            .map(|snapshot| {
                                format!(
                                    " pending_approvals={} pending_deferred={}",
                                    snapshot.pending_approvals, snapshot.pending_deferred
                                )
                            })
                            .unwrap_or_default();
                        state.push_run_status_line(format!(
                            "Run waiting: {} status=waiting{}",
                            waiting.run_id, waiting_counts
                        ));
                        active_run = None;
                        dirty = true;
                        break;
                    }
                    Ok(TuiRunMessage::Completed(completed)) => {
                        pending_hitl = None;
                        let was_cancelled = completed.status == "cancelled";
                        let failed = completed.status == "failed";
                        if was_cancelled {
                            suppressed_background_sessions.insert(completed.session_id.clone());
                            state.session_id = Some(completed.session_id.clone());
                            state.cancel_run("cancelled by user");
                        } else if failed {
                            state.session_id = Some(completed.session_id.clone());
                            state.fail_run(&terminal_run_error_message(&completed));
                        } else {
                            state.finish_run(Some(completed.session_id.clone()));
                        }
                        state.push_run_status_line(terminal_run_status_line(&completed));
                        active_run = None;
                        dirty = true;
                        if was_cancelled || failed {
                            if let Some(queued) = queued_prompt.take()
                                && queued.context_generation == context_generation
                            {
                                let reason = if was_cancelled {
                                    "the active run was cancelled"
                                } else {
                                    "the active run failed"
                                };
                                restore_queued_tui_prompt(&mut state, queued, reason);
                            }
                        } else if let Some(queued) = queued_prompt.take()
                            && queued.context_generation == context_generation
                        {
                            if let Some(session_id) = state.session_id.as_ref() {
                                pending_background_wakes.remove(session_id);
                            }
                            state.begin_run(&queued.display_prompt);
                            active_run = Some(spawn_tui_run(
                                &self.config,
                                coordinator.clone(),
                                state.session_id.clone(),
                                Some(state.session_affinity_id.clone()),
                                queued.prompt,
                                Some(state.profile.clone()),
                                queued.goal,
                                None,
                                None,
                            ));
                        }
                        break;
                    }
                    Ok(TuiRunMessage::Failed(error)) => {
                        pending_hitl = None;
                        let hitl_resume = run.hitl_resume;
                        state.fail_run(&error);
                        if hitl_resume
                            && queued_prompt.is_some()
                            && let Some(session_id) = state.session_id.clone()
                        {
                            state.require_hitl_reload(session_id);
                            state.push_transcript_notice(
                                "[SYS] Waiting continuation did not start. The queued prompt was retained; press Esc to reconcile and retry."
                                    .to_string(),
                            );
                        } else if !hitl_resume
                            && let Some(queued) = queued_prompt.take()
                            && queued.context_generation == context_generation
                        {
                            restore_queued_tui_prompt(
                                &mut state,
                                queued,
                                "the next run did not start",
                            );
                        }
                        active_run = None;
                        dirty = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        state.fail_run("background run channel closed");
                        if let Some(queued) = queued_prompt.take()
                            && queued.context_generation == context_generation
                        {
                            restore_queued_tui_prompt(
                                &mut state,
                                queued,
                                "the run channel closed before delivery",
                            );
                        }
                        active_run = None;
                        dirty = true;
                        break;
                    }
                }
            }
            state.end_projection_batch();

            let mut shell_settled = false;
            if let Some(shell) = active_shell.as_ref() {
                for _ in 0..8 {
                    match shell.run.events.try_recv() {
                        Ok(crate::tui::TuiShellEvent::Started { process_id }) => {
                            state.mark_shell_started(&process_id);
                            dirty = true;
                        }
                        Ok(crate::tui::TuiShellEvent::Finished { snapshot, elapsed }) => {
                            state.finish_shell_command(&snapshot, elapsed);
                            shell_settled = true;
                            dirty = true;
                            break;
                        }
                        Ok(crate::tui::TuiShellEvent::Failed(error)) => {
                            state.fail_shell_command(&error);
                            shell_settled = true;
                            dirty = true;
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            state.fail_shell_command("shell worker channel closed");
                            shell_settled = true;
                            dirty = true;
                            break;
                        }
                    }
                }
            }
            if shell_settled {
                active_shell = None;
            }

            if active_run.is_none()
                && active_shell.is_none()
                && let Some((session_id, attempt_id)) = take_pending_background_wake(
                    state.session_id.as_deref(),
                    &mut pending_background_wakes,
                    &suppressed_background_sessions,
                    |session_id, attempt_id| {
                        coordinator.background_completion_is_undelivered(session_id, attempt_id)
                    },
                )
            {
                state.begin_background_continuation();
                active_run = Some(spawn_tui_run(
                    &self.config,
                    coordinator.clone(),
                    Some(session_id),
                    Some(state.session_affinity_id.clone()),
                    PromptInput::text(""),
                    Some(state.profile.clone()),
                    None,
                    None,
                    Some(attempt_id),
                ));
                dirty = true;
            }

            if dirty && last_render.elapsed() >= TUI_FRAME_INTERVAL {
                tui.render(&mut state)?;
                last_render = Instant::now();
                dirty = false;
            }
            let poll_timeout = if dirty {
                TUI_FRAME_INTERVAL
                    .saturating_sub(last_render.elapsed())
                    .max(TUI_MIN_POLL_INTERVAL)
            } else {
                TUI_IDLE_POLL_INTERVAL
            };
            let event = crate::tui::InteractiveTui::poll_event(&mut state, poll_timeout)?;
            match event {
                Some(crate::tui::InteractiveTuiEvent::Quit)
                    if active_run.is_none() && active_shell.is_none() =>
                {
                    tui.restore()?;
                    shutdown_guard.shutdown()?;
                    return Ok(());
                }
                Some(crate::tui::InteractiveTuiEvent::Cancel) => {
                    if active_shell.as_ref().is_some_and(|shell| shell.cancelling) {
                        tui.restore()?;
                        let cleanup = active_shell
                            .as_mut()
                            .map(|shell| shell.run.cancel_and_wait(TUI_SHELL_SHUTDOWN_TIMEOUT))
                            .transpose();
                        drop(active_shell.take());
                        let shutdown = shutdown_guard.shutdown();
                        if let Err(error) = cleanup {
                            return Err(CliError::Run(format!(
                                "shell cleanup did not complete before exit: {error}"
                            )));
                        }
                        return shutdown;
                    }
                    if let Some(shell) = active_shell.as_mut() {
                        match shell.run.cancel() {
                            Ok(()) => shell.cancelling = true,
                            Err(error) => state.push_transcript_notice(format!(
                                "[SYS] Unable to cancel shell process: {error}"
                            )),
                        }
                    } else if let Some(run) = active_run.as_mut() {
                        if run.cancelling {
                            // A second interrupt is the escape hatch when cooperative
                            // cancellation or durable cleanup cannot finish promptly.
                            tui.restore()?;
                            return shutdown_guard.shutdown();
                        }
                        match run.coordinator.cancel_run(&run.run_id) {
                            Ok(()) => run.cancelling = true,
                            Err(error) => state.push_transcript_notice(format!(
                                "[SYS] Unable to request cancellation: {error}"
                            )),
                        }
                    }
                    dirty = true;
                }
                Some(
                    crate::tui::InteractiveTuiEvent::Redraw | crate::tui::InteractiveTuiEvent::Quit,
                ) => {
                    dirty = true;
                }
                None => {}
                Some(crate::tui::InteractiveTuiEvent::Steer(steering)) => {
                    if let Some(run) = active_run.as_ref()
                        && !run.cancelling
                        && run
                            .coordinator
                            .steer_run(
                                &run.run_id,
                                CliSteeringMessage {
                                    id: steering.id,
                                    text: steering.text,
                                },
                            )
                            .is_err()
                    {
                        state.fail_run("background steering channel closed");
                        active_run = None;
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Session(requested)) => {
                    if active_run.is_some() || active_shell.is_some() {
                        state.push_transcript_notice(
                            "[SYS] Session selection is available after the current run finishes."
                                .to_string(),
                        );
                    } else if let Some(session_id) = requested {
                        let hitl_refresh = state
                            .hitl_reload_session_id()
                            .is_some_and(|reload_session_id| reload_session_id == session_id);
                        if let Err(error) = self.reload_tui_session(&mut state, &session_id) {
                            if hitl_refresh {
                                state.require_hitl_reload(session_id.clone());
                            }
                            state.push_transcript_notice(format!("[SYS] {error}"));
                        } else {
                            pending_hitl = None;
                            if !hitl_refresh {
                                queued_prompt = None;
                                context_generation = context_generation.saturating_add(1);
                            }
                            if let Some(session_id) = state.session_id.clone() {
                                match self.tui_waiting_continuation(&session_id) {
                                    Ok(Some(TuiWaitingContinuation::Blocked(hitl))) => {
                                        project_pending_tui_hitl(&mut state, &hitl);
                                        pending_hitl = Some(hitl);
                                    }
                                    Ok(Some(TuiWaitingContinuation::Ready { source_run_id })) => {
                                        state.clear_hitl_reload_required();
                                        state.begin_background_continuation();
                                        active_run = Some(spawn_tui_run(
                                            &self.config,
                                            coordinator.clone(),
                                            Some(session_id),
                                            Some(state.session_affinity_id.clone()),
                                            PromptInput::text("resume waiting run"),
                                            Some(state.profile.clone()),
                                            None,
                                            Some(source_run_id),
                                            None,
                                        ));
                                    }
                                    Ok(Some(TuiWaitingContinuation::Busy { active_run_id })) => {
                                        state.require_hitl_reload(session_id);
                                        state.push_transcript_notice(format!(
                                            "[SYS] Run {active_run_id} is still active. Press Esc to refresh after it finishes."
                                        ));
                                    }
                                    Ok(None) => {
                                        state.clear_hitl_reload_required();
                                        if hitl_refresh
                                            && let Some(queued) = queued_prompt.take()
                                            && queued.context_generation == context_generation
                                        {
                                            pending_background_wakes.remove(&session_id);
                                            state.begin_run(&queued.display_prompt);
                                            active_run = Some(spawn_tui_run(
                                                &self.config,
                                                coordinator.clone(),
                                                Some(session_id),
                                                Some(state.session_affinity_id.clone()),
                                                queued.prompt,
                                                Some(state.profile.clone()),
                                                queued.goal,
                                                None,
                                                None,
                                            ));
                                        }
                                    }
                                    Err(error) => {
                                        state.require_hitl_reload(session_id);
                                        state.push_transcript_notice(format!(
                                            "[SYS] Could not load pending HITL records: {error}. Press Esc to retry."
                                        ));
                                    }
                                }
                            }
                        }
                    } else if let Err(error) = self.open_tui_session_picker(&mut state) {
                        state.push_transcript_notice(format!("[SYS] {error}"));
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Clear) => {
                    if active_run.is_some() || active_shell.is_some() {
                        state.reject_context_clear("clear unavailable during run");
                        state.push_transcript_notice(
                            "[SYS] Clear is available after the current run finishes.".to_string(),
                        );
                    } else if let Err(error) =
                        commit_tui_context_clear(&mut state, || clear_current_session(&self.config))
                    {
                        state.reject_context_clear("clear failed");
                        state.push_transcript_notice(format!("[SYS] {error}"));
                    } else {
                        queued_prompt = None;
                        pending_hitl = None;
                        context_generation = context_generation.saturating_add(1);
                        // Background scopes and wake queues stay keyed by the detached durable
                        // session so they cannot wake this fresh context and remain reloadable.
                        match self.tui_session_choices(50) {
                            Ok(choices) => state.set_session_choices(choices),
                            Err(error) => state.push_transcript_notice(format!("[SYS] {error}")),
                        }
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::PasteImage) => {
                    match crate::clipboard::read_clipboard_image(state.pasted_image_count() + 1) {
                        Ok(result) => {
                            if let Some(image) = result.image {
                                let description = image.description();
                                state.attach_image(image);
                                state.push_transcript_notice(format!(
                                    "[SYS] Attached {description} from clipboard"
                                ));
                            } else {
                                state.push_transcript_notice(format!(
                                    "[SYS] {}",
                                    result.error.unwrap_or_else(|| {
                                        "No clipboard image available.".to_string()
                                    })
                                ));
                            }
                        }
                        Err(error) => state.push_transcript_notice(format!("[SYS] {error}")),
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::ApprovalDecision(decision)) => {
                    let identity = pending_hitl.as_ref().and_then(|hitl| {
                        hitl.approvals.front().map(|approval| {
                            (
                                hitl.session_id.clone(),
                                hitl.source_run_id.clone(),
                                approval.approval_id.clone(),
                            )
                        })
                    });
                    let Some((session_id, source_run_id, approval_id)) = identity else {
                        state.push_transcript_notice(
                            "[SYS] No durable pending approval is available.".to_string(),
                        );
                        dirty = true;
                        continue;
                    };
                    let status = match decision {
                        crate::tui::TuiApprovalDecision::Approve => ApprovalStatus::Approved,
                        crate::tui::TuiApprovalDecision::Reject => ApprovalStatus::Denied,
                    };
                    let decided = self
                        .store()
                        .and_then(|store| store.decide_approval(&approval_id, status, None));
                    match decided {
                        Ok(record) => {
                            state.push_transcript_notice(format!(
                                "[SYS] Approval {} resolved as {:?}.",
                                record.approval_id, record.status
                            ));
                            match self.pending_tui_hitl(&session_id, &source_run_id) {
                                Ok(refreshed) if !refreshed.approvals.is_empty() => {
                                    if let Some(approval) = refreshed.approvals.front() {
                                        state.bind_pending_approval(approval);
                                    }
                                    pending_hitl = Some(refreshed);
                                }
                                Ok(refreshed) if refreshed.unresolved_deferred > 0 => {
                                    state.clear_pending_hitl();
                                    state.push_transcript_notice(format!(
                                        "[SYS] Waiting for {} deferred tool result(s).",
                                        refreshed.unresolved_deferred
                                    ));
                                    pending_hitl = Some(refreshed);
                                }
                                Ok(_) => {
                                    state.clear_pending_hitl();
                                    state.begin_background_continuation();
                                    active_run = Some(spawn_tui_run(
                                        &self.config,
                                        coordinator.clone(),
                                        Some(session_id),
                                        Some(state.session_affinity_id.clone()),
                                        PromptInput::text("resume waiting run"),
                                        Some(state.profile.clone()),
                                        None,
                                        Some(source_run_id),
                                        None,
                                    ));
                                    pending_hitl = None;
                                }
                                Err(error) => {
                                    state.require_hitl_reload(session_id.clone());
                                    state.push_transcript_notice(format!(
                                        "[SYS] Approval was saved, but HITL refresh failed: {error}. Press Esc to retry."
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            match self.pending_tui_hitl(&session_id, &source_run_id) {
                                Ok(refreshed) if !refreshed.approvals.is_empty() => {
                                    project_pending_tui_hitl(&mut state, &refreshed);
                                    pending_hitl = Some(refreshed);
                                }
                                Ok(_) => {
                                    state.clear_pending_hitl();
                                    state.require_hitl_reload(session_id.clone());
                                    pending_hitl = None;
                                    state.push_transcript_notice(
                                        "[SYS] Approval state changed externally; reload the session to reconcile continuation state."
                                            .to_string(),
                                    );
                                }
                                Err(refresh_error) => {
                                    state.clear_pending_hitl();
                                    state.require_hitl_reload(session_id.clone());
                                    pending_hitl = None;
                                    state.push_transcript_notice(format!(
                                        "[SYS] Approval decision failed ({error}) and durable refresh failed ({refresh_error}). Reload the session to retry."
                                    ));
                                }
                            }
                            state.push_transcript_notice(format!(
                                "[SYS] Could not resolve approval {approval_id}: {error}"
                            ));
                        }
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Shell(command)) => {
                    if active_run.is_some() || active_shell.is_some() {
                        state.push_transcript_notice(
                            "[SYS] Shell commands are available when the current activity finishes."
                                .to_string(),
                        );
                    } else {
                        let provider = (|| {
                            let attachments = tui_environment_attachments(&self.config)?;
                            let namespace = state
                                .session_id
                                .as_deref()
                                .unwrap_or(&state.session_affinity_id);
                            resolve_environment_for_session_with_attachments(
                                &self.config,
                                namespace,
                                &attachments,
                            )?
                            .process_provider
                            .ok_or_else(|| {
                                CliError::Run(
                                    "the selected environment does not support background processes"
                                        .to_string(),
                                )
                            })
                        })();
                        match provider {
                            Ok(provider) => {
                                state.queue_shell_command(&command);
                                active_shell = Some(ActiveTuiShell {
                                    run: crate::tui::spawn_shell_run(provider, command),
                                    cancelling: false,
                                });
                            }
                            Err(error) => state.fail_shell_command(&error.to_string()),
                        }
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Submit(prompt)) => {
                    let display_prompt = state
                        .take_pending_submission_display_prompt()
                        .unwrap_or_else(|| prompt.display_text());
                    let goal =
                        state
                            .take_pending_goal_submission()
                            .map(|(objective, max_iterations)| GoalCommandOptions {
                                objective,
                                max_iterations,
                            });
                    if active_shell.is_some() {
                        state.push_transcript_notice(
                            "[SYS] Prompts are available after the shell command finishes."
                                .to_string(),
                        );
                        dirty = true;
                        continue;
                    }
                    if active_run.is_some() {
                        queued_prompt = Some(QueuedTuiPrompt {
                            prompt,
                            display_prompt,
                            goal,
                            context_generation,
                        });
                        dirty = true;
                        continue;
                    }
                    if let Some(session_id) = state.session_id.clone() {
                        suppressed_background_sessions.remove(&session_id);
                        pending_background_wakes.remove(&session_id);
                        match self.tui_waiting_continuation(&session_id) {
                            Ok(Some(TuiWaitingContinuation::Blocked(hitl))) => {
                                queued_prompt = Some(QueuedTuiPrompt {
                                    prompt,
                                    display_prompt,
                                    goal,
                                    context_generation,
                                });
                                project_pending_tui_hitl(&mut state, &hitl);
                                pending_hitl = Some(hitl);
                                state.push_transcript_notice(
                                    "[SYS] Prompt queued until the waiting run is resolved."
                                        .to_string(),
                                );
                                dirty = true;
                                continue;
                            }
                            Ok(Some(TuiWaitingContinuation::Ready { source_run_id })) => {
                                queued_prompt = Some(QueuedTuiPrompt {
                                    prompt,
                                    display_prompt,
                                    goal,
                                    context_generation,
                                });
                                pending_hitl = None;
                                state.clear_pending_hitl();
                                state.begin_background_continuation();
                                active_run = Some(spawn_tui_run(
                                    &self.config,
                                    coordinator.clone(),
                                    Some(session_id),
                                    Some(state.session_affinity_id.clone()),
                                    PromptInput::text("resume waiting run"),
                                    Some(state.profile.clone()),
                                    None,
                                    Some(source_run_id),
                                    None,
                                ));
                                dirty = true;
                                continue;
                            }
                            Ok(Some(TuiWaitingContinuation::Busy { active_run_id })) => {
                                queued_prompt = Some(QueuedTuiPrompt {
                                    prompt,
                                    display_prompt,
                                    goal,
                                    context_generation,
                                });
                                state.require_hitl_reload(session_id);
                                state.push_transcript_notice(format!(
                                    "[SYS] Run {active_run_id} is active in another client. Prompt queued; press Esc to refresh."
                                ));
                                dirty = true;
                                continue;
                            }
                            Ok(None) => {}
                            Err(error) => {
                                queued_prompt = Some(QueuedTuiPrompt {
                                    prompt,
                                    display_prompt,
                                    goal,
                                    context_generation,
                                });
                                state.require_hitl_reload(session_id);
                                state.push_transcript_notice(format!(
                                    "[SYS] Could not verify durable continuation state: {error}. Prompt queued; press Esc to retry."
                                ));
                                dirty = true;
                                continue;
                            }
                        }
                    }
                    pending_hitl = None;
                    queued_prompt = None;
                    state.clear_pending_hitl();
                    state.begin_run(&display_prompt);
                    active_run = Some(spawn_tui_run(
                        &self.config,
                        coordinator.clone(),
                        state.session_id.clone(),
                        Some(state.session_affinity_id.clone()),
                        prompt,
                        Some(state.profile.clone()),
                        goal,
                        None,
                        None,
                    ));
                    dirty = true;
                }
            }
            if state.profile != persisted_profile {
                if let Err(error) = write_tui_selected_profile(&self.config, &state.profile) {
                    state.push_transcript_notice(format!(
                        "[SYS] Could not persist the selected profile: {error}"
                    ));
                    dirty = true;
                }
                persisted_profile.clone_from(&state.profile);
            }
            if state.render_mode() != persisted_render_mode {
                if let Err(error) = write_tui_render_mode(&self.config, state.render_mode()) {
                    state.push_transcript_notice(format!(
                        "[SYS] Could not persist the display preference: {error}"
                    ));
                    dirty = true;
                }
                persisted_render_mode = state.render_mode();
            }
        }
    }
}

fn pending_tui_hitl_from_records(
    session_id: &str,
    run_id: &str,
    approvals: Vec<ApprovalRecord>,
    deferred: Vec<DeferredToolRecord>,
) -> PendingTuiHitl {
    let approvals = approvals
        .into_iter()
        .filter(|approval| approval.status == ApprovalStatus::Pending)
        .collect::<VecDeque<_>>();
    let unresolved_deferred = deferred
        .into_iter()
        .filter(|record| deferred_status_is_unresolved(record.status))
        .count();
    PendingTuiHitl {
        session_id: session_id.to_string(),
        source_run_id: run_id.to_string(),
        approvals,
        unresolved_deferred,
    }
}

#[cfg(test)]
fn latest_pending_tui_hitl_from_records(
    session_id: &str,
    run_ids: &[String],
    approvals: Vec<ApprovalRecord>,
    deferred: Vec<DeferredToolRecord>,
) -> Option<PendingTuiHitl> {
    let active_run_ids = approvals
        .iter()
        .filter(|approval| approval.status == ApprovalStatus::Pending)
        .map(|approval| approval.run_id.as_str().to_string())
        .chain(
            deferred
                .iter()
                .filter(|record| deferred_status_is_unresolved(record.status))
                .map(|record| record.run_id.as_str().to_string()),
        )
        .collect::<BTreeSet<_>>();
    let run_id = run_ids
        .iter()
        .rev()
        .find(|run_id| active_run_ids.contains(run_id.as_str()))?
        .clone();
    let approvals = approvals
        .into_iter()
        .filter(|approval| approval.run_id.as_str() == run_id)
        .collect();
    let deferred = deferred
        .into_iter()
        .filter(|record| record.run_id.as_str() == run_id)
        .collect();
    Some(pending_tui_hitl_from_records(
        session_id, &run_id, approvals, deferred,
    ))
}

const fn deferred_status_is_unresolved(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting
    )
}

fn project_pending_tui_hitl(state: &mut crate::tui::InteractiveTuiState, hitl: &PendingTuiHitl) {
    state.require_hitl_reload(hitl.session_id.clone());
    if let Some(approval) = hitl.approvals.front() {
        state.bind_pending_approval(approval);
    } else {
        state.clear_pending_hitl();
        if hitl.unresolved_deferred > 0 {
            state.push_transcript_notice(format!(
                "[SYS] Run is waiting for {} deferred tool result(s).",
                hitl.unresolved_deferred
            ));
        } else {
            state.push_transcript_notice(
                "[SYS] Run is waiting without an actionable approval or deferred tool record."
                    .to_string(),
            );
        }
    }
}

fn take_pending_background_wake(
    current_session_id: Option<&str>,
    pending: &mut HashMap<String, VecDeque<String>>,
    suppressed: &BTreeSet<String>,
    is_undelivered: impl FnOnce(&str, &str) -> bool,
) -> Option<(String, String)> {
    let session_id = current_session_id?;
    if suppressed.contains(session_id) {
        return None;
    }
    let attempt_id = pending.get_mut(session_id)?.pop_front()?;
    is_undelivered(session_id, &attempt_id).then(|| (session_id.to_string(), attempt_id))
}

fn restore_queued_tui_prompt(
    state: &mut crate::tui::InteractiveTuiState,
    queued: QueuedTuiPrompt,
    reason: &str,
) {
    state.restore_submission_prompt(queued.prompt, queued.goal);
    state.push_transcript_notice(format!(
        "[SYS] Queued prompt restored to the composer: {reason}."
    ));
}

fn commit_tui_context_clear(
    state: &mut crate::tui::InteractiveTuiState,
    persist: impl FnOnce() -> CliResult<()>,
) -> CliResult<()> {
    persist()?;
    state.clear_context_view();
    Ok(())
}

const fn build_tui_run_command(
    prompt: String,
    session_id: Option<String>,
    session_affinity_id: Option<String>,
    profile: Option<String>,
    goal: Option<GoalCommandOptions>,
    restore_from_run_id: Option<String>,
    environment_attachments: Vec<EnvironmentAttachmentRef>,
) -> RunCommand {
    let hitl_resume = restore_from_run_id.is_some();
    RunCommand {
        prompt: Some(prompt),
        prompt_parts: Vec::new(),
        // `TuiCommand::session` is startup-only. The live state is the sole continuation
        // source so `/clear` cannot fall back to the session used to launch the TUI.
        session: session_id,
        continue_session: false,
        new_session: false,
        run: restore_from_run_id,
        branch_from: None,
        profile,
        output: Some(OutputMode::Text),
        hitl: None,
        goal,
        worker: None,
        worker_label: None,
        worktree: None,
        worktree_name: None,
        branch: None,
        session_affinity_id,
        environment_attachments,
        hitl_resume,
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_tui_run(
    config: &CliConfig,
    coordinator: CliRuntimeCoordinator,
    session_id: Option<String>,
    session_affinity_id: Option<String>,
    prompt_input: PromptInput,
    profile: Option<String>,
    goal: Option<GoalCommandOptions>,
    restore_from_run_id: Option<String>,
    background_attempt_id: Option<String>,
) -> ActiveTuiRun {
    let hitl_resume = restore_from_run_id.is_some();
    let (ui_sender, receiver) = mpsc::sync_channel::<TuiRunMessage>(256);
    let environment_attachments = match tui_environment_attachments(config) {
        Ok(attachments) => attachments,
        Err(error) => {
            let _ = ui_sender.send(TuiRunMessage::Failed(error.to_string()));
            return ActiveTuiRun {
                receiver,
                coordinator,
                run_id: String::new(),
                hitl_resume,
                cancelling: false,
            };
        }
    };
    let run_command = build_tui_run_command(
        prompt_input.text.clone(),
        session_id,
        session_affinity_id,
        profile,
        goal,
        restore_from_run_id,
        environment_attachments,
    );
    let started = match coordinator.start_run_with_raw(
        run_command,
        Some(prompt_input),
        background_attempt_id,
    ) {
        Ok(started) => started,
        Err(error) => {
            let _ = ui_sender.send(TuiRunMessage::Failed(error.to_string()));
            return ActiveTuiRun {
                receiver,
                coordinator,
                run_id: String::new(),
                hitl_resume,
                cancelling: false,
            };
        }
    };
    let run_id = started.control_id.clone();
    thread::spawn(move || {
        while let Ok(event) = started.events.recv() {
            match event {
                RunStreamEvent::Raw(record) => {
                    if ui_sender.send(TuiRunMessage::Stream(record)).is_err() {
                        break;
                    }
                }
                RunStreamEvent::Status(status) => {
                    if let Some(message) = tui_run_outcome_message(status) {
                        let _ = ui_sender.send(message);
                        break;
                    }
                }
                RunStreamEvent::StartFailed(error) => {
                    let _ = ui_sender.send(TuiRunMessage::Failed(error));
                    break;
                }
            }
        }
    });
    ActiveTuiRun {
        receiver,
        coordinator,
        run_id,
        hitl_resume,
        cancelling: false,
    }
}

fn tui_environment_attachments(config: &CliConfig) -> CliResult<Vec<EnvironmentAttachmentRef>> {
    let active_profiles = config
        .envd_profiles
        .iter()
        .filter(|(_, profile)| profile.enabled)
        .collect::<Vec<_>>();
    let explicit_defaults = active_profiles
        .iter()
        .filter_map(|(name, profile)| profile.is_default.then_some((*name, *profile)))
        .collect::<Vec<_>>();
    let default_profile_name = match explicit_defaults.as_slice() {
        [] => None,
        [(name, _profile)] => Some(name.as_str()),
        _ => {
            return Err(CliError::Config(
                "TUI envd profiles require at most one default profile".to_string(),
            ));
        }
    };

    let mut seen_mounts = BTreeSet::new();
    seen_mounts.insert(LOCAL_ENVIRONMENT_ATTACHMENT_ID.to_string());
    let mut attachments = vec![EnvironmentAttachmentRef {
        id: LOCAL_ENVIRONMENT_ATTACHMENT_ID.to_string(),
        kind: LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string(),
        mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
        is_default: default_profile_name.is_none(),
        is_default_for_shell: default_profile_name.is_none(),
        attachment_lease_id: None,
        endpoint_ref: None,
        environment_id: None,
        auth_token: None,
        metadata: serde_json::Map::new(),
    }];
    for (name, profile) in active_profiles {
        if !profile.endpoint.starts_with("http://") {
            return Err(CliError::Config(format!(
                "TUI envd profile {name} currently supports http:// endpoints"
            )));
        }
        if profile.mount_id == LOCAL_ENVIRONMENT_ATTACHMENT_ID {
            return Err(CliError::Config(format!(
                "TUI envd profile {name} cannot use reserved mount id: {}",
                profile.mount_id
            )));
        }
        if !seen_mounts.insert(profile.mount_id.clone()) {
            return Err(CliError::Config(format!(
                "duplicate TUI envd mount id: {}",
                profile.mount_id
            )));
        }
        let auth_token = tui_envd_profile_auth_token(name, profile)?;
        let mut metadata = serde_json::Map::new();
        metadata.insert("envd_profile".to_string(), serde_json::json!(name));
        attachments.push(EnvironmentAttachmentRef {
            id: profile.mount_id.clone(),
            kind: "envd".to_string(),
            mode: Some(profile.mode),
            is_default: default_profile_name == Some(name.as_str()),
            is_default_for_shell: false,
            attachment_lease_id: None,
            endpoint_ref: Some(profile.endpoint.clone()),
            environment_id: profile.environment_id.clone(),
            auth_token: Some(auth_token),
            metadata,
        });
    }
    Ok(attachments)
}

fn tui_envd_profile_auth_token(
    name: &str,
    profile: &crate::config::CliEnvdProfile,
) -> CliResult<String> {
    if let Some(token) = &profile.auth_token {
        return Ok(token.clone());
    }
    let Some(env_name) = profile.auth_token_env.as_deref() else {
        return Err(CliError::Config(format!(
            "TUI envd profile {name} requires auth_token or auth_token_env"
        )));
    };
    let token = env::var(env_name).map_err(|_| {
        CliError::Config(format!(
            "TUI envd profile {name} auth_token_env {env_name} is not set"
        ))
    })?;
    if token.trim().is_empty() {
        return Err(CliError::Config(format!(
            "TUI envd profile {name} auth_token_env {env_name} is empty"
        )));
    }
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(CliError::Config(format!(
            "TUI envd profile {name} auth_token_env {env_name} cannot contain newlines"
        )));
    }
    Ok(token)
}

fn tui_run_outcome_message(status: RunStatusItem) -> Option<TuiRunMessage> {
    let outcome = PromptRunOutcome {
        session_id: status.session_id,
        run_id: status.run_id,
        status: status.status,
        error: status.error,
    };
    match outcome.status.as_str() {
        "waiting" => Some(TuiRunMessage::Waiting(outcome)),
        "completed" | "failed" | "cancelled" => Some(TuiRunMessage::Completed(outcome)),
        _ => None,
    }
}

fn terminal_run_error_message(completed: &PromptRunOutcome) -> String {
    completed
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
        .unwrap_or("run failed")
        .to_string()
}

fn terminal_run_status_line(completed: &PromptRunOutcome) -> String {
    match completed.status.as_str() {
        "failed" => format!(
            "Run failed: {} error={}",
            completed.run_id,
            terminal_run_error_message(completed)
        ),
        "cancelled" => format!("Run cancelled: {}", completed.run_id),
        _ => format!(
            "Run completed: {} status={}",
            completed.run_id, completed.status
        ),
    }
}

pub(super) fn model_choices(config: &CliConfig) -> Vec<crate::tui::ModelChoice> {
    list_config_model_profiles(config)
        .into_iter()
        .map(model_choice_from_profile)
        .collect()
}

fn model_choice_from_profile(profile: ProfileSummary) -> crate::tui::ModelChoice {
    crate::tui::ModelChoice {
        profile: profile.name,
        label: profile.label,
        model_id: profile.model_id,
        model_settings: profile.model_settings,
        model_cfg: profile.model_cfg,
        context_window: profile.context_window,
        source: profile.source,
    }
}

fn run_prompt_text(run: &starweaver_session::RunRecord) -> Option<String> {
    let text = run
        .input
        .iter()
        .filter_map(|part| match part {
            starweaver_session::InputPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn session_choices_from_summaries(sessions: Vec<SessionSummary>) -> Vec<crate::tui::SessionChoice> {
    sessions
        .into_iter()
        .map(|session| crate::tui::SessionChoice {
            session_id: session.session_id,
            title: session.title,
            profile: session.profile,
            status: session.status,
            run_count: session.run_count,
            last_output_preview: session.last_output_preview,
            updated_at: session.updated_at,
        })
        .collect()
}

fn apply_tui_session_profile(
    config: &CliConfig,
    state: &mut crate::tui::InteractiveTuiState,
    profile: Option<&str>,
) {
    let Some(profile) = profile else {
        return;
    };
    if let Some(choice) = list_profiles(config)
        .into_iter()
        .find(|summary| summary.name == profile)
        .map(model_choice_from_profile)
    {
        state.set_profile(choice.profile.clone(), model_choice_label(&choice));
        state.set_context_window(choice.context_window);
    } else {
        state.set_profile(profile.to_string(), profile.to_string());
        state.set_context_window(None);
    }
}

fn selectable_profile(choices: &[crate::tui::ModelChoice], profile: &str) -> Option<String> {
    choices
        .iter()
        .find(|choice| choice.profile == profile)
        .map(|choice| choice.profile.clone())
}

fn model_choice_label(choice: &crate::tui::ModelChoice) -> String {
    let display_name = choice.label.as_deref().unwrap_or(&choice.profile);
    if display_name == choice.model_id {
        choice.model_id.clone()
    } else {
        format!("{} ({})", display_name, choice.model_id)
    }
}

fn read_tui_selected_profile(config: &CliConfig) -> CliResult<Option<String>> {
    client_state::read_selected_profile(config, "tui")
}

fn write_tui_selected_profile(config: &CliConfig, profile: &str) -> CliResult<()> {
    client_state::write_selected_profile(config, "tui", profile)
}

fn read_tui_render_mode(config: &CliConfig) -> CliResult<Option<TuiRenderMode>> {
    client_state::read_render_mode(config, "tui")
}

fn write_tui_render_mode(config: &CliConfig, render_mode: TuiRenderMode) -> CliResult<()> {
    client_state::write_render_mode(config, "tui", render_mode)
}

fn should_run_interactive_tui(command: &TuiCommand) -> bool {
    if command.snapshot || !matches!(command.output, OutputMode::Text) {
        return false;
    }
    command.interactive || (std::io::stdout().is_terminal() && std::io::stdin().is_terminal())
}

fn tui_empty_state(config: &CliConfig) -> String {
    format!(
        "Starweaver\n\nWelcome to Starweaver.\nstatus=ready\nsession=none\nconfig_dir={}\n\nSetup status: ready for configuration\n\nStart:\n  sw cli -p \"hello\"\n  sw cli setup --global\n  sw cli diagnostics\n\nRuntime state is created after the first run.\n",
        config.global_dir.display()
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{ConfigResolver, args};
    use starweaver_rpc_core::EnvironmentAttachmentAccessMode;
    use starweaver_session::RunStatus;
    use starweaver_stream::{DisplayMessage, DisplayMessageKind, ReplaySnapshot};

    fn config_with_envd_profiles(content: &str) -> CliConfig {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(global.join("config.toml"), content).unwrap();
        let cli = args::parse(["starweaver-cli".to_string(), "tui".to_string()]).unwrap();
        ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap()
    }

    #[test]
    fn session_snapshot_keeps_run_order_and_merges_durable_prompts() {
        let config = config_with_envd_profiles("");
        let mut service = CliService::open(config).unwrap();
        let session_id = {
            let store = service.store().unwrap();
            let session = store
                .create_session("general", Some("Replay order".to_string()))
                .unwrap();
            let session_id = session.session_id.as_str().to_string();
            for (prompt, deltas) in [
                ("first prompt", ["first-0|", "first-1|"]),
                ("second prompt", ["second-0", ""]),
            ] {
                let mut run = store
                    .append_run(&session_id, prompt.to_string(), None, "general")
                    .unwrap();
                let messages = deltas
                    .into_iter()
                    .filter(|delta| !delta.is_empty())
                    .enumerate()
                    .map(|(sequence, delta)| {
                        DisplayMessage::new(
                            sequence,
                            run.session_id.clone(),
                            run.run_id.clone(),
                            DisplayMessageKind::AssistantTextDelta,
                        )
                        .with_payload(serde_json::json!({"delta": delta}))
                    })
                    .collect::<Vec<_>>();
                store
                    .complete_run(
                        &mut run,
                        "done".to_string(),
                        crate::local_store::RunArtifacts {
                            state: starweaver_context::ResumableState::default(),
                            environment_state: None,
                            raw_records: Vec::new(),
                            display_messages: messages,
                            display_snapshot: ReplaySnapshot::default(),
                            approvals: Vec::new(),
                            deferred_tools: Vec::new(),
                            status: RunStatus::Completed,
                        },
                    )
                    .unwrap();
            }
            session_id
        };

        let snapshot = service
            .tui_snapshot_for_session(&session_id, None, None)
            .unwrap();

        assert_eq!(snapshot.assistant_text, "first-0|first-1|second-0");
        assert_eq!(snapshot.transcript_lines[0], "User: first prompt");
        assert!(
            snapshot
                .transcript_lines
                .iter()
                .any(|line| line == "User: second prompt")
        );
        let first_answer = snapshot
            .transcript_lines
            .iter()
            .position(|line| line.contains("first-0|first-1|"))
            .unwrap();
        let second_prompt = snapshot
            .transcript_lines
            .iter()
            .position(|line| line == "User: second prompt")
            .unwrap();
        let second_answer = snapshot
            .transcript_lines
            .iter()
            .position(|line| line.contains("second-0"))
            .unwrap();
        assert!(first_answer < second_prompt && second_prompt < second_answer);
    }

    #[test]
    fn detached_session_background_wake_stays_isolated_until_reload() {
        let mut pending = HashMap::from([(
            "session-old".to_string(),
            VecDeque::from(["attempt-old".to_string()]),
        )]);
        let suppressed = BTreeSet::new();

        assert_eq!(
            take_pending_background_wake(None, &mut pending, &suppressed, |_, _| true),
            None
        );
        assert_eq!(pending["session-old"].len(), 1);
        assert_eq!(
            take_pending_background_wake(
                Some("session-fresh"),
                &mut pending,
                &suppressed,
                |_, _| true,
            ),
            None
        );
        assert_eq!(pending["session-old"].len(), 1);
        assert_eq!(
            take_pending_background_wake(
                Some("session-old"),
                &mut pending,
                &suppressed,
                |session_id, attempt_id| {
                    session_id == "session-old" && attempt_id == "attempt-old"
                },
            ),
            Some(("session-old".to_string(), "attempt-old".to_string()))
        );
        assert!(pending["session-old"].is_empty());
    }

    #[test]
    fn tui_context_clear_commits_view_reset_only_after_persistence() {
        let mut state =
            crate::tui::InteractiveTuiState::welcome(std::path::Path::new("/tmp/config"));
        state.session_id = Some("session-old".to_string());
        state.body.push("old transcript".to_string());

        let error = commit_tui_context_clear(&mut state, || {
            Err(CliError::Run("state write failed".to_string()))
        })
        .unwrap_err();
        assert!(error.to_string().contains("state write failed"));
        assert_eq!(state.session_id.as_deref(), Some("session-old"));
        assert_eq!(state.body, ["old transcript"]);

        commit_tui_context_clear(&mut state, || Ok(())).unwrap();
        assert!(state.session_id.is_none());
        assert!(state.body.is_empty());
    }

    #[test]
    fn tui_run_command_uses_only_the_live_session_selection() {
        let fresh = build_tui_run_command(
            "fresh prompt".to_string(),
            None,
            Some("process-affinity".to_string()),
            Some("general".to_string()),
            None,
            None,
            Vec::new(),
        );
        assert!(fresh.session.is_none());

        let resumed = build_tui_run_command(
            "continued prompt".to_string(),
            Some("session-live".to_string()),
            Some("process-affinity".to_string()),
            Some("general".to_string()),
            None,
            None,
            Vec::new(),
        );
        assert_eq!(resumed.session.as_deref(), Some("session-live"));

        let resumed_waiting = build_tui_run_command(
            "resume waiting run".to_string(),
            Some("session-live".to_string()),
            Some("process-affinity".to_string()),
            Some("general".to_string()),
            None,
            Some("run-source".to_string()),
            Vec::new(),
        );
        assert_eq!(resumed_waiting.session.as_deref(), Some("session-live"));
        assert_eq!(resumed_waiting.run.as_deref(), Some("run-source"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn durable_waiting_continuation_distinguishes_blocked_ready_and_terminal_heads() {
        let config = config_with_envd_profiles("");
        let mut service = CliService::open(config).unwrap();
        let (session_id, run_id, approval_id) = {
            let store = service.store().unwrap();
            let session = store
                .create_session("general", Some("Waiting continuation".to_string()))
                .unwrap();
            let session_id = session.session_id.as_str().to_string();
            let mut run = store
                .append_run(&session_id, "wait".to_string(), None, "general")
                .unwrap();
            let run_id = run.run_id.as_str().to_string();
            let approval = ApprovalRecord::new(
                "approval-waiting",
                run.session_id.clone(),
                run.run_id.clone(),
                "call-waiting",
                "shell",
            );
            store
                .complete_run(
                    &mut run,
                    "waiting".to_string(),
                    crate::local_store::RunArtifacts {
                        state: starweaver_context::ResumableState::default(),
                        environment_state: None,
                        raw_records: Vec::new(),
                        display_messages: Vec::new(),
                        display_snapshot: ReplaySnapshot::default(),
                        approvals: vec![approval],
                        deferred_tools: Vec::new(),
                        status: RunStatus::Waiting,
                    },
                )
                .unwrap();
            (session_id, run_id, "approval-waiting".to_string())
        };

        let blocked = service
            .tui_waiting_continuation(&session_id)
            .unwrap()
            .unwrap();
        assert!(matches!(
            blocked,
            TuiWaitingContinuation::Blocked(ref hitl)
                if hitl.source_run_id == run_id && hitl.approvals.len() == 1
        ));

        service
            .store()
            .unwrap()
            .decide_approval(&approval_id, ApprovalStatus::Approved, None)
            .unwrap();
        let ready = service
            .tui_waiting_continuation(&session_id)
            .unwrap()
            .unwrap();
        assert!(matches!(
            ready,
            TuiWaitingContinuation::Ready { source_run_id } if source_run_id == run_id
        ));

        let mut failed_continuation = service
            .store()
            .unwrap()
            .append_run(
                &session_id,
                "resume".to_string(),
                Some(run_id.clone()),
                "general",
            )
            .unwrap();
        let busy_run_id = failed_continuation.run_id.as_str().to_string();
        failed_continuation.metadata.insert(
            HITL_RESUME_PREFLIGHT_SOURCE_RUN_ID_METADATA_KEY.to_string(),
            serde_json::json!(run_id),
        );
        assert!(matches!(
            service
                .tui_waiting_continuation(&session_id)
                .unwrap()
                .unwrap(),
            TuiWaitingContinuation::Busy { active_run_id }
                if active_run_id == busy_run_id
        ));
        service
            .store()
            .unwrap()
            .fail_run(&mut failed_continuation, "pre-start failure".to_string())
            .unwrap();
        assert!(matches!(
            service
                .tui_waiting_continuation(&session_id)
                .unwrap()
                .unwrap(),
            TuiWaitingContinuation::Ready { source_run_id } if source_run_id == run_id
        ));

        let terminal_session_id = {
            let store = service.store().unwrap();
            let session = store
                .create_session("general", Some("Terminal head".to_string()))
                .unwrap();
            let session_id = session.session_id.as_str().to_string();
            let mut run = store
                .append_run(&session_id, "done".to_string(), None, "general")
                .unwrap();
            store
                .complete_run(
                    &mut run,
                    "done".to_string(),
                    crate::local_store::RunArtifacts {
                        state: starweaver_context::ResumableState::default(),
                        environment_state: None,
                        raw_records: Vec::new(),
                        display_messages: Vec::new(),
                        display_snapshot: ReplaySnapshot::default(),
                        approvals: Vec::new(),
                        deferred_tools: Vec::new(),
                        status: RunStatus::Completed,
                    },
                )
                .unwrap();
            session_id
        };
        assert!(
            service
                .tui_waiting_continuation(&terminal_session_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pending_hitl_keeps_pending_approvals_and_unresolved_deferred_records() {
        let session_id = starweaver_core::SessionId::from_string("session-hitl");
        let run_id = starweaver_core::RunId::from_string("run-hitl");
        let pending_first = ApprovalRecord::new(
            "approval-first",
            session_id.clone(),
            run_id.clone(),
            "call-first",
            "shell",
        );
        let mut approved = ApprovalRecord::new(
            "approval-done",
            session_id.clone(),
            run_id.clone(),
            "call-done",
            "write",
        );
        approved.status = ApprovalStatus::Approved;
        let pending_second = ApprovalRecord::new(
            "approval-second",
            session_id.clone(),
            run_id.clone(),
            "call-second",
            "shell",
        );
        let deferred = [
            ExecutionStatus::Pending,
            ExecutionStatus::Running,
            ExecutionStatus::Waiting,
            ExecutionStatus::Completed,
            ExecutionStatus::Failed,
            ExecutionStatus::Cancelled,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, status)| {
            let mut record = DeferredToolRecord::new(
                format!("deferred-{index}"),
                session_id.clone(),
                run_id.clone(),
                format!("call-{index}"),
                "worker",
            );
            record.status = status;
            record
        })
        .collect();

        let hitl = pending_tui_hitl_from_records(
            session_id.as_str(),
            run_id.as_str(),
            vec![pending_first, approved, pending_second],
            deferred,
        );

        assert_eq!(hitl.session_id, "session-hitl");
        assert_eq!(hitl.source_run_id, "run-hitl");
        assert_eq!(
            hitl.approvals
                .iter()
                .map(|approval| approval.approval_id.as_str())
                .collect::<Vec<_>>(),
            ["approval-first", "approval-second"]
        );
        assert_eq!(hitl.unresolved_deferred, 3);
    }

    #[test]
    fn restored_hitl_selects_the_latest_actionable_run() {
        let session_id = starweaver_core::SessionId::from_string("session-hitl");
        let old_run_id = starweaver_core::RunId::from_string("run-old");
        let new_run_id = starweaver_core::RunId::from_string("run-new");
        let old_approval = ApprovalRecord::new(
            "approval-old",
            session_id.clone(),
            old_run_id,
            "call-old",
            "shell",
        );
        let new_approval = ApprovalRecord::new(
            "approval-new",
            session_id.clone(),
            new_run_id.clone(),
            "call-new",
            "write",
        );
        let mut deferred = DeferredToolRecord::new(
            "deferred-new",
            session_id,
            new_run_id,
            "call-deferred",
            "worker",
        );
        deferred.status = ExecutionStatus::Waiting;

        let hitl = latest_pending_tui_hitl_from_records(
            "session-hitl",
            &["run-old".to_string(), "run-new".to_string()],
            vec![old_approval, new_approval],
            vec![deferred],
        )
        .unwrap();

        assert_eq!(hitl.source_run_id, "run-new");
        assert_eq!(hitl.approvals.len(), 1);
        assert_eq!(hitl.approvals[0].approval_id, "approval-new");
        assert_eq!(hitl.unresolved_deferred, 1);
    }

    #[test]
    fn waiting_status_has_a_dedicated_tui_outcome() {
        let message = tui_run_outcome_message(RunStatusItem {
            session_id: "session_waiting".to_string(),
            run_id: "run_waiting".to_string(),
            status: "waiting".to_string(),
            error: None,
        });

        assert!(matches!(
            message,
            Some(TuiRunMessage::Waiting(PromptRunOutcome {
                session_id,
                run_id,
                status,
                ..
            })) if session_id == "session_waiting"
                && run_id == "run_waiting"
                && status == "waiting"
        ));
    }

    #[test]
    fn terminal_run_status_line_includes_failed_error_detail() {
        let completed = PromptRunOutcome {
            session_id: "session_test".to_string(),
            run_id: "run_test".to_string(),
            status: "failed".to_string(),
            error: Some("websocket closed before response.completed".to_string()),
        };

        assert_eq!(
            terminal_run_error_message(&completed),
            "websocket closed before response.completed"
        );
        assert_eq!(
            terminal_run_status_line(&completed),
            "Run failed: run_test error=websocket closed before response.completed"
        );
    }

    #[test]
    fn tui_environment_attachments_include_reserved_local_without_envd_profiles() {
        let config = config_with_envd_profiles("");

        let attachments = tui_environment_attachments(&config).unwrap();

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].id, "local");
        assert_eq!(attachments[0].kind, "local");
        assert!(attachments[0].is_default);
    }

    #[test]
    fn tui_envd_attachments_build_from_config_profiles() {
        let config = config_with_envd_profiles(
            r#"
[envd_profiles.data]
endpoint = "http://127.0.0.1:8766/rpc"
auth_token = "data-secret"
environment_id = "dataset"
mode = "read_only"

[envd_profiles.review]
endpoint = "http://127.0.0.1:8770/rpc"
auth_token = "review-secret"
environment_id = "review-env"
mode = "read_write"
default = true
"#,
        );

        let attachments = tui_environment_attachments(&config).unwrap();

        assert_eq!(attachments.len(), 3);
        assert_eq!(attachments[0].id, "local");
        assert_eq!(attachments[0].kind, "local");
        assert!(!attachments[0].is_default);
        assert!(attachments[0].requested_auth_token().is_none());

        assert_eq!(attachments[1].id, "data");
        assert_eq!(attachments[1].kind, "envd");
        assert_eq!(attachments[1].requested_auth_token(), Some("data-secret"));
        assert_eq!(
            attachments[1].resolved_mode(),
            EnvironmentAttachmentAccessMode::ReadOnly
        );
        let serialized = serde_json::to_value(&attachments[1]).unwrap();
        assert!(serialized.get("authToken").is_none());

        assert_eq!(attachments[2].id, "review");
        assert!(attachments[2].is_default);
        assert_eq!(
            attachments[2].requested_environment_id(),
            Some("review-env")
        );
        assert_eq!(
            attachments[2]
                .metadata
                .get("envd_profile")
                .and_then(serde_json::Value::as_str),
            Some("review")
        );
    }

    #[test]
    fn tui_envd_profile_requires_available_token_env() {
        let config = config_with_envd_profiles(
            r#"
[envd_profiles.remote]
endpoint = "http://127.0.0.1:8766/rpc"
auth_token_env = "STARWEAVER_TEST_MISSING_ENVD_TOKEN_DO_NOT_SET"
"#,
        );

        let error = tui_environment_attachments(&config).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("STARWEAVER_TEST_MISSING_ENVD_TOKEN_DO_NOT_SET")
        );
    }
}
