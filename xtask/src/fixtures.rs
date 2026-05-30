use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{json, Map, Value};

use crate::common::{read_json, read_json_object, sort_value, sorted_files, write_json};

const PROVIDERS: [&str; 5] = [
    "openai_chat",
    "openai_responses",
    "anthropic",
    "gemini",
    "bedrock",
];
const FIXTURE_KEYS: [&str; 10] = [
    "model",
    "history",
    "settings",
    "request_parameters",
    "tools",
    "native_tools",
    "expected_provider_request",
    "provider_response",
    "expected_response",
    "expected_error",
];

#[derive(Debug)]
struct HttpRequest {
    url: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

pub fn summarize_model_fixtures(args: &[String]) -> Result<(), String> {
    let mut fixtures_root = PathBuf::from("crates/starweaver-model/tests/fixtures");
    let mut output_path = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--fixtures-root" if index + 1 < args.len() => {
                fixtures_root = PathBuf::from(&args[index + 1]);
                index += 2;
            }
            "--output" if index + 1 < args.len() => {
                output_path = Some(PathBuf::from(&args[index + 1]));
                index += 2;
            }
            _ => {
                return Err(
                    "usage: summarize-model-fixtures [--fixtures-root PATH] [--output PATH]"
                        .to_string(),
                )
            }
        }
    }
    let mut providers = Map::new();
    let mut total = 0_u64;
    let mut total_kinds: BTreeMap<String, u64> = BTreeMap::new();
    for provider in PROVIDERS {
        let provider_dir = fixtures_root.join(provider);
        let mut fixtures = Vec::new();
        let mut kind_counts: BTreeMap<String, u64> = BTreeMap::new();
        if provider_dir.exists() {
            for path in sorted_files(&provider_dir, "json")? {
                let value = read_json(&path)?;
                let kind = fixture_kind(&value).to_string();
                *kind_counts.entry(kind.clone()).or_default() += 1;
                *total_kinds.entry(kind).or_default() += 1;
                fixtures.push(
                    path.file_stem()
                        .and_then(OsStr::to_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        total += fixtures.len() as u64;
        providers.insert(
            provider.to_string(),
            json!({"count": fixtures.len(), "kinds": kind_counts, "fixtures": fixtures}),
        );
    }
    let summary = json!({"providers": providers, "total": total, "kinds": total_kinds});
    let text = serde_json::to_string_pretty(&sort_value(&summary))
        .map_err(|error| error.to_string())?
        + "\n";
    if let Some(path) = output_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, text).map_err(|error| error.to_string())?;
    } else {
        print!("{text}");
    }
    Ok(())
}

fn fixture_kind(value: &Value) -> &'static str {
    if value.get("expected_error").is_some() {
        "error"
    } else if value.get("provider_response").is_some() {
        "replay"
    } else {
        "request"
    }
}

pub fn scrub_model_cassette(args: &[String]) -> Result<(), String> {
    let input = args
        .first()
        .ok_or_else(|| "usage: scrub-model-cassette CASSETTE [--output PATH]".to_string())?;
    let mut output_path = None;
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--output" && index + 1 < args.len() {
            output_path = Some(PathBuf::from(&args[index + 1]));
            index += 2;
        } else {
            return Err("usage: scrub-model-cassette CASSETTE [--output PATH]".to_string());
        }
    }
    let value = read_json(Path::new(input))?;
    let scrubbed = scrub(&value, None);
    let text = serde_json::to_string_pretty(&sort_value(&scrubbed))
        .map_err(|error| error.to_string())?
        + "\n";
    if let Some(path) = output_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, text).map_err(|error| error.to_string())?;
    } else {
        print!("{text}");
    }
    Ok(())
}

