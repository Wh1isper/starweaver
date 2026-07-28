//! Shared compile-time trust root for Desktop and RPC runtime updates.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use minisign_verify::PublicKey;

pub fn embedded_public_key() -> Option<&'static str> {
    let configured = option_env!("STARWEAVER_UPDATE_PUBLIC_KEY")
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    parse_public_key(configured).ok()?;
    Some(configured)
}

pub fn parse_public_key(configured: &str) -> Result<PublicKey, ()> {
    let configured = configured.trim();
    if configured.starts_with("untrusted comment:") {
        return PublicKey::decode(configured).map_err(|_| ());
    }
    if let Ok(decoded) = BASE64.decode(configured)
        && let Ok(text) = std::str::from_utf8(&decoded)
    {
        return PublicKey::decode(text).map_err(|_| ());
    }
    PublicKey::from_base64(configured).map_err(|_| ())
}
