//! JWT metadata extraction helpers.

use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use serde_json::Value;

use crate::{
    error::{OAuthError, OAuthResult},
    types::OAuthAccount,
};

/// Decode a JWT payload without signature validation for local metadata extraction.
pub fn decode_jwt_payload(jwt: &str) -> OAuthResult<Value> {
    let parts = jwt.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts[1].is_empty() {
        return Err(OAuthError::InvalidJwt("invalid JWT format".to_string()));
    }
    let mut payload = parts[1].to_string();
    payload.push_str(&"=".repeat((4 - payload.len() % 4) % 4));
    let decoded = URL_SAFE
        .decode(payload.as_bytes())
        .map_err(|error| OAuthError::InvalidJwt(error.to_string()))?;
    let value = serde_json::from_slice::<Value>(&decoded)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(OAuthError::InvalidJwt(
            "JWT payload is not an object".to_string(),
        ))
    }
}

/// Extract Codex-compatible `ChatGPT` account metadata from an ID token.
pub fn account_from_id_token(id_token: &str) -> OAuthResult<OAuthAccount> {
    let claims = decode_jwt_payload(id_token)?;
    let profile_data = claims
        .get("https://api.openai.com/profile")
        .and_then(Value::as_object);
    let auth_data = claims
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);
    Ok(OAuthAccount {
        email: string_claim_value(&claims, "email")
            .or_else(|| string_claim_map(profile_data, "email")),
        chatgpt_user_id: string_claim_map(auth_data, "chatgpt_user_id")
            .or_else(|| string_claim_map(auth_data, "user_id")),
        chatgpt_account_id: string_claim_map(auth_data, "chatgpt_account_id"),
        chatgpt_plan_type: plan_type_claim(
            auth_data.and_then(|object| object.get("chatgpt_plan_type")),
        ),
        chatgpt_account_is_fedramp: auth_data
            .and_then(|object| object.get("chatgpt_account_is_fedramp"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn string_claim_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn string_claim_map(value: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    value
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn plan_type_claim(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::Object(object)) => ["raw_value", "value", "name"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(ToString::to_string),
        _ => None,
    }
}

pub fn validate_same_account(old: &OAuthAccount, new: &OAuthAccount) -> OAuthResult<()> {
    if old
        .chatgpt_account_id
        .as_ref()
        .zip(new.chatgpt_account_id.as_ref())
        .is_some_and(|(old, new)| old != new)
    {
        return Err(OAuthError::AccountMismatch);
    }
    if old
        .chatgpt_user_id
        .as_ref()
        .zip(new.chatgpt_user_id.as_ref())
        .is_some_and(|(old, new)| old != new)
    {
        return Err(OAuthError::AccountMismatch);
    }
    Ok(())
}
