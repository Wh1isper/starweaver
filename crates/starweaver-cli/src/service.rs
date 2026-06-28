//! CLI service layer over local storage and SDK execution.
#![allow(clippy::redundant_pub_crate)]

use std::{fs, path::Path, sync::mpsc, thread, time::Duration};

use serde_json::{json, Value};
use starweaver_agent::ResumableState;
use starweaver_core::sdk_name;
use starweaver_oauth_provider::create_oauth_refresh_supervisor_for_models_with_options;
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{ApprovalStatus, RunRecord, RunStatus};
use starweaver_stream::{DisplayMessage, RealtimeCompactionBuffer, ReplayScope};

use crate::{
    args::{
        ApprovalCommand, ApprovalDecisionCommand, ApprovalListCommand, Cli, CliCommand,
        DeferredCommand, DeferredCompleteCommand, DeferredFailCommand, DeferredListCommand,
        OutputMode, ResumeCommand, RunCommand, SessionCommand, TuiCommand,
    },
    config::{
        read_current_session, read_last_retention_maintenance, write_current_session,
        write_last_retention_maintenance, CliConfig,
    },
    environment::{
        resolve_environment_for_session_with_attachments, validate_environment_config,
        ResolvedEnvironment,
    },
    local_store::LocalStore,
    profiles::{list_profiles, resolve_profile, ResolvedProfile},
    prompt_input::PromptInput,
    runner::{
        execute_agent_session, execute_agent_session_with_channels, failed_display_message,
        CliRunPolicy, CliSteeringMessage,
    },
    slash_commands::expand_slash_command,
    CliError, CliResult,
};

mod auth;
mod catalog;
mod rendering;
mod setup;
mod tui;
mod worktree;

use auth::oauth_cli_error;
use rendering::{
    approval_status_name, render_agui_jsonl, render_approvals, render_completion, render_deferred,
    render_deferred_decision, render_display_jsonl, render_display_text, render_prompt_run_json,
    render_session_delete, render_session_show, render_sessions, render_trim_report, session_value,
};
use setup::remove_file_if_exists;
#[cfg(test)]
use tui::model_choices;
use worktree::apply_starweaver_run_metadata;

pub(super) struct PromptRunExecution {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) status: String,
    pub(super) output_mode: OutputMode,
    pub(super) messages: Vec<DisplayMessage>,
}

pub(super) struct PreparedPromptRun {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) output_mode: OutputMode,
    pub(super) run: RunRecord,
    run_input: PromptInput,
    resolved_profile: ResolvedProfile,
    pub(super) environment: ResolvedEnvironment,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
}

pub(super) struct ExecutedPromptRun {
    run: RunRecord,
    output_mode: OutputMode,
    execution: crate::runner::CliRunExecution,
}

impl ExecutedPromptRun {
    pub(super) fn merge_display_message_inserts(
        &mut self,
        mut inserts: Vec<(usize, DisplayMessage)>,
    ) {
        if inserts.is_empty() {
            return;
        }
        inserts.sort_by_key(|(index, message)| (*index, message.sequence));
        let mut messages = self.execution.artifacts.display_messages.clone();
        for (offset, (index, message)) in inserts.into_iter().enumerate() {
            let position = index.saturating_add(offset).min(messages.len());
            messages.insert(position, message);
        }
        for (sequence, message) in messages.iter_mut().enumerate() {
            message.sequence = sequence;
        }
        let mut buffer = RealtimeCompactionBuffer::new(ReplayScope::run(self.run.run_id.as_str()));
        for message in messages.clone() {
            buffer.push(message);
        }
        self.execution.artifacts.display_snapshot = buffer.snapshot();
        self.execution.artifacts.display_messages = messages;
    }
}

const PROJECT_GUIDANCE_TAG: &str = "project-guidance";
const USER_RULES_TAG: &str = "user-rules";

