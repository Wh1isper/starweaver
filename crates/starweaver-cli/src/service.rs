//! CLI service layer over local storage and SDK execution.
#![allow(clippy::redundant_pub_crate)]

use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use chrono::Utc;
use ring::digest::{SHA256, digest};
use serde_json::{Value, json};
use starweaver_agent::materialization::STARWEAVER_AGENT_POLICY_VERSION;
use starweaver_agent::{
    ContinuationMaterialization, ResolvedAgentMaterialization, ResumableState,
    environment_binding_class,
};
use starweaver_core::{RunId, SessionId, sdk_name};
use starweaver_oauth_provider::create_oauth_refresh_supervisor_for_models_with_options;
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{
    ApprovalStatus, ExecutionStatus, HitlResumeAbortOutcome, HitlResumeClaim, PreparedContinuation,
    RunAdmissionLease, RunRecord, RunStatus, SessionSearchFilter, SessionSearchGranularity,
    SessionSearchQuery, SessionSearchSource, SessionStatus, SessionStore,
};
use starweaver_stream::DisplayMessage;

use crate::{
    CliError, CliResult,
    args::{
        ApprovalCommand, ApprovalDecisionCommand, ApprovalListCommand, Cli, CliCommand,
        DeferredCommand, DeferredCompleteCommand, DeferredFailCommand, DeferredListCommand,
        OutputMode, ResumeCommand, RunCommand, SessionCommand, SessionSearchCommand,
        SessionSearchGranularityArg, SessionSearchSourceArg, SessionSearchStatusArg,
        StorageCommand, StorageImportLegacyCommand, TuiCommand,
    },
    config::{
        CliConfig, read_current_session, read_last_retention_maintenance, write_current_session,
        write_last_retention_maintenance,
    },
    environment::{
        ResolvedEnvironment, resolve_environment_for_session_with_attachments,
        validate_environment_config,
    },
    local_store::{
        HITL_RESUME_CLAIM_ID_METADATA_KEY, HITL_RESUME_PREFLIGHT_SOURCE_RUN_ID_METADATA_KEY,
        HITL_RESUME_SOURCE_RUN_ID_METADATA_KEY, LocalSessionStore, LocalStore,
    },
    profiles::{ResolvedProfile, list_profiles, resolve_profile},
    prompt_input::PromptInput,
    runner::{
        CliAgentExecutionHost, CliRunPolicy, CliSteeringChannel, execute_agent_session_with_host,
        failed_display_message, validate_prepared_hitl_continuation,
    },
    slash_commands::{ExpandedExplicitSkills, expand_explicit_skills, expand_slash_command},
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
    render_session_delete, render_session_search, render_session_show, render_sessions,
    render_trim_report, session_value,
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
    pub(super) continuation: Option<ContinuationMaterialization>,
}

pub(super) struct PreparedPromptRun {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) output_mode: OutputMode,
    pub(super) run: RunRecord,
    pub(super) admission: CliRunAdmission,
    admission_cancel_receiver: Option<mpsc::Receiver<()>>,
    run_input: PromptInput,
    resolved_profile: ResolvedProfile,
    pub(super) environment: ResolvedEnvironment,
    restore_state: Option<ResumableState>,
    prepared_continuation: Option<PreparedContinuation>,
    policy: CliRunPolicy,
    execution_host: CliAgentExecutionHost,
    hitl_resume_claim: Option<HitlResumeClaim>,
}

impl PreparedPromptRun {
    pub(super) fn set_execution_host(&mut self, execution_host: CliAgentExecutionHost) {
        self.execution_host = execution_host;
    }
}

pub(super) struct ExecutedPromptRun {
    run: RunRecord,
    output_mode: OutputMode,
    execution: crate::runner::CliRunExecution,
    admission: CliRunAdmission,
}

enum HitlResumeClaimOperation {
    Claim(HitlResumeClaim),
    StartEffect {
        lease: RunAdmissionLease,
        source_run_id: RunId,
        claim_id: String,
    },
    Release {
        session_id: SessionId,
        run_id: RunId,
        claim_id: String,
    },
}

const PROJECT_GUIDANCE_TAG: &str = "project-guidance";
const USER_RULES_TAG: &str = "user-rules";

fn restore_requires_hitl_claim(status: RunStatus, hitl_resume: bool, branch_from: bool) -> bool {
    status == RunStatus::Waiting && !hitl_resume && !branch_from
}

fn deterministic_hitl_resume_claim_id(session_id: &str, run_id: &RunId) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let identity = format!(
        "starweaver.cli.hitl_resume_claim.v1\0{session_id}\0{}",
        run_id.as_str()
    );
    let digest = digest(&SHA256, identity.as_bytes());
    let mut fingerprint = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        fingerprint.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        fingerprint.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    format!("cli-hitl-resume-{fingerprint}")
}

