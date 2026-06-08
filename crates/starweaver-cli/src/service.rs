//! CLI service layer over local storage and SDK execution.

use std::{
    fmt::Write as _,
    fs,
    io::IsTerminal as _,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

use chrono::Utc;
use clap_complete::Shell;
use ring::digest;
use serde_json::{json, Value};
use starweaver_core::sdk_name;
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{ApprovalRecord, ApprovalStatus, DeferredToolRecord, RunRecord};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

use crate::{
    args::{
        ApprovalCommand, ApprovalDecisionCommand, ApprovalListCommand, AuthCommand, CatalogCommand,
        Cli, CliCommand, ConfigCommand, DeferredCommand, DeferredCompleteCommand,
        DeferredFailCommand, DeferredListCommand, OutputMode, ProfileCommand, ResetCommand,
        ResumeCommand, RunCommand, SessionCommand, SetupCommand, ToolsCommand, TuiCommand,
        UpdateCommand,
    },
    config::{
        get_config_value, init_config_file, mcp_servers, read_current_session, tool_need_approval,
        write_current_session, write_default_subagent_presets, CliConfig, ConfigScope,
        DEFAULT_GLOBAL_GITIGNORE_TEMPLATE, DEFAULT_MCP_TEMPLATE,
        DEFAULT_PROJECT_GITIGNORE_TEMPLATE, DEFAULT_TOOLS_TEMPLATE,
    },
    environment::resolve_environment,
    local_store::{LocalStore, RunSummary, SessionSummary, TrimReport},
    profiles::{
        doctor_mcp_servers, list_default_tools, list_mcp_servers, list_profiles, list_skills,
        list_subagents, resolve_profile, show_mcp_server, show_profile, show_skill, show_subagent,
    },
    runner::{
        execute_agent_session, execute_agent_session_with_channels, failed_display_message,
        CliRunPolicy, CliSteeringMessage,
    },
    CliError, CliResult,
};

struct PromptRunExecution {
    session_id: String,
    run_id: String,
    status: String,
    output_mode: OutputMode,
    messages: Vec<DisplayMessage>,
}

struct CompletedPromptRun {
    session_id: String,
    run_id: String,
    status: String,
    output_text: String,
}

struct ActiveTuiRun {
    receiver: mpsc::Receiver<TuiRunMessage>,
    steering_sender: mpsc::Sender<CliSteeringMessage>,
    cancel_sender: mpsc::Sender<()>,
    cancelling: bool,
}

enum TuiRunMessage {
    Stream(AgentStreamRecord),
    Completed(CompletedPromptRun),
    Failed(String),
}

/// CLI service.
pub struct CliService {
    config: CliConfig,
    store: Option<LocalStore>,
}

impl CliService {
    /// Open service from resolved config.
    pub const fn open(config: CliConfig) -> CliResult<Self> {
        Ok(Self {
            config,
            store: None,
        })
    }

    fn store(&mut self) -> CliResult<&mut LocalStore> {
        if self.store.is_none() {
            self.store = Some(LocalStore::open(&self.config)?);
        }
        self.store
            .as_mut()
            .ok_or_else(|| CliError::Storage("store initialization failed".to_string()))
    }

    /// Execute a parsed CLI command.
    pub fn execute(mut self, cli: Cli) -> CliResult<String> {
        if let Some(prompt) = cli.prompt.clone() {
            let command = RunCommand {
                prompt: Some(prompt),
                prompt_parts: Vec::new(),
                session: cli.session.clone(),
                continue_session: cli.continue_session,
                new_session: cli.new_session,
                run: cli.run.clone(),
                branch_from: cli.branch_from.clone(),
                profile: cli.profile.clone(),
                output: cli.output,
                hitl: cli.hitl,
                worker: cli.worker.clone(),
                worker_label: cli.worker_label.clone(),
                worktree: cli.worktree.clone(),
                worktree_name: cli.worktree_name.clone(),
                branch: cli.branch,
            };
            return self.run_prompt(&command);
        }
        let default_command = CliCommand::Tui(TuiCommand {
            session: cli.session.clone(),
            run: cli.run.clone(),
            after: None,
            interactive: false,
            snapshot: false,
            output: OutputMode::Text,
        });
        match cli.command.unwrap_or(default_command) {
            CliCommand::Version => Ok(format!("{}\n", sdk_name())),
            CliCommand::Diagnostics => Ok(self.diagnostics()?),
            CliCommand::ReplayCheck => {
                Ok("run `make replay-check` from the repository root\n".to_string())
            }
            CliCommand::Update(command) => Self::update(&command),
            CliCommand::Run(command) => self.run_prompt(&command),
            CliCommand::Rpc(_) => Err(CliError::Usage(
                "rpc owns stdin/stdout and must be run through run_from_env".to_string(),
            )),
            CliCommand::Session { command } => self.session(command),
            CliCommand::Profile { command } => self.profile(command),
            CliCommand::Setup(command) => self.setup(&command),
            CliCommand::Auth { command } => Self::auth(command),
            CliCommand::Skill { command } => self.skills(command),
            CliCommand::Subagent { command } => self.subagents(command),
            CliCommand::Mcp { command } => self.mcp(command),
            CliCommand::Tools { command } => self.tools(&command),
            CliCommand::Tui(command) => self.tui(&command),
            CliCommand::Approval { command } => self.approval(command),
            CliCommand::Deferred { command } => self.deferred(command),
            CliCommand::Resume(command) => self.resume(&command),
            CliCommand::Reset(command) => self.reset(&command),
            CliCommand::Config { command } => self.config(command),
            CliCommand::Completion { shell } => render_completion(shell),
        }
    }

    fn run_prompt(&mut self, command: &RunCommand) -> CliResult<String> {
        let execution = self.execute_prompt_run(command, None)?;
        match execution.output_mode {
            OutputMode::Text => Ok(render_display_text(&execution.messages)),
            OutputMode::DisplayJsonl => render_display_jsonl(&execution.messages),
            OutputMode::AguiJsonl => render_agui_jsonl(&execution.messages),
            OutputMode::Json => render_prompt_run_json(&execution),
            OutputMode::Silent => Ok(format!(
                "session_id={}\nrun_id={}\nstatus={}\n",
                execution.session_id, execution.run_id, execution.status
            )),
        }
    }

    fn run_prompt_streaming_with_steering(
        &mut self,
        command: &RunCommand,
        stream_sender: mpsc::Sender<AgentStreamRecord>,
        steering_receiver: mpsc::Receiver<CliSteeringMessage>,
        cancel_receiver: mpsc::Receiver<()>,
    ) -> CliResult<CompletedPromptRun> {
        let execution = self.execute_prompt_run_with_steering(
            command,
            stream_sender,
            steering_receiver,
            cancel_receiver,
        )?;
        let output_text = render_display_text(&execution.messages);
        Ok(CompletedPromptRun {
            session_id: execution.session_id,
            run_id: execution.run_id,
            status: execution.status,
            output_text,
        })
    }

    fn execute_prompt_run(
        &mut self,
        command: &RunCommand,
        stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
    ) -> CliResult<PromptRunExecution> {
        self.execute_prompt_run_with_channels(command, stream_sender, None, None)
    }

    fn execute_prompt_run_with_steering(
        &mut self,
        command: &RunCommand,
        stream_sender: mpsc::Sender<AgentStreamRecord>,
        steering_receiver: mpsc::Receiver<CliSteeringMessage>,
        cancel_receiver: mpsc::Receiver<()>,
    ) -> CliResult<PromptRunExecution> {
        self.execute_prompt_run_with_channels(
            command,
            Some(stream_sender),
            Some(steering_receiver),
            Some(cancel_receiver),
        )
    }

    fn execute_prompt_run_with_channels(
        &mut self,
        command: &RunCommand,
        stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
        steering_receiver: Option<mpsc::Receiver<CliSteeringMessage>>,
        cancel_receiver: Option<mpsc::Receiver<()>>,
    ) -> CliResult<PromptRunExecution> {
        let prompt = command.prompt_text()?;
        let worktree = self.resolve_worktree(command)?;
        let selected_profile = command
            .profile
            .as_deref()
            .unwrap_or(&self.config.default_profile);
        let resolved_profile = resolve_profile(&self.config, Some(selected_profile))?;
        let mut run_config = self.config.clone();
        if let Some(worktree) = worktree.as_ref() {
            run_config.workspace_root.clone_from(&worktree.path);
        }
        let environment = resolve_environment(&run_config)?;
        let (session_id, created) = self.resolve_session(command, &resolved_profile.name)?;
        let mut restore_from = command.run.clone().or_else(|| command.branch_from.clone());
        if restore_from.is_none() && !created {
            restore_from = self
                .store()?
                .load_session(&session_id)
                .ok()
                .and_then(|session| {
                    session
                        .head_success_run_id
                        .map(|run| run.as_str().to_string())
                });
        }
        let mut run = self.store()?.append_run(
            &session_id,
            prompt.clone(),
            restore_from.clone(),
            &resolved_profile.name,
        )?;
        apply_yaacli_run_metadata(&mut run, command, worktree.as_ref());
        write_current_session(&self.config, &session_id)?;
        let restore_state = self
            .store()?
            .load_restore_state(&session_id, restore_from.as_deref())?;
        let hitl = command.hitl.unwrap_or(self.config.default_hitl);
        let result = if stream_sender.is_some()
            || steering_receiver.is_some()
            || cancel_receiver.is_some()
        {
            execute_agent_session_with_channels(
                prompt,
                &run,
                &resolved_profile,
                &environment.provider,
                restore_state,
                CliRunPolicy { hitl },
                stream_sender,
                steering_receiver,
                cancel_receiver,
            )
        } else {
            execute_agent_session(
                prompt,
                &run,
                &resolved_profile,
                &environment.provider,
                restore_state,
                CliRunPolicy { hitl },
            )
        };
        let execution = match result {
            Ok(execution) => execution,
            Err(error) => {
                let messages = failed_display_message(&run, &error.to_string());
                self.store()?
                    .fail_run_with_messages(&mut run, error.to_string(), &messages)?;
                return Err(error);
            }
        };
        let messages =
            self.store()?
                .complete_run(&mut run, execution.output, execution.artifacts)?;
        if self.config.auto_trim {
            let keep_runs = self.config.current_session_keep_recent_runs;
            let _report = self
                .store()?
                .trim(vec![session_id.clone()], keep_runs, false)?;
        }
        Ok(PromptRunExecution {
            session_id,
            run_id: run.run_id.as_str().to_string(),
            status: run_status_name(run.status).to_string(),
            output_mode: command.output.unwrap_or(self.config.default_output),
            messages,
        })
    }

    fn resolve_session(
        &mut self,
        command: &RunCommand,
        profile: &str,
    ) -> CliResult<(String, bool)> {
        if command.new_session {
            let session = self
                .store()?
                .create_session(profile, Some("CLI session".to_string()))?;
            return Ok((session.session_id.as_str().to_string(), true));
        }
        if let Some(session_id) = command.session.as_ref() {
            self.store()?.load_session(session_id)?;
            return Ok((session_id.clone(), false));
        }
        if command.continue_session {
            if let Some(session_id) = read_current_session(&self.config)? {
                if self.store()?.load_session(&session_id).is_ok() {
                    return Ok((session_id, false));
                }
            }
            if let Some(session) = self.store()?.latest_session()? {
                return Ok((session.session_id.as_str().to_string(), false));
            }
        }
        let session = self
            .store()?
            .create_session(profile, Some("CLI session".to_string()))?;
        Ok((session.session_id.as_str().to_string(), true))
    }

    fn session(&mut self, command: SessionCommand) -> CliResult<String> {
        match command {
            SessionCommand::List(command) => {
                let sessions = self.store()?.list_sessions(command.limit)?;
                render_sessions(&sessions, command.output)
            }
            SessionCommand::Show(command) => {
                let session = self.store()?.load_session(&command.session_id)?;
                let runs = self.store()?.list_runs(&command.session_id, command.runs)?;
                let value = session_value(&session);
                render_session_show(&value, &runs, command.output)
            }
            SessionCommand::Replay(command) => {
                let messages = self.store()?.replay_display(
                    &command.session_id,
                    command.run.as_deref(),
                    command.after,
                )?;
                match command.output {
                    OutputMode::Text => Ok(render_display_text(&messages)),
                    OutputMode::DisplayJsonl => render_display_jsonl(&messages),
                    OutputMode::AguiJsonl => render_agui_jsonl(&messages),
                    OutputMode::Json => Ok(format!(
                        "{}\n",
                        serde_json::to_string(&json!({
                            "sessionId": command.session_id,
                            "runId": command.run,
                            "messages": messages,
                            "status": "replayed"
                        }))?
                    )),
                    OutputMode::Silent => Ok(format!(
                        "session_id={}\nmessages={}\nstatus=replayed\n",
                        command.session_id,
                        messages.len()
                    )),
                }
            }
            SessionCommand::Delete(command) => {
                if !command.yes {
                    return Err(CliError::Usage(
                        "pass --yes to delete a local session".to_string(),
                    ));
                }
                let session_id = self.store()?.resolve_session_prefix(&command.session_id)?;
                let deleted = self.store()?.delete_session(&session_id)?;
                if read_current_session(&self.config)?.as_deref() == Some(session_id.as_str()) {
                    let _removed =
                        remove_file_if_exists(&self.config.project_dir.join("state.json"))?;
                }
                render_session_delete(&session_id, deleted, command.output)
            }
            SessionCommand::Trim(command) => {
                let sessions = if command.all {
                    self.store()?.all_session_ids()?
                } else if let Some(session_id) = command.session {
                    vec![session_id]
                } else {
                    read_current_session(&self.config)?.into_iter().collect()
                };
                let older_than = command
                    .older_than
                    .as_deref()
                    .map(parse_duration)
                    .transpose()?;
                let report = self.store()?.trim_with_age(
                    sessions,
                    command.keep_runs,
                    older_than,
                    command.dry_run,
                )?;
                render_trim_report(&report, command.output)
            }
        }
    }

    fn profile(&self, command: ProfileCommand) -> CliResult<String> {
        match command {
            ProfileCommand::List => list_profiles(&self.config)
                .iter()
                .map(|profile| serde_json::to_string(profile).map(|line| format!("{line}\n")))
                .collect::<Result<String, _>>()
                .map_err(CliError::from),
            ProfileCommand::Show { name } => show_profile(&self.config, &name),
        }
    }

    fn update(command: &UpdateCommand) -> CliResult<String> {
        crate::launcher::update_component(&command.target)
    }

    fn setup(&self, command: &SetupCommand) -> CliResult<String> {
        let mut rows = Vec::new();
        if command.global || !command.project {
            rows.push(setup_config_file(
                &self.config,
                ConfigScope::Global,
                command.force,
            )?);
            setup_catalog_files(&self.config.global_dir, command.force, &mut rows)?;
            rows.push(write_template_if_missing(
                &self.config.global_dir.join(".gitignore"),
                DEFAULT_GLOBAL_GITIGNORE_TEMPLATE,
                command.force,
                "global-state-ignore",
            )?);
        }
        if command.project {
            rows.push(setup_config_file(
                &self.config,
                ConfigScope::Project,
                command.force,
            )?);
            setup_catalog_files(&self.config.project_dir, command.force, &mut rows)?;
            rows.push(write_template_if_missing(
                &self.config.project_dir.join(".gitignore"),
                DEFAULT_PROJECT_GITIGNORE_TEMPLATE,
                command.force,
                "state-ignore",
            )?);
        }
        render_json_lines(&rows)
    }

    fn auth(command: AuthCommand) -> CliResult<String> {
        let store = crate::oauth::OAuthStore::new(crate::oauth::OAuthStore::default_path());
        match command {
            AuthCommand::Status { provider } => {
                let record = store.load_provider(&provider)?;
                let value = json!({
                    "provider": provider,
                    "logged_in": record.is_some(),
                    "auth_path": store.path(),
                    "record": record.map(|record| record.status_value()),
                });
                Ok(format!("{}\n", serde_json::to_string(&value)?))
            }
            AuthCommand::Logout { provider } => {
                let removed = store.remove_provider(&provider)?;
                Ok(format!("provider={provider}\nremoved={removed}\n"))
            }
        }
    }

    fn skills(&self, command: CatalogCommand) -> CliResult<String> {
        match command {
            CatalogCommand::List => render_json_lines(&list_skills(&self.config)),
            CatalogCommand::Show { name } => show_skill(&self.config, &name),
            CatalogCommand::Doctor => Ok(format!(
                "skill_dirs={}\nskills={}\nstatus=ok\n",
                self.config
                    .skill_dirs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(":"),
                list_skills(&self.config).len()
            )),
        }
    }

    fn subagents(&self, command: CatalogCommand) -> CliResult<String> {
        match command {
            CatalogCommand::List => render_json_lines(&list_subagents(&self.config)),
            CatalogCommand::Show { name } => show_subagent(&self.config, &name),
            CatalogCommand::Doctor => Ok(format!(
                "subagent_dirs={}\nsubagents={}\ndisabled={}\nstatus=ok\n",
                self.config
                    .subagent_dirs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(":"),
                list_subagents(&self.config).len(),
                self.config.disabled_subagents.join(",")
            )),
        }
    }

    fn mcp(&self, command: CatalogCommand) -> CliResult<String> {
        match command {
            CatalogCommand::List => render_json_lines(&list_mcp_servers(&self.config)),
            CatalogCommand::Show { name } => show_mcp_server(&self.config, &name),
            CatalogCommand::Doctor => {
                let findings = doctor_mcp_servers(&self.config);
                let output = render_json_lines(&findings)?;
                if findings.iter().any(|finding| finding.status == "error") {
                    Err(CliError::Config(output))
                } else {
                    Ok(output)
                }
            }
        }
    }

    fn tools(&self, command: &ToolsCommand) -> CliResult<String> {
        match command {
            ToolsCommand::List => render_json_lines(&list_default_tools(&self.config)),
            ToolsCommand::Doctor => Ok(format!(
                "tools={}\nneed_approval={}\nstatus=ok\n",
                list_default_tools(&self.config).len(),
                tool_need_approval(&self.config).join(",")
            )),
        }
    }

    fn tui(&mut self, command: &TuiCommand) -> CliResult<String> {
        if should_run_interactive_tui(command) {
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
        let messages =
            self.store()?
                .replay_display(&session_id, command.run.as_deref(), command.after)?;
        let approvals = self
            .store()?
            .list_approvals(Some(&session_id), command.run.as_deref())?;
        let deferred = self
            .store()?
            .list_deferred_tools(Some(&session_id), command.run.as_deref())?;
        Ok(Some(crate::tui::TuiSnapshot::from_parts(
            session_id, messages, &approvals, &deferred,
        )))
    }

    #[allow(clippy::too_many_lines)]
    fn interactive_tui(&mut self, command: &TuiCommand) -> CliResult<()> {
        let mut state = crate::tui::InteractiveTuiState::welcome(&self.config.tui_state_dir);
        state.set_custom_commands(self.config.slash_commands.clone());
        state.set_model_choices(model_choices(&self.config));
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
        if command.session.is_some() {
            if let Some(snapshot) = self.tui_snapshot_state(command)? {
                state.set_snapshot(&snapshot);
            }
        }
        let mut tui = crate::tui::InteractiveTui::enter()?;
        let mut active_run: Option<ActiveTuiRun> = None;
        let mut queued_prompt: Option<String> = None;
        let mut persisted_profile = state.profile.clone();
        let mut dirty = true;
        loop {
            while let Some(run) = active_run.as_mut() {
                match run.receiver.try_recv() {
                    Ok(TuiRunMessage::Stream(record)) => {
                        state.apply_stream_record(&record);
                        dirty = true;
                    }
                    Ok(TuiRunMessage::Completed(completed)) => {
                        let was_cancelled = completed.status == "cancelled";
                        if was_cancelled {
                            state.session_id = Some(completed.session_id.clone());
                            state.cancel_run("cancelled by user");
                        } else {
                            state.finish_run(Some(completed.session_id.clone()));
                        }
                        state.body.push(format!(
                            "Run completed: {} status={}",
                            completed.run_id, completed.status
                        ));
                        active_run = None;
                        dirty = true;
                        if !was_cancelled {
                            match state.complete_goal_iteration(&completed.output_text) {
                                crate::tui::GoalIterationOutcome::Continue(prompt) => {
                                    state.begin_run(&prompt);
                                    active_run = Some(spawn_tui_run(
                                        &self.config,
                                        command,
                                        state.session_id.clone(),
                                        prompt,
                                        Some(state.profile.clone()),
                                    ));
                                }
                                crate::tui::GoalIterationOutcome::Inactive
                                | crate::tui::GoalIterationOutcome::Complete
                                | crate::tui::GoalIterationOutcome::MaxIterations => {
                                    if let Some(prompt) = queued_prompt.take() {
                                        state.begin_run(&prompt);
                                        active_run = Some(spawn_tui_run(
                                            &self.config,
                                            command,
                                            state.session_id.clone(),
                                            prompt,
                                            Some(state.profile.clone()),
                                        ));
                                    }
                                }
                            }
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

            if dirty {
                tui.render(&state)?;
                dirty = false;
            }
            let event =
                crate::tui::InteractiveTui::poll_event(&mut state, Duration::from_millis(33))?;
            match event {
                Some(crate::tui::InteractiveTuiEvent::Quit) if active_run.is_none() => {
                    return Ok(())
                }
                Some(crate::tui::InteractiveTuiEvent::Cancel) => {
                    if let Some(run) = active_run.as_mut() {
                        if !run.cancelling {
                            let _ = run.cancel_sender.send(());
                            run.cancelling = true;
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
                    if let Some(run) = active_run.as_ref() {
                        if !run.cancelling
                            && run
                                .steering_sender
                                .send(CliSteeringMessage {
                                    id: steering.id,
                                    text: steering.text,
                                })
                                .is_err()
                        {
                            state.fail_run("background steering channel closed");
                            active_run = None;
                        }
                    }
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Queue(prompt)) => {
                    queued_prompt = Some(prompt);
                    dirty = true;
                }
                Some(crate::tui::InteractiveTuiEvent::Submit(prompt)) => {
                    if active_run.is_some() {
                        queued_prompt = Some(prompt);
                        dirty = true;
                        continue;
                    }
                    state.begin_run(&prompt);
                    active_run = Some(spawn_tui_run(
                        &self.config,
                        command,
                        state.session_id.clone(),
                        prompt,
                        Some(state.profile.clone()),
                    ));
                    dirty = true;
                }
            }
            if state.profile != persisted_profile {
                write_tui_selected_profile(&self.config, &state.profile)?;
                persisted_profile.clone_from(&state.profile);
            }
        }
    }

    fn approval(&mut self, command: ApprovalCommand) -> CliResult<String> {
        match command {
            ApprovalCommand::List(command) => self.approval_list(&command),
            ApprovalCommand::Show { approval_id } => {
                let approval = self.store()?.load_approval(&approval_id)?;
                Ok(format!("{}\n", serde_json::to_string(&approval)?))
            }
            ApprovalCommand::Approve(command) => {
                self.approval_decision(&command, ApprovalStatus::Approved)
            }
            ApprovalCommand::Reject(command) => {
                self.approval_decision(&command, ApprovalStatus::Denied)
            }
        }
    }

    fn approval_list(&mut self, command: &ApprovalListCommand) -> CliResult<String> {
        let approvals = self
            .store()?
            .list_approvals(command.session.as_deref(), command.run.as_deref())?;
        render_approvals(&approvals, command.output)
    }

    fn approval_decision(
        &mut self,
        command: &ApprovalDecisionCommand,
        status: ApprovalStatus,
    ) -> CliResult<String> {
        let approval =
            self.store()?
                .decide_approval(&command.approval_id, status, command.reason.clone())?;
        match command.output {
            OutputMode::Text => Ok(format!(
                "approval_id={}\nstatus={}\nrun_id={}\n",
                approval.approval_id,
                approval_status_name(approval.status),
                approval.run_id.as_str()
            )),
            OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => {
                Ok(format!("{}\n", serde_json::to_string(&approval)?))
            }
            OutputMode::Silent => Ok(format!(
                "approval_id={}\nstatus={}\n",
                approval.approval_id,
                approval_status_name(approval.status)
            )),
        }
    }

    fn deferred(&mut self, command: DeferredCommand) -> CliResult<String> {
        match command {
            DeferredCommand::List(command) => self.deferred_list(&command),
            DeferredCommand::Show { deferred_id } => {
                let deferred = self.store()?.load_deferred_tool(&deferred_id)?;
                Ok(format!("{}\n", serde_json::to_string(&deferred)?))
            }
            DeferredCommand::Complete(command) => self.deferred_complete(&command),
            DeferredCommand::Fail(command) => self.deferred_fail(&command),
        }
    }

    fn deferred_list(&mut self, command: &DeferredListCommand) -> CliResult<String> {
        let records = self
            .store()?
            .list_deferred_tools(command.session.as_deref(), command.run.as_deref())?;
        render_deferred(&records, command.output)
    }

    fn deferred_complete(&mut self, command: &DeferredCompleteCommand) -> CliResult<String> {
        let value = serde_json::from_str::<Value>(&command.result)
            .map_err(|error| CliError::Usage(format!("invalid deferred result JSON: {error}")))?;
        let record = self
            .store()?
            .complete_deferred_tool(&command.deferred_id, value)?;
        render_deferred_decision(&record, command.output)
    }

    fn deferred_fail(&mut self, command: &DeferredFailCommand) -> CliResult<String> {
        let record = self
            .store()?
            .fail_deferred_tool(&command.deferred_id, &command.error)?;
        render_deferred_decision(&record, command.output)
    }

    fn resume(&mut self, command: &ResumeCommand) -> CliResult<String> {
        let session_id = self.resolve_session_id(command.session.as_deref())?;
        let source_run = self.resolve_resume_run(&session_id, command.run.as_deref())?;
        let run_command = RunCommand {
            prompt: Some(format!(
                "{}\n\nResuming from run {} with any persisted approval and deferred-tool decisions.",
                command.prompt,
                source_run.run_id.as_str()
            )),
            prompt_parts: Vec::new(),
            session: Some(session_id),
            continue_session: false,
            new_session: false,
            run: Some(source_run.run_id.as_str().to_string()),
            branch_from: None,
            profile: source_run.profile.clone(),
            output: command.output,
            hitl: command.hitl,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
        };
        self.run_prompt(&run_command)
    }

    fn resolve_session_id(&mut self, requested: Option<&str>) -> CliResult<String> {
        if let Some(session_id) = requested {
            self.store()?.load_session(session_id)?;
            return Ok(session_id.to_string());
        }
        if let Some(session_id) = read_current_session(&self.config)? {
            if self.store()?.load_session(&session_id).is_ok() {
                return Ok(session_id);
            }
        }
        self.store()?
            .latest_session()?
            .map(|session| session.session_id.as_str().to_string())
            .ok_or_else(|| CliError::NotFound("session".to_string()))
    }

    fn resolve_resume_run(
        &mut self,
        session_id: &str,
        requested: Option<&str>,
    ) -> CliResult<starweaver_session::RunRecord> {
        if let Some(run_id) = requested {
            return self.store()?.load_run(session_id, run_id);
        }
        let session = self.store()?.load_session(session_id)?;
        let run_id = session
            .active_run_id
            .as_ref()
            .or(session.head_run_id.as_ref())
            .ok_or_else(|| CliError::NotFound("run".to_string()))?;
        self.store()?.load_run(session_id, run_id.as_str())
    }

    fn reset(&mut self, command: &ResetCommand) -> CliResult<String> {
        if !command.yes {
            return Err(CliError::Usage(
                "pass --yes to remove runtime session state".to_string(),
            ));
        }
        self.store = None;
        let removed_database = remove_file_if_exists(&self.config.database_path)?;
        let removed_state = remove_file_if_exists(&self.config.project_dir.join("state.json"))?;
        let removed_store = remove_dir_if_exists(&self.config.file_store_path)?;
        match command.output {
            OutputMode::Text => Ok(format!(
                "removed_database={removed_database}\nremoved_state={removed_state}\nremoved_store={removed_store}\nstatus=reset\n"
            )),
            OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => Ok(format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "removed_database": removed_database,
                    "removed_state": removed_state,
                    "removed_store": removed_store,
                    "status": "reset"
                }))?
            )),
            OutputMode::Silent => Ok("status=reset\n".to_string()),
        }
    }

    fn config(&self, command: ConfigCommand) -> CliResult<String> {
        match command {
            ConfigCommand::Init {
                global,
                project: _,
                force,
            } => {
                let scope = if global {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project
                };
                let path = init_config_file(&self.config, scope, force)?;
                Ok(format!(
                    "config_path={}\nstatus=initialized\n",
                    path.display()
                ))
            }
            ConfigCommand::Get { key } => get_config_value(&self.config, &key),
            ConfigCommand::Set {
                global,
                project: _,
                key,
                value,
            } => {
                let scope = if global {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project
                };
                crate::config::set_config_value(&self.config, scope, &key, &value)?;
                Ok(format!("{key}={value}\n"))
            }
        }
    }

    fn diagnostics(&self) -> CliResult<String> {
        Ok(format!(
            "sdk={}\nworkspace_version={}\ndatabase_path={}\nfile_store_path={}\nprofile={}\ndefault_model={}\nmodel_profiles={}\nworkspace_root={}\nenvironment_provider={}\nfiles_policy={}\nshell_enabled={}\nskills={}\nsubagents={}\nmcp_servers={}\ntools={}\ntools.need_approval={}\nprovider.openai.ready={}\nprovider.openai.api_key_env={}\nprovider.openai.base_url={}\nprovider.codex.logged_in={}\nprovider.codex.base_url={}\nprovider.anthropic.ready={}\nprovider.anthropic.api_key_env={}\nprovider.anthropic.base_url={}\nprovider.gemini.ready={}\nprovider.gemini.api_key_env={}\nprovider.gemini.base_url={}\nwal=true\n",
            sdk_name(),
            env!("CARGO_PKG_VERSION"),
            self.config.database_path.display(),
            self.config.file_store_path.display(),
            self.config.default_profile,
            self.config
                .default_model
                .as_ref()
                .map(|profile| profile.model_id.as_str())
                .unwrap_or_default(),
            self.config.model_profiles.len(),
            self.config.workspace_root.display(),
            self.config.environment_provider,
            self.config.files_policy,
            self.config.shell_enabled,
            list_skills(&self.config).len(),
            list_subagents(&self.config).len(),
            mcp_servers(&self.config).len(),
            list_default_tools(&self.config).len(),
            tool_need_approval(&self.config).join(","),
            provider_ready(&self.config.providers.openai),
            self.config.providers.openai.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.openai.base_url.as_deref().unwrap_or_default(),
            crate::oauth::OAuthStore::new(crate::oauth::OAuthStore::default_path())
                .load_provider("codex")?
                .is_some(),
            self.config.providers.codex.base_url.as_deref().unwrap_or_default(),
            provider_ready(&self.config.providers.anthropic),
            self.config.providers.anthropic.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.anthropic.base_url.as_deref().unwrap_or_default(),
            provider_ready(&self.config.providers.gemini),
            self.config.providers.gemini.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.gemini.base_url.as_deref().unwrap_or_default()
        ))
    }
}

fn spawn_tui_run(
    config: &CliConfig,
    command: &TuiCommand,
    session_id: Option<String>,
    prompt: String,
    profile: Option<String>,
) -> ActiveTuiRun {
    let config = config.clone();
    let command = command.clone();
    let (ui_sender, receiver) = mpsc::channel::<TuiRunMessage>();
    let (stream_sender, stream_receiver) = mpsc::channel::<AgentStreamRecord>();
    let (steering_sender, steering_receiver) = mpsc::channel::<CliSteeringMessage>();
    let (cancel_sender, cancel_receiver) = mpsc::channel::<()>();
    let stream_ui_sender = ui_sender.clone();
    let forward_handle = thread::spawn(move || {
        while let Ok(record) = stream_receiver.recv() {
            if stream_ui_sender
                .send(TuiRunMessage::Stream(record))
                .is_err()
            {
                break;
            }
        }
    });
    thread::spawn(move || {
        let result = CliService::open(config).and_then(|mut service| {
            let run_command = RunCommand {
                prompt: Some(prompt),
                prompt_parts: Vec::new(),
                session: session_id.or(command.session),
                continue_session: false,
                new_session: false,
                run: None,
                branch_from: None,
                profile,
                output: Some(OutputMode::Text),
                hitl: None,
                worker: None,
                worker_label: None,
                worktree: None,
                worktree_name: None,
                branch: None,
            };
            service.run_prompt_streaming_with_steering(
                &run_command,
                stream_sender,
                steering_receiver,
                cancel_receiver,
            )
        });
        let _ = forward_handle.join();
        let message = match result {
            Ok(completed) => TuiRunMessage::Completed(completed),
            Err(error) => TuiRunMessage::Failed(error.to_string()),
        };
        let _ = ui_sender.send(message);
    });
    ActiveTuiRun {
        receiver,
        steering_sender,
        cancel_sender,
        cancelling: false,
    }
}

fn model_choices(config: &CliConfig) -> Vec<crate::tui::ModelChoice> {
    list_profiles(config)
        .into_iter()
        .filter(|profile| is_user_selectable_model_profile(&profile.name))
        .map(|profile| crate::tui::ModelChoice {
            profile: profile.name,
            label: profile.label,
            model_id: profile.model_id,
            model_settings: profile.model_settings,
            model_cfg: profile.model_cfg,
            context_window: profile.context_window,
            source: profile.source,
        })
        .collect()
}

fn is_user_selectable_model_profile(profile: &str) -> bool {
    !matches!(profile, "approval_model" | "deferred_model")
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
    let path = config.tui_state_dir.join("state.json");
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(&path).map_err(|error| crate::error::io_error(&path, error))?;
    let value = serde_json::from_str::<Value>(&content)?;
    Ok(value
        .get("selected_profile")
        .or_else(|| value.get("selectedProfile"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

fn write_tui_selected_profile(config: &CliConfig, profile: &str) -> CliResult<()> {
    fs::create_dir_all(&config.tui_state_dir)
        .map_err(|error| crate::error::io_error(&config.tui_state_dir, error))?;
    let path = config.tui_state_dir.join("state.json");
    let temp = config
        .tui_state_dir
        .join(format!("state.{}.json.tmp", std::process::id()));
    let value = json!({
        "selected_profile": profile,
        "updated_at": Utc::now().to_rfc3339(),
    });
    fs::write(&temp, serde_json::to_vec_pretty(&value)?)
        .map_err(|error| crate::error::io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| crate::error::io_error(&path, error))?;
    Ok(())
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

fn remove_file_if_exists(path: &Path) -> CliResult<bool> {
    if path.exists() {
        fs::remove_file(path).map_err(|error| crate::error::io_error(path, error))?;
        return Ok(true);
    }
    Ok(false)
}

fn remove_dir_if_exists(path: &Path) -> CliResult<bool> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|error| crate::error::io_error(path, error))?;
        return Ok(true);
    }
    Ok(false)
}

fn setup_catalog_files(root: &Path, force: bool, rows: &mut Vec<Value>) -> CliResult<()> {
    rows.push(write_template_if_missing(
        &root.join("tools.toml"),
        DEFAULT_TOOLS_TEMPLATE,
        force,
        "tools",
    )?);
    rows.push(write_template_if_missing(
        &root.join("mcp.json"),
        DEFAULT_MCP_TEMPLATE,
        force,
        "mcp",
    )?);
    for name in ["skills", "subagents"] {
        let path = root.join(name);
        fs::create_dir_all(&path).map_err(|error| crate::error::io_error(&path, error))?;
        rows.push(json!({"kind": "directory", "path": path, "status": "ready"}));
    }
    for path in write_default_subagent_presets(root, force)? {
        rows.push(json!({"kind": "subagent", "path": path, "status": "ready"}));
    }
    Ok(())
}

fn setup_config_file(config: &CliConfig, scope: ConfigScope, force: bool) -> CliResult<Value> {
    let root = match scope {
        ConfigScope::Global => &config.global_dir,
        ConfigScope::Project => &config.project_dir,
    };
    let path = root.join("config.toml");
    if path.exists() && !force {
        return Ok(
            json!({"kind": "config", "scope": scope_name(scope), "path": path, "status": "exists"}),
        );
    }
    let path = init_config_file(config, scope, force)?;
    Ok(json!({"kind": "config", "scope": scope_name(scope), "path": path, "status": "ready"}))
}

const fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "global",
        ConfigScope::Project => "project",
    }
}

fn write_template_if_missing(
    path: &Path,
    content: &str,
    force: bool,
    kind: &str,
) -> CliResult<Value> {
    if path.exists() && !force {
        return Ok(json!({"kind": kind, "path": path, "status": "exists"}));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| crate::error::io_error(parent, error))?;
    }
    fs::write(path, content).map_err(|error| crate::error::io_error(path, error))?;
    Ok(json!({"kind": kind, "path": path, "status": "ready"}))
}

fn render_json_lines<T: serde::Serialize>(items: &[T]) -> CliResult<String> {
    items
        .iter()
        .map(|item| serde_json::to_string(item).map(|line| format!("{line}\n")))
        .collect::<Result<String, _>>()
        .map_err(CliError::from)
}

fn provider_ready(provider: &crate::config::ProviderConfig) -> bool {
    provider.enabled
        && provider.api_key_env.as_deref().is_some_and(|name| {
            let name = name.trim();
            !name.is_empty() && std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
        })
}

#[derive(Clone, Debug)]
struct WorktreeResolution {
    git_root: PathBuf,
    path: PathBuf,
    branch: String,
    resumed: bool,
}

impl CliService {
    fn resolve_worktree(&self, command: &RunCommand) -> CliResult<Option<WorktreeResolution>> {
        if command.worktree.is_none() && command.worktree_name.is_none() && command.branch.is_none()
        {
            return Ok(None);
        }
        let git_root = git_root(&self.config.workspace_root)?;
        let branch = command
            .branch
            .clone()
            .unwrap_or_else(default_worktree_branch);
        let worktree_name = command
            .worktree_name
            .clone()
            .or_else(|| command.worktree.as_ref().and_then(explicit_flag_value))
            .unwrap_or_else(|| branch.clone());
        let path = worktree_path(&self.config.global_dir, &git_root, &worktree_name);
        let resumed = path.exists();
        let group_dir = path.parent().unwrap_or(&self.config.global_dir);
        fs::create_dir_all(group_dir).map_err(|error| crate::error::io_error(group_dir, error))?;
        write_worktree_group_metadata(group_dir, &git_root)?;
        if !resumed {
            let status = Command::new("git")
                .arg("worktree")
                .arg("add")
                .arg("-b")
                .arg(&branch)
                .arg(&path)
                .current_dir(&git_root)
                .status()
                .map_err(|error| CliError::Run(error.to_string()))?;
            if !status.success() {
                return Err(CliError::Run(format!(
                    "git worktree add failed with status {status}"
                )));
            }
        }
        Ok(Some(WorktreeResolution {
            git_root,
            path,
            branch,
            resumed,
        }))
    }
}

fn git_root(workspace_root: &Path) -> CliResult<PathBuf> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(workspace_root)
        .output()
        .map_err(|error| CliError::Run(error.to_string()))?;
    if !output.status.success() {
        return Err(CliError::Usage(
            "--worktree/--branch requires a git repository workspace".to_string(),
        ));
    }
    let root =
        String::from_utf8(output.stdout).map_err(|error| CliError::Run(error.to_string()))?;
    Ok(PathBuf::from(root.trim()))
}

fn default_worktree_branch() -> String {
    format!("yaacli/{}", Utc::now().format("%Y%m%d-%H%M%S"))
}

fn worktree_path(global_dir: &Path, git_root: &Path, name: &str) -> PathBuf {
    global_dir
        .join("worktrees")
        .join(project_hash(git_root))
        .join(sanitize_worktree_name(name))
}

fn write_worktree_group_metadata(group_dir: &Path, git_root: &Path) -> CliResult<()> {
    let path = group_dir.join("metadata.json");
    if path.exists() {
        return Ok(());
    }
    let value = json!({
        "git_root": git_root.display().to_string(),
        "created_at": Utc::now(),
    });
    fs::write(&path, serde_json::to_vec_pretty(&value)?)
        .map_err(|error| crate::error::io_error(&path, error))?;
    Ok(())
}

fn project_hash(path: &Path) -> String {
    let digest = digest::digest(&digest::SHA256, path.display().to_string().as_bytes());
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn sanitize_worktree_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn explicit_flag_value(value: &String) -> Option<String> {
    (value != "true").then(|| value.clone())
}

fn apply_yaacli_run_metadata(
    run: &mut RunRecord,
    command: &RunCommand,
    worktree: Option<&WorktreeResolution>,
) {
    if command.worker.is_some() || command.worker_label.is_some() {
        run.metadata
            .insert("cli.yaacli.worker_enabled".to_string(), json!(true));
    }
    let worker_label = command
        .worker_label
        .as_deref()
        .map(ToString::to_string)
        .or_else(|| command.worker.as_ref().and_then(explicit_flag_value));
    if let Some(worker) = worker_label {
        run.metadata
            .insert("cli.yaacli.worker".to_string(), json!(worker));
    }
    if let Some(worktree) = worktree {
        run.metadata.insert(
            "cli.yaacli.worktree".to_string(),
            json!(worktree.path.display().to_string()),
        );
        run.metadata.insert(
            "cli.yaacli.worktree_git_root".to_string(),
            json!(worktree.git_root.display().to_string()),
        );
        run.metadata.insert(
            "cli.yaacli.worktree_resumed".to_string(),
            json!(worktree.resumed),
        );
        run.metadata
            .insert("cli.yaacli.branch".to_string(), json!(worktree.branch));
    } else {
        let worktree_label = command
            .worktree_name
            .as_deref()
            .map(ToString::to_string)
            .or_else(|| command.worktree.as_ref().and_then(explicit_flag_value));
        if command.worktree.is_some() || command.worktree_name.is_some() {
            run.metadata
                .insert("cli.yaacli.worktree_enabled".to_string(), json!(true));
        }
        if let Some(worktree) = worktree_label {
            run.metadata
                .insert("cli.yaacli.worktree".to_string(), json!(worktree));
        }
        if let Some(branch) = command.branch.as_ref() {
            run.metadata
                .insert("cli.yaacli.branch".to_string(), json!(branch));
        }
    }
}

const fn run_status_name(status: starweaver_session::RunStatus) -> &'static str {
    match status {
        starweaver_session::RunStatus::Queued => "queued",
        starweaver_session::RunStatus::Running => "running",
        starweaver_session::RunStatus::Waiting => "waiting",
        starweaver_session::RunStatus::Completed => "completed",
        starweaver_session::RunStatus::Failed => "failed",
        starweaver_session::RunStatus::Cancelled => "cancelled",
    }
}

fn parse_duration(value: &str) -> CliResult<chrono::Duration> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::Usage("duration cannot be empty".to_string()));
    }
    let (number, unit) = trimmed.split_at(
        trimmed
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let amount = number
        .parse::<i64>()
        .map_err(|error| CliError::Usage(error.to_string()))?;
    let duration = match unit {
        "" | "s" | "sec" | "secs" => chrono::Duration::seconds(amount),
        "m" | "min" | "mins" => chrono::Duration::minutes(amount),
        "h" | "hr" | "hrs" => chrono::Duration::hours(amount),
        "d" | "day" | "days" => chrono::Duration::days(amount),
        other => return Err(CliError::Usage(format!("unknown duration unit: {other}"))),
    };
    Ok(duration)
}

