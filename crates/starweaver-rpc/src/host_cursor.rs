//! Opaque, integrity-protected durable host-event cursors.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read as _, Write as _},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use fs2::FileExt as _;
use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use starweaver_rpc_core::generated::{EventViewRequest, HostEventCursor};
use uuid::Uuid;

const CURSOR_VERSION: u8 = 1;
const TAG_BYTES: usize = 32;
const KEY_FILE_NAME: &str = "host-cursor.key";
const KEY_LOCK_FILE_NAME: &str = "host-cursor.lock";

type HmacSha256 = Hmac<Sha256>;

/// Exact reason a public cursor could not be admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorAdmissionError {
    Malformed,
    IntegrityFailed,
    ScopeMismatch,
    ViewMismatch,
    StorageMismatch,
}

#[derive(Clone)]
pub(crate) struct HostCursorCodec {
    key: [u8; TAG_BYTES],
    storage_binding: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CursorClaims {
    version: u8,
    kind: String,
    position: Value,
    storage: String,
    authority: String,
    view: String,
}

impl HostCursorCodec {
    pub(crate) fn new(key: [u8; TAG_BYTES], storage_identity: &str) -> Self {
        Self {
            key,
            storage_binding: digest_binding("storage", storage_identity.as_bytes()),
        }
    }

    pub(crate) fn load_or_create(state_dir: &Path, storage_identity: &str) -> io::Result<Self> {
        fs::create_dir_all(state_dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state_dir.join(KEY_LOCK_FILE_NAME))?;
        lock.lock_exclusive()?;
        let key_result = load_or_create_key(&state_dir.join(KEY_FILE_NAME));
        let unlock_result = fs2::FileExt::unlock(&lock);
        let key = key_result?;
        unlock_result?;
        Ok(Self::new(key, storage_identity))
    }

    pub(crate) fn encode(
        &self,
        position: u64,
        authority_identity: &str,
        view: &EventViewRequest,
    ) -> Result<HostEventCursor, String> {
        HostEventCursor::new(self.encode_opaque("events", &position, authority_identity, view)?)
    }

    pub(crate) fn decode(
        &self,
        cursor: &HostEventCursor,
        authority_identity: &str,
        view: &EventViewRequest,
    ) -> Result<u64, CursorAdmissionError> {
        self.decode_opaque("events", cursor.as_str(), authority_identity, view)
    }

    pub(crate) fn encode_page<P: Serialize, V: Serialize>(
        &self,
        kind: &str,
        position: &P,
        authority_identity: &str,
        view: &V,
    ) -> Result<String, String> {
        self.encode_opaque(kind, position, authority_identity, view)
    }

    pub(crate) fn decode_page<P: DeserializeOwned, V: Serialize>(
        &self,
        kind: &str,
        cursor: &str,
        authority_identity: &str,
        view: &V,
    ) -> Result<P, CursorAdmissionError> {
        self.decode_opaque(kind, cursor, authority_identity, view)
    }

    fn encode_opaque<P: Serialize, V: Serialize>(
        &self,
        kind: &str,
        position: &P,
        authority_identity: &str,
        view: &V,
    ) -> Result<String, String> {
        let claims = CursorClaims {
            version: CURSOR_VERSION,
            kind: kind.to_string(),
            position: serde_json::to_value(position)
                .map_err(|error| format!("failed to encode cursor position: {error}"))?,
            storage: self.storage_binding.clone(),
            authority: digest_binding("authority", authority_identity.as_bytes()),
            view: view_binding(view)?,
        };
        let payload = serde_json::to_vec(&claims)
            .map_err(|error| format!("failed to encode host cursor claims: {error}"))?;
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|_| "failed to initialize host cursor MAC".to_string())?;
        mac.update(&payload);
        let tag = mac.finalize().into_bytes();
        let mut frame = Vec::with_capacity(payload.len() + TAG_BYTES);
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(&tag);
        Ok(URL_SAFE_NO_PAD.encode(frame))
    }

    fn decode_opaque<P: DeserializeOwned, V: Serialize>(
        &self,
        kind: &str,
        cursor: &str,
        authority_identity: &str,
        view: &V,
    ) -> Result<P, CursorAdmissionError> {
        let frame = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| CursorAdmissionError::Malformed)?;
        if frame.len() <= TAG_BYTES {
            return Err(CursorAdmissionError::Malformed);
        }
        let (payload, tag) = frame.split_at(frame.len() - TAG_BYTES);
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|_| CursorAdmissionError::IntegrityFailed)?;
        mac.update(payload);
        mac.verify_slice(tag)
            .map_err(|_| CursorAdmissionError::IntegrityFailed)?;
        let claims = serde_json::from_slice::<CursorClaims>(payload)
            .map_err(|_| CursorAdmissionError::Malformed)?;
        if claims.version != CURSOR_VERSION || claims.kind != kind {
            return Err(CursorAdmissionError::Malformed);
        }
        if claims.storage != self.storage_binding {
            return Err(CursorAdmissionError::StorageMismatch);
        }
        if claims.authority != digest_binding("authority", authority_identity.as_bytes()) {
            return Err(CursorAdmissionError::ScopeMismatch);
        }
        let expected_view = view_binding(view).map_err(|_| CursorAdmissionError::Malformed)?;
        if claims.view != expected_view {
            return Err(CursorAdmissionError::ViewMismatch);
        }
        serde_json::from_value(claims.position).map_err(|_| CursorAdmissionError::Malformed)
    }
}

