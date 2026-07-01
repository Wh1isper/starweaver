//! SQLite migration application and status reporting.

use std::{collections::BTreeSet, path::Path};

use chrono::Utc;
use rusqlite::{Connection, params};
use starweaver_session::SessionStoreResult;

use crate::{
    schema::{SQLITE_MIGRATIONS, SQLITE_SCHEMA_MIGRATION_TABLE},
    sqlite::map_sqlite_session_error,
};

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

/// Run all pending SQLite schema migrations for a database file.
///
/// # Errors
///
/// Returns a store error when SQLite cannot open the database or apply a migration.
pub fn migrate_sqlite_database(path: impl AsRef<Path>) -> SessionStoreResult<Vec<&'static str>> {
    let mut connection = Connection::open(path).map_err(map_sqlite_session_error)?;
    apply_sqlite_migrations(&mut connection)
}

/// Inspect SQLite schema migration status without applying migrations.
///
/// # Errors
///
/// Returns a store error when SQLite cannot open or inspect the database.
pub fn sqlite_migration_status(
    path: impl AsRef<Path>,
) -> SessionStoreResult<SqliteMigrationStatus> {
    let connection = Connection::open(path).map_err(map_sqlite_session_error)?;
    sqlite_migration_status_for_connection(&connection)
}

fn sqlite_migration_status_for_connection(
    connection: &Connection,
) -> SessionStoreResult<SqliteMigrationStatus> {
    let migration_table_exists = table_exists(connection, SQLITE_SCHEMA_MIGRATION_TABLE)?;
    let applied = if migration_table_exists {
        load_applied_migration_records(connection)?
    } else {
        Vec::new()
    };
    let applied_ids = applied
        .iter()
        .map(|migration| migration.id.as_str())
        .collect::<BTreeSet<_>>();
    let pending = SQLITE_MIGRATIONS
        .iter()
        .filter(|migration| !applied_ids.contains(migration.id))
        .map(|migration| SqlitePendingMigration {
            id: migration.id,
            description: migration.description,
            checksum: migration.checksum(),
        })
        .collect::<Vec<_>>();
    Ok(SqliteMigrationStatus {
        migration_table_exists,
        applied,
        pending: pending.clone(),
        latest_migration: SQLITE_MIGRATIONS.last().map(|migration| migration.id),
        current: pending.is_empty(),
    })
}

fn load_applied_migration_records(
    connection: &Connection,
) -> SessionStoreResult<Vec<SqliteAppliedMigration>> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, description, checksum, applied_at FROM {SQLITE_SCHEMA_MIGRATION_TABLE} ORDER BY applied_at ASC, id ASC"
        ))
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(SqliteAppliedMigration {
                id: row.get::<_, String>(0)?,
                description: row.get::<_, String>(1)?,
                checksum: row.get::<_, Option<String>>(2)?,
                applied_at: row.get::<_, Option<String>>(3)?,
            })
        })
        .map_err(map_sqlite_session_error)?;
    let mut migrations = Vec::new();
    for row in rows {
        migrations.push(row.map_err(map_sqlite_session_error)?);
    }
    Ok(migrations)
}

pub fn apply_sqlite_migrations(
    connection: &mut Connection,
) -> SessionStoreResult<Vec<&'static str>> {
    connection
        .execute_batch(
            r"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            ",
        )
        .map_err(map_sqlite_session_error)?;
    connection
        .execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {SQLITE_SCHEMA_MIGRATION_TABLE} (
                    id TEXT PRIMARY KEY,
                    description TEXT NOT NULL,
                    checksum TEXT,
                    applied_at TEXT NOT NULL
                )"
            ),
            [],
        )
        .map_err(map_sqlite_session_error)?;
    ensure_migration_checksum_column(connection)?;
    let applied = load_applied_migrations(connection)?;
    let transaction = connection.transaction().map_err(map_sqlite_session_error)?;
    let mut newly_applied = Vec::new();
    for migration in SQLITE_MIGRATIONS {
        if applied.contains(migration.id) {
            continue;
        }
        transaction
            .execute_batch(migration.sql)
            .map_err(map_sqlite_session_error)?;
        transaction
            .execute(
                &format!(
                    "INSERT INTO {SQLITE_SCHEMA_MIGRATION_TABLE} (id, description, checksum, applied_at)
                     VALUES (?1, ?2, ?3, ?4)"
                ),
                params![
                    migration.id,
                    migration.description,
                    migration.checksum(),
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(map_sqlite_session_error)?;
        newly_applied.push(migration.id);
    }
    transaction.commit().map_err(map_sqlite_session_error)?;
    Ok(newly_applied)
}

fn ensure_migration_checksum_column(connection: &Connection) -> SessionStoreResult<()> {
    if !table_has_column(connection, SQLITE_SCHEMA_MIGRATION_TABLE, "checksum")? {
        connection
            .execute(
                &format!("ALTER TABLE {SQLITE_SCHEMA_MIGRATION_TABLE} ADD COLUMN checksum TEXT"),
                [],
            )
            .map_err(map_sqlite_session_error)?;
    }
    Ok(())
}

fn load_applied_migrations(connection: &Connection) -> SessionStoreResult<BTreeSet<String>> {
    let mut statement = connection
        .prepare(&format!("SELECT id FROM {SQLITE_SCHEMA_MIGRATION_TABLE}"))
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_session_error)?;
    let mut applied = BTreeSet::new();
    for row in rows {
        applied.insert(row.map_err(map_sqlite_session_error)?);
    }
    Ok(applied)
}

fn table_exists(connection: &Connection, table: &str) -> SessionStoreResult<bool> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(map_sqlite_session_error)?;
    Ok(count > 0)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> SessionStoreResult<bool> {
    if !table_exists(connection, table)? {
        return Ok(false);
    }
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection
        .prepare(&pragma)
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(map_sqlite_session_error)?;
    for row in rows {
        if row.map_err(map_sqlite_session_error)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