fn render_sessions(sessions: &[SessionSummary], output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for session in sessions {
                let _ = writeln!(
                    lines,
                    "{} profile={} runs={} status={}",
                    session.session_id,
                    session.profile.as_deref().unwrap_or_default(),
                    session.run_count,
                    session.status
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => sessions
            .iter()
            .map(|session| serde_json::to_string(session).map(|line| format!("{line}\n")))
            .collect::<Result<String, _>>()
            .map_err(CliError::from),
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"sessions": sessions, "status": "list"}))?
        )),
        OutputMode::Silent => Ok(format!("sessions={}\nstatus=list\n", sessions.len())),
    }
}

fn render_session_show(
    session: &Value,
    runs: &[RunSummary],
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = format!(
                "session_id={} profile={} status={}\n",
                session["session_id"].as_str().unwrap_or_default(),
                session["profile"].as_str().unwrap_or_default(),
                session["status"].as_str().unwrap_or_default()
            );
            for run in runs {
                let _ = writeln!(
                    lines,
                    "run_id={} sequence={} status={} preview={}",
                    run.run_id,
                    run.sequence_no,
                    run.status,
                    run.output_preview.as_deref().unwrap_or_default()
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => {
            let mut lines = String::new();
            lines.push_str(&serde_json::to_string(session)?);
            lines.push('\n');
            for run in runs {
                lines.push_str(&serde_json::to_string(run)?);
                lines.push('\n');
            }
            Ok(lines)
        }
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"session": session, "runs": runs}))?
        )),
        OutputMode::Silent => Ok(format!(
            "session_id={}\nruns={}\nstatus=shown\n",
            session["session_id"].as_str().unwrap_or_default(),
            runs.len()
        )),
    }
}

