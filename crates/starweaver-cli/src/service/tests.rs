#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::Utc;
use serde_json::json;
use starweaver_core::{AgentId, RunId, SessionId, TaskId};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, AgentStreamSource};
use starweaver_session::{
    ApprovalRecord, DeferredToolRecord, ExecutionStatus, RunStatus, SessionDeletionFence,
    SessionStatus,
};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind,
    DisplayMessageProjector as _, DisplayProjectionContext, ReplayScope, StreamArchive as _,
};

use crate::local_store::{RunSummary, SessionSummary, TrimReport};

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
    assert!(
        render_sessions(&sessions, OutputMode::Text)
            .unwrap()
            .contains("profile=general")
    );
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
    assert!(
        render_session_show(&session, &runs, OutputMode::Text)
            .unwrap()
            .contains("preview=hello")
    );
    assert!(
        render_session_show(&session, &runs, OutputMode::DisplayJsonl)
            .unwrap()
            .contains("run_test")
    );
    assert!(
        render_session_show(&session, &runs, OutputMode::Silent)
            .unwrap()
            .contains("status=shown")
    );

    let report = TrimReport {
        sessions_scanned: 1,
        runs_to_trim: 2,
        runs_trimmed: 1,
        bytes_reclaimed: 3,
        dry_run: false,
    };
    assert!(
        render_trim_report(&report, OutputMode::Text)
            .unwrap()
            .contains("bytes_reclaimed=3")
    );
    assert!(
        render_trim_report(&report, OutputMode::Silent)
            .unwrap()
            .contains("status=trimmed")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
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
    assert!(
        render_display_jsonl(&terminal_only)
            .unwrap()
            .contains("RUN_FINISHED")
    );

    let mut approval = ApprovalRecord::new(
        "approval_test",
        session_id.clone(),
        run_id.clone(),
        "action_test",
        "write",
    );
    approval.status = ApprovalStatus::Expired;
    assert!(
        render_approvals(&[approval.clone()], OutputMode::Text)
            .unwrap()
            .contains("status=expired")
    );
    approval.status = ApprovalStatus::Cancelled;
    assert!(
        render_approvals(&[approval], OutputMode::Silent)
            .unwrap()
            .contains("approvals=1")
    );

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
        assert!(
            render_deferred(&[deferred.clone()], OutputMode::Text)
                .unwrap()
                .contains("deferred_id=deferred_test")
        );
        assert!(
            render_deferred_decision(&deferred, OutputMode::DisplayJsonl)
                .unwrap()
                .contains("deferred_test")
        );
    }
}

#[test]
fn prompt_run_json_preview_skips_internal_compaction_messages() {
    let (session_id, run_id) = ids();
    let completed_json = render_prompt_run_json(&PromptRunExecution {
        session_id: session_id.as_str().to_string(),
        run_id: run_id.as_str().to_string(),
        status: "completed".to_string(),
        output_mode: OutputMode::Json,
        continuation: None,
        messages: vec![
            DisplayMessage::new(
                0,
                session_id.clone(),
                run_id.clone(),
                DisplayMessageKind::RunCompleted,
            )
            .with_payload(json!({"output": "final answer"}))
            .with_preview("final answer"),
            DisplayMessage::new(
                1,
                session_id,
                run_id,
                DisplayMessageKind::CompactionCompleted,
            )
            .with_preview("display compaction completed"),
        ],
    })
    .unwrap();
    let completed_json: serde_json::Value = serde_json::from_str(&completed_json).unwrap();
    assert_eq!(completed_json["outputPreview"], "final answer");
}

#[test]
fn guidance_files_append_project_guidance_and_user_rules_as_transient_guidance() {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("AGENTS.md"), "# Project\nUse cargo test.\n").unwrap();
    std::fs::write(
        global.join("RULES.md"),
        "# Rules\nPrefer Chinese replies.\n",
    )
    .unwrap();

    let mut input = PromptInput::text("implement feature");
    let config = CliConfig {
        global_dir: global.clone(),
        workspace_root: project,
        ..test_config(temp.path())
    };

    append_guidance_files(&mut input, &config);
    assert!(input.extra_text_parts.is_empty());
    assert_eq!(input.guidance_text_parts.len(), 2);
    assert_eq!(
        input.guidance_text_parts[0],
        "<project-guidance name=AGENTS.md>\n# Project\nUse cargo test.\n\n</project-guidance>"
    );
    assert!(input.guidance_text_parts[1].starts_with(&format!(
        "<user-rules location={}>",
        path_absolute_posix(&global.join("RULES.md"))
    )));
    assert!(input.guidance_text_parts[1].contains("Prefer Chinese replies."));
    assert!(input.guidance_text_parts[1].ends_with("</user-rules>"));
}

