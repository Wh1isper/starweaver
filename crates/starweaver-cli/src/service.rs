//! CLI service layer over local storage and SDK execution.

use std::fmt::Write as _;

use clap_complete::Shell;
use serde_json::{json, Value};
use starweaver_core::sdk_name;
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

use crate::{
    args::{
        Cli, CliCommand, ConfigCommand, OutputMode, ProfileCommand, RunCommand, SessionCommand,
        UpdateCommand,
    },
    config::{
        get_config_value, init_config_file, read_current_session, write_current_session, CliConfig,
        ConfigScope,
    },
    environment::resolve_environment,
    local_store::{LocalStore, RunSummary, SessionSummary, TrimReport},
    profiles::{list_profiles, resolve_profile, show_profile},
    runner::{execute_agent_session, failed_display_message, CliRunPolicy},
    CliError, CliResult,
};

/// CLI service.
pub struct CliService {
    config: CliConfig,
    store: LocalStore,
}

impl CliService {
    /// Open service from resolved config.
    pub fn open(config: CliConfig) -> CliResult<Self> {
        let store = LocalStore::open(&config)?;
        Ok(Self { config, store })
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
            };
            return self.run_prompt(&command);
        }
        match cli.command.unwrap_or(CliCommand::Version) {
            CliCommand::Version => Ok(format!("{}\n", sdk_name())),
            CliCommand::Diagnostics => Ok(self.diagnostics()),
            CliCommand::ReplayCheck => {
                Ok("run `make replay-check` from the repository root\n".to_string())
            }
            CliCommand::Update(command) => Self::update(&command),
            CliCommand::Run(command) => self.run_prompt(&command),
            CliCommand::Session { command } => self.session(command),
            CliCommand::Profile { command } => self.profile(command),
            CliCommand::Config { command } => self.config(command),
            CliCommand::Completion { shell } => render_completion(shell),
        }
    }

    fn run_prompt(&mut self, command: &RunCommand) -> CliResult<String> {
        let prompt = command.prompt_text()?;
        let selected_profile = command
            .profile
            .as_deref()
            .unwrap_or(&self.config.default_profile);
        let resolved_profile = resolve_profile(&self.config, Some(selected_profile))?;
        let environment = resolve_environment(&self.config)?;
        let (session_id, created) = self.resolve_session(command, &resolved_profile.name)?;
        let restore_from = command
            .run
            .clone()
            .or_else(|| command.branch_from.clone())
            .or_else(|| {
                if created {
                    None
                } else {
                    self.store
                        .load_session(&session_id)
                        .ok()
                        .and_then(|session| {
                            session
                                .head_success_run_id
                                .map(|run| run.as_str().to_string())
                        })
                }
            });
        let mut run = self.store.append_run(
            &session_id,
            prompt.clone(),
            restore_from.clone(),
            &resolved_profile.name,
        )?;
        write_current_session(&self.config, &session_id)?;
        let restore_state = self
            .store
            .load_restore_state(&session_id, restore_from.as_deref())?;
        let hitl = command.hitl.unwrap_or(self.config.default_hitl);
        let result = execute_agent_session(
            prompt,
            &run,
            &resolved_profile,
            &environment.provider,
            restore_state,
            CliRunPolicy { hitl },
        );
        let execution = match result {
            Ok(execution) => execution,
            Err(error) => {
                let messages = failed_display_message(&run, &error.to_string());
                self.store
                    .fail_run_with_messages(&mut run, error.to_string(), &messages)?;
                return Err(error);
            }
        };
        let messages = self
            .store
            .complete_run(&mut run, execution.output, execution.artifacts)?;
        if self.config.auto_trim {
            let _report = self.store.trim(
                vec![session_id.clone()],
                self.config.current_session_keep_recent_runs,
                false,
            )?;
        }
        let output_mode = command.output.unwrap_or(self.config.default_output);
        match output_mode {
            OutputMode::Text => Ok(render_display_text(&messages)),
            OutputMode::DisplayJsonl => render_display_jsonl(&messages),
            OutputMode::Silent => Ok(format!(
                "session_id={}\nrun_id={}\nstatus={}\n",
                session_id,
                run.run_id.as_str(),
                run_status_name(run.status)
            )),
        }
    }

    fn resolve_session(
        &mut self,
        command: &RunCommand,
        profile: &str,
    ) -> CliResult<(String, bool)> {
        if command.new_session {
            let session = self
                .store
                .create_session(profile, Some("CLI session".to_string()))?;
            return Ok((session.session_id.as_str().to_string(), true));
        }
        if let Some(session_id) = command.session.as_ref() {
            self.store.load_session(session_id)?;
            return Ok((session_id.clone(), false));
        }
        if command.continue_session {
            if let Some(session_id) = read_current_session(&self.config)? {
                if self.store.load_session(&session_id).is_ok() {
                    return Ok((session_id, false));
                }
            }
            if let Some(session) = self.store.latest_session()? {
                return Ok((session.session_id.as_str().to_string(), false));
            }
        }
        if let Some(session_id) = read_current_session(&self.config)? {
            if self.store.load_session(&session_id).is_ok() {
                return Ok((session_id, false));
            }
        }
        let session = self
            .store
            .create_session(profile, Some("CLI session".to_string()))?;
        Ok((session.session_id.as_str().to_string(), true))
    }

    fn session(&mut self, command: SessionCommand) -> CliResult<String> {
        match command {
            SessionCommand::List(command) => {
                let sessions = self.store.list_sessions(command.limit)?;
                render_sessions(&sessions, command.output)
            }
            SessionCommand::Show(command) => {
                let session = self.store.load_session(&command.session_id)?;
                let runs = self.store.list_runs(&command.session_id, command.runs)?;
                let value = session_value(&session);
                render_session_show(&value, &runs, command.output)
            }
            SessionCommand::Replay(command) => {
                let messages = self.store.replay_display(
                    &command.session_id,
                    command.run.as_deref(),
                    command.after,
                )?;
                match command.output {
                    OutputMode::Text => Ok(render_display_text(&messages)),
                    OutputMode::DisplayJsonl => render_display_jsonl(&messages),
                    OutputMode::Silent => Ok(format!(
                        "session_id={}\nmessages={}\nstatus=replayed\n",
                        command.session_id,
                        messages.len()
                    )),
                }
            }
            SessionCommand::Trim(command) => {
                let sessions = if command.all {
                    self.store.all_session_ids()?
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
                let report = self.store.trim_with_age(
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
            ProfileCommand::List => list_profiles(&self.config)?
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

    fn diagnostics(&self) -> String {
        format!(
            "sdk={}\nworkspace_version={}\ndatabase_path={}\nfile_store_path={}\nprofile={}\ndefault_model={}\nmodel_profiles={}\nworkspace_root={}\nenvironment_provider={}\nfiles_policy={}\nshell_enabled={}\nprovider.openai.ready={}\nprovider.openai.api_key_env={}\nprovider.openai.base_url={}\nprovider.anthropic.ready={}\nprovider.anthropic.api_key_env={}\nprovider.anthropic.base_url={}\nprovider.gemini.ready={}\nprovider.gemini.api_key_env={}\nprovider.gemini.base_url={}\nwal=true\n",
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
            provider_ready(&self.config.providers.openai),
            self.config.providers.openai.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.openai.base_url.as_deref().unwrap_or_default(),
            provider_ready(&self.config.providers.anthropic),
            self.config.providers.anthropic.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.anthropic.base_url.as_deref().unwrap_or_default(),
            provider_ready(&self.config.providers.gemini),
            self.config.providers.gemini.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.gemini.base_url.as_deref().unwrap_or_default()
        )
    }
}

fn provider_ready(provider: &crate::config::ProviderConfig) -> bool {
    provider.enabled
        && provider.api_key_env.as_deref().is_some_and(|name| {
            let name = name.trim();
            !name.is_empty() && std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
        })
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
        OutputMode::DisplayJsonl => sessions
            .iter()
            .map(|session| serde_json::to_string(session).map(|line| format!("{line}\n")))
            .collect::<Result<String, _>>()
            .map_err(CliError::from),
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
        OutputMode::DisplayJsonl => {
            let mut lines = String::new();
            lines.push_str(&serde_json::to_string(session)?);
            lines.push('\n');
            for run in runs {
                lines.push_str(&serde_json::to_string(run)?);
                lines.push('\n');
            }
            Ok(lines)
        }
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

fn render_display_text(messages: &[DisplayMessage]) -> String {
    let mut output = String::new();
    let mut last_was_text = false;
    for message in messages {
        match message.kind {
            DisplayMessageKind::AssistantTextDelta => {
                if let Some(delta) = message.payload.get("delta").and_then(Value::as_str) {
                    output.push_str(delta);
                    last_was_text = true;
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
        OutputMode::DisplayJsonl => Ok(format!("{}\n", serde_json::to_string(report)?)),
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