fn render_display_jsonl(messages: &[DisplayMessage]) -> CliResult<String> {
    messages
        .iter()
        .map(DisplayMessage::to_jsonl_line)
        .collect::<Result<String, _>>()
        .map_err(CliError::from)
}

fn render_agui_jsonl(messages: &[DisplayMessage]) -> CliResult<String> {
    messages
        .iter()
        .filter_map(display_message_to_agui_event)
        .map(|event| serde_json::to_string(&event).map(|line| format!("{line}\n")))
        .collect::<Result<String, _>>()
        .map_err(CliError::from)
}

fn render_prompt_run_json(execution: &PromptRunExecution) -> CliResult<String> {
    let output_preview = execution
        .messages
        .iter()
        .rev()
        .find_map(|message| {
            message
                .payload
                .get("output")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| message.preview.clone())
        })
        .unwrap_or_default();
    let latest_sequence = execution.messages.last().map(|message| message.sequence);
    Ok(format!(
        "{}\n",
        serde_json::to_string(&json!({
            "sessionId": execution.session_id,
            "runId": execution.run_id,
            "status": execution.status,
            "outputPreview": output_preview,
            "latestCursor": latest_sequence.map(|sequence| json!({
                "scope": format!("run:{}", execution.run_id),
                "sequence": sequence,
            })),
        }))?
    ))
}

