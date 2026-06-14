#![allow(clippy::unwrap_used)]

use chrono::Utc;
use serde_json::json;
use starweaver_core::{RunId, SessionId};
use starweaver_session::{ApprovalRecord, DeferredToolRecord, ExecutionStatus, RunStatus};
use starweaver_stream::DisplayMessageKind;

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
fn guidance_files_append_project_guidance_and_user_rules_parts() {
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
    assert_eq!(input.extra_text_parts.len(), 2);
    assert_eq!(
        input.extra_text_parts[0],
        "<project-guidance name=AGENTS.md>\n# Project\nUse cargo test.\n\n</project-guidance>"
    );
    assert!(input.extra_text_parts[1].starts_with(&format!(
        "<user-rules location={}>",
        global.join("RULES.md").display()
    )));
    assert!(input.extra_text_parts[1].contains("Prefer Chinese replies."));
    assert!(input.extra_text_parts[1].ends_with("</user-rules>"));
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
}

fn test_config(root: &Path) -> CliConfig {
    let cli = crate::args::parse(["starweaver-cli".to_string()]).unwrap();
    crate::ConfigResolver::for_tests(root)
        .resolve(&cli)
        .unwrap()
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
    assert!(state
        .body
        .iter()
        .any(|line| line.contains("hello from reload")));
    assert!(state
        .body
        .iter()
        .any(|line| line.contains("Loaded session")));
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
