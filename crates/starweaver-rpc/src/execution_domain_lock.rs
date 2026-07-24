//! Process-lifetime exclusivity and fencing for one execution domain/database identity.

use std::{fs, io, path::Path, sync::Arc};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    RpcHostError, RpcHostResult,
    private_fs::{atomic_write_json, open_private_lock},
};

const OWNER_VERSION: u32 = 1;

pub(crate) struct ExecutionDomainOwnerLease {
    _lock: fs::File,
    generation: u64,
    host_instance_id: String,
}

fn owner_conflict() -> RpcHostError {
    RpcHostError::RunConflict(
        "another host owns this execution domain and database identity".to_string(),
    )
}

fn is_lock_contended(error: &io::Error) -> bool {
    let contended = fs2::lock_contended_error();
    match (error.raw_os_error(), contended.raw_os_error()) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => error.kind() == contended.kind(),
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistedOwner {
    version: u32,
    execution_domain_id: String,
    database_identity: String,
    generation: u64,
    host_instance_id: String,
}

impl ExecutionDomainOwnerLease {
    pub(crate) fn acquire(
        execution_domain_id: &str,
        database_identity: &str,
    ) -> RpcHostResult<Arc<Self>> {
        let root = starweaver_storage::default_starweaver_config_dir()?
            .join("coordination")
            .join("execution-locks");
        Self::acquire_at(&root, execution_domain_id, database_identity)
    }

    fn acquire_at(
        root: &Path,
        execution_domain_id: &str,
        database_identity: &str,
    ) -> RpcHostResult<Arc<Self>> {
        fs::create_dir_all(root)?;
        let key = lock_key(execution_domain_id, database_identity);
        let lock_path = root.join(format!("execution-{key}.lock"));
        let owner_path = root.join(format!("execution-{key}.owner.json"));
        let lock = open_private_lock(&lock_path)?;
        lock.try_lock_exclusive().map_err(|error| {
            if is_lock_contended(&error) {
                owner_conflict()
            } else {
                RpcHostError::Io(error)
            }
        })?;

        let generation = match fs::read(&owner_path) {
            Ok(bytes) => {
                let owner: PersistedOwner = serde_json::from_slice(&bytes).map_err(|error| {
                    RpcHostError::Storage(format!("invalid execution-domain owner state: {error}"))
                })?;
                if owner.version != OWNER_VERSION
                    || owner.execution_domain_id != execution_domain_id
                    || owner.database_identity != database_identity
                {
                    return Err(RpcHostError::Storage(
                        "execution-domain owner state identity mismatch".to_string(),
                    ));
                }
                owner.generation.checked_add(1).ok_or_else(|| {
                    RpcHostError::Storage("execution-domain owner generation overflow".to_string())
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 1,
            Err(error) => return Err(error.into()),
        };
        let host_instance_id = format!("rpc-host-{generation}-{}", Uuid::new_v4());
        atomic_write_json(
            &owner_path,
            &PersistedOwner {
                version: OWNER_VERSION,
                execution_domain_id: execution_domain_id.to_string(),
                database_identity: database_identity.to_string(),
                generation,
                host_instance_id: host_instance_id.clone(),
            },
            "execution-domain owner state",
        )?;
        Ok(Arc::new(Self {
            _lock: lock,
            generation,
            host_instance_id,
        }))
    }

    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn host_instance_id(&self) -> &str {
        &self.host_instance_id
    }
}

fn lock_key(execution_domain_id: &str, database_identity: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"starweaver.execution-domain-lock.v1\0");
    digest.update(execution_domain_id.len().to_be_bytes());
    digest.update(execution_domain_id.as_bytes());
    digest.update(database_identity.len().to_be_bytes());
    digest.update(database_identity.as_bytes());
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::process::Command;

    use super::*;

    const PROBE_ROOT_ENV: &str = "STARWEAVER_RPC_EXECUTION_LOCK_PROBE_ROOT";
    const PROBE_EXPECT_ENV: &str = "STARWEAVER_RPC_EXECUTION_LOCK_PROBE_EXPECT";

    #[test]
    fn lease_is_process_lifetime_exclusive_and_fenced_across_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let first =
            ExecutionDomainOwnerLease::acquire_at(temp.path(), "domain", "database").unwrap();
        assert_eq!(first.generation(), 1);
        assert!(matches!(
            ExecutionDomainOwnerLease::acquire_at(temp.path(), "domain", "database"),
            Err(RpcHostError::RunConflict(_))
        ));
        run_subprocess_probe(temp.path(), "conflict");

        drop(first);
        let second =
            ExecutionDomainOwnerLease::acquire_at(temp.path(), "domain", "database").unwrap();
        assert_eq!(second.generation(), 2);
        drop(second);
        run_subprocess_probe(temp.path(), "success");
    }

    #[test]
    fn failed_acquisition_releases_os_lock() {
        let temp = tempfile::tempdir().unwrap();
        let key = lock_key("domain", "database");
        let owner_path = temp.path().join(format!("execution-{key}.owner.json"));
        fs::write(&owner_path, b"not-json").unwrap();

        assert!(matches!(
            ExecutionDomainOwnerLease::acquire_at(temp.path(), "domain", "database"),
            Err(RpcHostError::Storage(_))
        ));
        fs::remove_file(owner_path).unwrap();
        run_subprocess_probe(temp.path(), "success");

        let lease =
            ExecutionDomainOwnerLease::acquire_at(temp.path(), "domain", "database").unwrap();
        assert_eq!(lease.generation(), 2);
    }

    #[test]
    fn subprocess_lock_probe() {
        let Some(root) = std::env::var_os(PROBE_ROOT_ENV) else {
            return;
        };
        let expected = std::env::var(PROBE_EXPECT_ENV).unwrap();
        let result = ExecutionDomainOwnerLease::acquire_at(Path::new(&root), "domain", "database");
        match expected.as_str() {
            "conflict" => assert!(matches!(result, Err(RpcHostError::RunConflict(_)))),
            "success" => assert!(result.is_ok()),
            other => panic!("unexpected lock probe expectation: {other}"),
        }
    }

    fn run_subprocess_probe(root: &Path, expected: &str) {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "execution_domain_lock::tests::subprocess_lock_probe",
                "--nocapture",
            ])
            .env(PROBE_ROOT_ENV, root)
            .env(PROBE_EXPECT_ENV, expected)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
