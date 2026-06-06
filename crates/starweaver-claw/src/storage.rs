//! Re-exported storage adapters for Claw.
//!
//! Shared SQLite migrations, durable session storage, and replay event storage
//! live in `starweaver-storage` so the CLI and service layers can share the
//! same persistence contracts.

pub use starweaver_storage::{migrate_sqlite_database, SqliteReplayEventLog, SqliteSessionStore};