fn scrub(value: &Value, key: Option<&str>) -> Value {
    let key_lower = key.unwrap_or_default().to_ascii_lowercase();
    if matches!(
        key_lower.as_str(),
        "api_key"
            | "apikey"
            | "authorization"
            | "cookie"
            | "password"
            | "secret"
            | "token"
            | "x-api-key"
    ) {
        return Value::String("REDACTED".to_string());
    }
    if matches!(
        key_lower.as_str(),
        "created"
            | "created_at"
            | "date"
            | "expires_at"
            | "request-id"
            | "request_id"
            | "server"
            | "set-cookie"
            | "timestamp"
            | "x-request-id"
    ) {
        return Value::String("NORMALIZED".to_string());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), scrub(v, Some(k))))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|item| scrub(item, None)).collect()),
        Value::String(text) => Value::String(scrub_string(text)),
        other => other.clone(),
    }
}

fn scrub_string(value: &str) -> String {
    let mut output = value.to_string();
    for (prefix, replacement) in [
        ("chatcmpl-", "chatcmpl_REDACTED"),
        ("resp_", "resp_REDACTED"),
        ("msg_", "msg_REDACTED"),
        ("call_", "call_REDACTED"),
    ] {
        output = scrub_prefixed_id(&output, prefix, replacement);
    }
    scrub_uuids(&output)
}

fn scrub_prefixed_id(input: &str, prefix: &str, replacement: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    while let Some(pos) = rest.find(prefix) {
        output.push_str(&rest[..pos]);
        let after_prefix = &rest[pos + prefix.len()..];
        let id_len = after_prefix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
            .map(char::len_utf8)
            .sum::<usize>();
        output.push_str(replacement);
        rest = &after_prefix[id_len..];
    }
    output.push_str(rest);
    output
}

fn scrub_uuids(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::new();
    let mut index = 0;
    while index < input.len() {
        if index + 36 <= input.len() && is_uuid(&bytes[index..index + 36]) {
            output.push_str("uuid_REDACTED");
            index += 36;
        } else {
            let ch = input[index..].chars().next().unwrap_or_default();
            output.push(ch);
            index += ch.len_utf8();
        }
    }
    output
}

fn is_uuid(bytes: &[u8]) -> bool {
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

pub fn import_model_cassettes(args: &[String]) -> Result<(), String> {
    let input = args.first().ok_or_else(|| {
        "usage: import-model-cassettes CASSETTE [--fixtures-root PATH] [--dry-run]".to_string()
    })?;
    let mut fixtures_root = PathBuf::from("crates/starweaver-model/tests/fixtures");
    let mut dry_run = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--fixtures-root" if index + 1 < args.len() => {
                fixtures_root = PathBuf::from(&args[index + 1]);
                index += 2;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            _ => {
                return Err(
                    "usage: import-model-cassettes CASSETTE [--fixtures-root PATH] [--dry-run]"
                        .to_string(),
                )
            }
        }
    }
    let data = read_json(Path::new(input))?;
    let entries = match data {
        Value::Object(_) => vec![data],
        Value::Array(items) => items,
        _ => return Err("cassette root must be an object or array".to_string()),
    };
    for (entry_index, entry) in entries.iter().enumerate() {
        let map = entry
            .as_object()
            .ok_or_else(|| format!("cassette entry {entry_index} must be an object"))?;
        validate_import_entry(map)?;
        let provider = map["provider"].as_str().unwrap_or_default();
        let name = map["name"].as_str().unwrap_or_default();
        let target = fixtures_root.join(provider).join(format!("{name}.json"));
        if !dry_run {
            write_json(&target, &fixture_body(map), false)?;
        }
        println!("{}", target.display());
    }
    Ok(())
}

fn validate_import_entry(map: &Map<String, Value>) -> Result<(), String> {
    let required = [
        "provider",
        "name",
        "model",
        "history",
        "expected_provider_request",
    ];
    let optional = [
        "settings",
        "request_parameters",
        "tools",
        "native_tools",
        "provider_response",
        "expected_response",
        "expected_error",
    ];
    for key in required {
        if !map.contains_key(key) {
            return Err(format!("missing required fields: {key}"));
        }
    }
    for key in map.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("unknown fields: {key}"));
        }
    }
    let provider = map["provider"].as_str().unwrap_or_default();
    if !PROVIDERS.contains(&provider) {
        return Err(format!("unknown provider: {provider}"));
    }
    let name = map["name"].as_str().unwrap_or_default();
    if !valid_fixture_name(name) {
        return Err(format!("invalid fixture name: {name:?}"));
    }
    if map.contains_key("expected_response") == map.contains_key("expected_error") {
        return Err(
            "entry must include exactly one of expected_response or expected_error".to_string(),
        );
    }
    if !map.contains_key("provider_response") {
        return Err("entry must include provider_response for replay/error fixtures".to_string());
    }
    Ok(())
}

