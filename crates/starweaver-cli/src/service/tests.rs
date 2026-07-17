#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::Utc;
use serde_json::json;
use starweaver_core::{AgentId, RunId, SessionId, TaskId};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, AgentStreamSource};
use starweaver_session::{ApprovalRecord, DeferredToolRecord, ExecutionStatus, RunStatus};
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
fn ordinary_active_run_without_waiting_lineage_allows_concurrent_admission() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());
    let session_id = {
        let mut store = LocalStore::open(&config).unwrap();
        let session = store
            .create_session("general", Some("Concurrent admission".to_string()))
            .unwrap();
        store
            .append_run(
                session.session_id.as_str(),
                "ordinary active run".to_string(),
                None,
                "general",
            )
            .unwrap();
        session.session_id.as_str().to_string()
    };

    let mut service = CliService::open(config).unwrap();
    service
        .reject_ordinary_admission_during_waiting_continuation(&session_id, false)
        .unwrap();
}

#[test]
#[allow(clippy::too_many_lines)]
fn hitl_resume_preflight_claim_precedes_run_allocation_and_blocks_ordinary_admission() {
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
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
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
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Waiting,
                },
            )
            .unwrap();
        (session_id, source_run_id, terminal_run_id)
    };

    let mut winner = CliService::open(config.clone()).unwrap();
    let _prepared = winner
        .prepare_prompt_run(
            &test_run_command(&session_id, Some(&source_run_id), true),
            None,
        )
        .unwrap();

    let mut ordinary = CliService::open(config.clone()).unwrap();
    let ordinary_error = ordinary
        .prepare_prompt_run(&test_run_command(&session_id, None, false), None)
        .err()
        .expect("ordinary admission must be rejected");
    assert!(
        ordinary_error
            .to_string()
            .contains("continuing waiting run")
    );

    let mut explicit = CliService::open(config.clone()).unwrap();
    let explicit_error = explicit
        .prepare_prompt_run(
            &test_run_command(&session_id, Some(&terminal_run_id), false),
            None,
        )
        .err()
        .expect("explicit terminal restore must not bypass active HITL continuation");
    assert!(
        explicit_error
            .to_string()
            .contains("continuing waiting run")
    );

    let mut duplicate = CliService::open(config.clone()).unwrap();
    let duplicate_error = duplicate
        .prepare_prompt_run(
            &test_run_command(&session_id, Some(&source_run_id), true),
            None,
        )
        .err()
        .expect("duplicate HITL admission must be rejected");
    assert!(duplicate_error.to_string().contains("active resume claim"));

    assert_eq!(
        LocalStore::open(&config)
            .unwrap()
            .list_run_records(&session_id)
            .unwrap()
            .len(),
        3,
        "rejected admissions must not allocate orphan runs"
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
                    display_messages: Vec::new(),
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
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
                display_messages: Vec::new(),
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Waiting,
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
    run_hitl_resume_claim_operation(
        config,
        HitlResumeClaimOperation::Start {
            session_id: session_id.clone(),
            run_id: source.run_id.clone(),
            claim_id: claim_id.clone(),
        },
    )
    .unwrap();

    let mut continuation = store
        .append_run(
            session_id.as_str(),
            "continue".to_string(),
            Some(source.run_id.as_str().to_string()),
            "general",
        )
        .unwrap();
    continuation.metadata.insert(
        HITL_RESUME_CLAIM_ID_METADATA_KEY.to_string(),
        json!(claim_id),
    );
    continuation.metadata.insert(
        HITL_RESUME_SOURCE_RUN_ID_METADATA_KEY.to_string(),
        json!(source.run_id.as_str()),
    );
    store
        .complete_run(
            &mut continuation,
            "continued".to_string(),
            crate::local_store::RunArtifacts {
                state: starweaver_context::ResumableState::default(),
                environment_state: None,
                raw_records: Vec::new(),
                display_messages: Vec::new(),
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
            },
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
            },
        )
        .unwrap();

    let saved_run = store.load_run(&session_id, run.run_id.as_str()).unwrap();
    assert_eq!(saved_run.status, RunStatus::Failed);
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
                display_messages: display,
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
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
                display_messages: vec![message],
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
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
                display_messages: projected,
                display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                approvals: Vec::new(),
                deferred_tools: Vec::new(),
                status: RunStatus::Completed,
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
                    display_messages: messages,
                    display_snapshot: starweaver_stream::ReplaySnapshot::default(),
                    approvals: Vec::new(),
                    deferred_tools: Vec::new(),
                    status: RunStatus::Completed,
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