fn search_query(command: SessionSearchCommand) -> SessionSearchQuery {
    let session_statuses = command
        .status
        .map(|status| match status {
            SessionSearchStatusArg::Active => SessionStatus::Active,
            SessionSearchStatusArg::Archived => SessionStatus::Archived,
            SessionSearchStatusArg::Failed => SessionStatus::Failed,
        })
        .into_iter()
        .collect();
    let sources = command
        .sources
        .into_iter()
        .map(|source| match source {
            SessionSearchSourceArg::SessionMetadata => SessionSearchSource::SessionMetadata,
            SessionSearchSourceArg::RunInput => SessionSearchSource::RunInput,
            SessionSearchSourceArg::RunOutputPreview => SessionSearchSource::RunOutputPreview,
            SessionSearchSourceArg::DisplayMessage => SessionSearchSource::DisplayMessage,
        })
        .collect::<BTreeSet<_>>();
    let granularity = match command.granularity {
        SessionSearchGranularityArg::Session => SessionSearchGranularity::Session,
        SessionSearchGranularityArg::Run => SessionSearchGranularity::Run,
        SessionSearchGranularityArg::Occurrence => SessionSearchGranularity::Occurrence,
    };
    SessionSearchQuery {
        text: command.text,
        filter: SessionSearchFilter {
            session_statuses,
            profile: command.profile,
            workspace: command.workspace,
            ..SessionSearchFilter::default()
        },
        sources,
        granularity,
        limit: command.limit,
        cursor: command.cursor,
        ..SessionSearchQuery::default()
    }
}

fn append_guidance_files(input: &mut PromptInput, config: &CliConfig) {
    if let Some(project_guidance) = load_project_guidance(&config.workspace_root) {
        input.push_guidance_text_part(project_guidance);
    }
    if let Some(user_rules) = load_user_rules(&config.global_dir) {
        input.push_guidance_text_part(user_rules);
    }
}