fn fixture_body(map: &Map<String, Value>) -> Value {
    let mut body = Map::new();
    for key in FIXTURE_KEYS {
        if let Some(value) = map.get(key) {
            body.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(body)
}

fn valid_fixture_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

pub fn record_model_cassette(args: &[String]) -> Result<(), String> {
    let request = args.first().ok_or_else(record_usage)?;
    let mut provider = None;
    let mut name = None;
    let mut fixtures_root = PathBuf::from("crates/starweaver-model/tests/fixtures");
    let mut output = None;
    let mut import_fixture = false;
    let mut mock_response = None;
    let mut endpoint_url = None;
    let mut headers = Vec::new();
    let mut timeout_seconds = 120_u64;
    let mut dry_run = false;
    let mut fail_on_http_error = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--provider" if index + 1 < args.len() => {
                provider = Some(args[index + 1].clone());
                index += 2;
            }
            "--name" if index + 1 < args.len() => {
                name = Some(args[index + 1].clone());
                index += 2;
            }
            "--fixtures-root" if index + 1 < args.len() => {
                fixtures_root = PathBuf::from(&args[index + 1]);
                index += 2;
            }
            "--output" if index + 1 < args.len() => {
                output = Some(PathBuf::from(&args[index + 1]));
                index += 2;
            }
            "--import-fixture" => {
                import_fixture = true;
                index += 1;
            }
            "--mock-response" if index + 1 < args.len() => {
                mock_response = Some(PathBuf::from(&args[index + 1]));
                index += 2;
            }
            "--endpoint-url" if index + 1 < args.len() => {
                endpoint_url = Some(args[index + 1].clone());
                index += 2;
            }
            "--header" if index + 1 < args.len() => {
                headers.push(args[index + 1].clone());
                index += 2;
            }
            "--timeout" if index + 1 < args.len() => {
                timeout_seconds = args[index + 1]
                    .parse::<u64>()
                    .map_err(|error| format!("invalid timeout: {error}"))?;
                index += 2;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            "--fail-on-http-error" => {
                fail_on_http_error = true;
                index += 1;
            }
            _ => return Err(record_usage()),
        }
    }
    let data = read_json_object(Path::new(request))?;
    validate_request_data(&data, import_fixture)?;
    let (provider, name) =
        infer_provider_and_name(Path::new(request), &fixtures_root, &data, provider, name)?;
    let http_request = resolve_provider_request(
        &provider,
        &data,
        endpoint_url,
        &headers,
        !(dry_run || mock_response.is_some()),
    )?;
    if dry_run {
        let dry = json!({
            "provider": provider,
            "name": name,
            "url": scrub_string(&http_request.url),
            "headers": redacted_headers(&http_request.headers),
            "body": http_request.body,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&sort_value(&dry)).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let (status, provider_response) = if let Some(path) = mock_response {
        (200_u16, read_json(&path)?)
    } else {
        send_http_request(&http_request, timeout_seconds)?
    };
    if fail_on_http_error && !(200..300).contains(&status) {
        return Err(format!("provider returned HTTP {status}"));
    }
    let cassette = build_recorded_cassette(&data, &provider, &name, &provider_response)?;
    if let Some(path) = output {
        write_json(&path, &cassette, false)?;
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&cassette).map_err(|error| error.to_string())?
        );
    }
    if import_fixture {
        let target = fixtures_root.join(&provider).join(format!("{name}.json"));
        let body = fixture_body(
            cassette
                .as_object()
                .ok_or_else(|| "cassette root changed".to_string())?,
        );
        write_json(&target, &body, false)?;
        eprintln!("{}", target.display());
    }
    Ok(())
}