#[allow(clippy::too_many_lines)]
fn display_message_to_agui_event(message: &DisplayMessage) -> Option<Value> {
    let mut event = match message.kind {
        DisplayMessageKind::RunQueued => {
            let value = json!({"sequence_no": message.payload.get("sequence_no").cloned()});
            custom_agui_event("yaacli.run_queued", message, &value)
        }
        DisplayMessageKind::RunStarted => json!({
            "type": "RUN_STARTED",
            "threadId": message.session_id.as_str(),
            "runId": message.run_id.as_str(),
        }),
        DisplayMessageKind::AssistantTextStart => {
            if is_reasoning_message(message) {
                json!({
                    "type": "REASONING_MESSAGE_START",
                    "messageId": message_id(message),
                    "role": "reasoning",
                })
            } else {
                json!({
                    "type": "TEXT_MESSAGE_START",
                    "messageId": message_id(message),
                    "role": message.payload.get("role").and_then(Value::as_str).unwrap_or("assistant"),
                    "name": message.agent_name,
                })
            }
        }
        DisplayMessageKind::AssistantTextDelta => {
            if is_reasoning_message(message) {
                json!({
                    "type": "REASONING_MESSAGE_CHUNK",
                    "messageId": message_id(message),
                    "delta": message_delta(message),
                })
            } else {
                json!({
                    "type": "TEXT_MESSAGE_CHUNK",
                    "messageId": message_id(message),
                    "role": "assistant",
                    "name": message.agent_name,
                    "delta": message_delta(message),
                })
            }
        }
        DisplayMessageKind::AssistantTextEnd => {
            if is_reasoning_message(message) {
                json!({
                    "type": "REASONING_MESSAGE_END",
                    "messageId": message_id(message),
                })
            } else {
                json!({
                    "type": "TEXT_MESSAGE_END",
                    "messageId": message_id(message),
                })
            }
        }
        DisplayMessageKind::ToolCallStart => json!({
            "type": "TOOL_CALL_START",
            "toolCallId": tool_call_id(message),
            "toolCallName": tool_call_name(message),
            "parentMessageId": message.payload.get("parent_message_id").cloned(),
        }),
        DisplayMessageKind::ToolCallDelta => json!({
            "type": "TOOL_CALL_CHUNK",
            "toolCallId": tool_call_id(message),
            "toolCallName": tool_call_name(message),
            "delta": message_delta(message),
        }),
        DisplayMessageKind::ToolCallEnd => json!({
            "type": "TOOL_CALL_END",
            "toolCallId": tool_call_id(message),
        }),
        DisplayMessageKind::ToolResult => json!({
            "type": "TOOL_CALL_RESULT",
            "messageId": format!("{}:result", tool_call_id(message)),
            "toolCallId": tool_call_id(message),
            "toolCallName": tool_call_name(message),
            "content": message.payload.get("content").cloned().unwrap_or_else(|| json!(message.preview)),
            "role": "tool",
            "error": message.payload.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        }),
        DisplayMessageKind::RunCompleted => {
            let output = message
                .payload
                .get("output")
                .cloned()
                .or_else(|| message.preview.clone().map(Value::String));
            json!({
                "type": "RUN_FINISHED",
                "threadId": message.session_id.as_str(),
                "runId": message.run_id.as_str(),
                "result": output.map(|output| json!({"output_text": output})),
            })
        }
        DisplayMessageKind::RunFailed => json!({
            "type": "RUN_ERROR",
            "message": message.preview.as_deref().unwrap_or("run failed"),
            "code": message.payload.get("code").and_then(Value::as_str),
        }),
        DisplayMessageKind::RunCancelled => {
            custom_agui_event("yaacli.run_cancelled", message, &message.payload)
        }
        DisplayMessageKind::ApprovalRequested
        | DisplayMessageKind::ApprovalResolved
        | DisplayMessageKind::Checkpoint
        | DisplayMessageKind::SubagentStarted
        | DisplayMessageKind::SubagentCompleted
        | DisplayMessageKind::CompactionStarted
        | DisplayMessageKind::CompactionCompleted => custom_agui_event(
            display_extension_name(message.kind),
            message,
            &message.payload,
        ),
    };
    strip_null_object_fields(&mut event);
    event.as_object_mut().map(|object| {
        object.insert(
            "timestamp".to_string(),
            json!(message.timestamp.timestamp_millis()),
        );
        object.insert("starweaverSequence".to_string(), json!(message.sequence));
    })?;
    Some(event)
}

