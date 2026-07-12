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
];