#[test]
fn explicit_skills_append_ordered_full_bodies_as_guidance() {
    let skill = |name: &str, body: &str| starweaver_agent::SkillPackage {
        name: name.to_string(),
        description: format!("Use {name}"),
        path: format!("/skills/{name}/SKILL.md"),
        body: Some(body.to_string()),
        metadata: serde_json::Map::default(),
    };
    let expanded = crate::slash_commands::ExpandedExplicitSkills {
        prompt: "build it".to_string(),
        skills: vec![
            crate::slash_commands::ExplicitSkillSelection {
                invoked_name: "primary".to_string(),
                package: skill("primary", "Primary workflow"),
            },
            crate::slash_commands::ExplicitSkillSelection {
                invoked_name: "supporting".to_string(),
                package: skill("supporting", "Supporting workflow"),
            },
        ],
    };
    let mut input = PromptInput::text("build it");

    append_explicit_skill_guidance(&mut input, Some(&expanded));

    assert_eq!(input.guidance_text_parts.len(), 3);
    assert!(input.guidance_text_parts[0].contains("primary, supporting"));
    assert!(input.guidance_text_parts[1].contains("Primary workflow"));
    assert!(input.guidance_text_parts[2].contains("Supporting workflow"));
    assert!(input.extra_text_parts.is_empty());
}

#[test]
fn guidance_files_skip_missing_or_blank_files() {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("AGENTS.md"), "  \n").unwrap();

    let mut input = PromptInput::text("hello");
    let config = CliConfig {
        global_dir: global,
        workspace_root: project,
        ..test_config(temp.path())
    };

    append_guidance_files(&mut input, &config);
    assert!(input.extra_text_parts.is_empty());
    assert!(input.guidance_text_parts.is_empty());
}

fn test_config(root: &Path) -> CliConfig {
    let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
    crate::ConfigResolver::for_tests(root)
        .resolve(&cli)
        .unwrap()
}

fn test_run_command(
    session_id: &str,
    source_run_id: Option<&str>,
    hitl_resume: bool,
) -> RunCommand {
    RunCommand {
        prompt: Some("continue".to_string()),
        prompt_parts: Vec::new(),
        session: Some(session_id.to_string()),
        continue_session: false,
        new_session: false,
        run: source_run_id.map(ToString::to_string),
        branch_from: None,
        profile: Some("general".to_string()),
        continuation_mode: crate::args::ContinuationModeArg::Switch,
        output: Some(OutputMode::Silent),
        hitl: None,
        goal: None,
        worker: None,
        worker_label: None,
        worktree: None,
        worktree_name: None,
        branch: None,
        session_affinity_id: None,
        environment_attachments: Vec::new(),
        hitl_resume,
    }
}

#[test]
fn implicit_session_selection_is_scoped_to_the_current_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let shared_global = temp.path().join("global");
    let shared_database = starweaver_storage::canonical_session_database_path(&shared_global);
    let mut config_a = test_config(&temp.path().join("workspace-a"));
    config_a.global_dir = shared_global.clone();
    config_a.database_path = shared_database.clone();
    let mut config_b = test_config(&temp.path().join("workspace-b"));
    config_b.global_dir = shared_global;
    config_b.database_path = shared_database;

    let session_a = LocalStore::open(&config_a)
        .unwrap()
        .create_session("general", Some("Workspace A".to_string()))
        .unwrap();
    let session_b = LocalStore::open(&config_b)
        .unwrap()
        .create_session("general", Some("Workspace B".to_string()))
        .unwrap();

    let store_a = LocalStore::open(&config_a).unwrap();
    let listed_a = store_a.list_sessions(10).unwrap();
    assert_eq!(listed_a.len(), 1);
    assert_eq!(listed_a[0].session_id, session_a.session_id.as_str());
    assert_eq!(
        store_a.latest_session().unwrap().unwrap().session_id,
        session_a.session_id
    );
    assert!(
        store_a
            .load_workspace_session(session_b.session_id.as_str())
            .is_err()
    );

    let store_b = LocalStore::open(&config_b).unwrap();
    let listed_b = store_b.list_sessions(10).unwrap();
    assert_eq!(listed_b.len(), 1);
    assert_eq!(listed_b[0].session_id, session_b.session_id.as_str());
}

#[test]
fn invalid_current_session_pointer_cannot_escape_workspace_for_resume_or_trim() {
    let temp = tempfile::tempdir().unwrap();
    let shared_global = temp.path().join("global");
    let shared_database = starweaver_storage::canonical_session_database_path(&shared_global);
    let mut config_a = test_config(&temp.path().join("workspace-a"));
    config_a.global_dir = shared_global.clone();
    config_a.database_path = shared_database.clone();
    let mut config_b = test_config(&temp.path().join("workspace-b"));
    config_b.global_dir = shared_global;
    config_b.database_path = shared_database;

    let session_a = LocalStore::open(&config_a)
        .unwrap()
        .create_session("general", Some("Workspace A".to_string()))
        .unwrap();
    let session_b = LocalStore::open(&config_b)
        .unwrap()
        .create_session("general", Some("Workspace B".to_string()))
        .unwrap();
    write_current_session(&config_a, session_b.session_id.as_str()).unwrap();

    let mut service = CliService::open(config_a).unwrap();
    assert_eq!(
        service.resolve_session_id(None).unwrap(),
        session_a.session_id.as_str()
    );
    let trim = service
        .session(SessionCommand::Trim(crate::args::SessionTrimCommand {
            current: true,
            all: false,
            session: None,
            keep_runs: 0,
            older_than: None,
            dry_run: true,
            output: OutputMode::Json,
        }))
        .unwrap();
    let report: serde_json::Value = serde_json::from_str(&trim).unwrap();
    assert_eq!(report["sessions_scanned"], 0);
}