fn custom_agui_event(name: &str, message: &DisplayMessage, value: &Value) -> Value {
    json!({
        "type": "CUSTOM",
        "name": name,
        "value": {
            "run_id": message.run_id.as_str(),
            "session_id": message.session_id.as_str(),
            "payload": value,
            "preview": message.preview,
        }
    })
}

const fn display_extension_name(kind: DisplayMessageKind) -> &'static str {
    match kind {
        DisplayMessageKind::ApprovalRequested => "yaacli.approval_requested",
        DisplayMessageKind::ApprovalResolved => "yaacli.approval_resolved",
        DisplayMessageKind::Checkpoint => "ya_agent.checkpoint",
        DisplayMessageKind::SubagentStarted => "ya_agent.subagent_started",
        DisplayMessageKind::SubagentCompleted => "ya_agent.subagent_completed",
        DisplayMessageKind::CompactionStarted => "ya_agent.compaction_started",
        DisplayMessageKind::CompactionCompleted => "ya_agent.compaction_completed",
        _ => "ya_agent.display_message",
    }
}

fn message_id(message: &DisplayMessage) -> String {
    message
        .payload
        .get("message_id")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("{}:message:{}", message.run_id.as_str(), message.sequence),
            ToString::to_string,
        )
}

fn tool_call_id(message: &DisplayMessage) -> String {
    message
        .payload
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("{}:tool:{}", message.run_id.as_str(), message.sequence),
            ToString::to_string,
        )
}

