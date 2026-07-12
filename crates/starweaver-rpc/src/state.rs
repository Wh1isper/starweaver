//! RPC-owned client selection state.

use std::path::Path;

use tokio::fs;
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use crate::{RpcHostError, RpcHostResult};

#[derive(Default, Deserialize, Serialize)]
struct RpcClientState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_session_id: Option<String>,
}

pub async fn read_current_session(state_dir: &Path) -> RpcHostResult<Option<String>> {
    let path = state_dir.join("state.json");
    match fs::read_to_string(&path).await {
        Ok(content) => serde_json::from_str::<RpcClientState>(&content)
            .map(|state| state.current_session_id)
            .map_err(|error| RpcHostError::Invalid(format!("invalid RPC state: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RpcHostError::Io(error)),
    }
}

pub async fn write_current_session(state_dir: &Path, session_id: &str) -> RpcHostResult<()> {
    fs::create_dir_all(state_dir).await?;
    let path = state_dir.join("state.json");
    let temporary = state_dir.join(format!("state.json.{}.tmp", Uuid::new_v4()));
    let payload = serde_json::to_vec_pretty(&RpcClientState {
        current_session_id: Some(session_id.to_string()),
    })
    .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
    fs::write(&temporary, payload).await?;
    if let Err(error) = fs::rename(&temporary, path).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(RpcHostError::Io(error));
    }
    Ok(())
}