#[test]
fn reset_tombstones_only_current_workspace_and_preserves_shared_database() {
    let temp = tempfile::tempdir().unwrap();
    let shared_global = temp.path().join("global");
    let shared_database = starweaver_storage::canonical_session_database_path(&shared_global);
    let mut config_a = test_config(&temp.path().join("workspace-a"));
    config_a.global_dir = shared_global.clone();
    config_a.database_path = shared_database.clone();
    let mut config_b = test_config(&temp.path().join("workspace-b"));
    config_b.global_dir = shared_global;
    config_b.database_path = shared_database.clone();

    let session_a = LocalStore::open(&config_a)
        .unwrap()
        .create_session("general", Some("Workspace A".to_string()))
        .unwrap();
    let session_b = LocalStore::open(&config_b)
        .unwrap()
        .create_session("general", Some("Workspace B".to_string()))
        .unwrap();

    let mut service = CliService::open(config_a).unwrap();
    service
        .reset(&crate::args::ResetCommand {
            yes: true,
            output: OutputMode::Json,
        })
        .unwrap();

    assert!(shared_database.exists());
    let store = starweaver_storage::SqliteStorage::open(&shared_database).unwrap();
    assert_eq!(
        store.load_session(&session_a.session_id).unwrap().status,
        SessionStatus::Deleted
    );
    assert_eq!(
        store.load_session(&session_b.session_id).unwrap().status,
        SessionStatus::Active
    );
}