fn tool_call_name(message: &DisplayMessage) -> Option<String> {
    message
        .payload
        .get("tool_name")
        .or_else(|| message.payload.get("name"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn message_delta(message: &DisplayMessage) -> String {
    message
        .payload
        .get("delta")
        .and_then(Value::as_str)
        .or(message.preview.as_deref())
        .unwrap_or_default()
        .to_string()
}

fn is_reasoning_message(message: &DisplayMessage) -> bool {
    message.payload.get("part_kind").and_then(Value::as_str) == Some("thinking")
        || message
            .metadata
            .get("reasoning")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn strip_null_object_fields(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
}

fn render_display_text(messages: &[DisplayMessage]) -> String {
    let mut output = String::new();
    let mut last_was_text = false;
    for message in messages {
        match message.kind {
            DisplayMessageKind::AssistantTextDelta => {
                if let Some(delta) = message.payload.get("delta").and_then(Value::as_str) {
                    if message.payload.get("part_kind").and_then(Value::as_str) == Some("thinking")
                        || message
                            .metadata
                            .get("reasoning")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    {
                        if last_was_text && !output.ends_with('\n') {
                            output.push('\n');
                        }
                        let _ = writeln!(output, "thinking={delta}");
                        last_was_text = false;
                    } else {
                        output.push_str(delta);
                        last_was_text = true;
                    }
                }
            }
            DisplayMessageKind::ToolCallStart => {
                if last_was_text && !output.ends_with('\n') {
                    output.push('\n');
                }
                let _ = writeln!(
                    output,
                    "tool_call={}",
                    message
                        .payload
                        .get("name")
                        .or_else(|| message.payload.get("tool_name"))
                        .and_then(Value::as_str)
                        .or(message.preview.as_deref())
                        .unwrap_or("tool")
                );
                last_was_text = false;
            }
            DisplayMessageKind::ToolResult => {
                if let Some(preview) = message.preview.as_deref() {
                    let _ = writeln!(output, "tool_result={preview}");
                }
                last_was_text = false;
            }
            DisplayMessageKind::ApprovalRequested => {
                if last_was_text && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("approval=requested\n");
                last_was_text = false;
            }
            DisplayMessageKind::RunFailed => {
                if last_was_text && !output.ends_with('\n') {
                    output.push('\n');
                }
                let preview = message.preview.as_deref().unwrap_or("run failed");
                let _ = writeln!(output, "status=failed message={preview}");
                last_was_text = false;
            }
            _ => {}
        }
    }
    if last_was_text && !output.ends_with('\n') {
        output.push('\n');
    }
    if output.is_empty() {
        if let Some(message) = messages
            .iter()
            .rev()
            .find(|message| message.kind.is_terminal())
        {
            let _ = writeln!(
                output,
                "status={}",
                match message.kind {
                    DisplayMessageKind::RunCompleted => "completed",
                    DisplayMessageKind::RunFailed => "failed",
                    DisplayMessageKind::RunCancelled => "cancelled",
                    _ => "unknown",
                }
            );
        }
    }
    output
}

fn render_completion(shell: Shell) -> CliResult<String> {
    let mut command = crate::args::command();
    let mut buffer = Vec::new();
    clap_complete::generate(shell, &mut command, "starweaver-cli", &mut buffer);
    String::from_utf8(buffer).map_err(|error| CliError::Run(error.to_string()))
}

fn render_approvals(approvals: &[ApprovalRecord], output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for approval in approvals {
                let _ = writeln!(
                    lines,
                    "approval_id={} run_id={} action={} status={}",
                    approval.approval_id,
                    approval.run_id.as_str(),
                    approval.action_name,
                    approval_status_name(approval.status)
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => render_json_lines(approvals),
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"approvals": approvals, "status": "list"}))?
        )),
        OutputMode::Silent => Ok(format!("approvals={}\nstatus=list\n", approvals.len())),
    }
}

