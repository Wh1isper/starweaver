//! Data URL parsing and base64 budget helpers.

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::ParsedDataUrl;

/// Parse a `data:<media-type>;base64,<payload>` URL.
///
/// # Errors
///
/// Returns an error when the data URL is unsupported or the payload is invalid base64.
pub fn parse_data_url(data_url: &str) -> Result<ParsedDataUrl, String> {
    let (prefix, payload) = data_url
        .split_once(',')
        .ok_or_else(|| "data URL is missing a comma separator".to_string())?;
    let media_type = prefix
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| "only base64 data URLs are supported".to_string())?;
    let data = STANDARD
        .decode(payload)
        .map_err(|error| format!("invalid base64 data URL payload: {error}"))?;
    Ok(ParsedDataUrl {
        media_type: media_type.to_string(),
        data,
    })
}

/// Compute base64 encoded length without line wrapping.
#[must_use]
pub const fn base64_encoded_len(raw_bytes: usize) -> usize {
    raw_bytes.div_ceil(3) * 4
}

/// Return the largest raw byte count that fits in a base64 encoded byte budget.
#[must_use]
pub const fn raw_budget_from_base64_limit(base64_limit: usize) -> usize {
    (base64_limit / 4) * 3
}