fn record_usage() -> String {
    "usage: record-model-cassette REQUEST [--provider PROVIDER] [--name NAME] [--fixtures-root PATH] [--output PATH] [--import-fixture] [--mock-response PATH] [--endpoint-url URL] [--header 'NAME: VALUE'] [--timeout SECONDS] [--dry-run] [--fail-on-http-error]".to_string()
}

fn validate_request_data(data: &Map<String, Value>, import_fixture: bool) -> Result<(), String> {
    for key in ["model", "history", "expected_provider_request"] {
        if !data.contains_key(key) {
            return Err(format!("request is missing required fields: {key}"));
        }
    }
    if !data["model"].is_string() {
        return Err("request must include string model".to_string());
    }
    if !data["history"].is_array() {
        return Err("request must include array history".to_string());
    }
    if !data["expected_provider_request"].is_object() {
        return Err("request must include object expected_provider_request".to_string());
    }
    if import_fixture
        && (data.contains_key("expected_response") == data.contains_key("expected_error"))
    {
        return Err(
            "--import-fixture requires exactly one of expected_response or expected_error"
                .to_string(),
        );
    }
    Ok(())
}

fn infer_provider_and_name(
    request: &Path,
    fixtures_root: &Path,
    data: &Map<String, Value>,
    provider_override: Option<String>,
    name_override: Option<String>,
) -> Result<(String, String), String> {
    let mut provider = provider_override.or_else(|| {
        data.get("provider")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    });
    let mut name = name_override.or_else(|| {
        data.get("name")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    });
    if let Ok(relative) = request.canonicalize().and_then(|path| {
        fixtures_root.canonicalize().and_then(|root| {
            path.strip_prefix(root)
                .map(Path::to_path_buf)
                .map_err(std::io::Error::other)
        })
    }) {
        let parts: Vec<_> = relative.components().collect();
        if parts.len() >= 2 {
            provider.get_or_insert_with(|| parts[0].as_os_str().to_string_lossy().to_string());
            name.get_or_insert_with(|| {
                relative
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default()
                    .to_string()
            });
        }
    }
    let provider = provider.ok_or_else(|| "provider is required".to_string())?;
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(format!(
            "provider is required and must be one of: {}",
            PROVIDERS.join(", ")
        ));
    }
    let name = name.ok_or_else(|| "fixture name is required".to_string())?;
    if !valid_fixture_name(&name) {
        return Err("fixture name is required and must match ^[a-z0-9][a-z0-9_-]*$".to_string());
    }
    Ok((provider, name))
}