fn render_deferred(records: &[DeferredToolRecord], output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for record in records {
                let _ = writeln!(
                    lines,
                    "deferred_id={} run_id={} tool={} status={}",
                    record.deferred_id,
                    record.run_id.as_str(),
                    record.tool_name,
                    execution_status_name(record.status)
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => render_json_lines(records),
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"deferred": records, "status": "list"}))?
        )),
        OutputMode::Silent => Ok(format!("deferred={}\nstatus=list\n", records.len())),
    }
}

fn render_deferred_decision(record: &DeferredToolRecord, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => Ok(format!(
            "deferred_id={}\nstatus={}\nrun_id={}\n",
            record.deferred_id,
            execution_status_name(record.status),
            record.run_id.as_str()
        )),
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => {
            Ok(format!("{}\n", serde_json::to_string(record)?))
        }
        OutputMode::Silent => Ok(format!(
            "deferred_id={}\nstatus={}\n",
            record.deferred_id,
            execution_status_name(record.status)
        )),
    }
}

const fn approval_status_name(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Expired => "expired",
        ApprovalStatus::Cancelled => "cancelled",
    }
}

const fn execution_status_name(status: starweaver_session::ExecutionStatus) -> &'static str {
    match status {
        starweaver_session::ExecutionStatus::Pending => "pending",
        starweaver_session::ExecutionStatus::Running => "running",
        starweaver_session::ExecutionStatus::Waiting => "waiting",
        starweaver_session::ExecutionStatus::Completed => "completed",
        starweaver_session::ExecutionStatus::Failed => "failed",
        starweaver_session::ExecutionStatus::Cancelled => "cancelled",
    }
}

