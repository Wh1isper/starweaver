//! Host-verifiable, short-lived and durable one-command configuration grants.

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    RpcHostError, RpcHostResult,
    private_fs::{atomic_write, atomic_write_json},
};

const KEY_FILE: &str = "config-authorization.key";
const STATE_FILE: &str = "config-authorization-state.json";
const STATE_VERSION: u32 = 1;
const MAX_CLOCK_SKEW_MS: i64 = 30_000;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub(crate) struct ConfigAuthorizationManager {
    inner: Arc<Inner>,
}

struct Inner {
    root: std::path::PathBuf,
    key: Vec<u8>,
    state: Mutex<PersistedState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ConfigAuthorizationClaims {
    pub(crate) version: u32,
    pub(crate) execution_domain_id: String,
    pub(crate) operation: String,
    pub(crate) expected_revision: String,
    pub(crate) candidate_fingerprint: String,
    pub(crate) idempotency_key: String,
    pub(crate) nonce: String,
    pub(crate) issued_at_ms: i64,
    pub(crate) expires_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedState {
    version: u32,
    consumed: BTreeMap<String, String>,
}

impl ConfigAuthorizationManager {
    pub(crate) fn load_or_create(state_dir: &Path) -> RpcHostResult<Self> {
        fs::create_dir_all(state_dir)?;
        let key_path = state_dir.join(KEY_FILE);
        let key = match fs::read(&key_path) {
            Ok(bytes) => URL_SAFE_NO_PAD.decode(trim_ascii(&bytes)).map_err(|_| {
                RpcHostError::Storage("invalid config authorization key".to_string())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut material = Vec::new();
                material.extend_from_slice(Uuid::new_v4().as_bytes());
                material.extend_from_slice(Uuid::new_v4().as_bytes());
                let key = Sha256::digest(material).to_vec();
                atomic_write(
                    &key_path,
                    URL_SAFE_NO_PAD.encode(&key).as_bytes(),
                    "config authorization key",
                )?;
                key
            }
            Err(error) => return Err(error.into()),
        };
        if key.len() != 32 {
            return Err(RpcHostError::Storage(
                "config authorization key has invalid length".to_string(),
            ));
        }
        let root = state_dir.to_path_buf();
        let state = match fs::read(root.join(STATE_FILE)) {
            Ok(bytes) => {
                let state: PersistedState = serde_json::from_slice(&bytes).map_err(|error| {
                    RpcHostError::Storage(format!("invalid config authorization state: {error}"))
                })?;
                if state.version != STATE_VERSION {
                    return Err(RpcHostError::Storage(
                        "unsupported config authorization state version".to_string(),
                    ));
                }
                state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedState {
                version: STATE_VERSION,
                consumed: BTreeMap::new(),
            },
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                key,
                state: Mutex::new(state),
            }),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn verify_and_consume(
        &self,
        token: &str,
        execution_domain_id: &str,
        operation: &str,
        expected_revision: &str,
        candidate_fingerprint: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<()> {
        let (payload, signature) = token.split_once('.').ok_or_else(denied)?;
        let payload_bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| denied())?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| denied())?;
        let mut mac = HmacSha256::new_from_slice(&self.inner.key).map_err(|_| denied())?;
        mac.update(&payload_bytes);
        mac.verify_slice(&signature).map_err(|_| denied())?;
        let claims: ConfigAuthorizationClaims =
            serde_json::from_slice(&payload_bytes).map_err(|_| denied())?;
        if claims.version != 1
            || claims.execution_domain_id != execution_domain_id
            || claims.operation != operation
            || claims.expected_revision != expected_revision
            || claims.candidate_fingerprint != candidate_fingerprint
            || claims.idempotency_key != idempotency_key
            || Uuid::parse_str(&claims.nonce).is_err()
        {
            return Err(denied());
        }
        let binding = format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    operation,
                    expected_revision,
                    candidate_fingerprint,
                    idempotency_key,
                    command_fingerprint,
                ))
                .map_err(|_| denied())?
            )
        );
        let mut state =
            self.inner.state.lock().map_err(|_| {
                RpcHostError::Runtime("config authorization lock poisoned".to_string())
            })?;
        if let Some(existing) = state.consumed.get(&claims.nonce) {
            return if existing == &binding {
                Ok(())
            } else {
                Err(denied())
            };
        }
        let now = chrono::Utc::now().timestamp_millis();
        if claims.issued_at_ms > now.saturating_add(MAX_CLOCK_SKEW_MS)
            || claims.expires_at_ms < now
            || claims.expires_at_ms.saturating_sub(claims.issued_at_ms) > 300_000
        {
            return Err(denied());
        }
        let mut persisted = state.clone();
        persisted.consumed.insert(claims.nonce, binding);
        atomic_write_json(
            &self.inner.root.join(STATE_FILE),
            &persisted,
            "config authorization state",
        )?;
        *state = persisted;
        drop(state);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn sign_for_test(
        &self,
        claims: &ConfigAuthorizationClaims,
    ) -> RpcHostResult<String> {
        let payload =
            serde_json::to_vec(claims).map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let mut mac = HmacSha256::new_from_slice(&self.inner.key)
            .map_err(|_| RpcHostError::Runtime("invalid test signing key".to_string()))?;
        mac.update(&payload);
        let signature = mac.finalize().into_bytes();
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|value| !value.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|value| !value.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

fn denied() -> RpcHostError {
    RpcHostError::Invalid("configuration authorization grant is invalid".to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::{thread, time::Duration};

    use super::*;

    const DOMAIN: &str = "desktop-local";
    const OPERATION: &str = "config.update";
    const REVISION: &str = "sha256:active";
    const CANDIDATE: &str = "sha256:candidate";
    const IDEMPOTENCY_KEY: &str = "desktop-config-update";
    const COMMAND: &str = "sha256:command";

    fn claims(nonce: String, issued_at_ms: i64, expires_at_ms: i64) -> ConfigAuthorizationClaims {
        ConfigAuthorizationClaims {
            version: 1,
            execution_domain_id: DOMAIN.to_string(),
            operation: OPERATION.to_string(),
            expected_revision: REVISION.to_string(),
            candidate_fingerprint: CANDIDATE.to_string(),
            idempotency_key: IDEMPOTENCY_KEY.to_string(),
            nonce,
            issued_at_ms,
            expires_at_ms,
        }
    }

    fn verify(manager: &ConfigAuthorizationManager, token: &str) -> RpcHostResult<()> {
        manager.verify_and_consume(
            token,
            DOMAIN,
            OPERATION,
            REVISION,
            CANDIDATE,
            IDEMPOTENCY_KEY,
            COMMAND,
        )
    }

    #[test]
    fn rejects_tampered_and_unconsumed_expired_grants() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ConfigAuthorizationManager::load_or_create(temp.path()).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let valid_claims = claims(Uuid::new_v4().to_string(), now, now + 60_000);
        let token = manager.sign_for_test(&valid_claims).unwrap();
        let (payload, signature) = token.split_once('.').unwrap();
        let mut tampered_claims: ConfigAuthorizationClaims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        tampered_claims.candidate_fingerprint = "sha256:tampered".to_string();
        let tampered_payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&tampered_claims).unwrap());
        assert!(verify(&manager, &format!("{tampered_payload}.{signature}")).is_err());

        let expired_claims = claims(Uuid::new_v4().to_string(), now - 60_000, now - 1);
        let expired = manager.sign_for_test(&expired_claims).unwrap();
        assert!(verify(&manager, &expired).is_err());
        assert!(verify(&manager, &token).is_ok());
    }

