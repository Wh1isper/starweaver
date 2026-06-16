//! Provider usage parsing helpers.

use serde_json::Value;

pub fn usage_from_openai(value: &Value) -> starweaver_usage::Usage {
    let usage = value.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_write_tokens = usage.map_or(0, usage_cache_write_tokens);
    let cache_read_tokens = usage.map_or(0, usage_cache_read_tokens);
    let output_tokens = usage
        .and_then(|u| {
            u.get("completion_tokens")
                .or_else(|| u.get("output_tokens"))
        })
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let computed_total = input_tokens.saturating_add(output_tokens);
    starweaver_usage::Usage {
        requests: 1,
        input_tokens,
        cache_write_tokens,
        cache_read_tokens,
        output_tokens,
        total_tokens: usage
            .and_then(|u| u.get("total_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(computed_total)
            .max(computed_total),
        tool_calls: 0,
    }
}

#[cfg(test)]
pub fn usage_from_named(value: &Value, input: &str, output: &str) -> starweaver_usage::Usage {
    usage_from_named_with_options(value, input, output, false, &[])
}

pub fn usage_from_named_including_cache_input(
    value: &Value,
    input: &str,
    output: &str,
) -> starweaver_usage::Usage {
    usage_from_named_with_options(value, input, output, true, &[])
}

pub fn usage_from_named_with_output_extras(
    value: &Value,
    input: &str,
    output: &str,
    output_extras: &[&str],
) -> starweaver_usage::Usage {
    usage_from_named_with_options(value, input, output, false, output_extras)
}

fn usage_from_named_with_options(
    value: &Value,
    input: &str,
    output: &str,
    add_cache_to_input: bool,
    output_extras: &[&str],
) -> starweaver_usage::Usage {
    let usage = value.get("usage").or_else(|| value.get("usageMetadata"));
    let input_base = usage
        .and_then(|u| u.get(input))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_write_tokens = usage.map_or(0, usage_cache_write_tokens);
    let cache_read_tokens = usage.map_or(0, usage_cache_read_tokens);
    let input_tokens = if add_cache_to_input {
        input_base
            .saturating_add(cache_write_tokens)
            .saturating_add(cache_read_tokens)
    } else {
        input_base
    };
    let output_tokens = usage
        .and_then(|u| u.get(output))
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .saturating_add(usage.map_or(0, |usage| usage_u64(usage, output_extras)));
    let computed_total = input_tokens.saturating_add(output_tokens);
    let total_tokens = usage
        .and_then(|u| u.get("totalTokens").or_else(|| u.get("total_tokens")))
        .and_then(Value::as_u64)
        .unwrap_or(computed_total)
        .max(computed_total);
    starweaver_usage::Usage {
        requests: 1,
        input_tokens,
        cache_write_tokens,
        cache_read_tokens,
        output_tokens,
        total_tokens,
        tool_calls: 0,
    }
}

fn usage_cache_write_tokens(usage: &Value) -> u64 {
    usage_u64(
        usage,
        &[
            "cache_write_tokens",
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cacheWriteInputTokens",
            "cache_write_input_tokens",
        ],
    )
}

fn usage_cache_read_tokens(usage: &Value) -> u64 {
    let direct = usage_u64(
        usage,
        &[
            "cache_read_tokens",
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cacheReadTokens",
            "cachedContentTokenCount",
            "cached_content_token_count",
        ],
    );
    if direct > 0 {
        return direct;
    }
    usage_nested_u64(
        usage,
        &[
            &["prompt_tokens_details", "cached_tokens"],
            &["input_tokens_details", "cached_tokens"],
            &["input_token_details", "cached_tokens"],
            &["inputTokenDetails", "cacheReadTokens"],
        ],
    )
}

fn usage_u64(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or_default()
}

fn usage_nested_u64(value: &Value, paths: &[&[&str]]) -> u64 {
    paths
        .iter()
        .find_map(|path| {
            path.iter()
                .try_fold(value, |current, key| current.get(*key))
                .and_then(Value::as_u64)
        })
        .unwrap_or_default()
}
