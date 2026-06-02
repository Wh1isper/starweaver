//! CLI service layer over local storage and SDK execution.

use std::sync::Arc;

use clap_complete::Shell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_agent::{AgentBuilder, AgentStreamRecord, FunctionModel};
use starweaver_core::sdk_name;
use starweaver_model::ModelResponse;
use starweaver_runtime::{AgentStreamEvent, ModelResponseStreamEvent};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext,
};

use crate::{
    args::{Cli, CliCommand, ConfigCommand, OutputMode, RunCommand, SessionCommand},
    config::{get_config_value, read_current_session, write_current_session, CliConfig},
    local_store::{LocalStore, RunSummary, SessionSummary, TrimReport},
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
            CliCommand::Run(command) => self.run_prompt(&command),
            CliCommand::Session { command } => self.session(command),
            CliCommand::Config { command } => self.config(command),
            CliCommand::Completion { shell } => render_completion(shell),
        }
    }

    fn run_prompt(&mut self, command: &RunCommand) -> CliResult<String> {
        let prompt = command.prompt_text()?;
        let (session_id, created) = self.resolve_session(command)?;
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
            restore_from,
            &self.config.default_profile,
        )?;
        write_current_session(&self.config, &session_id)?;
        let result = execute_local_agent(prompt, &run, CliRunPolicy { hitl: command.hitl });
        let execution = match result {
            Ok(execution) => execution,
            Err(error) => {
                self.store.fail_run(&mut run, error.to_string())?;
                return Err(error);
            }
        };
        let messages = self.store.complete_run(
            &mut run,
            execution.output,
            &execution.raw_records,
            &execution.display_messages,
        )?;
        if self.config.auto_trim {
            let _report = self.store.trim(
                vec![session_id.clone()],
                self.config.current_session_keep_recent_runs,
                false,
            )?;
        }
        match command.output {
            OutputMode::DisplayJsonl => render_display_jsonl(&messages),
            OutputMode::Silent => Ok(format!(
                "session_id={}\nrun_id={}\nstatus=completed\n",
                session_id,
                run.run_id.as_str()
            )),
        }
    }

    fn resolve_session(&mut self, command: &RunCommand) -> CliResult<(String, bool)> {
        if command.new_session {
            let session = self.store.create_session(
                &self.config.default_profile,
                Some("CLI session".to_string()),
            )?;
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
        let session = self.store.create_session(
            &self.config.default_profile,
            Some("CLI session".to_string()),
        )?;
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
                let report = self
                    .store
                    .trim(sessions, command.keep_runs, command.dry_run)?;
                render_trim_report(&report, command.output)
            }
        }
    }

    fn config(&self, command: ConfigCommand) -> CliResult<String> {
        match command {
            ConfigCommand::Get { key } => get_config_value(&self.config, &key),
            ConfigCommand::Set { key, value } => {
                crate::config::set_project_config_value(&self.config, &key, &value)?;
                Ok(format!("{key}={value}\n"))
            }
        }
    }

    fn diagnostics(&self) -> String {
        format!(
            "sdk={}\nworkspace_version={}\ndatabase_path={}\nfile_store_path={}\nwal=true\n",
            sdk_name(),
            env!("CARGO_PKG_VERSION"),
            self.config.database_path.display(),
            self.config.file_store_path.display()
        )
    }
}

struct CliRunExecution {
    output: String,
    raw_records: Vec<AgentStreamRecord>,
    display_messages: Vec<DisplayMessage>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CliRunPolicy {
    hitl: crate::args::HitlPolicy,
}

fn execute_local_agent(
    prompt: String,
    run: &starweaver_session::RunRecord,
    policy: CliRunPolicy,
) -> CliResult<CliRunExecution> {
    let policy_metadata = serde_json::to_value(policy)?;
    let prompt_for_model = prompt.clone();
    let model = FunctionModel::new(move |_messages, _settings, _info| {
        Ok(ModelResponse::text(format!(
            "local echo: {prompt_for_model}"
        )))
    });
    let agent = AgentBuilder::new(Arc::new(model)).build();
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let stream = runtime
        .block_on(agent.run_stream(prompt))
        .map_err(|error| CliError::Run(error.to_string()))?;
    let output = stream.result.output.clone();
    let context = DisplayProjectionContext::new(run.session_id.clone(), run.run_id.clone());
    let projector = DefaultDisplayMessageProjector;
    let mut display_messages = Vec::new();
    let mut final_result_projected_text = false;
    for record in &stream.events {
        let projected = runtime.block_on(projector.project(&context, record));
        let projected_has_text_delta = projected
            .iter()
            .any(|message| message.kind == DisplayMessageKind::AssistantTextDelta);
        match &record.event {
            AgentStreamEvent::ModelStream {
                event: ModelResponseStreamEvent::FinalResult(_),
                ..
            } => {
                if projected_has_text_delta {
                    final_result_projected_text = true;
                }
                display_messages.extend(projected);
            }
            AgentStreamEvent::ModelResponse { .. }
                if final_result_projected_text && projected_has_text_delta =>
            {
                final_result_projected_text = false;
            }
            AgentStreamEvent::ModelResponse { .. } => {
                final_result_projected_text = false;
                display_messages.extend(projected);
            }
            _ => display_messages.extend(projected),
        }
    }
    resequence_display_messages(&mut display_messages);
    for message in &mut display_messages {
        message
            .metadata
            .insert("cli_run_policy".to_string(), policy_metadata.clone());
    }
    Ok(CliRunExecution {
        output,
        raw_records: stream.events,
        display_messages,
    })
}

fn resequence_display_messages(messages: &mut [DisplayMessage]) {
    for (sequence, message) in messages.iter_mut().enumerate() {
        message.sequence = sequence;
    }
}

fn render_sessions(sessions: &[SessionSummary], output: OutputMode) -> CliResult<String> {
    match output {
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

fn render_completion(shell: Shell) -> CliResult<String> {
    let mut command = crate::args::command();
    let mut buffer = Vec::new();
    clap_complete::generate(shell, &mut command, "starweaver-cli", &mut buffer);
    String::from_utf8(buffer).map_err(|error| CliError::Run(error.to_string()))
}

fn render_trim_report(report: &TrimReport, output: OutputMode) -> CliResult<String> {
    match output {
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
