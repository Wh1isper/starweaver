//! Storage migration report DTOs.

/// Applied SQLite schema migration metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteAppliedMigration {
    /// Migration id.
    pub id: String,
    /// Migration description.
    pub description: String,
    /// Migration SQL checksum when recorded by the database.
    pub checksum: Option<String>,
    /// Application timestamp if recorded by the database.
    pub applied_at: Option<String>,
}

/// Pending SQLite schema migration metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlitePendingMigration {
    /// Migration id.
    pub id: &'static str,
    /// Migration description.
    pub description: &'static str,
    /// Migration SQL checksum.
    pub checksum: String,
}

/// SQLite schema migration status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteMigrationStatus {
    /// True when the migration tracking table exists.
    pub migration_table_exists: bool,
    /// Applied migrations recorded by the database.
    pub applied: Vec<SqliteAppliedMigration>,
    /// Workspace migrations still pending.
    pub pending: Vec<SqlitePendingMigration>,
    /// Latest known migration id.
    pub latest_migration: Option<&'static str>,
    /// True when every known migration has been applied.
    pub current: bool,
}
