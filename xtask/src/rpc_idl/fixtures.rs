use std::{fs, path::Path};

use serde_json::Value;

use super::{bundle, model::ProtocolIr, validate};

pub fn check(repository: &Path) -> Result<(), String> {
    let protocol_root = repository.join("protocol/host");
    let bundled = bundle::build(&protocol_root)?;
    let ir = ProtocolIr::from_bundle(&bundled.value)?;
    let examples = protocol_root.join("examples");
    let mut valid_count = 0;
    for entry in fs::read_dir(&examples).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let value = read(&path)?;
        validate_request(&ir, &value, &path)?;
        valid_count += 1;
    }
    let invalid_params = protocol_root.join("fixtures/invalid/params");
    let mut invalid_count = 0;
    if invalid_params.is_dir() {
        for entry in fs::read_dir(&invalid_params).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let value = read(&path)?;
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{} has no method", path.display()))?;
            let params = value
                .get("params")
                .ok_or_else(|| format!("{} has no params", path.display()))?;
            let contract = ir
                .methods
                .get(method)
                .ok_or_else(|| format!("{} has unknown method", path.display()))?;
            if validate::value(
                &ir,
                &ir.schemas[&contract.params_type].kind,
                params,
                "params",
            )
            .is_ok()
            {
                return Err(format!("{} was expected to be invalid", path.display()));
            }
            invalid_count += 1;
        }
    }
    if valid_count == 0 || invalid_count == 0 {
        return Err("fixtures require canonical and invalid examples".to_string());
    }
    println!("validated {valid_count} canonical examples and {invalid_count} invalid fixtures");
    Ok(())
}

fn validate_request(ir: &ProtocolIr, value: &Value, path: &Path) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{} must be an object", path.display()))?;
    let expected = ["jsonrpc", "id", "method", "params"]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let actual = object
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if actual != expected
        || object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(format!(
            "{} has an invalid request envelope",
            path.display()
        ));
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{} has no method", path.display()))?;
    let contract = ir
        .methods
        .get(method)
        .ok_or_else(|| format!("{} uses unknown method {method}", path.display()))?;
    validate::value(
        ir,
        &ir.schemas[&contract.params_type].kind,
        &object["params"],
        "params",
    )?;
    if method == "initialize" {
        validate_initialize_identity(ir, &object["params"], path)?;
    }
    Ok(())
}

fn validate_initialize_identity(
    ir: &ProtocolIr,
    params: &Value,
    path: &Path,
) -> Result<(), String> {
    let protocol = params
        .get("protocol")
        .ok_or_else(|| format!("{} initialize params have no protocol", path.display()))?;
    let expected = serde_json::json!({
        "name": ir.identity.name,
        "major": ir.identity.major,
        "revision": ir.identity.revision,
        "schemaDigest": ir.identity.schema_digest,
    });
    if protocol != &expected {
        return Err(format!(
            "{} initialize protocol identity does not match the bundled contract",
            path.display()
        ));
    }
    Ok(())
}

fn read(path: &Path) -> Result<Value, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn canonical_initialize_example_must_match_bundled_identity() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repository root");
        let protocol_root = repository.join("protocol/host");
        let bundled = bundle::build(&protocol_root).expect("bundle protocol");
        let ir = ProtocolIr::from_bundle(&bundled.value).expect("protocol IR");
        let path = protocol_root.join("examples/initialize.request.json");
        let mut request = read(&path).expect("initialize example");
        request["params"]["protocol"]["schemaDigest"] = Value::String(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        );

        let error = validate_request(&ir, &request, &path).expect_err("stale digest must fail");
        assert!(error.contains("does not match the bundled contract"));
    }
}