fn append_guidance_files(input: &mut PromptInput, config: &CliConfig) {
    if let Some(project_guidance) = load_project_guidance(&config.workspace_root) {
        input.push_guidance_text_part(project_guidance);
    }
    if let Some(user_rules) = load_user_rules(&config.global_dir) {
        input.push_guidance_text_part(user_rules);
    }
}

fn load_project_guidance(workspace_root: &Path) -> Option<String> {
    let path = workspace_root.join("AGENTS.md");
    let content = read_non_empty_utf8_file(&path)?;
    Some(format!(
        "<{PROJECT_GUIDANCE_TAG} name=AGENTS.md>\n{content}\n</{PROJECT_GUIDANCE_TAG}>"
    ))
}

fn load_user_rules(global_dir: &Path) -> Option<String> {
    let path = global_dir.join("RULES.md");
    let content = read_non_empty_utf8_file(&path)?;
    Some(format!(
        "<{USER_RULES_TAG} location={}>\n{content}\n</{USER_RULES_TAG}>",
        path_absolute_posix(&path)
    ))
}

fn read_non_empty_utf8_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    (!content.trim().is_empty()).then_some(content)
}

fn path_absolute_posix(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
        .replace('\\', "/")
}

#[allow(dead_code)]
struct OAuthRefreshGuard {
    stop_sender: mpsc::Sender<()>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for OAuthRefreshGuard {
    fn drop(&mut self) {
        let _ = self.stop_sender.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
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
                goal: None,
                worker: cli.worker.clone(),
                worker_label: cli.worker_label.clone(),
                worktree: cli.worktree.clone(),
                worktree_name: cli.worktree_name.clone(),
                branch: cli.branch,
                session_affinity_id: None,
                environment_attachments: Vec::new(),
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

    pub(crate) fn run_prompt(&mut self, command: &RunCommand) -> CliResult<String> {
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

    fn execute_prompt_run(
        &mut self,
        command: &RunCommand,
        stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
    ) -> CliResult<PromptRunExecution> {
        self.execute_prompt_run_with_channels(command, None, stream_sender, None, None)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_prompt_run_with_channels(
        &mut self,
        command: &RunCommand,
        prompt_input: Option<PromptInput>,
        stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
        steering_receiver: Option<mpsc::Receiver<CliSteeringMessage>>,
        cancel_receiver: Option<mpsc::Receiver<()>>,
    ) -> CliResult<PromptRunExecution> {
        let prepared = self.prepare_prompt_run(command, prompt_input)?;
        let run_on_error = prepared.run.clone();
        let executed = match Self::run_prepared_prompt(
            prepared,
            stream_sender,
            steering_receiver,
            cancel_receiver,
        ) {
            Ok(executed) => executed,
            Err(error) => {
                self.fail_prepared_prompt_run(run_on_error, &error)?;
                return Err(error);
            }
        };
        self.complete_prompt_run(executed)
    }

    pub(super) fn prepare_prompt_run(
        &mut self,
        command: &RunCommand,
        prompt_input: Option<PromptInput>,
    ) -> CliResult<PreparedPromptRun> {
        let input =
            prompt_input.map_or_else(|| command.prompt_text().map(PromptInput::text), Ok)?;
        let raw_prompt = input.text.clone();
        let slash_expansion = expand_slash_command(&self.config.slash_commands, &raw_prompt);
        let prompt = slash_expansion
            .as_ref()
            .map_or(raw_prompt, |expanded| expanded.prompt.clone());
        let mut run_input = PromptInput {
            text: prompt.clone(),
            attachments: input.attachments,
            extra_text_parts: input.extra_text_parts,
            guidance_text_parts: input.guidance_text_parts,
        };
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
        validate_environment_config(&run_config)?;
        append_guidance_files(&mut run_input, &run_config);
        let (session_id, created) = self.resolve_session(command, &resolved_profile.name)?;
        let environment = resolve_environment_for_session_with_attachments(
            &run_config,
            &session_id,
            &command.environment_attachments,
        )?;
        let mut restore_from = command.run.clone().or_else(|| command.branch_from.clone());
        if restore_from.is_none() && !created {
            restore_from = self
                .store()?
                .load_session(&session_id)
                .ok()
                .and_then(|session| {
                    session
                        .active_run_id
                        .or(session.head_run_id)
                        .or(session.head_success_run_id)
                        .map(|run| run.as_str().to_string())
                });
        }
        let restore_state = self
            .store()?
            .load_restore_state(&session_id, restore_from.as_deref())?;
        let mut run =
            self.store()?
                .append_run(&session_id, prompt, restore_from, &resolved_profile.name)?;
        apply_starweaver_run_metadata(
            &mut run,
            command,
            worktree.as_ref(),
            slash_expansion.as_ref(),
        );
        if let Some(session_affinity_id) = command.session_affinity_id.as_deref() {
            run.metadata.insert(
                "starweaver.session_affinity_id".to_string(),
                json!(session_affinity_id),
            );
        }
        if !command.environment_attachments.is_empty() {
            run.metadata.insert(
                "starweaver.environment_attachments".to_string(),
                json!(command.environment_attachments),
            );
            run.metadata.insert(
                "starweaver.environment_attachment_ids".to_string(),
                json!(command
                    .environment_attachments
                    .iter()
                    .map(|attachment| attachment.id.as_str())
                    .collect::<Vec<_>>()),
            );
        }
        write_current_session(&self.config, &session_id)?;
        let hitl = command.hitl.unwrap_or(self.config.default_hitl);
        let goal = command
            .goal
            .as_ref()
            .map(|goal| crate::runner::CliGoalRunPolicy {
                objective: goal.objective.clone(),
                max_iterations: goal.max_iterations.max(1),
            });
        let output_mode = command.output.unwrap_or(self.config.default_output);
        Ok(PreparedPromptRun {
            session_id,
            run_id: run.run_id.as_str().to_string(),
            output_mode,
            run,
            run_input,
            resolved_profile,
            environment,
            restore_state,
            policy: CliRunPolicy { hitl, goal },
        })
    }

    pub(super) fn run_prepared_prompt(
        prepared: PreparedPromptRun,
        stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
        steering_receiver: Option<mpsc::Receiver<CliSteeringMessage>>,
        cancel_receiver: Option<mpsc::Receiver<()>>,
    ) -> CliResult<ExecutedPromptRun> {
        let PreparedPromptRun {
            output_mode,
            run,
            run_input,
            resolved_profile,
            environment,
            restore_state,
            policy,
            ..
        } = prepared;
        let result = if stream_sender.is_some()
            || steering_receiver.is_some()
            || cancel_receiver.is_some()
        {
            execute_agent_session_with_channels(
                run_input,
                &run,
                &resolved_profile,
                &environment.provider,
                environment.process_provider.as_ref(),
                restore_state,
                &policy,
                stream_sender,
                steering_receiver,
                cancel_receiver,
            )
        } else {
            execute_agent_session(
                run_input,
                &run,
                &resolved_profile,
                &environment.provider,
                environment.process_provider.as_ref(),
                restore_state,
                &policy,
            )
        };
        result.map(|execution| ExecutedPromptRun {
            run,
            output_mode,
            execution,
        })
    }

    pub(super) fn fail_prepared_prompt_run(
        &mut self,
        mut run: RunRecord,
        error: &CliError,
    ) -> CliResult<()> {
        let messages = failed_display_message(&run, &error.to_string());
        self.store()?
            .fail_run_with_messages(&mut run, error.to_string(), &messages)
    }

    pub(super) fn complete_prompt_run(
        &mut self,
        executed: ExecutedPromptRun,
    ) -> CliResult<PromptRunExecution> {
        let ExecutedPromptRun {
            mut run,
            output_mode,
            execution,
        } = executed;
        let execution_failed = execution.artifacts.status == RunStatus::Failed;
        let output = execution.output;
        let messages = self
            .store()?
            .complete_run(&mut run, output.clone(), execution.artifacts)?;
        if execution_failed && matches!(output_mode, OutputMode::Text | OutputMode::Silent) {
            return Err(CliError::Run(output));
        }
        self.run_retention_maintenance(run.session_id.as_str())?;
        Ok(PromptRunExecution {
            session_id: run.session_id.as_str().to_string(),
            run_id: run.run_id.as_str().to_string(),
            status: run_status_name(run.status).to_string(),
            output_mode,
            messages,
        })
    }

    fn run_retention_maintenance(&mut self, current_session_id: &str) -> CliResult<()> {
        if !self.config.auto_trim {
            return Ok(());
        }
        let current_keep_runs = self.config.current_session_keep_recent_runs;
        self.store()?.trim(
            vec![current_session_id.to_string()],
            current_keep_runs,
            false,
        )?;
        if !self.should_run_all_sessions_retention()? {
            return Ok(());
        }
        let sessions = self.store()?.all_session_ids()?;
        let all_sessions_keep_runs = self.config.all_sessions_keep_recent_runs;
        let older_than = chrono::Duration::days(
            i64::try_from(self.config.all_sessions_keep_days)
                .unwrap_or(i64::MAX)
                .max(0),
        );
        self.store()?
            .trim_with_age(sessions, all_sessions_keep_runs, Some(older_than), false)?;
        write_last_retention_maintenance(&self.config, chrono::Utc::now())?;
        Ok(())
    }

    fn should_run_all_sessions_retention(&self) -> CliResult<bool> {
        if self.config.all_sessions_keep_days == 0 || self.config.all_sessions_interval_hours == 0 {
            return Ok(false);
        }
        let Some(last_run) = read_last_retention_maintenance(&self.config)? else {
            return Ok(true);
        };
        let elapsed = chrono::Utc::now().signed_duration_since(last_run);
        Ok(elapsed
            >= chrono::Duration::hours(
                i64::try_from(self.config.all_sessions_interval_hours)
                    .unwrap_or(i64::MAX)
                    .max(0),
            ))
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
            goal: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
            session_affinity_id: None,
            environment_attachments: Vec::new(),
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
}

#[allow(dead_code)]
fn start_oauth_refresh_guard(config: &CliConfig) -> CliResult<Option<OAuthRefreshGuard>> {
    if !config.oauth_refresh.enabled {
        return Ok(None);
    }
    let models = list_profiles(config)
        .into_iter()
        .map(|profile| profile.model_id)
        .collect::<Vec<_>>();
    if config.oauth_refresh.interval_seconds == 0 {
        return Err(CliError::Usage(
            "invalid oauth_refresh.interval_seconds: value must be positive".to_string(),
        ));
    }
    if config.oauth_refresh.failure_retry_seconds == 0 {
        return Err(CliError::Usage(
            "invalid oauth_refresh.failure_retry_seconds: value must be positive".to_string(),
        ));
    }
    let mut supervisor = create_oauth_refresh_supervisor_for_models_with_options(
        models.iter().map(String::as_str),
        Duration::from_secs(config.oauth_refresh.interval_seconds),
        Duration::from_secs(config.oauth_refresh.failure_retry_seconds),
        config.oauth_refresh.refresh_on_startup,
    )
    .map_err(oauth_cli_error)?;
    let Some(mut supervisor) = supervisor.take() else {
        return Ok(None);
    };
    let (stop_sender, stop_receiver) = mpsc::channel::<()>();
    let handle = thread::Builder::new()
        .name("starweaver-oauth-refresh".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                supervisor.start().await;
                let _ = tokio::task::spawn_blocking(move || stop_receiver.recv()).await;
                supervisor.shutdown().await;
            });
        })
        .map_err(|error| CliError::Run(error.to_string()))?;
    Ok(Some(OAuthRefreshGuard {
        stop_sender,
        handle: Some(handle),
    }))
}

fn render_json_lines<T: serde::Serialize>(items: &[T]) -> CliResult<String> {
    items
        .iter()
        .map(|item| serde_json::to_string(item).map(|line| format!("{line}\n")))
        .collect::<Result<String, _>>()
        .map_err(CliError::from)
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

#[cfg(test)]
mod tests;
