use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use super::InteractiveTuiState;

const HISTORY_VERSION: u32 = 1;
const HISTORY_MAX_ENTRIES: usize = 100;
pub(super) const HISTORY_MAX_ENTRY_BYTES: usize = 16 * 1024;
const HISTORY_MAX_FILE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PromptHistoryFile {
    version: u32,
    workspace: String,
    entries: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct HistorySearchState {
    pub(in crate::tui) query: String,
    matches: Vec<usize>,
    selected: usize,
}

impl HistorySearchState {
    fn new(query: String, history: &[String]) -> Self {
        let mut state = Self {
            query,
            matches: Vec::new(),
            selected: 0,
        };
        state.refresh(history);
        state
    }

    fn refresh(&mut self, history: &[String]) {
        let query = self.query.to_lowercase();
        self.matches = history
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(index, entry)| {
                (query.is_empty() || entry.to_lowercase().contains(&query)).then_some(index)
            })
            .collect();
        self.selected = 0;
    }

    const fn move_selection(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let len = self.matches.len();
        let steps = delta.unsigned_abs() % len;
        self.selected = if delta.is_negative() {
            (self.selected + len - steps) % len
        } else {
            (self.selected + steps) % len
        };
    }

    fn selected_history_index(&self) -> Option<usize> {
        self.matches.get(self.selected).copied()
    }

    pub(in crate::tui) fn position(&self) -> Option<(usize, usize)> {
        (!self.matches.is_empty()).then_some((self.selected + 1, self.matches.len()))
    }
}

pub(super) fn load_prompt_history(config_dir: &Path, workspace: &str) -> (PathBuf, Vec<String>) {
    let path = prompt_history_path(config_dir, workspace);
    let entries = load_history_file(&path, workspace).unwrap_or_default();
    (path, entries)
}

fn prompt_history_path(config_dir: &Path, workspace: &str) -> PathBuf {
    let hash = digest(&SHA256, workspace.as_bytes());
    let key = hash.as_ref()[..16]
        .iter()
        .fold(String::with_capacity(32), |mut key, byte| {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            key.push(char::from(HEX[usize::from(*byte >> 4)]));
            key.push(char::from(HEX[usize::from(*byte & 0x0f)]));
            key
        });
    config_dir
        .join("prompt-history")
        .join(format!("{key}.json"))
}

fn load_history_file(path: &Path, workspace: &str) -> Option<Vec<String>> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > HISTORY_MAX_FILE_BYTES as u64 {
        return None;
    }
    let content = fs::read(path).ok()?;
    let stored = serde_json::from_slice::<PromptHistoryFile>(&content).ok()?;
    if stored.version != HISTORY_VERSION || stored.workspace != workspace {
        return None;
    }
    Some(bound_entries(stored.entries))
}

pub(super) fn bound_entries(entries: Vec<String>) -> Vec<String> {
    let mut total = 0usize;
    let mut retained = Vec::new();
    for entry in entries.into_iter().rev() {
        let entry = entry.trim().to_string();
        let bytes = entry.len();
        if entry.is_empty() || bytes > HISTORY_MAX_ENTRY_BYTES {
            continue;
        }
        if total.saturating_add(bytes) > HISTORY_MAX_FILE_BYTES {
            break;
        }
        total = total.saturating_add(bytes);
        retained.push(entry);
        if retained.len() >= HISTORY_MAX_ENTRIES {
            break;
        }
    }
    retained.reverse();
    retained
}