fn render_session_delete(session_id: &str, deleted: bool, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => Ok(format!(
            "session_id={session_id}\ndeleted={deleted}\nstatus=deleted\n"
        )),
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({
                "session_id": session_id,
                "deleted": deleted,
                "status": "deleted"
            }))?
        )),
        OutputMode::Silent => Ok(format!("session_id={session_id}\nstatus=deleted\n")),
    }
}

fn render_trim_report(report: &TrimReport, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => Ok(format!(
            "sessions_scanned={} runs_to_trim={} runs_trimmed={} bytes_reclaimed={} dry_run={}\n",
            report.sessions_scanned,
            report.runs_to_trim,
            report.runs_trimmed,
            report.bytes_reclaimed,
            report.dry_run
        )),
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => {
            Ok(format!("{}\n", serde_json::to_string(report)?))
        }
        OutputMode::Silent => Ok(format!(
            "sessions_scanned={}\nruns_to_trim={}\nruns_trimmed={}\nbytes_reclaimed={}\ndry_run={}\nstatus=trimmed\n",
            report.sessions_scanned,
            report.runs_to_trim,
            report.runs_trimmed,
            report.bytes_reclaimed,
            report.dry_run
        )),
    }
}

fn session_value(session: &starweaver_session::SessionRecord) -> Value {
    json!({
        "session_id": session.session_id.as_str(),
        "title": session.title,
        "profile": session.profile,
        "status": format!("{:?}", session.status).to_lowercase(),
        "head_run_id": session.head_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "head_success_run_id": session.head_success_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "active_run_id": session.active_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "created_at": session.created_at.to_rfc3339(),
        "updated_at": session.updated_at.to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::Utc;
    use serde_json::json;
    use starweaver_core::{RunId, SessionId};
    use starweaver_session::{ExecutionStatus, RunStatus};

    use super::*;

    fn ids() -> (SessionId, RunId) {
        (
            SessionId::from_string("session_test"),
            RunId::from_string("run_test"),
        )
    }

    #[test]
    fn render_helpers_cover_text_silent_and_json_modes() {
        let sessions = vec![SessionSummary {
            session_id: "session_test".to_string(),
            title: Some("Title".to_string()),
            profile: Some("general".to_string()),
            status: "active".to_string(),
            head_run_id: Some("run_test".to_string()),
            head_success_run_id: Some("run_test".to_string()),
            active_run_id: None,
            run_count: 1,
            last_output_preview: Some("preview".to_string()),
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }];
        assert!(render_sessions(&sessions, OutputMode::Text)
            .unwrap()
            .contains("profile=general"));
        assert_eq!(
            render_sessions(&sessions, OutputMode::Silent).unwrap(),
            "sessions=1\nstatus=list\n"
        );

        let session = json!({"session_id":"session_test","profile":"general","status":"active"});
        let runs = vec![RunSummary {
            run_id: "run_test".to_string(),
            sequence_no: 1,
            status: "completed".to_string(),
            restore_from_run_id: None,
            output_preview: Some("hello".to_string()),
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }];
        assert!(render_session_show(&session, &runs, OutputMode::Text)
            .unwrap()
            .contains("preview=hello"));
        assert!(
            render_session_show(&session, &runs, OutputMode::DisplayJsonl)
                .unwrap()
                .contains("run_test")
        );
        assert!(render_session_show(&session, &runs, OutputMode::Silent)
            .unwrap()
            .contains("status=shown"));

        let report = TrimReport {
            sessions_scanned: 1,
            runs_to_trim: 2,
            runs_trimmed: 1,
            bytes_reclaimed: 3,
            dry_run: false,
        };
        assert!(render_trim_report(&report, OutputMode::Text)
            .unwrap()
            .contains("bytes_reclaimed=3"));
        assert!(render_trim_report(&report, OutputMode::Silent)
            .unwrap()
            .contains("status=trimmed"));
    }

    #[test]
    fn display_and_control_renderers_cover_edge_branches() {
        let (session_id, run_id) = ids();
        let messages = vec![
            DisplayMessage::new(
                0,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::AssistantTextDelta,
            )
            .with_payload(json!({"delta":"hello"})),
            DisplayMessage::new(
                1,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::ToolCallStart,
            )
            .with_payload(json!({"name":"lookup"})),
            DisplayMessage::new(
                2,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::ToolResult,
            )
            .with_preview("ok"),
            DisplayMessage::new(
                3,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::ApprovalRequested,
            ),
            DisplayMessage::new(
                4,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::RunFailed,
            )
            .with_preview("boom"),
        ];
        let text = render_display_text(&messages);
        assert!(text.contains("hello"));
        assert!(text.contains("tool_call=lookup"));
        assert!(text.contains("tool_result=ok"));
        assert!(text.contains("approval=requested"));
        assert!(text.contains("status=failed message=boom"));
        let terminal_only = vec![DisplayMessage::new(
            0,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )];
        assert_eq!(render_display_text(&terminal_only), "status=completed\n");
        assert!(render_display_jsonl(&terminal_only)
            .unwrap()
            .contains("RUN_FINISHED"));

        let mut approval = ApprovalRecord::new(
            "approval_test",
            session_id.clone(),
            run_id.clone(),
            "action_test",
            "write",
        );
        approval.status = ApprovalStatus::Expired;
        assert!(render_approvals(&[approval.clone()], OutputMode::Text)
            .unwrap()
            .contains("status=expired"));
        approval.status = ApprovalStatus::Cancelled;
        assert!(render_approvals(&[approval], OutputMode::Silent)
            .unwrap()
            .contains("approvals=1"));

        let mut deferred = DeferredToolRecord::new(
            "deferred_test",
            session_id,
            run_id,
            "tool_call_test",
            "worker",
        );
        for status in [
            ExecutionStatus::Pending,
            ExecutionStatus::Running,
            ExecutionStatus::Waiting,
            ExecutionStatus::Completed,
            ExecutionStatus::Failed,
            ExecutionStatus::Cancelled,
        ] {
            deferred.status = status;
            assert!(render_deferred(&[deferred.clone()], OutputMode::Text)
                .unwrap()
                .contains("deferred_id=deferred_test"));
            assert!(
                render_deferred_decision(&deferred, OutputMode::DisplayJsonl)
                    .unwrap()
                    .contains("deferred_test")
            );
        }
    }

    #[test]
    fn tui_model_choices_filter_internal_profiles_and_keep_config_details() {
        let temp = tempfile::tempdir().unwrap();
        let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = crate::ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let choices = model_choices(&config);

        assert!(choices.iter().any(|choice| choice.profile == "coding"));
        assert!(!choices
            .iter()
            .any(|choice| choice.profile == "approval_model"));
        assert!(!choices
            .iter()
            .any(|choice| choice.profile == "deferred_model"));

        let coding = choices
            .iter()
            .find(|choice| choice.profile == "coding")
            .unwrap();
        assert_eq!(coding.model_id, "openai:gpt-5");
        assert_eq!(
            coding.model_settings.as_deref(),
            Some("openai_responses_medium")
        );
        assert_eq!(coding.model_cfg.as_deref(), Some("gpt5_270k"));
        assert_eq!(coding.context_window, Some(270_000));
    }

    #[test]
    fn duration_and_status_helpers_cover_errors() {
        assert_eq!(parse_duration("10s").unwrap().num_seconds(), 10);
        assert_eq!(parse_duration("2m").unwrap().num_seconds(), 120);
        assert_eq!(parse_duration("1h").unwrap().num_seconds(), 3600);
        assert_eq!(parse_duration("1d").unwrap().num_seconds(), 86_400);
        assert!(parse_duration("").is_err());
        assert!(parse_duration("1w").is_err());
        for status in [
            RunStatus::Queued,
            RunStatus::Running,
            RunStatus::Waiting,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Cancelled,
        ] {
            assert!(!run_status_name(status).is_empty());
        }
        for status in [
            ApprovalStatus::Pending,
            ApprovalStatus::Approved,
            ApprovalStatus::Denied,
            ApprovalStatus::Expired,
            ApprovalStatus::Cancelled,
        ] {
            assert!(!approval_status_name(status).is_empty());
        }
    }
}
