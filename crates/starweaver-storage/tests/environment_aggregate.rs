//! Focused durable environment aggregate integration tests.

#![allow(clippy::too_many_arguments, clippy::unwrap_used)]

use chrono::{Duration, TimeZone, Utc};
use rusqlite::Connection;
use starweaver_session::{
    AttachEnvironment, DetachEnvironment, DurableEnvironmentScope, DurableEnvironmentStatus,
    DurableHostEventScope, EnvironmentAttachmentPageKey, EnvironmentAttachmentQuery,
    EnvironmentHostEventContext, EnvironmentMountQuery, EnvironmentMutationContext,
    MountEnvironmentResource, SessionStore, SessionStoreError, UnmountEnvironmentResource,
};
use starweaver_storage::SqliteStorage;

fn time(offset: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000 + offset, 0).unwrap()
}

fn context(
    authority: &str,
    key: &str,
    fingerprint: &str,
    offset: i64,
    event: bool,
) -> EnvironmentMutationContext {
    let occurred_at = time(offset);
    EnvironmentMutationContext {
        authority_binding: authority.to_string(),
        idempotency_key: key.to_string(),
        command_fingerprint: fingerprint.to_string(),
        occurred_at,
        host_event: event.then(|| EnvironmentHostEventContext {
            transition_identity: format!("{authority}:{key}"),
            scope: DurableHostEventScope::Global,
        }),
    }
}

fn attach(
    authority: &str,
    key: &str,
    fingerprint: &str,
    attachment_id: &str,
    environment_id: &str,
    scope: DurableEnvironmentScope,
    offset: i64,
    event: bool,
) -> AttachEnvironment {
    AttachEnvironment {
        context: context(authority, key, fingerprint, offset, event),
        attachment_id: attachment_id.to_string(),
        environment_id: environment_id.to_string(),
        display_name: Some(format!("{environment_id} display")),
        scope,
        status: DurableEnvironmentStatus::Ready,
    }
}

#[tokio::test]
async fn attachment_state_receipt_and_event_are_atomic_and_replay_exactly() {
    let storage = SqliteStorage::in_memory().unwrap();
    let command = attach(
        "authority-a",
        "attach-key",
        "sha256:attach",
        "attachment-a",
        "configured-a",
        DurableEnvironmentScope::Connection {
            connection_id: "connection-a".to_string(),
        },
        0,
        true,
    );

    let first = storage.attach_environment(command.clone()).unwrap();
    assert_eq!(first.attachment.revision, 1);
    assert!(!first.receipt.replayed);
    let replay = storage.attach_environment(command).unwrap();
    assert_eq!(replay.attachment, first.attachment);
    assert!(replay.receipt.replayed);

    let pending = storage
        .session_store()
        .pending_host_event_publications(10)
        .await
        .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "exact retry must not duplicate the outbox row"
    );
    assert_eq!(
        pending[0].projection,
        serde_json::json!({
            "kind": "environment_changed",
            "attachment": {
                "attachmentId": "attachment-a",
                "displayName": "configured-a display",
                "environmentId": "configured-a",
                "revision": "1",
                "scope": {"kind": "connection"},
                "status": "ready"
            }
        })
    );

    let conflict = storage
        .attach_environment(attach(
            "authority-a",
            "attach-key",
            "sha256:different",
            "attachment-other",
            "configured-a",
            DurableEnvironmentScope::Connection {
                connection_id: "connection-a".to_string(),
            },
            1,
            false,
        ))
        .unwrap_err();
    assert!(matches!(
        conflict,
        SessionStoreError::IdempotencyConflict(_)
    ));
}

#[test]
fn receipt_first_attachment_replay_needs_no_provider_state() {
    let storage = SqliteStorage::in_memory().unwrap();
    let command = attach(
        "authority-a",
        "attach-key",
        "sha256:attach",
        "attachment-a",
        "configured-a",
        DurableEnvironmentScope::Session {
            session_id: "session-a".to_string(),
        },
        0,
        false,
    );
    let first = storage.attach_environment(command).unwrap();

    let replay = storage
        .replay_environment_attachment_mutation(
            "authority-a",
            "attach-key",
            "sha256:attach",
            starweaver_session::ENVIRONMENT_ATTACH_OPERATION,
        )
        .unwrap()
        .unwrap();
    assert_eq!(replay.attachment, first.attachment);
    assert!(replay.receipt.replayed);

    let conflict = storage
        .replay_environment_attachment_mutation(
            "authority-a",
            "attach-key",
            "sha256:changed",
            starweaver_session::ENVIRONMENT_ATTACH_OPERATION,
        )
        .unwrap_err();
    assert!(matches!(
        conflict,
        SessionStoreError::IdempotencyConflict(_)
    ));
}