fn load_or_create_key(path: &Path) -> io::Result<[u8; TAG_BYTES]> {
    match OpenOptions::new().read(true).open(path) {
        Ok(mut file) => {
            let mut key = [0_u8; TAG_BYTES];
            file.read_exact(&mut key)?;
            let mut trailing = [0_u8; 1];
            if file.read(&mut trailing)? != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "host cursor key must contain exactly 32 bytes",
                ));
            }
            Ok(key)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut key = [0_u8; TAG_BYTES];
            key[..16].copy_from_slice(Uuid::new_v4().as_bytes());
            key[16..].copy_from_slice(Uuid::new_v4().as_bytes());
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            file.write_all(&key)?;
            file.sync_all()?;
            Ok(key)
        }
        Err(error) => Err(error),
    }
}

fn view_binding(view: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(view)
        .map_err(|error| format!("failed to encode host event view: {error}"))?;
    Ok(digest_binding("view", &bytes))
}

fn digest_binding(domain: &str, value: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"starweaver.host.cursor.v1\0");
    digest.update(domain.len().to_be_bytes());
    digest.update(domain.as_bytes());
    digest.update(value.len().to_be_bytes());
    digest.update(value);
    format!("sha256:{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::needless_pass_by_value,
        clippy::unwrap_used
    )]

    use serde_json::json;
    use starweaver_rpc_core::generated::EventViewRequest;

    use super::*;

    fn view(scope: serde_json::Value) -> EventViewRequest {
        serde_json::from_value(json!({
            "scope": scope,
            "profile": "conversation.v1",
            "optionalFeatures": []
        }))
        .expect("valid view")
    }

    #[test]
    fn cursor_is_opaque_and_bound_to_storage_authority_and_exact_view() {
        let codec = HostCursorCodec::new([7; TAG_BYTES], "database-a");
        let requested = view(json!({"kind": "session", "sessionId": "session-1"}));
        let cursor = codec
            .encode(42, "principal-a:read", &requested)
            .expect("cursor");
        assert_eq!(
            codec.decode(&cursor, "principal-a:read", &requested),
            Ok(42)
        );

        let other_storage = HostCursorCodec::new([7; TAG_BYTES], "database-b");
        assert_eq!(
            other_storage.decode(&cursor, "principal-a:read", &requested),
            Err(CursorAdmissionError::StorageMismatch)
        );
        assert_eq!(
            codec.decode(&cursor, "principal-b:read", &requested),
            Err(CursorAdmissionError::ScopeMismatch)
        );
        let other_view = view(json!({"kind": "session", "sessionId": "session-2"}));
        assert_eq!(
            codec.decode(&cursor, "principal-a:read", &other_view),
            Err(CursorAdmissionError::ViewMismatch)
        );
    }

    #[test]
    fn persistent_key_survives_reopen_and_is_storage_bound() {
        let temp = tempfile::tempdir().expect("temporary state directory");
        let first = HostCursorCodec::load_or_create(temp.path(), "database-a").expect("first");
        let reopened =
            HostCursorCodec::load_or_create(temp.path(), "database-a").expect("reopened");
        let requested = view(json!({"kind": "global"}));
        let cursor = first.encode(7, "local", &requested).expect("cursor");
        assert_eq!(reopened.decode(&cursor, "local", &requested), Ok(7));
        assert_eq!(
            std::fs::read(temp.path().join(KEY_FILE_NAME))
                .expect("key")
                .len(),
            TAG_BYTES
        );
    }

    #[test]
    fn page_cursor_binds_kind_authority_storage_and_filter_view() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Position {
            updated_at: String,
            id: String,
        }

        let codec = HostCursorCodec::new([8; TAG_BYTES], "database-a");
        let position = Position {
            updated_at: "2026-07-21T00:00:00Z".to_string(),
            id: "session-a".to_string(),
        };
        let view = json!({"status": "active"});
        let cursor = codec
            .encode_page("session.list", &position, "principal-a:read", &view)
            .expect("page cursor");
        assert_eq!(
            codec.decode_page::<Position, _>("session.list", &cursor, "principal-a:read", &view,),
            Ok(position)
        );
        assert_eq!(
            codec.decode_page::<Position, _>("approval.list", &cursor, "principal-a:read", &view,),
            Err(CursorAdmissionError::Malformed)
        );
        assert_eq!(
            codec.decode_page::<Position, _>("session.list", &cursor, "principal-b:read", &view,),
            Err(CursorAdmissionError::ScopeMismatch)
        );
        assert_eq!(
            codec.decode_page::<Position, _>(
                "session.list",
                &cursor,
                "principal-a:read",
                &json!({"status": "archived"}),
            ),
            Err(CursorAdmissionError::ViewMismatch)
        );
    }

    #[test]
    fn cursor_rejects_tampering_and_malformed_payloads() {
        let codec = HostCursorCodec::new([9; TAG_BYTES], "database-a");
        let requested = view(json!({"kind": "global"}));
        let cursor = codec.encode(1, "local", &requested).expect("cursor");
        let mut encoded = cursor.into_string().into_bytes();
        let last = encoded.last_mut().expect("non-empty cursor");
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = HostEventCursor::new(String::from_utf8(encoded).expect("ASCII"))
            .expect("syntactically valid cursor");
        assert_eq!(
            codec.decode(&tampered, "local", &requested),
            Err(CursorAdmissionError::IntegrityFailed)
        );
        let malformed = HostEventCursor::new("AA").expect("base64url syntax");
        assert_eq!(
            codec.decode(&malformed, "local", &requested),
            Err(CursorAdmissionError::Malformed)
        );
    }
}