fn persist_prompt_history(path: &Path, workspace: &str, entries: &[String]) -> Result<(), String> {
    let Some(dir) = path.parent() else {
        return Err("history path has no parent".to_string());
    };
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    set_private_directory_permissions(dir).map_err(|error| error.to_string())?;
    let stored = PromptHistoryFile {
        version: HISTORY_VERSION,
        workspace: workspace.to_string(),
        entries: bound_entries(entries.to_vec()),
    };
    let data = serde_json::to_vec_pretty(&stored).map_err(|error| error.to_string())?;
    let temp = dir.join(format!("history.{}.json.tmp", std::process::id()));
    write_private_file(&temp, &data).map_err(|error| error.to_string())?;
    fs::rename(&temp, path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn write_private_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

impl InteractiveTuiState {
    pub(in crate::tui) const fn history_search_visible(&self) -> bool {
        self.history_search.is_some()
    }

    pub(in crate::tui) const fn history_search(&self) -> Option<&HistorySearchState> {
        self.history_search.as_ref()
    }

    pub(in crate::tui) fn history_search_result(&self) -> Option<&str> {
        let index = self.history_search.as_ref()?.selected_history_index()?;
        self.history.get(index).map(String::as_str)
    }

    pub(in crate::tui) fn open_history_search(&mut self) {
        if self.history.is_empty() {
            self.input_status = Some("prompt history is empty".to_string());
            return;
        }
        self.close_command_palette();
        self.history_search = Some(HistorySearchState::new(self.input.clone(), &self.history));
        self.input_status = Some("history search".to_string());
    }

    pub(in crate::tui) fn close_history_search(&mut self) {
        self.history_search = None;
        self.input_status = Some("history search closed".to_string());
    }

    pub(in crate::tui) fn accept_history_search(&mut self) {
        let selected = self
            .history_search
            .as_ref()
            .and_then(HistorySearchState::selected_history_index)
            .and_then(|index| self.history.get(index))
            .cloned();
        self.history_search = None;
        if let Some(selected) = selected {
            self.input = selected;
            self.move_composer_cursor_to_end();
            self.reset_composer_scroll();
            self.refresh_command_palette();
            self.input_status = Some("history recalled".to_string());
        } else {
            self.input_status = Some("no matching history".to_string());
        }
    }

    pub(in crate::tui) const fn move_history_search(&mut self, delta: isize) {
        if let Some(search) = self.history_search.as_mut() {
            search.move_selection(delta);
        }
    }

    pub(in crate::tui) fn push_history_search_char(&mut self, ch: char) {
        if let Some(search) = self.history_search.as_mut() {
            search.query.push(ch);
            search.refresh(&self.history);
        }
    }

    pub(in crate::tui) fn backspace_history_search(&mut self) {
        if let Some(search) = self.history_search.as_mut()
            && let Some((index, _)) = search.query.grapheme_indices(true).next_back()
        {
            search.query.truncate(index);
            search.refresh(&self.history);
        }
    }

    pub(in crate::tui) const fn repeat_history_search(&mut self) {
        self.move_history_search(1);
    }

    pub(super) fn clear_persisted_history(&mut self) {
        self.history.clear();
        self.history_index = None;
        self.history_draft.clear();
        self.history_search = None;
        if !self.history_persistence_enabled {
            return;
        }
        if let Err(error) =
            persist_prompt_history(&self.history_path, &self.workspace_dir, &self.history)
        {
            self.input_status = Some(format!("history clear failed: {error}"));
        }
    }

    pub(super) fn persist_history(&mut self) {
        if !self.history_persistence_enabled {
            return;
        }
        if let Err(error) =
            persist_prompt_history(&self.history_path, &self.workspace_dir, &self.history)
        {
            self.input_status = Some(format!("history save failed: {error}"));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn history_store_is_workspace_scoped_bounded_and_private() {
        let dir = tempdir().unwrap();
        let workspace = "/workspace/example";
        let (path, entries) = load_prompt_history(dir.path(), workspace);
        assert!(entries.is_empty());
        persist_prompt_history(&path, workspace, &["first".into(), "second".into()]).unwrap();
        assert_eq!(
            load_prompt_history(dir.path(), workspace).1,
            ["first", "second"]
        );
        assert!(
            load_prompt_history(dir.path(), "/workspace/other")
                .1
                .is_empty()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn history_store_recovers_from_malformed_and_oversized_state() {
        let dir = tempdir().unwrap();
        let workspace = "/workspace/example";
        let (path, _) = load_prompt_history(dir.path(), workspace);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not-json").unwrap();
        assert!(load_prompt_history(dir.path(), workspace).1.is_empty());

        let entries = (0..150)
            .map(|index| format!("prompt {index}"))
            .chain(std::iter::once("x".repeat(HISTORY_MAX_ENTRY_BYTES + 1)))
            .collect::<Vec<_>>();
        persist_prompt_history(&path, workspace, &entries).unwrap();
        let loaded = load_prompt_history(dir.path(), workspace).1;
        assert_eq!(loaded.len(), HISTORY_MAX_ENTRIES);
        assert_eq!(loaded.first().map(String::as_str), Some("prompt 50"));
        assert_eq!(loaded.last().map(String::as_str), Some("prompt 149"));
    }

    #[test]
    fn history_search_filters_newest_first() {
        let state = HistorySearchState::new(
            "fix".to_string(),
            &[
                "fix old".to_string(),
                "other".to_string(),
                "fix new".to_string(),
            ],
        );
        assert_eq!(state.matches, [2, 0]);
        assert_eq!(state.position(), Some((1, 2)));
    }
}