#[tokio::test]
async fn connection_revocation_rolls_back_every_attachment_on_late_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connection-revoke.sqlite3");
    let storage = SqliteStorage::open(&path).unwrap();
    let scope = DurableEnvironmentScope::Connection {
        connection_id: "connection-a".to_string(),
    };
    for suffix in ["a", "b"] {
        storage
            .attach_environment(attach(
                "authority-a",
                &format!("attach-{suffix}"),
                &format!("sha256:attach-{suffix}"),
                &format!("attachment-{suffix}"),
                "configured-a",
                scope.clone(),
                0,
                false,
            ))
            .unwrap();
    }
    let commands = ["a", "b"]
        .into_iter()
        .map(|suffix| DetachEnvironment {
            context: context(
                "authority-a",
                &format!("revoke-{suffix}"),
                &format!("sha256:revoke-{suffix}"),
                1,
                true,
            ),
            attachment_id: format!("attachment-{suffix}"),
        })
        .collect::<Vec<_>>();
    let trigger = Connection::open(&path).unwrap();
    trigger
        .execute_batch(
            "CREATE TRIGGER fail_second_connection_detach
             BEFORE UPDATE OF status ON environment_attachment_records
             WHEN NEW.attachment_id = 'attachment-b' AND NEW.status = 'detached'
             BEGIN SELECT RAISE(ABORT, 'injected late connection detach failure'); END;",
        )
        .unwrap();

    assert!(
        storage
            .detach_connection_environments("authority-a", "connection-a", commands.clone(),)
            .is_err()
    );
    for suffix in ["a", "b"] {
        assert_eq!(
            storage
                .get_environment_attachment("authority-a", &format!("attachment-{suffix}"),)
                .unwrap()
                .unwrap()
                .status,
            DurableEnvironmentStatus::Ready
        );
    }
    assert!(
        storage
            .session_store()
            .pending_host_event_publications(10)
            .await
            .unwrap()
            .is_empty()
    );
    trigger
        .execute_batch("DROP TRIGGER fail_second_connection_detach;")
        .unwrap();

    let detached = storage
        .detach_connection_environments("authority-a", "connection-a", commands)
        .unwrap();
    assert_eq!(detached.len(), 2);
    assert!(detached.iter().all(|result| {
        result.attachment.status == DurableEnvironmentStatus::Detached && !result.receipt.replayed
    }));
}