fn append_explicit_skill_guidance(
    input: &mut PromptInput,
    explicit_skills: Option<&ExpandedExplicitSkills>,
) {
    let Some(explicit_skills) = explicit_skills else {
        return;
    };
    let names = explicit_skills
        .skills
        .iter()
        .map(|skill| skill.package.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    input.push_guidance_text_part(format!(
        "<explicit-skill-composition>\nThe user explicitly activated these skills in priority order: {names}. Apply all compatible instructions. Treat the first skill as the primary workflow and later skills as supporting workflows. The current user request overrides skill defaults. If selected skills conflict irreconcilably, ask the user for clarification.\n</explicit-skill-composition>"
    ));
    for skill in &explicit_skills.skills {
        let package = &skill.package;
        let Some(body) = package.body.as_deref() else {
            continue;
        };
        input.push_guidance_text_part(format!(
            "<explicitly-activated-skill>\n<name>{}</name>\n<path>{}</path>\n<instructions>\n{}\n</instructions>\n</explicitly-activated-skill>",
            escape_xml_text(&package.name),
            escape_xml_text(&package.path),
            body
        ));
    }
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

#[derive(Clone)]
pub(super) struct CliRunAdmission {
    config: CliConfig,
    lease: Arc<Mutex<RunAdmissionLease>>,
    stop_sender: mpsc::Sender<()>,
    handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    released: Arc<AtomicBool>,
    lost: Arc<AtomicBool>,
}

impl CliRunAdmission {
    fn start(config: CliConfig, lease: RunAdmissionLease) -> (Self, mpsc::Receiver<()>) {
        const RETRY_INTERVAL: Duration = Duration::from_secs(1);
        const SAFETY_MARGIN: chrono::Duration = chrono::Duration::seconds(5);

        let lease = Arc::new(Mutex::new(lease));
        let (stop_sender, stop_receiver) = mpsc::channel();
        let (cancel_sender, cancel_receiver) = mpsc::channel();
        let lost = Arc::new(AtomicBool::new(false));
        let heartbeat_lease = Arc::clone(&lease);
        let heartbeat_lost = Arc::clone(&lost);
        let heartbeat_config = config.clone();
        let handle = thread::spawn(move || {
            let mut wait = Duration::from_secs(10);
            loop {
                match stop_receiver.recv_timeout(wait) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let current = if let Ok(lease) = heartbeat_lease.lock() {
                    lease.clone()
                } else {
                    heartbeat_lost.store(true, Ordering::Release);
                    let _ = cancel_sender.send(());
                    break;
                };
                if let Ok(refreshed) =
                    LocalStore::heartbeat_run_admission(&heartbeat_config, &current)
                {
                    if let Ok(mut lease) = heartbeat_lease.lock() {
                        *lease = refreshed;
                    } else {
                        heartbeat_lost.store(true, Ordering::Release);
                        let _ = cancel_sender.send(());
                        break;
                    }
                    wait = Duration::from_secs(10);
                } else {
                    if Utc::now() + SAFETY_MARGIN >= current.lease_expires_at {
                        heartbeat_lost.store(true, Ordering::Release);
                        let _ = cancel_sender.send(());
                        break;
                    }
                    wait = RETRY_INTERVAL;
                }
            }
        });
        (
            Self {
                config,
                lease,
                stop_sender,
                handle: Arc::new(Mutex::new(Some(handle))),
                released: Arc::new(AtomicBool::new(false)),
                lost,
            },
            cancel_receiver,
        )
    }

    fn refresh(&self) -> CliResult<()> {
        if self.lost.load(Ordering::Acquire) {
            return Err(CliError::Run(
                "run admission lease was lost before completion".to_string(),
            ));
        }
        if self.released.load(Ordering::Acquire) {
            return Err(CliError::Run(
                "run admission lease was already released".to_string(),
            ));
        }
        let current = self
            .lease
            .lock()
            .map_err(|error| CliError::Storage(error.to_string()))?
            .clone();
        let refreshed = LocalStore::heartbeat_run_admission(&self.config, &current)?;
        *self
            .lease
            .lock()
            .map_err(|error| CliError::Storage(error.to_string()))? = refreshed;
        Ok(())
    }

    fn current_lease(&self) -> CliResult<RunAdmissionLease> {
        if self.lost.load(Ordering::Acquire) {
            return Err(CliError::Run(
                "run admission lease was lost before evidence commit".to_string(),
            ));
        }
        self.lease
            .lock()
            .map_err(|error| CliError::Storage(error.to_string()))
            .map(|lease| lease.clone())
    }

    fn release(&self, store: &LocalStore) -> CliResult<()> {
        if self.released.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let _ = self.stop_sender.send(());
        if let Ok(mut handle) = self.handle.lock()
            && let Some(handle) = handle.take()
        {
            let _ = handle.join();
        }
        let lease = self
            .lease
            .lock()
            .map_err(|error| CliError::Storage(error.to_string()))?
            .clone();
        store.release_run_admission(&lease)
    }
}

/// CLI service.
pub struct CliService {
    config: CliConfig,
    store: Option<LocalStore>,
    host_instance_id: String,
}

impl CliService {
    /// Open service from resolved config.
    pub fn open(config: CliConfig) -> CliResult<Self> {
        Ok(Self {
            config,
            store: None,
            host_instance_id: format!("cli-host-{}", uuid::Uuid::new_v4()),
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
                continuation_mode: cli.continuation_mode,
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
                hitl_resume: false,
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
            render_mode: None,
        });
        match cli.command.unwrap_or(default_command) {
            CliCommand::Version => Ok(format!("{}\n", sdk_name())),
            CliCommand::Diagnostics => Ok(self.diagnostics()?),
            CliCommand::ReplayCheck => {
                Ok("run `make replay-check` from the repository root\n".to_string())
            }
            CliCommand::Update(command) => Self::update(&command),
            CliCommand::Run(command) => self.run_prompt(&command),
            CliCommand::Session { command } => self.session(command),
            CliCommand::Storage { command } => self.storage(command),
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
            OutputMode::Text => {
                let output = render_display_text(&execution.messages);
                Ok(continuation_drift_prefix(execution.continuation.as_ref()) + &output)
            }
            OutputMode::DisplayJsonl => render_display_jsonl(&execution.messages),
            OutputMode::AguiJsonl => render_agui_jsonl(&execution.messages),
            OutputMode::Json => render_prompt_run_json(&execution),
            OutputMode::Silent => Ok(format!(
                "{}session_id={}\nrun_id={}\nstatus={}\n",
                continuation_drift_prefix(execution.continuation.as_ref()),
                execution.session_id,
                execution.run_id,
                execution.status
            )),
        }
    }

    fn execute_prompt_run(
        &mut self,
        command: &RunCommand,
        stream_sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
    ) -> CliResult<PromptRunExecution> {
        self.execute_prompt_run_with_channels(command, None, stream_sender, None, None)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_prompt_run_with_channels(
        &mut self,
        command: &RunCommand,
        prompt_input: Option<PromptInput>,
        stream_sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
        steering_channel: Option<CliSteeringChannel>,
        cancel_receiver: Option<mpsc::Receiver<()>>,
    ) -> CliResult<PromptRunExecution> {
        let mut prepared = self.prepare_prompt_run(command, prompt_input)?;
        let admission_on_error = prepared.admission.clone();
        if let Err(error) = self.start_prepared_hitl_resume(&mut prepared) {
            // The phase-aware start path records the source claim relation before it attempts
            // the durable fence. Preserve that mutated run when cleanup must consume Started.
            self.fail_prepared_prompt_run(prepared.run.clone(), &error, &admission_on_error)?;
            return Err(error);
        }
        let run_on_error = prepared.run.clone();
        let admission_on_error = prepared.admission.clone();
        let executed = match Self::run_prepared_prompt(
            prepared,
            stream_sender,
            steering_channel,
            cancel_receiver,
        ) {
            Ok(executed) => executed,
            Err(error) => {
                self.fail_prepared_prompt_run(run_on_error, &error, &admission_on_error)?;
                return Err(error);
            }
        };
        self.complete_prompt_run(executed)
    }

    fn reject_ordinary_admission_during_waiting_continuation(
        &mut self,
        session_id: &str,
        hitl_resume: bool,
    ) -> CliResult<()> {
        if hitl_resume {
            return Ok(());
        }
        let session = self.store()?.load_session(session_id)?;
        let runs = self.store()?.list_run_records(session_id)?;
        let active_run_id = session.active_run_id.as_ref();
        for run in runs.iter().filter(|run| {
            active_run_id == Some(&run.run_id)
                || (run.status.is_active() && run.status != RunStatus::Waiting)
        }) {
            let waiting_source_run_id = if run.status == RunStatus::Waiting {
                Some(&run.run_id)
            } else {
                run.restore_from_run_id.as_ref().filter(|source_run_id| {
                    runs.iter().any(|source| {
                        source.run_id.as_str() == source_run_id.as_str()
                            && source.status == RunStatus::Waiting
                    })
                })
            };
            if let Some(waiting_source_run_id) = waiting_source_run_id {
                return Err(CliError::Run(format!(
                    "run {} is continuing waiting run {}; wait for it to finish before starting another prompt",
                    run.run_id.as_str(),
                    waiting_source_run_id.as_str()
                )));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn prepare_prompt_run(
        &mut self,
        command: &RunCommand,
        prompt_input: Option<PromptInput>,
    ) -> CliResult<PreparedPromptRun> {
        let input =
            prompt_input.map_or_else(|| command.prompt_text().map(PromptInput::text), Ok)?;
        let raw_prompt = input.text.clone();
        let worktree = self.resolve_worktree(command)?;
        let selected_profile = command
            .profile
            .as_deref()
            .unwrap_or(&self.config.default_profile);
        let resolved_profile = resolve_profile(&self.config, Some(selected_profile))?;
        let slash_expansion = expand_slash_command(&self.config.slash_commands, &raw_prompt);
        let explicit_skills = slash_expansion
            .is_none()
            .then(|| expand_explicit_skills(&resolved_profile.skills, &raw_prompt))
            .flatten();
        let prompt = slash_expansion.as_ref().map_or_else(
            || {
                explicit_skills
                    .as_ref()
                    .map_or_else(|| raw_prompt.clone(), |expanded| expanded.prompt.clone())
            },
            |expanded| expanded.prompt.clone(),
        );
        let mut run_input = PromptInput {
            text: prompt.clone(),
            attachments: input.attachments,
            extra_text_parts: input.extra_text_parts,
            guidance_text_parts: input.guidance_text_parts,
        };
        let mut run_config = self.config.clone();
        if let Some(worktree) = worktree.as_ref() {
            run_config.workspace_root.clone_from(&worktree.path);
        }
        validate_environment_config(&run_config)?;
        append_guidance_files(&mut run_input, &run_config);
        append_explicit_skill_guidance(&mut run_input, explicit_skills.as_ref());
        let (session_id, created) = self.resolve_session(command, &resolved_profile.name)?;
        if command.run.is_some() || command.branch_from.is_some() {
            self.reject_ordinary_admission_during_waiting_continuation(
                &session_id,
                command.hitl_resume,
            )?;
        }
        let environment = resolve_environment_for_session_with_attachments(
            &run_config,
            &session_id,
            &command.environment_attachments,
        )?;
        let hitl_resume_source_run_id = command
            .hitl_resume
            .then(|| {
                command
                    .run
                    .as_deref()
                    .ok_or_else(|| {
                        CliError::Usage("HITL continuation requires a source run id".to_string())
                    })
                    .map(RunId::from_string)
            })
            .transpose()?;
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
        let mut waiting_restore_without_claim = None;
        if let Some(source_run_id) = restore_from.as_deref() {
            let source = self.store()?.load_run(&session_id, source_run_id)?;
            if restore_requires_hitl_claim(
                source.status,
                command.hitl_resume,
                command.branch_from.is_some(),
            ) {
                waiting_restore_without_claim = Some(source_run_id.to_string());
            }
            if source.status.is_active()
                && source.status != RunStatus::Waiting
                && command.branch_from.is_none()
                && let Some(waiting_source_run_id) = source.restore_from_run_id.as_ref()
                && self
                    .store()?
                    .load_run(&session_id, waiting_source_run_id.as_str())?
                    .status
                    == RunStatus::Waiting
            {
                return Err(CliError::Run(format!(
                    "run {source_run_id} is continuing waiting run {}; wait for it to finish before starting another prompt",
                    waiting_source_run_id.as_str()
                )));
            }
        }
        let prepared_continuation = match hitl_resume_source_run_id.as_ref() {
            Some(source_run_id) => Some(
                self.store()?
                    .prepare_waiting_continuation(&session_id, source_run_id.as_str())?,
            ),
            None => None,
        };
        let restore_state = match prepared_continuation.as_ref() {
            Some(prepared) => Some(prepared.snapshot.state.clone()),
            None => self
                .store()?
                .load_restore_state(&session_id, restore_from.as_deref())?,
        };
        if let Some(source_run_id) = waiting_restore_without_claim {
            let mut pending = self
                .store()?
                .list_approvals(Some(&session_id), Some(&source_run_id))?
                .into_iter()
                .filter(|approval| approval.status == ApprovalStatus::Pending)
                .map(|approval| approval.approval_id)
                .collect::<Vec<_>>();
            pending.extend(
                self.store()?
                    .list_deferred_tools(Some(&session_id), Some(&source_run_id))?
                    .into_iter()
                    .filter(|deferred| {
                        matches!(
                            deferred.status,
                            ExecutionStatus::Pending
                                | ExecutionStatus::Running
                                | ExecutionStatus::Waiting
                        )
                    })
                    .map(|deferred| deferred.deferred_id),
            );
            if !pending.is_empty() {
                return Err(CliError::Run(format!(
                    "cannot resume run {source_run_id} while HITL items are pending: {}",
                    pending.join(", ")
                )));
            }
            return Err(CliError::Run(format!(
                "run {source_run_id} is waiting and requires an explicit HITL resume"
            )));
        }
        let binding_class = environment_binding_class(environment.attachments.iter().map(|item| {
            (
                item.kind.clone(),
                match item.resolved_mode() {
                    starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadOnly => "read_only",
                    starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadWrite => "read_write",
                }
                .to_string(),
            )
        }));
        let materialization = resolved_profile
            .spec
            .resolved_materialization(
                &resolved_profile.registry,
                STARWEAVER_AGENT_POLICY_VERSION,
                binding_class,
            )
            .map_err(|error| CliError::Config(error.to_string()))?;
        let continuation = restore_from
            .as_deref()
            .map(|source_run_id| {
                let source = self.store()?.load_run(&session_id, source_run_id)?;
                let source_materialization =
                    ResolvedAgentMaterialization::from_metadata(&source.metadata)
                        .map_err(|error| CliError::Run(error.to_string()))?;
                let assessment = ContinuationMaterialization::assess(
                    source_materialization.as_ref(),
                    &materialization,
                    command.continuation_mode.into(),
                );
                if !assessment.allowed {
                    return Err(CliError::Run(format!(
                        "continuation materialization mode {} rejected drift: {}; retry with --continuation-mode compatible or switch after review",
                        assessment.mode.as_str(),
                        assessment.drift_summary()
                    )));
                }
                Ok(assessment)
            })
            .transpose()?;
        if let Some(prepared) = prepared_continuation.as_ref() {
            validate_prepared_hitl_continuation(
                &resolved_profile,
                &environment.provider,
                environment.process_provider.as_ref(),
                prepared,
            )?;
        }
        let mut admission_metadata = serde_json::Map::new();
        materialization
            .insert_into(&mut admission_metadata)
            .map_err(CliError::from)?;
        if let Some(continuation) = continuation.as_ref() {
            continuation
                .insert_into(&mut admission_metadata)
                .map_err(CliError::from)?;
        }
        write_current_session(&self.config, &session_id)?;
        self.reject_ordinary_admission_during_waiting_continuation(
            &session_id,
            command.hitl_resume,
        )?;
        let hitl_resume_claim = hitl_resume_source_run_id.clone().map(|source_run_id| {
            HitlResumeClaim::new(
                deterministic_hitl_resume_claim_id(&session_id, &source_run_id),
                SessionId::from_string(&session_id),
                source_run_id,
                Utc::now(),
            )
        });
        if let Some(claim) = hitl_resume_claim.clone() {
            run_hitl_resume_claim_operation(
                self.config.clone(),
                HitlResumeClaimOperation::Claim(claim),
            )?;
        }
        let host_instance_id = self.host_instance_id.clone();
        let (mut run, admission_lease) = match self.store()?.admit_run(
            &session_id,
            prompt,
            restore_from,
            &resolved_profile.name,
            admission_metadata,
            &host_instance_id,
            hitl_resume_source_run_id,
            hitl_resume_claim
                .as_ref()
                .map(|claim| claim.claim_id.clone()),
        ) {
            Ok(admitted) => admitted,
            Err(error) => {
                if let Some(claim) = hitl_resume_claim.as_ref() {
                    let release = run_hitl_resume_claim_operation(
                        self.config.clone(),
                        HitlResumeClaimOperation::Release {
                            session_id: claim.session_id.clone(),
                            run_id: claim.run_id.clone(),
                            claim_id: claim.claim_id.clone(),
                        },
                    );
                    if let Err(release_error) = release {
                        return Err(CliError::Storage(format!(
                            "{error}; failed to release preflight HITL claim: {release_error}"
                        )));
                    }
                }
                return Err(error);
            }
        };
        let (admission, admission_cancel_receiver) =
            CliRunAdmission::start(self.config.clone(), admission_lease);
        if let Some(claim) = hitl_resume_claim.as_ref() {
            run.metadata.insert(
                HITL_RESUME_PREFLIGHT_SOURCE_RUN_ID_METADATA_KEY.to_string(),
                json!(claim.run_id.as_str()),
            );
        }
        apply_starweaver_run_metadata(
            &mut run,
            command,
            worktree.as_ref(),
            slash_expansion.as_ref(),
        );
        if let Some(explicit_skills) = explicit_skills.as_ref() {
            run.metadata.insert(
                "cli.skills.activated".to_string(),
                json!(
                    explicit_skills
                        .skills
                        .iter()
                        .map(|skill| json!({
                            "name": skill.package.name,
                            "invoked": skill.invoked_name,
                            "description": skill.package.description,
                            "path": skill.package.path,
                            "metadata": skill.package.metadata,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        }
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
                json!(
                    command
                        .environment_attachments
                        .iter()
                        .map(|attachment| attachment.id.as_str())
                        .collect::<Vec<_>>()
                ),
            );
        }
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
            admission,
            admission_cancel_receiver: Some(admission_cancel_receiver),
            run_input,
            resolved_profile,
            environment,
            restore_state,
            prepared_continuation,
            policy: CliRunPolicy { hitl, goal },
            execution_host: if command.worker.is_some() || command.worker_label.is_some() {
                CliAgentExecutionHost::disabled()
            } else {
                CliAgentExecutionHost::blocking()
            },
            hitl_resume_claim,
        })
    }

    /// Advance a fenced waiting-run claim immediately before approved-tool execution.
    ///
    /// The shared store transition is the only operation that moves an admitted continuation to
    /// `Started`; after it succeeds a process loss is deliberately reconciled as indeterminate
    /// rather than allowing a second product to repeat the approved effect.
    pub(super) fn start_prepared_hitl_resume(
        &self,
        prepared: &mut PreparedPromptRun,
    ) -> CliResult<()> {
        let Some(claim) = prepared.hitl_resume_claim.take() else {
            return Ok(());
        };
        let source_run_id = claim.run_id;
        let claim_id = claim.claim_id;
        // Record the relation before attempting the effect fence. If the store committed
        // `Started` but its response was lost, ordinary error cleanup must atomically consume
        // this source claim instead of terminalizing only the replacement.
        prepared.run.metadata.insert(
            HITL_RESUME_CLAIM_ID_METADATA_KEY.to_string(),
            json!(claim_id),
        );
        prepared.run.metadata.insert(
            HITL_RESUME_SOURCE_RUN_ID_METADATA_KEY.to_string(),
            json!(source_run_id.as_str()),
        );
        let lease_before_refresh = prepared.admission.current_lease()?;
        if let Err(refresh_error) = prepared.admission.refresh() {
            return Err(phase_aware_hitl_start_error(
                self.config.clone(),
                lease_before_refresh,
                source_run_id,
                claim_id,
                refresh_error,
            ));
        }
        let lease = prepared.admission.current_lease()?;
        if let Err(start_error) = run_hitl_resume_claim_operation(
            self.config.clone(),
            HitlResumeClaimOperation::StartEffect {
                lease: lease.clone(),
                source_run_id: source_run_id.clone(),
                claim_id: claim_id.clone(),
            },
        ) {
            return Err(phase_aware_hitl_start_error(
                self.config.clone(),
                lease,
                source_run_id,
                claim_id,
                start_error,
            ));
        }
        Ok(())
    }

    pub(super) fn run_prepared_prompt(
        prepared: PreparedPromptRun,
        stream_sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
        steering_channel: Option<CliSteeringChannel>,
        cancel_receiver: Option<mpsc::Receiver<()>>,
    ) -> CliResult<ExecutedPromptRun> {
        let PreparedPromptRun {
            output_mode,
            run,
            admission,
            admission_cancel_receiver,
            run_input,
            resolved_profile,
            environment,
            restore_state,
            prepared_continuation,
            policy,
            execution_host,
            ..
        } = prepared;
        let result = execute_agent_session_with_host(
            run_input,
            &run,
            &resolved_profile,
            &environment.provider,
            environment.process_provider.as_ref(),
            restore_state,
            prepared_continuation.as_ref(),
            &policy,
            stream_sender,
            steering_channel,
            cancel_receiver,
            admission_cancel_receiver,
            execution_host,
        );
        result.map(|execution| ExecutedPromptRun {
            run,
            output_mode,
            execution,
            admission,
        })
    }

    pub(super) fn fail_prepared_prompt_run(
        &mut self,
        mut run: RunRecord,
        error: &CliError,
        admission: &CliRunAdmission,
    ) -> CliResult<()> {
        // `abort_admitted_hitl_resume` has already terminalized an admitted replacement in the
        // no-effect branch. Do not overwrite that durable decision with generic evidence; just
        // release its matching lease. Started and uncertain branches remain non-terminal here,
        // so the normal fenced evidence path consumes the source claim recorded on `run`.
        if self
            .store()?
            .load_run(run.session_id.as_str(), run.run_id.as_str())?
            .status
            .is_terminal()
        {
            return admission.release(self.store()?);
        }
        admission.refresh()?;
        let lease = admission.current_lease()?;
        let messages = failed_display_message(&run, &error.to_string());
        self.store()?.fail_run_with_messages_fenced(
            &mut run,
            error.to_string(),
            &messages,
            &lease,
        )?;
        admission.release(self.store()?)
    }

    pub(super) fn complete_prompt_run(
        &mut self,
        executed: ExecutedPromptRun,
    ) -> CliResult<PromptRunExecution> {
        let ExecutedPromptRun {
            mut run,
            output_mode,
            execution,
            admission,
        } = executed;
        admission.refresh()?;
        let execution_failed = execution.artifacts.status == RunStatus::Failed;
        let output = execution.output;
        let continuation = run
            .metadata
            .get(starweaver_agent::AGENT_CONTINUATION_METADATA_KEY)
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        let lease = admission.current_lease()?;
        let messages = self.store()?.complete_run_fenced(
            &mut run,
            output.clone(),
            execution.artifacts,
            &lease,
        )?;
        admission.release(self.store()?)?;
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
            continuation,
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
            if let Some(session_id) = read_current_session(&self.config)?
                && self.store()?.load_workspace_session(&session_id).is_ok()
            {
                return Ok((session_id, false));
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

    fn storage(&mut self, command: StorageCommand) -> CliResult<String> {
        match command {
            StorageCommand::ImportLegacy(command) => self.import_legacy_database(command),
        }
    }

    fn import_legacy_database(&mut self, command: StorageImportLegacyCommand) -> CliResult<String> {
        let source = command
            .source
            .unwrap_or_else(|| self.config.project_dir.join("starweaver.sqlite"));
        let workspace = command
            .workspace
            .unwrap_or_else(|| self.config.workspace_root.clone());
        let report = self.store()?.import_legacy_database(&source, &workspace)?;
        match command.output {
            OutputMode::Text => Ok(format!(
                "source={}\nworkspace={}\nsessions_imported={}\nrows_imported={}\nstatus={}\n",
                report.source_path.display(),
                report.workspace,
                report.sessions_imported,
                report.rows_imported,
                if report.imported {
                    "imported"
                } else {
                    "unchanged"
                }
            )),
            OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => Ok(format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "sourcePath": report.source_path,
                    "workspace": report.workspace,
                    "sessionsImported": report.sessions_imported,
                    "rowsImported": report.rows_imported,
                    "imported": report.imported,
                }))?
            )),
            OutputMode::Silent => Ok(format!(
                "sessions_imported={}\nrows_imported={}\nstatus={}\n",
                report.sessions_imported,
                report.rows_imported,
                if report.imported {
                    "imported"
                } else {
                    "unchanged"
                }
            )),
        }
    }

    fn session(&mut self, command: SessionCommand) -> CliResult<String> {
        match command {
            SessionCommand::List(command) => {
                let sessions = self.store()?.list_sessions(command.limit)?;
                render_sessions(&sessions, command.output)
            }
            SessionCommand::Search(command) => {
                let output = command.output;
                let page = self.store()?.search_sessions(search_query(command))?;
                render_session_search(&page, output)
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
                    read_current_session(&self.config)?
                        .filter(|session_id| {
                            self.store()
                                .is_ok_and(|store| store.load_workspace_session(session_id).is_ok())
                        })
                        .into_iter()
                        .collect()
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
        let hitl_resume = source_run.status == RunStatus::Waiting;
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
            continuation_mode: command.continuation_mode,
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
            hitl_resume,
        };
        self.run_prompt(&run_command)
    }

    fn resolve_session_id(&mut self, requested: Option<&str>) -> CliResult<String> {
        if let Some(session_id) = requested {
            self.store()?.load_session(session_id)?;
            return Ok(session_id.to_string());
        }
        if let Some(session_id) = read_current_session(&self.config)?
            && self.store()?.load_workspace_session(&session_id).is_ok()
        {
            return Ok(session_id);
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

fn continuation_drift_prefix(continuation: Option<&ContinuationMaterialization>) -> String {
    continuation
        .filter(|item| !item.drift.is_empty())
        .map_or_else(String::new, |item| {
            format!(
                "materialization_drift mode={} fields={}\n",
                item.mode.as_str(),
                item.drift_summary()
            )
        })
}

fn phase_aware_hitl_start_error(
    config: CliConfig,
    lease: RunAdmissionLease,
    source_run_id: RunId,
    claim_id: String,
    start_error: CliError,
) -> CliError {
    // A response error is not proof that the state transition did not commit. Abort is
    // phase-aware: an admitted claim safely terminalizes only this replacement, while a started
    // claim is left for the caller's fenced related-run evidence.
    match abort_admitted_hitl_resume_operation(
        config,
        lease,
        source_run_id,
        claim_id,
        "HITL continuation failed before effect start",
    ) {
        Ok(HitlResumeAbortOutcome::AbortedBeforeEffect | HitlResumeAbortOutcome::EffectStarted) => {
            start_error
        }
        Err(abort_error) => CliError::Storage(format!(
            "{start_error}; failed to determine HITL effect phase: {abort_error}"
        )),
    }
}

fn abort_admitted_hitl_resume_operation(
    config: CliConfig,
    lease: RunAdmissionLease,
    source_run_id: RunId,
    claim_id: String,
    output_preview: &'static str,
) -> CliResult<HitlResumeAbortOutcome> {
    thread::spawn(move || {
        let store =
            LocalSessionStore::new(config).map_err(|error| CliError::Storage(error.to_string()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| CliError::Run(error.to_string()))?;
        runtime
            .block_on(store.abort_admitted_hitl_resume(
                &lease,
                &source_run_id,
                &claim_id,
                output_preview,
            ))
            .map_err(|error| CliError::Storage(error.to_string()))
    })
    .join()
    .map_err(|_| CliError::Run("HITL resume abort worker panicked".to_string()))?
}

fn run_hitl_resume_claim_operation(
    config: CliConfig,
    operation: HitlResumeClaimOperation,
) -> CliResult<()> {
    thread::spawn(move || {
        let store =
            LocalSessionStore::new(config).map_err(|error| CliError::Storage(error.to_string()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| CliError::Run(error.to_string()))?;
        runtime
            .block_on(async {
                match operation {
                    HitlResumeClaimOperation::Claim(claim) => {
                        store
                            .reconcile_expired_run_admissions(
                                starweaver_session::LOCAL_SESSION_NAMESPACE,
                                Utc::now(),
                            )
                            .await?;
                        store.claim_hitl_resume(claim).await
                    }
                    HitlResumeClaimOperation::StartEffect {
                        lease,
                        source_run_id,
                        claim_id,
                    } => {
                        store
                            .start_hitl_resume_effect(&lease, &source_run_id, &claim_id)
                            .await
                    }
                    HitlResumeClaimOperation::Release {
                        session_id,
                        run_id,
                        claim_id,
                    } => {
                        store
                            .release_hitl_resume_claim(&session_id, &run_id, &claim_id)
                            .await
                    }
                }
            })
            .map_err(|error| CliError::Storage(error.to_string()))
    })
    .join()
    .map_err(|_| CliError::Run("HITL resume claim worker panicked".to_string()))?
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
    status.as_str()
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