#[test]
fn ordinary_active_run_rejects_cross_connection_admission() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let (session_id, first_lease) = {
        let mut store = LocalStore::open(&config).unwrap();
        let session = store
            .create_session("general", Some("Exclusive admission".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        let (_, lease) = store
            .admit_run(
                &session_id,
                "first active run".to_string(),
                None,
                "general",
                serde_json::Map::new(),
                "cli-host-a",
                None,
                None,
            )
            .unwrap();
        (session_id, lease)
    };

    let mut competing = LocalStore::open(&config).unwrap();
    let error = competing
        .admit_run(
            &session_id,
            "competing run".to_string(),
            None,
            "general",
            serde_json::Map::new(),
            "rpc-host-b",
            None,
            None,
        )
        .expect_err("a second host must not acquire the same session");
    assert!(error.to_string().contains("active run"));

    let owner = LocalStore::open(&config).unwrap();
    owner.release_run_admission(&first_lease).unwrap();
}

#[test]
fn delete_session_is_fenced_by_active_work_and_tombstones_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();

    let active = store
        .create_session("general", Some("Active deletion fence".to_string()))
        .unwrap();
    let active_id = active.session_id.as_str().to_string();
    let (_, lease) = store
        .admit_run(
            &active_id,
            "still running".to_string(),
            None,
            "general",
            serde_json::Map::new(),
            "cli-delete-test-host",
            None,
            None,
        )
        .unwrap();
    let error = store
        .delete_session(&active_id)
        .expect_err("active work must fence deletion");
    assert!(error.to_string().contains("active run"));
    assert_eq!(
        store.load_session(&active_id).unwrap().deletion_fence,
        SessionDeletionFence::Stable,
        "failed deletion must not leave a partial fence"
    );
    store.release_run_admission(&lease).unwrap();

    let deletable = store
        .create_session("general", Some("Durable tombstone".to_string()))
        .unwrap();
    let deletable_id = deletable.session_id.as_str().to_string();
    assert!(store.delete_session(&deletable_id).unwrap());
    let tombstone = store
        .load_session(&deletable_id)
        .expect("tombstoned record remains durable");
    assert_eq!(tombstone.status, SessionStatus::Deleted);
    assert!(matches!(
        tombstone.deletion_fence,
        SessionDeletionFence::Deleted { .. }
    ));
    let stale_blob_dir = config
        .file_store_path
        .join("sessions")
        .join(&deletable_id)
        .join("runs")
        .join("stale");
    std::fs::create_dir_all(&stale_blob_dir).unwrap();
    std::fs::write(stale_blob_dir.join("display.compact.json"), b"stale").unwrap();
    assert!(!store.delete_session(&deletable_id).unwrap());
    assert!(
        !config
            .file_store_path
            .join("sessions")
            .join(&deletable_id)
            .exists(),
        "a tombstone retry must retry CLI-owned blob cleanup"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn hitl_resume_preflight_rejects_invalid_evidence_before_claim_or_run_allocation() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let (session_id, source_run_id, terminal_run_id) = {
        let mut store = LocalStore::open(&config).unwrap();
        let session = store
            .create_session("general", Some("HITL admission".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        let mut terminal = store
            .append_run(&session_id, "terminal".to_string(), None, "general")
            .unwrap();
        let terminal_run_id = terminal.run_id.as_str().to_string();
        store
            .complete_run(
                &mut terminal,
                "done".to_string(),
                crate::local_store::RunArtifacts {
                    state: starweaver_context::ResumableState::default(),
                    environment_state: None,
                    raw_records: Vec::new(),
                    checkpoints: Vec::new(),
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
                    terminal_error: None,
                },
            )
            .unwrap();
        let mut source = store
            .append_run(&session_id, "waiting source".to_string(), None, "general")
            .unwrap();
        let source_run_id = source.run_id.as_str().to_string();
        store
            .complete_run(
                &mut source,
                "waiting".to_string(),
                crate::local_store::RunArtifacts {
                    state: starweaver_context::ResumableState::default(),
                    environment_state: None,
                    raw_records: Vec::new(),
                    checkpoints: Vec::new(),
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Waiting,
                    terminal_error: None,
                },
            )
            .unwrap();
        (session_id, source_run_id, terminal_run_id)
    };

    let mut invalid = CliService::open(config.clone()).unwrap();
    let error = invalid
        .prepare_prompt_run(
            &test_run_command(&session_id, Some(&source_run_id), true),
            None,
        )
        .err()
        .expect("invalid durable waiting evidence must fail preflight");
    assert!(error.to_string().contains("no resumable checkpoint"));

    let source_id = RunId::from_string(&source_run_id);
    let claim_id = deterministic_hitl_resume_claim_id(&session_id, &source_id);
    run_hitl_resume_claim_operation(
        config.clone(),
        HitlResumeClaimOperation::Claim(HitlResumeClaim::new(
            claim_id.clone(),
            SessionId::from_string(&session_id),
            source_id.clone(),
            Utc::now(),
        )),
    )
    .expect("failed validation must not acquire the deterministic claim");
    run_hitl_resume_claim_operation(
        config.clone(),
        HitlResumeClaimOperation::Release {
            session_id: SessionId::from_string(&session_id),
            run_id: source_id,
            claim_id,
        },
    )
    .unwrap();

    let mut ordinary = CliService::open(config.clone()).unwrap();
    let ordinary_error = ordinary
        .prepare_prompt_run(&test_run_command(&session_id, None, false), None)
        .err()
        .expect("ordinary admission must be rejected");
    assert!(ordinary_error.to_string().contains("explicit HITL resume"));

    let mut explicit = CliService::open(config.clone()).unwrap();
    let explicit_error = explicit
        .prepare_prompt_run(
            &test_run_command(&session_id, Some(&terminal_run_id), false),
            None,
        )
        .err()
        .expect("explicit terminal restore must not bypass the waiting source");
    assert!(
        explicit_error
            .to_string()
            .contains("continuing waiting run")
    );

    assert_eq!(
        LocalStore::open(&config)
            .unwrap()
            .list_run_records(&session_id)
            .unwrap()
            .len(),
        2,
        "rejected preflight paths must not allocate orphan runs"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn hitl_resume_preflight_reconciles_expired_started_orphan_before_new_claim() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let (session_id, source) = {
        let mut store = LocalStore::open(&config).unwrap();
        let session = store
            .create_session("general", Some("Expired HITL orphan".to_string()))
            .unwrap();
        let mut source = store
            .append_run(
                session.session_id.as_str(),
                "waiting source".to_string(),
                None,
                "general",
            )
            .unwrap();
        store
            .complete_run(
                &mut source,
                "waiting".to_string(),
                crate::local_store::RunArtifacts {
                    state: starweaver_context::ResumableState::default(),
                    environment_state: None,
                    raw_records: Vec::new(),
                    checkpoints: Vec::new(),
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Waiting,
                    terminal_error: None,
                },
            )
            .unwrap();
        (session.session_id, source)
    };
    let claim_id = deterministic_hitl_resume_claim_id(session_id.as_str(), &source.run_id);
    let mut replacement = RunRecord::new(
        session_id.clone(),
        RunId::from_string("expired-cli-hitl-replacement"),
        starweaver_core::ConversationId::new(),
    );
    replacement.restore_from_run_id = Some(source.run_id.clone());
    let request = starweaver_session::AcquireRunAdmission {
        run: replacement,
        namespace_id: starweaver_session::LOCAL_SESSION_NAMESPACE.to_string(),
        host_instance_id: "expired-cli-host".to_string(),
        admission_id: "expired-cli-admission".to_string(),
        lease_expires_at: Utc::now() + chrono::Duration::seconds(30),
        idempotency_key: "expired-cli-key".to_string(),
        command_fingerprint: "expired-cli-fingerprint".to_string(),
        replaces_waiting_run_id: Some(source.run_id.clone()),
        hitl_resume_claim_id: Some(claim_id.clone()),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let adapter = LocalSessionStore::new(config.clone()).unwrap();
    runtime
        .block_on(adapter.claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        )))
        .unwrap();
    let receipt = runtime
        .block_on(adapter.acquire_run_admission(request.clone()))
        .unwrap();
    runtime
        .block_on(adapter.start_hitl_resume_effect(&receipt.lease, &source.run_id, &claim_id))
        .expect("start the admitted continuation before simulating process loss");
    runtime
        .block_on(
            adapter
                .heartbeat_run_admission(&receipt.lease, Utc::now() - chrono::Duration::seconds(1)),
        )
        .expect("expire the started continuation after its effect boundary");

    let mut service = CliService::open(config.clone()).unwrap();
    service
        .prepare_prompt_run(
            &test_run_command(session_id.as_str(), Some(source.run_id.as_str()), true),
            None,
        )
        .err()
        .expect("preflight must fail after terminalizing the expired started orphan");

    let store = LocalStore::open(&config).unwrap();
    for run_id in [&source.run_id, &receipt.run.run_id] {
        assert_eq!(
            store
                .load_run(session_id.as_str(), run_id.as_str())
                .unwrap()
                .status,
            RunStatus::Cancelled
        );
    }
    assert!(
        store
            .load_session(session_id.as_str())
            .unwrap()
            .active_run_id
            .is_none()
    );
    assert!(
        runtime
            .block_on(adapter.load_run_admission(&receipt.lease.target))
            .unwrap()
            .is_none()
    );
    runtime
        .block_on(adapter.mark_hitl_resume_started(&session_id, &source.run_id, &claim_id))
        .expect_err("preflight reconciliation must consume the started claim");
    let replay = runtime
        .block_on(adapter.acquire_run_admission(request))
        .expect("exact retry must return only the durable receipt");
    assert!(replay.idempotent_replay);
    assert!(
        runtime
            .block_on(adapter.load_run_admission(&receipt.lease.target))
            .unwrap()
            .is_none(),
        "exact retry must not restore the admission or replay the effect"
    );
    assert_eq!(
        store.list_run_records(session_id.as_str()).unwrap().len(),
        2
    );
}

#[test]
fn cli_hitl_resume_claim_identity_is_deterministic_per_waiting_source() {
    let session_id = "session-deterministic";
    let source = RunId::from_string("run-waiting");
    assert_eq!(
        deterministic_hitl_resume_claim_id(session_id, &source),
        deterministic_hitl_resume_claim_id(session_id, &source)
    );
    assert_ne!(
        deterministic_hitl_resume_claim_id(session_id, &source),
        deterministic_hitl_resume_claim_id(session_id, &RunId::from_string("run-other"))
    );
}

#[test]
fn resume_terminal_head_continues_without_hitl_claim_or_orphan_run() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let session_id = {
        let mut store = LocalStore::open(&config).unwrap();
        let session = store
            .create_session("general", Some("Terminal resume".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        let mut source = store
            .append_run(&session_id, "completed source".to_string(), None, "general")
            .unwrap();
        store
            .complete_run(
                &mut source,
                "completed".to_string(),
                crate::local_store::RunArtifacts {
                    state: starweaver_context::ResumableState::default(),
                    environment_state: None,
                    raw_records: Vec::new(),
                    checkpoints: Vec::new(),
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
                    terminal_error: None,
                },
            )
            .unwrap();
        session_id
    };

    let mut service = CliService::open(config.clone()).unwrap();
    service
        .resume(&ResumeCommand {
            session: Some(session_id.clone()),
            run: None,
            prompt: "continue terminal head".to_string(),
            output: Some(OutputMode::Silent),
            hitl: None,
            continuation_mode: crate::args::ContinuationModeArg::Switch,
        })
        .unwrap();

    let store = LocalStore::open(&config).unwrap();
    let runs = store.list_run_records(&session_id).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].status, RunStatus::Completed);
    assert!(
        store
            .load_session(&session_id)
            .unwrap()
            .active_run_id
            .is_none()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn hitl_resume_claim_allows_only_one_continuation_and_consumes_source_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("HITL resume".to_string()))
        .unwrap();
    let session_id = session.session_id;
    let mut source = store
        .append_run(
            session_id.as_str(),
            "waiting source".to_string(),
            None,
            "general",
        )
        .unwrap();
    store
        .complete_run(
            &mut source,
            "waiting".to_string(),
            crate::local_store::RunArtifacts {
                state: starweaver_context::ResumableState::default(),
                environment_state: None,
                raw_records: Vec::new(),
                checkpoints: Vec::new(),
                display_messages: Vec::new(),
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Waiting,
                terminal_error: None,
            },
        )
        .unwrap();

    let claim_id = "claim-winner".to_string();
    run_hitl_resume_claim_operation(
        config.clone(),
        HitlResumeClaimOperation::Claim(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        )),
    )
    .unwrap();
    let duplicate = run_hitl_resume_claim_operation(
        config.clone(),
        HitlResumeClaimOperation::Claim(HitlResumeClaim::new(
            "claim-loser".to_string(),
            session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        )),
    );
    assert!(duplicate.is_err());
    let (mut continuation, admission) = store
        .admit_run(
            session_id.as_str(),
            "continue".to_string(),
            Some(source.run_id.as_str().to_string()),
            "general",
            serde_json::Map::new(),
            "cli-test-host",
            Some(source.run_id.clone()),
            Some(claim_id.clone()),
        )
        .unwrap();
    run_hitl_resume_claim_operation(
        config,
        HitlResumeClaimOperation::StartEffect {
            lease: admission.clone(),
            source_run_id: source.run_id.clone(),
            claim_id: claim_id.clone(),
        },
    )
    .expect("start the admitted continuation before committing its effect evidence");
    continuation.metadata.insert(
        HITL_RESUME_CLAIM_ID_METADATA_KEY.to_string(),
        json!(claim_id),
    );
    continuation.metadata.insert(
        HITL_RESUME_SOURCE_RUN_ID_METADATA_KEY.to_string(),
        json!(source.run_id.as_str()),
    );
    store
        .complete_run_fenced(
            &mut continuation,
            "continued".to_string(),
            crate::local_store::RunArtifacts {
                state: starweaver_context::ResumableState::default(),
                environment_state: None,
                raw_records: Vec::new(),
                checkpoints: Vec::new(),
                display_messages: Vec::new(),
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
                terminal_error: None,
            },
            &admission,
        )
        .unwrap();

    assert_eq!(
        store
            .load_run(session_id.as_str(), source.run_id.as_str())
            .unwrap()
            .status,
        RunStatus::Completed
    );
}

#[test]
fn failed_run_complete_persists_restore_state_for_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
    let config = crate::ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .unwrap();
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Failed run".to_string()))
        .unwrap();
    let session_id = session.session_id.as_str().to_string();
    let mut run = store
        .append_run(
            &session_id,
            "fail after progress".to_string(),
            None,
            "general",
        )
        .unwrap();
    let mut state = starweaver_context::ResumableState::default();
    state
        .message_history
        .push(starweaver_model::ModelMessage::Request(
            starweaver_model::ModelRequest {
                parts: vec![starweaver_model::ModelRequestPart::UserPrompt {
                    content: vec![starweaver_model::ContentPart::Text {
                        text: "fail after progress".to_string(),
                    }],
                    name: None,
                    metadata: serde_json::Map::default(),
                }],
                timestamp: None,
                instructions: None,
                run_id: Some(run.run_id.clone()),
                conversation_id: Some(run.conversation_id.clone()),
                metadata: serde_json::Map::default(),
            },
        ));
    let run_session_id = run.session_id.clone();
    let run_id = run.run_id.clone();
    store
        .complete_run(
            &mut run,
            "step limit exceeded after 1 steps".to_string(),
            crate::local_store::RunArtifacts {
                state,
                environment_state: None,
                raw_records: Vec::new(),
                checkpoints: Vec::new(),
                display_messages: vec![DisplayMessage::new(
                    0,
                    run_session_id,
                    run_id,
                    DisplayMessageKind::RunFailed,
                )],
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Failed,
                terminal_error: None,
            },
        )
        .unwrap();

    let saved_run = store.load_run(&session_id, run.run_id.as_str()).unwrap();
    assert_eq!(saved_run.status, RunStatus::Failed);
    assert_eq!(saved_run.output_preview, None);
    assert_eq!(
        saved_run
            .terminal_error
            .as_ref()
            .map(|error| error.message.as_str()),
        Some("step limit exceeded after 1 steps")
    );
    let saved_session = store.load_session(&session_id).unwrap();
    assert_eq!(saved_session.head_run_id.as_ref(), Some(&run.run_id));
    assert_eq!(saved_session.active_run_id, None);
    let restored = store
        .load_restore_state(&session_id, Some(run.run_id.as_str()))
        .unwrap()
        .unwrap();
    assert_eq!(restored.message_history.len(), 1);
}