#[tokio::test]
async fn concurrent_mount_events_project_transaction_final_attachment_revisions() {
    let storage = SqliteStorage::in_memory().unwrap();
    storage
        .attach_environment(attach(
            "authority-a",
            "attach",
            "fp-attach",
            "attachment-a",
            "configured-a",
            DurableEnvironmentScope::Session {
                session_id: "session-a".to_string(),
            },
            0,
            false,
        ))
        .unwrap();
    let commands = [1_i64, 2_i64].map(|index| {
        let mut command = MountEnvironmentResource {
            context: context(
                "authority-a",
                &format!("mount-{index}"),
                &format!("fp-mount-{index}"),
                index,
                true,
            ),
            mount_id: format!("mount-{index}"),
            attachment_id: "attachment-a".to_string(),
            session_id: "session-a".to_string(),
            run_id: "run-a".to_string(),
            connection_id: None,
            resource_label: format!("Safe dataset {index}"),
        };
        command.context.host_event.as_mut().unwrap().scope = DurableHostEventScope::run(
            starweaver_core::SessionId::from_string("session-a"),
            starweaver_core::RunId::from_string("run-a"),
        );
        command
    });
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let handles = commands.map(|command| {
        let storage = storage.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            storage.mount_environment_resource(command).unwrap()
        })
    });
    barrier.wait();
    for handle in handles {
        handle.join().unwrap();
    }

    let mut revisions = storage
        .session_store()
        .pending_host_event_publications(10)
        .await
        .unwrap()
        .into_iter()
        .map(|publication| {
            assert_eq!(
                publication.projection["kind"],
                serde_json::json!("environment_changed")
            );
            publication.projection["attachment"]["revision"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .collect::<Vec<_>>();
    revisions.sort_unstable();
    assert_eq!(revisions, vec![2, 3]);
    assert_eq!(
        storage
            .get_environment_attachment("authority-a", "attachment-a")
            .unwrap()
            .unwrap()
            .revision,
        3
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn mount_unmount_and_detach_enforce_scope_lifecycle_and_revisions() {
    let storage = SqliteStorage::in_memory().unwrap();
    storage
        .attach_environment(attach(
            "authority-a",
            "attach",
            "fp-attach",
            "attachment-a",
            "configured-a",
            DurableEnvironmentScope::Session {
                session_id: "session-a".to_string(),
            },
            0,
            false,
        ))
        .unwrap();

    let wrong_scope = storage
        .mount_environment_resource(MountEnvironmentResource {
            context: context("authority-a", "wrong", "fp-wrong", 1, false),
            mount_id: "mount-wrong".to_string(),
            attachment_id: "attachment-a".to_string(),
            session_id: "session-b".to_string(),
            run_id: "run-b".to_string(),
            connection_id: None,
            resource_label: "Safe dataset".to_string(),
        })
        .unwrap_err();
    assert!(matches!(wrong_scope, SessionStoreError::Conflict(_)));

    let mounted = storage
        .mount_environment_resource(MountEnvironmentResource {
            context: context("authority-a", "mount", "fp-mount", 2, true),
            mount_id: "mount-a".to_string(),
            attachment_id: "attachment-a".to_string(),
            session_id: "session-a".to_string(),
            run_id: "run-a".to_string(),
            connection_id: None,
            resource_label: "Safe dataset".to_string(),
        })
        .unwrap();
    assert_eq!(mounted.mount.revision, 1);
    assert_eq!(mounted.mount.resource_label, "Safe dataset");
    assert_eq!(
        storage
            .get_environment_attachment("authority-a", "attachment-a")
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        storage
            .list_environment_mounts(EnvironmentMountQuery {
                authority_binding: "authority-a".to_string(),
                session_id: "session-a".to_string(),
                run_id: "run-a".to_string(),
                limit: 100,
            })
            .unwrap(),
        vec![mounted.mount]
    );

    let detach_while_mounted = storage
        .detach_environment(DetachEnvironment {
            context: context("authority-a", "detach-early", "fp-detach-early", 3, false),
            attachment_id: "attachment-a".to_string(),
        })
        .unwrap_err();
    assert!(matches!(
        detach_while_mounted,
        SessionStoreError::RunConflict(_)
    ));

    let unmounted = storage
        .unmount_environment_resource(UnmountEnvironmentResource {
            context: context("authority-a", "unmount", "fp-unmount", 4, false),
            mount_id: "mount-a".to_string(),
        })
        .unwrap();
    assert_eq!(unmounted.mount.revision, 2);
    assert_eq!(
        storage
            .get_environment_attachment("authority-a", "attachment-a")
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    assert_eq!(
        storage
            .get_environment_mount("authority-a", "mount-a")
            .unwrap()
            .unwrap(),
        unmounted.mount
    );
    assert!(
        storage
            .list_environment_mounts(EnvironmentMountQuery {
                authority_binding: "authority-a".to_string(),
                session_id: "session-a".to_string(),
                run_id: "run-a".to_string(),
                limit: 100,
            })
            .unwrap()
            .is_empty()
    );

    let detached = storage
        .detach_environment(DetachEnvironment {
            context: context("authority-a", "detach", "fp-detach", 5, true),
            attachment_id: "attachment-a".to_string(),
        })
        .unwrap();
    assert_eq!(detached.attachment.revision, 4);
    assert_eq!(
        detached.attachment.status,
        DurableEnvironmentStatus::Detached
    );
}

#[test]
fn attachment_queries_are_authority_isolated_bounded_and_stable() {
    let storage = SqliteStorage::in_memory().unwrap();
    for index in 0..4 {
        storage
            .attach_environment(attach(
                "authority-a",
                &format!("key-{index}"),
                &format!("fp-{index}"),
                &format!("attachment-{index}"),
                "configured-a",
                DurableEnvironmentScope::Session {
                    session_id: "session-a".to_string(),
                },
                index,
                false,
            ))
            .unwrap();
    }
    storage
        .attach_environment(attach(
            "authority-b",
            "key-private",
            "fp-private",
            "attachment-private",
            "configured-private",
            DurableEnvironmentScope::Connection {
                connection_id: "connection-a".to_string(),
            },
            100,
            false,
        ))
        .unwrap();

    let first = storage
        .list_environment_attachments(EnvironmentAttachmentQuery {
            authority_binding: "authority-a".to_string(),
            connection_id: None,
            scope: Some(DurableEnvironmentScope::Session {
                session_id: "session-a".to_string(),
            }),
            limit: 2,
            after: None,
        })
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.attachment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["attachment-3", "attachment-2"]
    );
    assert!(
        first
            .items
            .iter()
            .all(|item| item.authority_binding == "authority-a")
    );
    let cursor = first.next.unwrap();
    let second = storage
        .list_environment_attachments(EnvironmentAttachmentQuery {
            authority_binding: "authority-a".to_string(),
            connection_id: None,
            scope: Some(DurableEnvironmentScope::Session {
                session_id: "session-a".to_string(),
            }),
            limit: 2,
            after: Some(EnvironmentAttachmentPageKey {
                updated_at: cursor.updated_at,
                attachment_id: cursor.attachment_id,
            }),
        })
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.attachment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["attachment-1", "attachment-0"]
    );
    assert!(second.next.is_none());

    let invalid = storage
        .list_environment_attachments(EnvironmentAttachmentQuery {
            authority_binding: "authority-a".to_string(),
            connection_id: None,
            scope: None,
            limit: 201,
            after: None,
        })
        .unwrap_err();
    assert!(matches!(invalid, SessionStoreError::Failed(_)));

    // Keep chrono arithmetic exercised independently of wall-clock time in this test module.
    assert_eq!(time(0) + Duration::seconds(3), time(3));
}
