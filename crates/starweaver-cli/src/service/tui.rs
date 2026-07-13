use std::{
    collections::BTreeSet,
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

use super::CliService;
use crate::{
    CliError, CliResult,
    args::{GoalCommandOptions, OutputMode, RunCommand, TuiCommand, TuiRenderMode},
    client_state,
    config::{CliConfig, clear_current_session, write_current_session},
    local_store::SessionSummary,
    profiles::{ProfileSummary, list_config_model_profiles, list_profiles},
    prompt_input::PromptInput,
    runner::CliSteeringMessage,
    runtime_coordinator::{CliRuntimeCoordinator, RunStreamEvent},
};

struct CompletedPromptRun {
    session_id: String,
    run_id: String,
    status: String,
    error: Option<String>,
}

struct ActiveTuiRun {
    receiver: mpsc::Receiver<TuiRunMessage>,
    coordinator: CliRuntimeCoordinator,
    run_id: String,
    cancelling: bool,
}

enum TuiRunMessage {
    Stream(Box<AgentStreamRecord>),
    Completed(CompletedPromptRun),
    Failed(String),
}

const TUI_FRAME_INTERVAL: Duration = Duration::from_millis(33);
const TUI_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(33);
const TUI_MIN_POLL_INTERVAL: Duration = Duration::from_millis(1);

impl CliService {
    pub(super) fn tui(&mut self, command: &TuiCommand) -> CliResult<String> {
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
        let messages = self.store()?.replay_display(session_id, run_id, after)?;
        let approvals = self.store()?.list_approvals(Some(session_id), run_id)?;
        let deferred = self
            .store()?
            .list_deferred_tools(Some(session_id), run_id)?;
        Ok(crate::tui::TuiSnapshot::from_parts(
            session_id.to_string(),
            messages,
            &approvals,
            &deferred,
        ))
    }

    pub(super) fn reload_tui_session(
        &mut self,
        state: &mut crate::tui::InteractiveTuiState,
        session_id_or_prefix: &str,
    ) -> CliResult<()> {
        let session_id = self.store()?.resolve_session_prefix(session_id_or_prefix)?;
        let session = self.store()?.load_session(&session_id)?;
        let snapshot = self.tui_snapshot_for_session(&session_id, None, None)?;
        state.set_snapshot(&snapshot);
        apply_tui_session_profile(&self.config, state, session.profile.as_deref());
        write_current_session(&self.config, &session_id)?;
        state.set_session_choices(self.tui_session_choices(50)?);
        state.push_transcript_notice(format!(
            "[SYS] Loaded session {session_id}. Next message will continue from loaded history."
        ));
        Ok(())
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
        if command.session.is_some()
            && let Some(snapshot) = self.tui_snapshot_state(command)?
        {
            state.set_snapshot(&snapshot);
        }
        let mut tui = crate::tui::InteractiveTui::enter()?;
        let mut active_run: Option<ActiveTuiRun> = None;
        let mut queued_prompt: Option<(PromptInput, String, Option<GoalCommandOptions>)> = None;
        let mut persisted_profile = state.profile.clone();
        let mut persisted_render_mode = initial_render_mode;
        let mut dirty = true;
        let now = Instant::now();
        let mut last_render = now.checked_sub(TUI_FRAME_INTERVAL).unwrap_or(now);
        loop {
            while let Some(run) = active_run.as_mut() {
                match run.receiver.try_recv() {
                    Ok(TuiRunMessage::Stream(record)) => {
                        state.apply_stream_record(&record);
                        dirty = true;
                    }
                    Ok(TuiRunMessage::Completed(completed)) => {
                        let was_cancelled = completed.status == "cancelled";
                        let failed = completed.status == "failed";
                        if was_cancelled {
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
                        if !was_cancelled
                            && !failed
                            && let Some((prompt, display_prompt, goal)) = queued_prompt.take()
                        {
                            state.begin_run(&display_prompt);
                            active_run = Some(spawn_tui_run(
                                &self.config,
                                command,
                                state.session_id.clone(),
                                Some(state.session_affinity_id.clone()),
                                prompt,
                                Some(state.profile.clone()),
                                goal,
                            ));
                        }
                        break;
                    }
                    Ok(TuiRunMessage::Failed(error)) => {
                        state.fail_run(&error);
                        active_run = None;
                        dirty = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        state.fail_run("background run channel closed");
                        active_run = None;
                        dirty = true;
                        break;
                    }
                }
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
                Some(crate::tui::InteractiveTuiEvent::Quit) if active_run.is_none() => {
                    return Ok(());
                }
                Some(crate::tui::InteractiveTuiEvent::Cancel) => {
                    if let Some(run) = active_run.as_mut()
                        && !run.cancelling
                    {
                        let _ = run.coordinator.cancel_run(&run.run_id);
                        run.cancelling = true;
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
                    if active_run.is_some() {
                        state.push_transcript_notice(
                            "[SYS] Session selection is available after the current run finishes."
                                .to_string(),
                        );
                    } else if let Some(session_id) = requested {
                        if let Err(error) = self.reload_tui_session(&mut state, &session_id) {
                            state.push_transcript_notice(format!("[SYS] {error}"));
                        }
                    } else if let Err(error) = self.open_tui_session_picker(&mut state) {
                        state.push_transcript_notice(format!("[SYS] {error}"));
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Clear) => {
                    if active_run.is_some() {
                        state.push_transcript_notice(
                            "[SYS] Clear is available after the current run finishes.".to_string(),
                        );
                    } else if let Err(error) = clear_current_session(&self.config) {
                        state.push_transcript_notice(format!("[SYS] {error}"));
                    } else {
                        queued_prompt = None;
                        state.set_session_choices(self.tui_session_choices(50)?);
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
                    if active_run.is_some() {
                        queued_prompt = Some((prompt, display_prompt, goal));
                        dirty = true;
                        continue;
                    }
                    state.begin_run(&display_prompt);
                    active_run = Some(spawn_tui_run(
                        &self.config,
                        command,
                        state.session_id.clone(),
                        Some(state.session_affinity_id.clone()),
                        prompt,
                        Some(state.profile.clone()),
                        goal,
                    ));
                    dirty = true;
                }
            }
            if state.profile != persisted_profile {
                write_tui_selected_profile(&self.config, &state.profile)?;
                persisted_profile.clone_from(&state.profile);
            }
            if state.render_mode() != persisted_render_mode {
                write_tui_render_mode(&self.config, state.render_mode())?;
                persisted_render_mode = state.render_mode();
            }
        }
    }
}

fn spawn_tui_run(
    config: &CliConfig,
    command: &TuiCommand,
    session_id: Option<String>,
    session_affinity_id: Option<String>,
    prompt_input: PromptInput,
    profile: Option<String>,
    goal: Option<GoalCommandOptions>,
) -> ActiveTuiRun {
    let command = command.clone();
    let (ui_sender, receiver) = mpsc::channel::<TuiRunMessage>();
    let mut config = config.clone();
    config.oauth_refresh.enabled = false;
    let environment_attachments = match tui_environment_attachments(&config) {
        Ok(attachments) => attachments,
        Err(error) => {
            let _ = ui_sender.send(TuiRunMessage::Failed(error.to_string()));
            let coordinator = CliRuntimeCoordinator::new(config);
            return ActiveTuiRun {
                receiver,
                coordinator,
                run_id: String::new(),
                cancelling: false,
            };
        }
    };
    let coordinator = CliRuntimeCoordinator::new(config);
    let run_command = RunCommand {
        prompt: Some(prompt_input.text.clone()),
        prompt_parts: Vec::new(),
        session: session_id.or(command.session),
        continue_session: false,
        new_session: false,
        run: None,
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
    };
    let started = match coordinator.start_run_with_raw(run_command, Some(prompt_input)) {
        Ok(started) => started,
        Err(error) => {
            let _ = ui_sender.send(TuiRunMessage::Failed(error.to_string()));
            return ActiveTuiRun {
                receiver,
                coordinator,
                run_id: String::new(),
                cancelling: false,
            };
        }
    };
    let run_id = started.run_id.clone();
    thread::spawn(move || {
        while let Ok(event) = started.events.recv() {
            match event {
                RunStreamEvent::Raw(record) => {
                    if ui_sender.send(TuiRunMessage::Stream(record)).is_err() {
                        break;
                    }
                }
                RunStreamEvent::Status(status) if status_is_terminal(&status.status) => {
                    let _ = ui_sender.send(TuiRunMessage::Completed(CompletedPromptRun {
                        session_id: status.session_id,
                        run_id: status.run_id,
                        status: status.status,
                        error: status.error,
                    }));
                    break;
                }
                RunStreamEvent::Status(_) => {}
            }
        }
    });
    ActiveTuiRun {
        receiver,
        coordinator,
        run_id,
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

fn status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn terminal_run_error_message(completed: &CompletedPromptRun) -> String {
    completed
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
        .unwrap_or("run failed")
        .to_string()
}

fn terminal_run_status_line(completed: &CompletedPromptRun) -> String {
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
    fn terminal_run_status_line_includes_failed_error_detail() {
        let completed = CompletedPromptRun {
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