#[test]
fn compatibility_mirror_failure_does_not_reclassify_canonical_completion() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_config(temp.path());
    config.file_store_path = temp.path().join("compatibility-mirror");
    let mirror_root = config.file_store_path.clone();
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Mirror failure".to_string()))
        .unwrap();
    let session_id = session.session_id.as_str().to_string();
    let mut run = store
        .append_run(
            &session_id,
            "finish canonically".to_string(),
            None,
            "general",
        )
        .unwrap();
    let run_id = run.run_id.clone();
    let display = vec![DisplayMessage::new(
        0,
        run.session_id.clone(),
        run_id.clone(),
        DisplayMessageKind::RunCompleted,
    )];

    std::fs::remove_dir_all(&mirror_root).unwrap();
    std::fs::write(&mirror_root, b"blocks compatibility mirror directories").unwrap();

    let returned = store
        .complete_run(
            &mut run,
            "done".to_string(),
            crate::local_store::RunArtifacts {
                state: starweaver_context::ResumableState::default(),
                environment_state: None,
                raw_records: Vec::new(),
                checkpoints: Vec::new(),
                display_messages: display,
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
                terminal_error: None,
            },
        )
        .expect("canonical completion must not fail with its optional mirror");

    assert_eq!(returned.len(), 1);
    let saved = store.load_run(&session_id, run_id.as_str()).unwrap();
    assert_eq!(saved.status, RunStatus::Completed);
    assert_eq!(
        store
            .replay_display(&session_id, Some(run_id.as_str()), None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn complete_run_stores_source_attributed_display_messages_under_parent_run() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Source display".to_string()))
        .unwrap();
    let session_id = session.session_id.as_str().to_string();
    let mut run = store
        .append_run(&session_id, "delegate".to_string(), None, "general")
        .unwrap();
    let parent_run_id = run.run_id.as_str().to_string();
    let child_run_id = RunId::from_string("run_child_model_transport_fallback");
    let message = DisplayMessage::new(
        0,
        run.session_id.clone(),
        child_run_id.clone(),
        DisplayMessageKind::HostEvent,
    )
    .with_payload(json!({
        "from": "websocket",
        "to": "http",
        "reason": "websocket_transport_error",
        "detail": "websocket closed before response.completed",
        "message": "model transport: websocket -> http fallback (websocket_transport_error)"
    }))
    .with_preview("model transport: websocket -> http fallback (websocket_transport_error)");

    store
        .complete_run(
            &mut run,
            "done".to_string(),
            crate::local_store::RunArtifacts {
                state: starweaver_context::ResumableState::default(),
                environment_state: None,
                raw_records: Vec::new(),
                checkpoints: Vec::new(),
                display_messages: vec![message],
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
                terminal_error: None,
            },
        )
        .unwrap();

    let messages = store
        .replay_display(&session_id, Some(&parent_run_id), None)
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].run_id, child_run_id);
    assert_eq!(messages[0].payload["reason"], "websocket_transport_error");
}

