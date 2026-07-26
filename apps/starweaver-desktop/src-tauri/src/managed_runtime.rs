//! Desktop-owned selection of the bundled local RPC runtime and public launch envelope.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use sha2::{Digest as _, Sha256};
use starweaver_rpc_core::generated as host;
use tauri::{AppHandle, Manager as _};

use crate::supervisor::{LocalLaunchSpec, SupervisorError};

const LOCAL_EXECUTION_DOMAIN_ID: &str = "local-default";
const LOCAL_DATABASE_IDENTITY: &str = "canonical-local";
const CONFIGURATION_GENERATION: u64 = 1;
const DEFAULT_PROFILE: &str = "default";
const DEFAULT_MODEL_ID: &str = "oauth@codex:gpt-5.6-sol";

/// Resolve the bundled runtime and materialize its immutable public launch selection.
pub fn prepare(app: &AppHandle) -> Result<LocalLaunchSpec, SupervisorError> {
    let current_executable = std::env::current_exe().map_err(|_| {
        SupervisorError::invalid_configuration("Desktop executable location is unavailable")
    })?;
    let runtime_path = bundled_runtime_path(&current_executable)?;
    let home = app.path().home_dir().map_err(|_| {
        SupervisorError::invalid_configuration("user home directory is unavailable")
    })?;
    let root = app
        .path()
        .app_local_data_dir()
        .map_err(|_| {
            SupervisorError::invalid_configuration("Desktop application data is unavailable")
        })?
        .join("supervisor");
    prepare_from_paths(&runtime_path, &home, &root)
}

fn bundled_runtime_path(current_executable: &Path) -> Result<PathBuf, SupervisorError> {
    let directory = current_executable.parent().ok_or_else(|| {
        SupervisorError::invalid_configuration("Desktop executable location is unavailable")
    })?;
    let name = if cfg!(windows) {
        "starweaver-rpc.exe"
    } else {
        "starweaver-rpc"
    };
    Ok(directory.join(name))
}

fn prepare_from_paths(
    runtime_path: &Path,
    home: &Path,
    supervisor_root: &Path,
) -> Result<LocalLaunchSpec, SupervisorError> {
    if !runtime_path.is_absolute() || !home.is_absolute() || !supervisor_root.is_absolute() {
        return Err(SupervisorError::invalid_configuration(
            "managed runtime locations must be absolute",
        ));
    }
    let runtime_path = fs::canonicalize(runtime_path).map_err(|_| {
        SupervisorError::invalid_configuration("bundled Starweaver runtime is unavailable")
    })?;
    if !runtime_path.is_file() {
        return Err(SupervisorError::invalid_configuration(
            "bundled Starweaver runtime is unavailable",
        ));
    }

    let domain_root = supervisor_root
        .join("managed")
        .join(LOCAL_EXECUTION_DOMAIN_ID);
    let state_directory = domain_root.join("state");
    let database_path = home.join(".starweaver").join("starweaver.sqlite");
    let database_path = exact_wire_path(&database_path)?;
    let state_directory_wire = exact_wire_path(&state_directory)?;
    create_private_directory(&state_directory)?;

    let envelope = default_launch_envelope(database_path, state_directory_wire);
    let envelope_bytes = host::encode_launch_envelope(&envelope).map_err(|_| {
        SupervisorError::invalid_configuration("managed launch envelope is invalid")
    })?;
    let launch_envelope_path = domain_root.join("launch.json");
    persist_if_changed(&launch_envelope_path, &envelope_bytes)?;

    Ok(LocalLaunchSpec {
        runtime_digest: sha256_file(&runtime_path)?,
        runtime_path,
        runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        build_revision: option_env!("STARWEAVER_BUILD_REVISION")
            .unwrap_or("source")
            .to_string(),
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        launch_envelope_digest: sha256_bytes(&envelope_bytes),
        launch_envelope_path,
        configuration_generation: CONFIGURATION_GENERATION,
        execution_domain_id: LOCAL_EXECUTION_DOMAIN_ID.to_string(),
    })
}

fn default_launch_envelope(database_path: String, state_directory: String) -> host::LaunchEnvelope {
    host::LaunchEnvelope {
        capability_caps: host::LaunchCapabilityCaps {
            clarifying_questions: true,
            hitl: true,
            native_local_shell: false,
        },
        configuration_generation: host::DecimalU64::new(CONFIGURATION_GENERATION),
        database: host::LaunchDatabase {
            identity: LOCAL_DATABASE_IDENTITY.to_string(),
            path: database_path,
        },
        default_profile: DEFAULT_PROFILE.to_string(),
        execution_domain_id: LOCAL_EXECUTION_DOMAIN_ID.to_string(),
        mode: host::LaunchEnvelopeMode::Value,
        profiles: vec![host::LaunchProfile {
            instructions: Vec::new(),
            model_config: Some("gpt5_350k".to_string()),
            model_id: DEFAULT_MODEL_ID.to_string(),
            model_settings: Some("openai_responses_high".to_string()),
            name: DEFAULT_PROFILE.to_string(),
            toolsets: vec!["filesystem".to_string()],
        }],
        providers: vec![host::LaunchProvider {
            base_url: None,
            credential_env: None,
            enabled: true,
            endpoint_path: None,
            name: "codex".to_string(),
        }],
        schema: host::LaunchSchemaIdentity {
            name: host::LaunchSchemaIdentityName::Value,
            version: 1,
        },
        state_directory,
    }
}

