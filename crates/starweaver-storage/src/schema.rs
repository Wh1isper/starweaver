//! SQLite schema definitions and migration SQL.

pub const SQLITE_SCHEMA_MIGRATION_TABLE: &str = "starweaver_schema_migrations";

#[derive(Clone, Copy, Debug)]
pub struct SqliteMigration {
    pub id: &'static str,
    pub description: &'static str,
    pub sql: &'static str,
    /// Version of a Rust-side migration hook, when the migration has one.
    pub hook_version: Option<&'static str>,
}

impl SqliteMigration {
    pub fn checksum(&self) -> String {
        self.hook_version.map_or_else(
            || migration_checksum(self.sql),
            |hook_version| migration_checksum(&format!("{}\nhook:{hook_version}", self.sql)),
        )
    }
}

pub fn migration_checksum(sql: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in sql.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv1a64:{hash:016x}")
}

pub const SQLITE_MIGRATIONS: &[SqliteMigration] = &[
    SqliteMigration {
        id: "20260605_000001_session_stream_store",
        description: "create durable session, run, checkpoint, stream, approval, deferred tool, and replay tables",
        sql: r"
        CREATE TABLE IF NOT EXISTS session_records (
            session_id TEXT PRIMARY KEY,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS run_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            record TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id)
        );
        CREATE INDEX IF NOT EXISTS ix_run_records_session_sequence
            ON run_records(session_id, sequence_no);

        CREATE TABLE IF NOT EXISTS checkpoints (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            checkpoint_id TEXT NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, sequence_no, checkpoint_id)
        );

        CREATE TABLE IF NOT EXISTS stream_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            record TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, sequence_no)
        );

        CREATE TABLE IF NOT EXISTS approvals (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            approval_id TEXT NOT NULL,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, approval_id)
        );

        CREATE TABLE IF NOT EXISTS deferred_tools (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            deferred_id TEXT NOT NULL,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, deferred_id)
        );

        CREATE TABLE IF NOT EXISTS replay_events (
            scope TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (scope, sequence_no)
        );
        CREATE INDEX IF NOT EXISTS ix_replay_events_scope_sequence
            ON replay_events(scope, sequence_no);

        CREATE TABLE IF NOT EXISTS replay_snapshots (
            scope TEXT PRIMARY KEY,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260711_000002_namespaced_evidence_tables",
        description: "create collision-safe durable evidence tables and per-run state tables",
        sql: r"
        CREATE TABLE IF NOT EXISTS checkpoint_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            checkpoint_id TEXT NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, sequence_no, checkpoint_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS approval_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            approval_id TEXT NOT NULL,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, approval_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS deferred_tool_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            deferred_id TEXT NOT NULL,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, deferred_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS replay_snapshot_records (
            scope TEXT PRIMARY KEY,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS run_context_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS run_environment_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS ux_run_records_session_sequence
            ON run_records(session_id, sequence_no);
        CREATE UNIQUE INDEX IF NOT EXISTS ux_checkpoint_records_run_identity
            ON checkpoint_records(session_id, run_id, checkpoint_id);
    ",
        hook_version: Some("typed-legacy-backfill-v1"),
    },
    SqliteMigration {
        id: "20260711_000003_split_display_and_replay_families",
        description: "separate display archive records from typed replay event records",
        sql: r"
        CREATE TABLE IF NOT EXISTS display_message_records (
            scope TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (scope, sequence_no)
        );
        CREATE INDEX IF NOT EXISTS ix_display_message_records_scope_sequence
            ON display_message_records(scope, sequence_no);

        CREATE TABLE IF NOT EXISTS display_snapshot_records (
            scope TEXT PRIMARY KEY,
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

    ",
        hook_version: Some("display-backfill-v2"),
    },
    SqliteMigration {
        id: "20260712_000004_evidence_outbox_and_resume_claims",
        description: "add sealed evidence digests, transactional stream publication outbox, and exclusive HITL resume claims",
        sql: r"
        CREATE TABLE IF NOT EXISTS run_evidence_commits (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            digest TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS stream_publication_outbox (
            publication_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            record TEXT NOT NULL,
            archive_pending INTEGER NOT NULL,
            replay_pending INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (session_id, run_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS ix_stream_publication_outbox_session
            ON stream_publication_outbox(session_id, created_at, publication_id);

        CREATE TABLE IF NOT EXISTS hitl_resume_claims (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            claim_id TEXT NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id),
            UNIQUE (claim_id),
            FOREIGN KEY (session_id, run_id)
                REFERENCES run_records(session_id, run_id) ON DELETE CASCADE
        );
    ",
        hook_version: Some("evidence-digest-backfill-v2"),
    },
    SqliteMigration {
        id: "20260714_000005_agent_session_management",
        description: "add revision/idempotency, deletion fence, run admission lease, fencing, and control receipt storage",
        sql: r"
        CREATE TABLE IF NOT EXISTS session_mutation_receipts (
            namespace_id TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            session_id TEXT NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (namespace_id, idempotency_key)
        );

        CREATE TABLE IF NOT EXISTS run_admission_generations (
            namespace_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            PRIMARY KEY (namespace_id, session_id)
        );

        CREATE TABLE IF NOT EXISTS run_admissions (
            namespace_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            host_instance_id TEXT NOT NULL,
            lease_expires_at TEXT NOT NULL,
            record TEXT NOT NULL,
            PRIMARY KEY (namespace_id, session_id),
            UNIQUE (namespace_id, session_id, run_id, generation)
        );
        CREATE INDEX IF NOT EXISTS ix_run_admissions_expiry
            ON run_admissions(namespace_id, lease_expires_at);

        CREATE TABLE IF NOT EXISTS run_admission_receipts (
            namespace_id TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (namespace_id, idempotency_key)
        );

        CREATE TABLE IF NOT EXISTS run_control_receipts (
            receipt_id TEXT PRIMARY KEY,
            namespace_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            generation INTEGER NOT NULL,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE (namespace_id, session_id, run_id, idempotency_key)
        );
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260714_000006_async_subagent_delivery",
        description: "add durable background-subagent execution, delivery, retention, and continuation linkage",
        sql: r"
        CREATE TABLE IF NOT EXISTS background_subagent_records (
            attempt_id TEXT PRIMARY KEY,
            namespace_id TEXT NOT NULL,
            parent_session_id TEXT NOT NULL,
            parent_run_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            execution_status TEXT NOT NULL,
            delivery_status TEXT NOT NULL,
            retention_status TEXT NOT NULL,
            claim_deadline TEXT,
            continuation_run_id TEXT,
            owner_host_instance_id TEXT NOT NULL,
            owner_generation INTEGER NOT NULL,
            owner_heartbeat_at TEXT NOT NULL,
            owner_lease_expires_at TEXT NOT NULL,
            retention_expires_at TEXT,
            record TEXT NOT NULL,
            accepted_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (parent_session_id, parent_run_id)
                REFERENCES run_records(session_id, run_id)
        );
        CREATE INDEX IF NOT EXISTS ix_background_subagent_session_updated
            ON background_subagent_records(namespace_id, parent_session_id, updated_at, attempt_id);
        CREATE UNIQUE INDEX IF NOT EXISTS ux_background_subagent_active_agent
            ON background_subagent_records(namespace_id, parent_session_id, agent_id)
            WHERE execution_status IN ('accepted', 'starting', 'running', 'waiting');
        CREATE INDEX IF NOT EXISTS ix_background_subagent_reconcile
            ON background_subagent_records(namespace_id, execution_status, owner_lease_expires_at,
                                             delivery_status, claim_deadline);
        CREATE INDEX IF NOT EXISTS ix_background_subagent_retention
            ON background_subagent_records(namespace_id, retention_status,
                                             retention_expires_at, attempt_id);
        CREATE TABLE IF NOT EXISTS background_subagent_artifacts (
            artifact_ref TEXT PRIMARY KEY,
            namespace_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL UNIQUE,
            expires_at TEXT NOT NULL,
            artifact TEXT NOT NULL,
            FOREIGN KEY (attempt_id) REFERENCES background_subagent_records(attempt_id)
                ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS ix_background_subagent_artifact_expiry
            ON background_subagent_artifacts(namespace_id, expires_at, artifact_ref);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260715_000007_background_terminal_fingerprint",
        description: "persist canonical background-subagent terminal commit fingerprints",
        sql: r"
        ALTER TABLE background_subagent_records
            ADD COLUMN terminal_fingerprint TEXT;
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260718_000008_local_store_imports",
        description: "track idempotent imports from legacy project-local session databases",
        sql: r"
        CREATE TABLE IF NOT EXISTS local_store_imports (
            source_path TEXT PRIMARY KEY,
            workspace TEXT NOT NULL,
            sessions_imported INTEGER NOT NULL,
            rows_imported INTEGER NOT NULL,
            imported_at TEXT NOT NULL
        );
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260718_000009_incremental_local_store_imports",
        description: "track source provenance per imported session for incremental legacy evidence imports",
        sql: r"
        CREATE TABLE IF NOT EXISTS local_store_import_sessions (
            source_path TEXT NOT NULL,
            session_id TEXT NOT NULL,
            imported_at TEXT NOT NULL,
            PRIMARY KEY (source_path, session_id),
            FOREIGN KEY (session_id) REFERENCES session_records(session_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS ix_local_store_import_sessions_session
            ON local_store_import_sessions(session_id, source_path);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260718_000010_durable_replay_source_selection",
        description: "persist immutable replay evidence-family selection per scope",
        sql: r"
        CREATE TABLE IF NOT EXISTS replay_source_selections (
            scope TEXT PRIMARY KEY,
            source TEXT NOT NULL CHECK (source IN ('replay_events', 'display_messages')),
            selected_at TEXT NOT NULL
        );
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260718_000011_local_store_import_tombstones",
        description: "prevent physically deleted legacy imports from being recreated",
        sql: r"
        CREATE TABLE IF NOT EXISTS local_store_import_tombstones (
            source_path TEXT NOT NULL,
            session_id TEXT NOT NULL,
            deleted_at TEXT NOT NULL,
            PRIMARY KEY (source_path, session_id)
        );
        CREATE INDEX IF NOT EXISTS ix_local_store_import_tombstones_session
            ON local_store_import_tombstones(session_id, source_path);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260721_000012_durable_host_events",
        description: "add canonical durable host-event records, deterministic publication outbox, and monotonic positions",
        sql: r"
        CREATE TABLE IF NOT EXISTS host_event_log_state (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            last_position INTEGER NOT NULL CHECK (last_position >= 0)
        );
        INSERT OR IGNORE INTO host_event_log_state (singleton, last_position) VALUES (1, 0);

        CREATE TABLE IF NOT EXISTS host_event_records (
            position INTEGER PRIMARY KEY CHECK (position > 0),
            publication_key TEXT NOT NULL UNIQUE,
            event_id TEXT NOT NULL UNIQUE,
            scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'session', 'run')),
            session_id TEXT,
            run_id TEXT,
            event_class TEXT NOT NULL CHECK (event_class IN (
                'session_changed', 'run_changed', 'output_available', 'approval_changed',
                'deferred_changed', 'clarification_changed', 'environment_changed', 'diagnostic'
            )),
            record TEXT NOT NULL,
            occurred_at TEXT NOT NULL,
            CHECK (
                (scope_kind = 'global' AND session_id IS NULL AND run_id IS NULL) OR
                (scope_kind = 'session' AND session_id IS NOT NULL AND run_id IS NULL) OR
                (scope_kind = 'run' AND session_id IS NOT NULL AND run_id IS NOT NULL)
            )
        );
        CREATE INDEX IF NOT EXISTS ix_host_event_records_session_position
            ON host_event_records(session_id, position);
        CREATE INDEX IF NOT EXISTS ix_host_event_records_run_position
            ON host_event_records(session_id, run_id, position);
        CREATE INDEX IF NOT EXISTS ix_host_event_records_class_position
            ON host_event_records(event_class, position);

        CREATE TABLE IF NOT EXISTS host_event_outbox_state (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            last_sequence INTEGER NOT NULL CHECK (last_sequence >= 0)
        );
        INSERT OR IGNORE INTO host_event_outbox_state (singleton, last_sequence) VALUES (1, 0);

        CREATE TABLE IF NOT EXISTS host_event_publication_outbox (
            publication_key TEXT PRIMARY KEY,
            enqueue_sequence INTEGER NOT NULL UNIQUE CHECK (enqueue_sequence > 0),
            event_id TEXT NOT NULL UNIQUE,
            scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'session', 'run')),
            session_id TEXT,
            run_id TEXT,
            event_class TEXT NOT NULL CHECK (event_class IN (
                'session_changed', 'run_changed', 'output_available', 'approval_changed',
                'deferred_changed', 'clarification_changed', 'environment_changed', 'diagnostic'
            )),
            record TEXT NOT NULL,
            occurred_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            CHECK (
                (scope_kind = 'global' AND session_id IS NULL AND run_id IS NULL) OR
                (scope_kind = 'session' AND session_id IS NOT NULL AND run_id IS NULL) OR
                (scope_kind = 'run' AND session_id IS NOT NULL AND run_id IS NOT NULL)
            )
        );
        CREATE INDEX IF NOT EXISTS ix_host_event_outbox_sequence
            ON host_event_publication_outbox(enqueue_sequence);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260721_000013_stable_keyset_pages",
        description: "add stable updated-time and identity indexes for bounded session and HITL pagination",
        sql: r"
        CREATE INDEX IF NOT EXISTS ix_session_records_updated_identity
            ON session_records(updated_at DESC, session_id DESC);

        CREATE INDEX IF NOT EXISTS ix_approval_records_updated_identity
            ON approval_records(updated_at DESC, approval_id DESC);
        CREATE INDEX IF NOT EXISTS ix_approval_records_session_updated_identity
            ON approval_records(session_id, updated_at DESC, approval_id DESC);
        CREATE INDEX IF NOT EXISTS ix_approval_records_run_updated_identity
            ON approval_records(run_id, updated_at DESC, approval_id DESC);
        CREATE INDEX IF NOT EXISTS ix_approval_records_session_run_updated_identity
            ON approval_records(session_id, run_id, updated_at DESC, approval_id DESC);

        CREATE INDEX IF NOT EXISTS ix_deferred_records_updated_identity
            ON deferred_tool_records(updated_at DESC, deferred_id DESC);
        CREATE INDEX IF NOT EXISTS ix_deferred_records_session_updated_identity
            ON deferred_tool_records(session_id, updated_at DESC, deferred_id DESC);
        CREATE INDEX IF NOT EXISTS ix_deferred_records_run_updated_identity
            ON deferred_tool_records(run_id, updated_at DESC, deferred_id DESC);
        CREATE INDEX IF NOT EXISTS ix_deferred_records_session_run_updated_identity
            ON deferred_tool_records(session_id, run_id, updated_at DESC, deferred_id DESC);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260721_000014_model_selection_receipts",
        description: "add authority-bound model selections and durable idempotent mutation receipts",
        sql: r"
        CREATE TABLE IF NOT EXISTS model_selection_records (
            authority_binding TEXT PRIMARY KEY,
            revision INTEGER NOT NULL CHECK (revision > 0),
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS model_selection_mutation_receipts (
            authority_binding TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            receipt_id TEXT NOT NULL UNIQUE,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (authority_binding, idempotency_key),
            FOREIGN KEY (authority_binding) REFERENCES model_selection_records(authority_binding)
        );
        CREATE INDEX IF NOT EXISTS ix_model_selection_receipts_created
            ON model_selection_mutation_receipts(authority_binding, created_at, receipt_id);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260721_000015_interaction_mutation_receipts",
        description: "add authority-scoped idempotent receipts for atomic approval, deferred, and clarification mutations",
        sql: r"
        CREATE TABLE IF NOT EXISTS interaction_mutation_receipts (
            authority_binding TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            operation TEXT NOT NULL CHECK (operation IN (
                'approval.decide', 'deferred.complete', 'deferred.fail',
                'clarification.resolve'
            )),
            target_ref TEXT NOT NULL,
            receipt_id TEXT NOT NULL UNIQUE,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (authority_binding, idempotency_key)
        );
        CREATE INDEX IF NOT EXISTS ix_interaction_mutation_receipts_created
            ON interaction_mutation_receipts(authority_binding, created_at, receipt_id);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260721_000016_environment_aggregate",
        description: "add durable authority-bound environment attachments, mounts, and atomic mutation receipts",
        sql: r"
        CREATE TABLE IF NOT EXISTS environment_attachment_records (
            authority_binding TEXT NOT NULL,
            attachment_id TEXT NOT NULL,
            environment_id TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('attaching', 'ready', 'degraded', 'detached')),
            revision INTEGER NOT NULL CHECK (revision > 0),
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (authority_binding, attachment_id)
        );
        CREATE INDEX IF NOT EXISTS ix_environment_attachments_list
            ON environment_attachment_records(authority_binding, updated_at DESC, attachment_id DESC);
        CREATE INDEX IF NOT EXISTS ix_environment_attachments_scope_list
            ON environment_attachment_records(authority_binding, scope_key, updated_at DESC, attachment_id DESC);

        CREATE TABLE IF NOT EXISTS environment_mount_records (
            authority_binding TEXT NOT NULL,
            mount_id TEXT NOT NULL,
            attachment_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('mounted', 'unmounted')),
            revision INTEGER NOT NULL CHECK (revision > 0),
            record TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (authority_binding, mount_id),
            FOREIGN KEY (authority_binding, attachment_id)
                REFERENCES environment_attachment_records(authority_binding, attachment_id)
        );
        CREATE INDEX IF NOT EXISTS ix_environment_mounts_run
            ON environment_mount_records(authority_binding, session_id, run_id, status, mount_id);
        CREATE INDEX IF NOT EXISTS ix_environment_mounts_attachment
            ON environment_mount_records(authority_binding, attachment_id, status, mount_id);

        CREATE TABLE IF NOT EXISTS environment_mutation_receipts (
            authority_binding TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            operation TEXT NOT NULL CHECK (operation IN (
                'environment.attach', 'environment.detach', 'environment.mount',
                'environment.unmount'
            )),
            target_ref TEXT NOT NULL,
            receipt_id TEXT NOT NULL UNIQUE,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (authority_binding, idempotency_key)
        );
        CREATE INDEX IF NOT EXISTS ix_environment_receipts_created
            ON environment_mutation_receipts(authority_binding, created_at, receipt_id);
    ",
        hook_version: None,
    },
    SqliteMigration {
        id: "20260721_000017_durable_run_control_effects",
        description: "atomically bind run control receipts to durable steering and interrupt intents",
        sql: r"
        CREATE TABLE IF NOT EXISTS run_control_intents (
            namespace_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            authority_binding TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            command_fingerprint TEXT NOT NULL,
            admission_id TEXT NOT NULL,
            host_instance_id TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK (generation > 0),
            operation TEXT NOT NULL CHECK (operation IN ('steer', 'interrupt')),
            status TEXT NOT NULL CHECK (status IN (
                'pending', 'delivered', 'consumed', 'reconciled'
            )),
            receipt_id TEXT NOT NULL UNIQUE,
            record TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (namespace_id, session_id, run_id, operation_id),
            UNIQUE (authority_binding, idempotency_key),
            FOREIGN KEY (receipt_id) REFERENCES run_control_receipts(receipt_id)
                ON DELETE CASCADE,
            FOREIGN KEY (session_id, run_id) REFERENCES run_records(session_id, run_id)
                ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS ix_run_control_inbox
            ON run_control_intents(namespace_id, session_id, run_id, status, created_at, operation_id);
        CREATE INDEX IF NOT EXISTS ix_run_control_admission
            ON run_control_intents(namespace_id, session_id, run_id, generation, admission_id);
    ",
        hook_version: None,
    },
];