#[test]
fn complete_run_persists_default_projector_source_records_under_parent_run() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Projected source display".to_string()))
        .unwrap();
    let session_id = session.session_id.as_str().to_string();
    let mut run = store
        .append_run(&session_id, "delegate".to_string(), None, "general")
        .unwrap();
    let parent_run_id = run.run_id.clone();
    let child_run_id = RunId::from_string("run_child_projected_transport_fallback");
    let raw_record = AgentStreamRecord::new(
        10,
        AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "model_transport_fallback",
                json!({
                    "from": "websocket",
                    "to": "http",
                    "reason": "websocket_transport_error",
                    "message": "model transport: websocket -> http fallback (websocket_transport_error)"
                }),
            ),
        },
    )
    .with_source(AgentStreamSource::subagent(
        AgentId::from_string("child-agent"),
        "child",
        TaskId::from_string("task-child"),
        Some(child_run_id.clone()),
        Some(parent_run_id.clone()),
        3,
    ));
    let projector = DefaultDisplayMessageProjector;
    let context = DisplayProjectionContext::new(run.session_id.clone(), parent_run_id.clone());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let projected = runtime.block_on(projector.project(&context, &raw_record));
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].kind, DisplayMessageKind::HostEvent);
    assert_eq!(projected[0].session_id.as_str(), session_id);
    assert_eq!(projected[0].run_id, child_run_id);

    store
        .complete_run(
            &mut run,
            "done".to_string(),
            crate::local_store::RunArtifacts {
                state: starweaver_context::ResumableState::default(),
                environment_state: None,
                raw_records: vec![raw_record],
                checkpoints: Vec::new(),
                display_messages: projected,
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
                terminal_error: None,
            },
        )
        .unwrap();

    let replayed = store
        .replay_display(&session_id, Some(parent_run_id.as_str()), None)
        .unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].run_id, child_run_id);
    assert_eq!(replayed[0].metadata["source_task_id"], json!("task-child"));
    assert_eq!(replayed[0].payload["reason"], "websocket_transport_error");
}