    #[test]
    fn consumed_exact_binding_can_retry_after_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ConfigAuthorizationManager::load_or_create(temp.path()).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let grant_claims = claims(Uuid::new_v4().to_string(), now, now + 100);
        let token = manager.sign_for_test(&grant_claims).unwrap();
        verify(&manager, &token).unwrap();
        thread::sleep(Duration::from_millis(150));
        verify(&manager, &token).unwrap();
    }

    #[test]
    fn consumed_nonce_rejects_a_different_command_binding() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ConfigAuthorizationManager::load_or_create(temp.path()).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let update_claims = claims(nonce.clone(), now, now + 60_000);
        let update_token = manager.sign_for_test(&update_claims).unwrap();
        verify(&manager, &update_token).unwrap();

        let mut discard_claims = claims(nonce, now, now + 60_000);
        discard_claims.operation = "config.discard".to_string();
        discard_claims.expected_revision = "sha256:desired".to_string();
        discard_claims.candidate_fingerprint = "sha256:desired".to_string();
        discard_claims.idempotency_key = "desktop-config-discard".to_string();
        let discard_token = manager.sign_for_test(&discard_claims).unwrap();
        assert!(
            manager
                .verify_and_consume(
                    &discard_token,
                    DOMAIN,
                    "config.discard",
                    "sha256:desired",
                    "sha256:desired",
                    "desktop-config-discard",
                    "sha256:discard-command",
                )
                .is_err()
        );
        verify(&manager, &update_token).unwrap();
    }
}
