//! RPC-owned client selection state.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use fs2::FileExt as _;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{RpcHostError, RpcHostResult};

const STATE_FILE_NAME: &str = "state.json";

#[derive(Clone, Default)]
pub struct RpcStateRepository {
    state_dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Default, Deserialize, Serialize)]
struct RpcClientState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    selected_profiles: BTreeMap<String, String>,
}

impl RpcStateRepository {
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn read_current_session(&self) -> RpcHostResult<Option<String>> {
        let _guard = self.lock_state()?;
        let _file_lock = lock_state_file(&self.state_dir)?;
        Ok(read_state(&self.state_dir)?.current_session_id)
    }

    pub fn write_current_session(&self, session_id: &str) -> RpcHostResult<()> {
        let _guard = self.lock_state()?;
        let _file_lock = lock_state_file(&self.state_dir)?;
        let mut state = read_state(&self.state_dir)?;
        state.current_session_id = Some(session_id.to_string());
        write_state(&self.state_dir, &state)
    }

    pub fn read_selected_profile(&self, scope: &str) -> RpcHostResult<Option<String>> {
        let _guard = self.lock_state()?;
        let _file_lock = lock_state_file(&self.state_dir)?;
        Ok(read_state(&self.state_dir)?
            .selected_profiles
            .get(scope)
            .cloned())
    }

    pub fn write_selected_profile(&self, scope: &str, profile: &str) -> RpcHostResult<()> {
        let _guard = self.lock_state()?;
        let _file_lock = lock_state_file(&self.state_dir)?;
        let mut state = read_state(&self.state_dir)?;
        state
            .selected_profiles
            .insert(scope.to_string(), profile.to_string());
        write_state(&self.state_dir, &state)
    }

    fn lock_state(&self) -> RpcHostResult<std::sync::MutexGuard<'_, ()>> {
        self.lock
            .lock()
            .map_err(|error| RpcHostError::Runtime(format!("RPC state lock poisoned: {error}")))
    }
}

fn lock_state_file(state_dir: &Path) -> RpcHostResult<StateFileLock> {
    fs::create_dir_all(state_dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state_dir.join("state.lock"))?;
    file.lock_exclusive()?;
    Ok(StateFileLock(file))
}

struct StateFileLock(File);

impl Drop for StateFileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn read_state(state_dir: &Path) -> RpcHostResult<RpcClientState> {
    let path = state_dir.join(STATE_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<RpcClientState>(&content)
            .map_err(|error| RpcHostError::Invalid(format!("invalid RPC state: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RpcClientState::default()),
        Err(error) => Err(RpcHostError::Io(error)),
    }
}

fn write_state(state_dir: &Path, state: &RpcClientState) -> RpcHostResult<()> {
    fs::create_dir_all(state_dir)?;
    let path = state_dir.join(STATE_FILE_NAME);
    let temporary = state_dir.join(format!("{STATE_FILE_NAME}.{}.tmp", Uuid::new_v4()));
    let payload = serde_json::to_vec_pretty(state)
        .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&payload)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(RpcHostError::Io(error));
    }
    if let Ok(directory) = File::open(state_dir) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn state_updates_preserve_session_and_scoped_model_selections() {
        let temp = tempfile::tempdir().unwrap();
        let state = RpcStateRepository::new(temp.path());
        state.write_current_session("session_test").unwrap();
        state.write_selected_profile("desktop", "coding").unwrap();
        state.write_selected_profile("tui", "general").unwrap();

        let reopened = RpcStateRepository::new(temp.path());
        assert_eq!(
            reopened.read_current_session().unwrap().as_deref(),
            Some("session_test")
        );
        assert_eq!(
            reopened
                .read_selected_profile("desktop")
                .unwrap()
                .as_deref(),
            Some("coding")
        );
        assert_eq!(
            reopened.read_selected_profile("tui").unwrap().as_deref(),
            Some("general")
        );
    }
}