#[test]
fn local_stream_archive_stores_run_scoped_source_messages_under_scope_run() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Archive source display".to_string()))
        .unwrap();
    let session_id = session.session_id;
    let run = store
        .append_run(session_id.as_str(), "delegate".to_string(), None, "general")
        .unwrap();
    let parent_run_id = run.run_id;
    drop(store);

    let child_run_id = RunId::from_string("run_child_archive_transport_fallback");
    let message = DisplayMessage::new(
        0,
        session_id.clone(),
        child_run_id.clone(),
        DisplayMessageKind::HostEvent,
    )
    .with_payload(json!({
        "from": "websocket",
        "to": "http",
        "reason": "websocket_transport_error",
        "message": "model transport: websocket -> http fallback (websocket_transport_error)"
    }));
    let archive = crate::LocalStreamArchive::new(config).expect("open CLI stream archive");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let scope = ReplayScope::run(parent_run_id.as_str());

    runtime
        .block_on(archive.append_display_messages(scope.clone(), vec![message]))
        .unwrap();
    let messages = runtime
        .block_on(archive.replay_display_after(&scope, None))
        .unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].run_id, child_run_id);
    assert_eq!(messages[0].payload["reason"], "websocket_transport_error");
}

#[test]
fn local_stream_archive_empty_run_scoped_append_is_noop() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let archive = crate::LocalStreamArchive::new(config).expect("open CLI stream archive");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    runtime
        .block_on(
            archive.append_display_messages(
                ReplayScope::run("missing_run_with_no_messages"),
                Vec::new(),
            ),
        )
        .unwrap();
}

#[test]
fn local_stream_archive_rejects_run_scope_session_mismatch_before_sqlite_fk() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Archive mismatch".to_string()))
        .unwrap();
    let parent_run = store
        .append_run(
            session.session_id.as_str(),
            "delegate".to_string(),
            None,
            "general",
        )
        .unwrap();
    drop(store);

    let other_session_id = SessionId::from_string("session_other_source");
    let message = DisplayMessage::new(
        0,
        other_session_id,
        RunId::from_string("run_child_wrong_session"),
        DisplayMessageKind::HostEvent,
    )
    .with_payload(json!({"reason": "websocket_transport_error"}));
    let archive = crate::LocalStreamArchive::new(config).expect("open CLI stream archive");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error =
        runtime
            .block_on(archive.append_display_messages(
                ReplayScope::run(parent_run.run_id.as_str()),
                vec![message],
            ))
            .unwrap_err();
    let error = error.to_string();

    assert!(error.contains("run scope belongs to session_id"));
    assert!(!error.contains("FOREIGN KEY"));
}