fn resolve_provider_request(
    provider: &str,
    data: &Map<String, Value>,
    endpoint_url: Option<String>,
    raw_headers: &[String],
    require_credentials: bool,
) -> Result<HttpRequest, String> {
    let model = data["model"].as_str().unwrap_or_default();
    let mut endpoint = endpoint_url;
    let mut headers =
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
    match provider {
        "openai_chat" => {
            endpoint.get_or_insert_with(|| {
                endpoint_from_base(
                    &env::var("OPENAI_BASE_URL")
                        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
                    "chat/completions",
                )
            });
            add_optional_header(
                &mut headers,
                "authorization",
                bearer("OPENAI_API_KEY", require_credentials)?,
            );
        }
        "openai_responses" => {
            endpoint.get_or_insert_with(|| {
                endpoint_from_base(
                    &env::var("OPENAI_BASE_URL")
                        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
                    "responses",
                )
            });
            add_optional_header(
                &mut headers,
                "authorization",
                bearer("OPENAI_API_KEY", require_credentials)?,
            );
        }
        "anthropic" => {
            endpoint.get_or_insert_with(|| {
                endpoint_from_base(
                    &env::var("ANTHROPIC_BASE_URL")
                        .unwrap_or_else(|_| "https://api.anthropic.com/v1".to_string()),
                    "messages",
                )
            });
            add_optional_header(
                &mut headers,
                "x-api-key",
                env_token("ANTHROPIC_API_KEY", require_credentials)?,
            );
            headers.insert(
                "anthropic-version".to_string(),
                env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string()),
            );
        }
        "gemini" => {
            let key = env_token("GEMINI_API_KEY", require_credentials)?
                .unwrap_or_else(|| "API_KEY".to_string());
            endpoint.get_or_insert_with(|| {
                endpoint_from_base(
                    &env::var("GEMINI_BASE_URL").unwrap_or_else(|_| {
                        "https://generativelanguage.googleapis.com/v1beta".to_string()
                    }),
                    &format!("models/{model}:generateContent?key={key}"),
                )
            });
        }
        "bedrock" => {
            endpoint = endpoint.or_else(|| env::var("BEDROCK_CONVERSE_ENDPOINT_URL").ok());
            if endpoint.is_none() {
                return Err(
                    "bedrock recording requires --endpoint-url or BEDROCK_CONVERSE_ENDPOINT_URL"
                        .to_string(),
                );
            }
        }
        _ => return Err("unknown provider".to_string()),
    }
    for raw in raw_headers {
        let Some((name, value)) = raw.split_once(':') else {
            return Err(format!("invalid header {raw:?}; expected NAME: VALUE"));
        };
        headers.insert(name.trim().to_string(), value.trim().to_string());
    }
    Ok(HttpRequest {
        url: endpoint.ok_or_else(|| "missing endpoint".to_string())?,
        headers,
        body: data["expected_provider_request"].clone(),
    })
}

fn endpoint_from_base(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn env_token(name: &str, required: bool) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(_) if required => Err(format!("environment variable {name} is required")),
        Err(_) => Ok(None),
    }
}

fn bearer(name: &str, required: bool) -> Result<Option<String>, String> {
    Ok(env_token(name, required)?.map(|token| format!("Bearer {token}")))
}

fn add_optional_header(headers: &mut BTreeMap<String, String>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        headers.insert(name.to_string(), value);
    }
}

fn redacted_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, Value> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                scrub(&Value::String(value.clone()), Some(name)),
            )
        })
        .collect()
}

fn send_http_request(request: &HttpRequest, timeout_seconds: u64) -> Result<(u16, Value), String> {
    let mut command = Command::new("curl");
    command
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--max-time")
        .arg(timeout_seconds.to_string())
        .arg("--request")
        .arg("POST")
        .arg("--write-out")
        .arg("\n%{http_code}");
    for (name, value) in &request.headers {
        command.arg("--header").arg(format!("{name}: {value}"));
    }
    let output = command
        .arg("--data")
        .arg(serde_json::to_string(&request.body).map_err(|error| error.to_string())?)
        .arg(&request.url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some((body, code)) = text.rsplit_once('\n') else {
        return Err("curl output did not include HTTP code".to_string());
    };
    let status = code
        .trim()
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    let value = serde_json::from_str(body).unwrap_or_else(|_| json!({"error": body}));
    Ok((status, value))
}

fn build_recorded_cassette(
    data: &Map<String, Value>,
    provider: &str,
    name: &str,
    provider_response: &Value,
) -> Result<Value, String> {
    let mut cassette = Map::new();
    cassette.insert("provider".to_string(), Value::String(provider.to_string()));
    cassette.insert("name".to_string(), Value::String(name.to_string()));
    for key in FIXTURE_KEYS {
        if key == "provider_response" {
            cassette.insert(key.to_string(), scrub(provider_response, None));
        } else if let Some(value) = data.get(key) {
            cassette.insert(key.to_string(), value.clone());
        }
    }
    for key in [
        "model",
        "history",
        "expected_provider_request",
        "provider_response",
    ] {
        if !cassette.contains_key(key) {
            return Err(format!("cassette is missing required field: {key}"));
        }
    }
    Ok(Value::Object(cassette))
}