fn exact_wire_path(path: &Path) -> Result<String, SupervisorError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        SupervisorError::invalid_configuration(
            "managed runtime locations cannot be represented exactly",
        )
    })
}

fn create_private_directory(path: &Path) -> Result<(), SupervisorError> {
    fs::create_dir_all(path).map_err(|_| {
        SupervisorError::invalid_configuration("Desktop managed runtime storage is unavailable")
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
            SupervisorError::invalid_configuration("Desktop managed runtime storage is not private")
        })?;
    }
    Ok(())
}

fn persist_if_changed(path: &Path, bytes: &[u8]) -> Result<(), SupervisorError> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| {
        SupervisorError::invalid_configuration("managed launch location is invalid")
    })?;
    create_private_directory(parent)?;
    let temporary = parent.join(format!(".launch-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        atomic_replace(&temporary, path)?;
        sync_parent(parent)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| {
        SupervisorError::invalid_configuration("managed launch envelope could not be persisted")
    })
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    atomicwrites::replace_atomic(source, destination)
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, SupervisorError> {
    let mut file = File::open(path).map_err(|_| {
        SupervisorError::invalid_configuration("bundled Starweaver runtime is unavailable")
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            SupervisorError::invalid_configuration("bundled Starweaver runtime is unreadable")
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn prepares_a_closed_local_launch_without_shell_authority() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let runtime = temp.path().join("starweaver-rpc");
        fs::write(&runtime, b"runtime fixture").expect("runtime fixture");
        let spec = prepare_from_paths(&runtime, temp.path(), &temp.path().join("desktop"))
            .expect("managed launch");
        let envelope_bytes = fs::read(&spec.launch_envelope_path).expect("launch envelope");
        let envelope =
            host::decode_launch_envelope(&envelope_bytes).expect("valid launch envelope");

        assert_eq!(envelope.execution_domain_id, LOCAL_EXECUTION_DOMAIN_ID);
        assert_eq!(envelope.database.identity, LOCAL_DATABASE_IDENTITY);
        assert_eq!(
            Path::new(&envelope.database.path),
            temp.path().join(".starweaver/starweaver.sqlite")
        );
        assert!(!envelope.capability_caps.native_local_shell);
        assert!(envelope.capability_caps.hitl);
        assert!(envelope.capability_caps.clarifying_questions);
        assert_eq!(envelope.profiles[0].toolsets, ["filesystem"]);
        assert_eq!(envelope.profiles[0].model_id, DEFAULT_MODEL_ID);
        assert_eq!(
            envelope.profiles[0].model_config.as_deref(),
            Some("gpt5_350k")
        );
        assert_eq!(
            envelope.profiles[0].model_settings.as_deref(),
            Some("openai_responses_high")
        );
        assert_eq!(envelope.providers[0].name, "codex");
        assert!(envelope.providers[0].credential_env.is_none());
        assert_eq!(spec.runtime_digest, sha256_bytes(b"runtime fixture"));
        assert_eq!(spec.launch_envelope_digest, sha256_bytes(&envelope_bytes));
    }

    #[test]
    fn preparation_fails_without_the_bundled_runtime() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let error = prepare_from_paths(
            &temp.path().join("missing"),
            temp.path(),
            &temp.path().join("desktop"),
        )
        .expect_err("missing runtime");
        assert_eq!(error.message, "bundled Starweaver runtime is unavailable");
    }

    #[cfg(unix)]
    #[test]
    fn preparation_rejects_authority_paths_that_cannot_round_trip_through_the_wire() {
        use std::{
            ffi::OsString,
            os::unix::ffi::{OsStrExt as _, OsStringExt as _},
        };

        let temp = tempfile::tempdir().expect("temporary directory");
        let runtime = temp.path().join("starweaver-rpc");
        fs::write(&runtime, b"runtime fixture").expect("runtime fixture");
        let mut home = temp.path().as_os_str().as_bytes().to_vec();
        home.extend_from_slice(b"/invalid-\xff");
        let error = prepare_from_paths(
            &runtime,
            &PathBuf::from(OsString::from_vec(home)),
            &temp.path().join("desktop"),
        )
        .expect_err("non-Unicode authority path must fail closed");

        assert_eq!(
            error.message,
            "managed runtime locations cannot be represented exactly"
        );
    }

    #[test]
    fn identical_launch_selection_keeps_the_persisted_envelope_stable() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let runtime = temp.path().join("starweaver-rpc");
        fs::write(&runtime, b"runtime fixture").expect("runtime fixture");
        let root = temp.path().join("desktop");
        let first = prepare_from_paths(&runtime, temp.path(), &root).expect("first launch");
        let first_identity = fs::metadata(&first.launch_envelope_path)
            .expect("first metadata")
            .modified()
            .expect("first modified time");
        let second = prepare_from_paths(&runtime, temp.path(), &root).expect("second launch");
        let second_identity = fs::metadata(&second.launch_envelope_path)
            .expect("second metadata")
            .modified()
            .expect("second modified time");

        assert_eq!(first.launch_envelope_digest, second.launch_envelope_digest);
        assert_eq!(first_identity, second_identity);
    }
}
