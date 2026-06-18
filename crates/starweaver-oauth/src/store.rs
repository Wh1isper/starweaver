//! File-backed OAuth credential store.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use fs2::FileExt;

use crate::{
    error::{io_error, OAuthResult},
    types::{AuthFile, OAuthProviderRecord},
    STARWEAVER_OAUTH_AUTH_FILE_ENV,
};

/// File-backed OAuth credential store with process-level locking.
#[derive(Clone, Debug)]
pub struct OAuthStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl OAuthStore {
    /// Create a store at an explicit path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let lock_path = path.with_extension(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map_or_else(
                    || "lock".to_string(),
                    |extension| format!("{extension}.lock"),
                ),
        );
        Self { path, lock_path }
    }

    /// Create a store at the default Starweaver auth path.
    pub fn default_store() -> Self {
        Self::new(default_auth_path())
    }

    /// Return the default auth path.
    pub fn default_path() -> PathBuf {
        default_auth_path()
    }

    /// Return the backing auth file path.
    pub const fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Return the backing lock file path.
    pub const fn lock_path(&self) -> &PathBuf {
        &self.lock_path
    }

    /// Load the full auth file.
    pub fn load(&self) -> OAuthResult<AuthFile> {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.lock_path)?;
        self.load_unlocked()
    }

    /// Save the full auth file.
    pub fn save(&self, auth_file: &AuthFile) -> OAuthResult<()> {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.lock_path)?;
        self.save_unlocked(auth_file)
    }

    /// Load one provider record.
    pub fn get_provider(&self, provider_name: &str) -> OAuthResult<Option<OAuthProviderRecord>> {
        Ok(self.load()?.providers.get(provider_name).cloned())
    }

    /// Save one provider record.
    pub fn set_provider(
        &self,
        provider_name: &str,
        record: OAuthProviderRecord,
    ) -> OAuthResult<()> {
        self.update(|auth_file| {
            auth_file
                .providers
                .insert(provider_name.to_string(), record);
            Ok(())
        })
    }

    /// Delete one provider record and return the deleted record.
    pub fn delete_provider(&self, provider_name: &str) -> OAuthResult<Option<OAuthProviderRecord>> {
        self.update(|auth_file| Ok(auth_file.providers.remove(provider_name)))
    }

    /// Remove one provider record and return whether it existed.
    pub fn remove_provider(&self, provider_name: &str) -> OAuthResult<bool> {
        Ok(self.delete_provider(provider_name)?.is_some())
    }

    /// Update the auth file while holding the store lock.
    pub fn update<T>(
        &self,
        updater: impl FnOnce(&mut AuthFile) -> OAuthResult<T>,
    ) -> OAuthResult<T> {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.lock_path)?;
        let mut auth_file = self.load_unlocked()?;
        let result = updater(&mut auth_file)?;
        self.save_unlocked(&auth_file)?;
        Ok(result)
    }

    fn ensure_parent(&self) -> OAuthResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
            set_dir_private(parent);
        }
        Ok(())
    }

    fn load_unlocked(&self) -> OAuthResult<AuthFile> {
        if !self.path.exists() {
            return Ok(AuthFile::default());
        }
        set_file_private(&self.path);
        let mut content = String::new();
        File::open(&self.path)
            .map_err(|error| io_error(&self.path, error))?
            .read_to_string(&mut content)
            .map_err(|error| io_error(&self.path, error))?;
        Ok(serde_json::from_str(&content)?)
    }

    fn save_unlocked(&self, auth_file: &AuthFile) -> OAuthResult<()> {
        let temp_path = self.path.with_extension("json.tmp");
        {
            let mut file = File::create(&temp_path).map_err(|error| io_error(&temp_path, error))?;
            set_file_private(&temp_path);
            serde_json::to_writer_pretty(&mut file, auth_file)?;
            file.write_all(b"\n")
                .map_err(|error| io_error(&temp_path, error))?;
            file.sync_all()
                .map_err(|error| io_error(&temp_path, error))?;
        }
        fs::rename(&temp_path, &self.path).map_err(|error| io_error(&self.path, error))?;
        set_file_private(&self.path);
        Ok(())
    }
}

impl Default for OAuthStore {
    fn default() -> Self {
        Self::default_store()
    }
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire(path: &Path) -> OAuthResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| io_error(path, error))?;
        file.lock_exclusive()
            .map_err(|error| io_error(path, error))?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(unix)]
pub fn set_dir_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub(crate) fn set_dir_private(_path: &Path) {}

#[cfg(unix)]
pub fn set_file_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
pub(crate) fn set_file_private(_path: &Path) {}

/// Return the default auth directory under `~/.starweaver`.
pub fn default_auth_dir() -> PathBuf {
    env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".starweaver")
}

/// Return the default auth file path under `~/.starweaver/auth.json`.
pub fn default_auth_path() -> PathBuf {
    env::var_os(STARWEAVER_OAUTH_AUTH_FILE_ENV)
        .map_or_else(|| default_auth_dir().join("auth.json"), PathBuf::from)
}
