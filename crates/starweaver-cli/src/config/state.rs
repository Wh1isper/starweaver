use std::fs::{self, File, OpenOptions};

use chrono::{DateTime, Utc};
use fs2::FileExt as _;

use crate::{CliResult, config::CliConfig, error::io_error};

const LAST_RETENTION_MAINTENANCE_KEY: &str = "last_retention_maintenance_at";
const STATE_FILE_NAME: &str = "state.json";
const STATE_LOCK_FILE_NAME: &str = "state.lock";

/// Ensure local config directories exist.
pub fn ensure_config_dirs(config: &CliConfig) -> CliResult<()> {
    fs::create_dir_all(&config.file_store_path)
        .map_err(|error| io_error(&config.file_store_path, error))?;
    if let Some(parent) = config.database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    Ok(())
}

/// Read current session pointer from project state.
pub fn read_current_session(config: &CliConfig) -> CliResult<Option<String>> {
    let value = read_project_state(config)?;
    Ok(value
        .get("current_session_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

/// Read the last automatic retention maintenance timestamp.
pub fn read_last_retention_maintenance(config: &CliConfig) -> CliResult<Option<DateTime<Utc>>> {
    let value = read_project_state(config)?;
    value
        .get(LAST_RETENTION_MAINTENANCE_KEY)
        .and_then(serde_json::Value::as_str)
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|parsed| parsed.with_timezone(&Utc))
                .map_err(|error| crate::CliError::Config(error.to_string()))
        })
        .transpose()
}

/// Write current session pointer into project state through atomic rename.
pub fn write_current_session(config: &CliConfig, session_id: &str) -> CliResult<()> {
    update_project_state(config, |value| {
        value["current_session_id"] = serde_json::json!(session_id);
        value["database_path"] = serde_json::json!(config.database_path);
        value["profile"] = serde_json::json!(config.default_profile);
    })
}

/// Write the last automatic retention maintenance timestamp.
pub fn write_last_retention_maintenance(
    config: &CliConfig,
    timestamp: DateTime<Utc>,
) -> CliResult<()> {
    update_project_state(config, |value| {
        value[LAST_RETENTION_MAINTENANCE_KEY] = serde_json::json!(timestamp.to_rfc3339());
        value["database_path"] = serde_json::json!(config.database_path);
    })
}

/// Clear the current session pointer without deleting other project state fields.
pub fn clear_current_session(config: &CliConfig) -> CliResult<()> {
    update_project_state(config, |value| {
        if let Some(object) = value.as_object_mut() {
            object.remove("current_session_id");
        }
        value["database_path"] = serde_json::json!(config.database_path);
        value["profile"] = serde_json::json!(config.default_profile);
        value["cleared_at"] = serde_json::json!(Utc::now());
    })
}

/// Remove the project state file while excluding concurrent readers and writers.
pub fn remove_project_state(config: &CliConfig) -> CliResult<bool> {
    let _lock = lock_project_state(config)?;
    remove_project_state_locked(config)
}

/// Remove the project state file only while `session_id` remains current.
pub fn remove_project_state_if_current_session(
    config: &CliConfig,
    session_id: &str,
) -> CliResult<bool> {
    let _lock = lock_project_state(config)?;
    let value = read_project_state_locked(config)?;
    if value
        .get("current_session_id")
        .and_then(serde_json::Value::as_str)
        != Some(session_id)
    {
        return Ok(false);
    }
    remove_project_state_locked(config)
}

fn remove_project_state_locked(config: &CliConfig) -> CliResult<bool> {
    let path = project_state_path(config);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(&path, error)),
    }
}

fn read_project_state(config: &CliConfig) -> CliResult<serde_json::Value> {
    let _lock = lock_project_state(config)?;
    read_project_state_locked(config)
}

fn read_project_state_locked(config: &CliConfig) -> CliResult<serde_json::Value> {
    let path = project_state_path(config);
    match fs::read_to_string(&path) {
        Ok(content) => {
            let mut value = serde_json::from_str::<serde_json::Value>(&content)?;
            if !value.is_object() {
                value = serde_json::json!({});
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(error) => Err(io_error(&path, error)),
    }
}

fn update_project_state(
    config: &CliConfig,
    update: impl FnOnce(&mut serde_json::Value),
) -> CliResult<()> {
    let _lock = lock_project_state(config)?;
    let path = project_state_path(config);
    let mut value = read_project_state_locked(config)?;
    update(&mut value);
    let payload = serde_json::to_vec_pretty(&value)?;
    crate::atomic_file::replace(&path, &payload).map_err(|error| io_error(&path, error))?;
    Ok(())
}

fn project_state_path(config: &CliConfig) -> std::path::PathBuf {
    config.project_dir.join(STATE_FILE_NAME)
}

fn lock_project_state(config: &CliConfig) -> CliResult<ProjectStateLock> {
    let dir = &config.project_dir;
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    let path = dir.join(STATE_LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| io_error(&path, error))?;
    file.lock_exclusive()
        .map_err(|error| io_error(&path, error))?;
    Ok(ProjectStateLock(file))
}

struct ProjectStateLock(File);

impl Drop for ProjectStateLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{ConfigResolver, args};

    fn test_config(root: &std::path::Path) -> CliConfig {
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        ConfigResolver::for_tests(root).resolve(&cli).unwrap()
    }

    #[test]
    fn conditional_state_removal_preserves_a_new_current_session() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        write_current_session(&config, "session-new").unwrap();

        assert!(
            !remove_project_state_if_current_session(&config, "session-old").unwrap(),
            "a stale comparison must not remove a newer session pointer"
        );
        assert_eq!(
            read_current_session(&config).unwrap().as_deref(),
            Some("session-new")
        );
        assert!(remove_project_state_if_current_session(&config, "session-new").unwrap());
        assert!(!project_state_path(&config).exists());
    }
}
