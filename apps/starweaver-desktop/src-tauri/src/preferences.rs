use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

const PREFERENCES_SCHEMA_VERSION: u32 = 1;
const PREFERENCES_FILE_NAME: &str = "preferences-v1.json";
const MAX_PREFERENCES_BYTES: u64 = 16 * 1024;
const MAX_MUTATION_ID_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopTheme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopDensity {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowCloseBehavior {
    #[default]
    KeepRunning,
    Quit,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopPreferencesInput {
    pub theme: DesktopTheme,
    pub density: DesktopDensity,
    pub window_close_behavior: WindowCloseBehavior,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreferencesSnapshot {
    pub schema_version: u32,
    pub revision: String,
    pub preferences: DesktopPreferencesInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_issue: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopPreferencesUpdate {
    pub expected_revision: String,
    pub mutation_id: String,
    pub preferences: DesktopPreferencesInput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopPreferencesErrorCode {
    NotReady,
    InvalidRequest,
    Conflict,
    Storage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreferencesError {
    pub code: DesktopPreferencesErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,
}

impl DesktopPreferencesError {
    fn not_ready() -> Self {
        Self {
            code: DesktopPreferencesErrorCode::NotReady,
            message: "Desktop preferences are not ready".to_string(),
            current_revision: None,
        }
    }

    fn invalid_request(message: &'static str) -> Self {
        Self {
            code: DesktopPreferencesErrorCode::InvalidRequest,
            message: message.to_string(),
            current_revision: None,
        }
    }

    fn conflict(current_revision: u64) -> Self {
        Self {
            code: DesktopPreferencesErrorCode::Conflict,
            message: "Desktop preferences changed in another window".to_string(),
            current_revision: Some(current_revision.to_string()),
        }
    }

    fn storage(message: &'static str) -> Self {
        Self {
            code: DesktopPreferencesErrorCode::Storage,
            message: message.to_string(),
            current_revision: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DesktopPreferencesRecord {
    schema_version: u32,
    revision: u64,
    preferences: DesktopPreferencesInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_mutation_id: Option<String>,
}

impl Default for DesktopPreferencesRecord {
    fn default() -> Self {
        Self {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            revision: 0,
            preferences: DesktopPreferencesInput::default(),
            last_mutation_id: None,
        }
    }
}

#[derive(Default)]
struct PreferencesState {
    path: Option<PathBuf>,
    record: DesktopPreferencesRecord,
    load_issue: Option<String>,
}

/// Backend-owned application preferences. The renderer never chooses the storage path.
pub struct DesktopPreferencesStore {
    state: Mutex<PreferencesState>,
}

impl Default for DesktopPreferencesStore {
    fn default() -> Self {
        Self {
            state: Mutex::new(PreferencesState::default()),
        }
    }
}

impl DesktopPreferencesStore {
    pub fn configure(&self, root: &Path) -> Result<(), DesktopPreferencesError> {
        create_private_directory(root)?;
        let path = root.join(PREFERENCES_FILE_NAME);
        let (record, load_issue) = match load_record(&path) {
            Ok(Some(record)) => (record, None),
            Ok(None) => (DesktopPreferencesRecord::default(), None),
            Err(error) => (DesktopPreferencesRecord::default(), Some(error.message)),
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| DesktopPreferencesError::storage("Desktop preferences are unavailable"))?;
        state.path = Some(path);
        state.record = record;
        state.load_issue = load_issue;
        drop(state);
        Ok(())
    }

    pub fn snapshot(&self) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
        let state = self
            .state
            .lock()
            .map_err(|_| DesktopPreferencesError::storage("Desktop preferences are unavailable"))?;
        if state.path.is_none() {
            return Err(DesktopPreferencesError::not_ready());
        }
        let value = snapshot(&state);
        drop(state);
        Ok(value)
    }

    pub fn update(
        &self,
        update: DesktopPreferencesUpdate,
    ) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
        validate_mutation_id(&update.mutation_id)?;
        let expected_revision = update.expected_revision.parse::<u64>().map_err(|_| {
            DesktopPreferencesError::invalid_request(
                "expectedRevision must be a canonical decimal revision",
            )
        })?;
        if expected_revision.to_string() != update.expected_revision {
            return Err(DesktopPreferencesError::invalid_request(
                "expectedRevision must be a canonical decimal revision",
            ));
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| DesktopPreferencesError::storage("Desktop preferences are unavailable"))?;
        let path = state
            .path
            .clone()
            .ok_or_else(DesktopPreferencesError::not_ready)?;
        if state.record.last_mutation_id.as_deref() == Some(update.mutation_id.as_str()) {
            if expected_revision.checked_add(1) != Some(state.record.revision)
                || update.preferences != state.record.preferences
            {
                return Err(DesktopPreferencesError::invalid_request(
                    "mutationId is already bound to a different preference update",
                ));
            }
            let value = snapshot(&state);
            drop(state);
            return Ok(value);
        }
        if state.record.revision != expected_revision {
            return Err(DesktopPreferencesError::conflict(state.record.revision));
        }
        let revision = state.record.revision.checked_add(1).ok_or_else(|| {
            DesktopPreferencesError::storage("Desktop preference revision is exhausted")
        })?;
        let record = DesktopPreferencesRecord {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            revision,
            preferences: update.preferences,
            last_mutation_id: Some(update.mutation_id),
        };
        persist_record(&path, &record)?;
        state.record = record;
        state.load_issue = None;
        let value = snapshot(&state);
        drop(state);
        Ok(value)
    }

    pub fn reload(&self) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| DesktopPreferencesError::storage("Desktop preferences are unavailable"))?;
        let path = state
            .path
            .clone()
            .ok_or_else(DesktopPreferencesError::not_ready)?;
        let record = match load_record(&path)? {
            Some(record) if record.revision < state.record.revision => {
                return Err(DesktopPreferencesError::storage(
                    "Desktop preference file has a stale revision",
                ));
            }
            Some(record) if record.revision == state.record.revision && record != state.record => {
                return Err(DesktopPreferencesError::storage(
                    "Desktop preference revision does not identify the current snapshot",
                ));
            }
            Some(record) => record,
            None if state.record.revision == 0 => DesktopPreferencesRecord::default(),
            None => {
                return Err(DesktopPreferencesError::storage(
                    "Desktop preference file is unavailable",
                ));
            }
        };
        state.record = record;
        state.load_issue = None;
        let value = snapshot(&state);
        drop(state);
        Ok(value)
    }

    pub fn window_close_behavior(&self) -> WindowCloseBehavior {
        self.state
            .lock()
            .map_or(WindowCloseBehavior::KeepRunning, |state| {
                state.record.preferences.window_close_behavior
            })
    }
}

fn snapshot(state: &PreferencesState) -> DesktopPreferencesSnapshot {
    DesktopPreferencesSnapshot {
        schema_version: PREFERENCES_SCHEMA_VERSION,
        revision: state.record.revision.to_string(),
        preferences: state.record.preferences,
        load_issue: state.load_issue.clone(),
    }
}

fn validate_mutation_id(value: &str) -> Result<(), DesktopPreferencesError> {
    if value.is_empty()
        || value.len() > MAX_MUTATION_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DesktopPreferencesError::invalid_request(
            "mutationId must use 1 to 128 safe ASCII identity characters",
        ));
    }
    Ok(())
}

fn create_private_directory(root: &Path) -> Result<(), DesktopPreferencesError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(DesktopPreferencesError::storage(
                "Desktop preference storage is not a private directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| {
                DesktopPreferencesError::storage("Desktop preference storage could not be created")
            })?;
        }
        Err(_) => {
            return Err(DesktopPreferencesError::storage(
                "Desktop preference storage could not be inspected",
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|_| {
            DesktopPreferencesError::storage("Desktop preference storage could not be secured")
        })?;
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| {
        DesktopPreferencesError::storage("Desktop preference storage could not be inspected")
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(DesktopPreferencesError::storage(
            "Desktop preference storage is not a private directory",
        ));
    }
    Ok(())
}

fn load_record(path: &Path) -> Result<Option<DesktopPreferencesRecord>, DesktopPreferencesError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(DesktopPreferencesError::storage(
                "Desktop preferences could not be inspected",
            ));
        }
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PREFERENCES_BYTES
    {
        return Err(DesktopPreferencesError::storage(
            "Desktop preference file is invalid",
        ));
    }
    let bytes = fs::read(path)
        .map_err(|_| DesktopPreferencesError::storage("Desktop preferences could not be read"))?;
    let record: DesktopPreferencesRecord = serde_json::from_slice(&bytes)
        .map_err(|_| DesktopPreferencesError::storage("Desktop preference file is invalid"))?;
    if record.schema_version != PREFERENCES_SCHEMA_VERSION {
        return Err(DesktopPreferencesError::storage(
            "Desktop preference schema is not supported",
        ));
    }
    Ok(Some(record))
}

#[cfg(windows)]
fn publish_temporary(temporary: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    let (temporary_file, temporary_path) = temporary.keep().map_err(|error| error.error)?;
    let result = atomicwrites::replace_atomic(&temporary_path, path);
    drop(temporary_file);
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn publish_temporary(temporary: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

fn persist_record(
    path: &Path,
    record: &DesktopPreferencesRecord,
) -> Result<(), DesktopPreferencesError> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err(DesktopPreferencesError::storage(
            "Desktop preference destination is invalid",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        DesktopPreferencesError::storage("Desktop preference storage is unavailable")
    })?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| {
        DesktopPreferencesError::storage("Desktop preferences could not be encoded")
    })?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".preferences-")
        .tempfile_in(parent)
        .map_err(|_| DesktopPreferencesError::storage("Desktop preferences could not be staged"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| {
                DesktopPreferencesError::storage("Desktop preferences could not be secured")
            })?;
    }
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| {
            DesktopPreferencesError::storage("Desktop preferences could not be written")
        })?;
    publish_temporary(temporary, path).map_err(|_| {
        DesktopPreferencesError::storage("Desktop preferences could not be published")
    })?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            DesktopPreferencesError::storage("Desktop preferences could not be committed")
        })?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn update(
        store: &DesktopPreferencesStore,
        revision: &str,
        mutation_id: &str,
        theme: DesktopTheme,
    ) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
        store.update(DesktopPreferencesUpdate {
            expected_revision: revision.to_string(),
            mutation_id: mutation_id.to_string(),
            preferences: DesktopPreferencesInput {
                theme,
                density: DesktopDensity::Compact,
                window_close_behavior: WindowCloseBehavior::Quit,
            },
        })
    }

    #[test]
    fn missing_preferences_use_safe_defaults() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let store = DesktopPreferencesStore::default();
        store
            .configure(&temporary.path().join("preferences"))
            .expect("configure preferences");

        assert_eq!(
            store.snapshot().expect("default snapshot"),
            DesktopPreferencesSnapshot {
                schema_version: 1,
                revision: "0".to_string(),
                preferences: DesktopPreferencesInput::default(),
                load_issue: None,
            }
        );
    }

    #[test]
    fn corrupt_preferences_keep_defaults_and_report_bounded_issue() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("preferences");
        create_private_directory(&root).expect("private preferences root");
        fs::write(root.join(PREFERENCES_FILE_NAME), b"not-json").expect("corrupt fixture");
        let store = DesktopPreferencesStore::default();

        store.configure(&root).expect("configure preferences");
        let snapshot = store.snapshot().expect("fallback snapshot");
        assert_eq!(snapshot.preferences, DesktopPreferencesInput::default());
        assert_eq!(snapshot.revision, "0");
        assert_eq!(
            snapshot.load_issue.as_deref(),
            Some("Desktop preference file is invalid")
        );
    }

    #[test]
    fn update_is_atomic_revision_fenced_and_idempotent() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("preferences");
        let store = DesktopPreferencesStore::default();
        store.configure(&root).expect("configure preferences");

        let first =
            update(&store, "0", "preference-mutation-1", DesktopTheme::Dark).expect("first update");
        assert_eq!(first.revision, "1");
        assert_eq!(first.preferences.theme, DesktopTheme::Dark);
        assert_eq!(
            update(&store, "0", "preference-mutation-1", DesktopTheme::Dark)
                .expect("idempotent replay"),
            first
        );
        let rebound = update(&store, "0", "preference-mutation-1", DesktopTheme::Light)
            .expect_err("mutation identity must stay payload-bound");
        assert_eq!(rebound.code, DesktopPreferencesErrorCode::InvalidRequest);
        let conflict = update(&store, "0", "preference-mutation-2", DesktopTheme::Light)
            .expect_err("stale revision");
        assert_eq!(conflict.code, DesktopPreferencesErrorCode::Conflict);
        assert_eq!(conflict.current_revision.as_deref(), Some("1"));

        let restarted = DesktopPreferencesStore::default();
        restarted.configure(&root).expect("reload preferences");
        assert_eq!(restarted.snapshot().expect("persisted snapshot"), first);
    }

    #[test]
    fn reload_failure_keeps_last_known_good_snapshot() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("preferences");
        let store = DesktopPreferencesStore::default();
        store.configure(&root).expect("configure preferences");
        let first =
            update(&store, "0", "preference-mutation-1", DesktopTheme::Dark).expect("first update");
        fs::write(root.join(PREFERENCES_FILE_NAME), b"corrupt").expect("corrupt preferences");

        assert!(store.reload().is_err());
        assert_eq!(store.snapshot().expect("last known good snapshot"), first);
    }

    #[test]
    fn reload_rejects_revision_rollback_and_keeps_last_known_good_snapshot() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("preferences");
        let store = DesktopPreferencesStore::default();
        store.configure(&root).expect("configure preferences");
        let first =
            update(&store, "0", "preference-mutation-1", DesktopTheme::Dark).expect("first update");
        fs::write(
            root.join(PREFERENCES_FILE_NAME),
            serde_json::to_vec(&DesktopPreferencesRecord::default()).expect("rollback fixture"),
        )
        .expect("write rollback fixture");

        assert!(store.reload().is_err());
        assert_eq!(store.snapshot().expect("last known good snapshot"), first);
    }

    #[test]
    fn missing_persisted_file_cannot_reset_a_published_revision() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("preferences");
        let store = DesktopPreferencesStore::default();
        store.configure(&root).expect("configure preferences");
        let first =
            update(&store, "0", "preference-mutation-1", DesktopTheme::Dark).expect("first update");
        fs::remove_file(root.join(PREFERENCES_FILE_NAME)).expect("remove preferences fixture");

        assert!(store.reload().is_err());
        assert_eq!(store.snapshot().expect("last known good snapshot"), first);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_storage_root_is_rejected_without_following_it() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temporary = tempfile::tempdir().expect("temporary directory");
        let target = temporary.path().join("target");
        fs::create_dir(&target).expect("target directory");
        let target_mode = fs::metadata(&target)
            .expect("target metadata")
            .permissions()
            .mode()
            & 0o777;
        let root = temporary.path().join("preferences");
        symlink(&target, &root).expect("symbolic link fixture");
        let store = DesktopPreferencesStore::default();

        let error = store
            .configure(&root)
            .expect_err("symbolic link must fail closed");
        assert_eq!(error.code, DesktopPreferencesErrorCode::Storage);
        assert_eq!(
            fs::metadata(target)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o777,
            target_mode
        );
    }
}