#[test]
fn local_stream_archive_rejects_session_scope_source_run_without_fk_failure() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let mut store = LocalStore::open(&config).unwrap();
    let session = store
        .create_session("general", Some("Session source reject".to_string()))
        .unwrap();
    let session_id = session.session_id;
    store
        .append_run(session_id.as_str(), "delegate".to_string(), None, "general")
        .unwrap();
    drop(store);

    let message = DisplayMessage::new(
        0,
        session_id.clone(),
        RunId::from_string("run_child_session_scope_missing"),
        DisplayMessageKind::HostEvent,
    )
    .with_payload(json!({"reason": "websocket_transport_error"}));
    let archive = crate::LocalStreamArchive::new(config).expect("open CLI stream archive");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime
        .block_on(
            archive
                .append_display_messages(ReplayScope::session(session_id.as_str()), vec![message]),
        )
        .unwrap_err();
    let error = error.to_string();

    assert!(error.contains("not a run in session scope"));
    assert!(!error.contains("FOREIGN KEY"));
}

#[test]
fn tui_model_choices_only_include_configured_models_and_keep_config_details() {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(
        global.join("config.toml"),
        r#"
[general]
model = "openai-responses:gpt-5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"

[model_profiles.codex]
label = "Codex OAuth"
model = "oauth@codex:gpt-5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"
"#,
    )
    .unwrap();
    let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
    let config = crate::ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .unwrap();
    let choices = model_choices(&config);

    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.profile.as_str())
            .collect::<Vec<_>>(),
        vec!["default_model", "codex"]
    );
    assert!(!choices.iter().any(|choice| choice.model_id == "local_echo"));
    assert!(!choices.iter().any(|choice| choice.source == "built-in"));

    let default_model = choices
        .iter()
        .find(|choice| choice.profile == "default_model")
        .unwrap();
    assert_eq!(default_model.model_id, "openai-responses:gpt-5");
    assert_eq!(
        default_model.model_settings.as_deref(),
        Some("openai_responses_high")
    );
    assert_eq!(default_model.model_cfg.as_deref(), Some("gpt5_270k"));
    assert_eq!(default_model.context_window, Some(270_000));

    let codex = choices
        .iter()
        .find(|choice| choice.profile == "codex")
        .unwrap();
    assert_eq!(codex.label.as_deref(), Some("Codex OAuth"));
    assert_eq!(codex.model_id, "oauth@codex:gpt-5");
}

#[test]
fn tui_model_choices_are_empty_without_configured_models() {
    let temp = tempfile::tempdir().unwrap();
    let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
    let config = crate::ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .unwrap();
    assert!(model_choices(&config).is_empty());
}

#[test]
#[allow(clippy::too_many_lines)]
fn tui_session_reload_resolves_prefix_restores_snapshot_and_current_pointer() {
    let temp = tempfile::tempdir().unwrap();
    let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
    let config = crate::ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .unwrap();
    let mut service = CliService::open(config.clone()).unwrap();
    let session_id = {
        let store = service.store().unwrap();
        let session = store
            .create_session("coding", Some("Reload session".to_string()))
            .unwrap();
        let session_id = session.session_id.as_str().to_string();
        let mut run = store
            .append_run(&session_id, "remember this".to_string(), None, "coding")
            .unwrap();
        let messages = vec![
            DisplayMessage::new(
                0,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::AssistantTextDelta,
            )
            .with_payload(json!({"delta":"hello from reload"})),
            DisplayMessage::new(
                1,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::RunCompleted,
            ),
        ];
        store
            .complete_run(
                &mut run,
                "hello from reload".to_string(),
                crate::local_store::RunArtifacts {
                    state: starweaver_context::ResumableState::default(),
                    environment_state: None,
                    raw_records: Vec::new(),
                    checkpoints: Vec::new(),
                    display_messages: messages,
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
                    terminal_error: None,
                },
            )
            .unwrap();
        session_id
    };

    let choices = service.tui_session_choices(10).unwrap();
    let choice = choices
        .iter()
        .find(|choice| choice.session_id == session_id)
        .unwrap();
    assert_eq!(choice.title.as_deref(), Some("Reload session"));
    assert_eq!(choice.profile.as_deref(), Some("coding"));
    assert_eq!(choice.run_count, 1);
    assert_eq!(
        choice.last_output_preview.as_deref(),
        Some("hello from reload")
    );

    let mut state = crate::tui::InteractiveTuiState::welcome(Path::new("/tmp/config"));
    service
        .reload_tui_session(&mut state, &session_id[..16])
        .unwrap();
    assert_eq!(state.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(state.profile, "coding");
    assert!(state.model.contains("openai:gpt-5"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("hello from reload"))
    );
    assert!(
        state.body.iter().any(|line| line == "User: remember this"),
        "reloaded transcript should include durable run input"
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Loaded session"))
    );

    let mut startup_state = crate::tui::InteractiveTuiState::welcome(Path::new("/tmp/config"));
    startup_state.set_profile("general", "General");
    service
        .restore_tui_session(&mut startup_state, &session_id[..16], None, None, false)
        .unwrap();
    assert_eq!(startup_state.profile, state.profile);
    assert_eq!(startup_state.model, state.model);
    assert_eq!(startup_state.session_id, state.session_id);
    assert!(
        !startup_state
            .body
            .iter()
            .any(|line| line.contains("Loaded session")),
        "startup restore should share profile semantics without adding reload-only notice"
    );
    assert_eq!(
        read_current_session(&config).unwrap().as_deref(),
        Some(session_id.as_str())
    );
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
