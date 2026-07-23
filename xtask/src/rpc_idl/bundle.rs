use std::{collections::BTreeMap, path::Path};

use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::common::sort_value;

use super::source::{load_yaml, resolve_local_path};

pub struct BundledProtocol {
    pub value: Value,
    pub bytes: Vec<u8>,
    pub digest: String,
    pub sources: BTreeMap<String, String>,
}

pub fn build(protocol_root: &Path) -> Result<BundledProtocol, String> {
    let source_path = protocol_root.join("openrpc.yaml");
    let mut root = load_yaml(&source_path)?;
    let mut sources = BTreeMap::new();
    sources.insert(
        relative(protocol_root, &source_path)?,
        file_hash(&source_path)?,
    );
    for family in ["schemas", "errors"] {
        materialize_components(protocol_root, &source_path, &mut root, family, &mut sources)?;
    }
    rewrite_external_refs(protocol_root, &source_path, &mut root, &mut sources)?;
    reject_external_refs(&root)?;
    validate_component_refs(&root)?;
    validate_vendored_schemas(protocol_root, &root)?;
    let digest = protocol_digest(&root)?;
    root.pointer_mut("/x-starweaver-protocol/schemaDigest")
        .ok_or("protocol schemaDigest member is missing")?
        .clone_from(&Value::String(digest.clone()));
    let sorted = sort_value(&root);
    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&sorted).map_err(|error| error.to_string())?
    )
    .into_bytes();
    Ok(BundledProtocol {
        value: sorted,
        bytes,
        digest,
        sources,
    })
}

fn materialize_components(
    protocol_root: &Path,
    source_path: &Path,
    root: &mut Value,
    family: &str,
    sources: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let entries = root
        .pointer(&format!("/components/{family}"))
        .and_then(Value::as_object)
        .ok_or_else(|| format!("components.{family} must be an object"))?
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    for (name, placeholder) in entries {
        let reference = placeholder
            .get("$ref")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("components.{family}.{name} must be an external ref"))?;
        let (path, fragment) = resolve_local_path(protocol_root, source_path, reference)?;
        let document = load_yaml(&path)?;
        sources
            .entry(relative(protocol_root, &path)?)
            .or_insert(file_hash(&path)?);
        let pointer = if fragment.is_empty() {
            "/"
        } else {
            fragment.as_str()
        };
        let target = document
            .pointer(pointer)
            .ok_or_else(|| format!("unresolved ref {reference}"))?
            .clone();
        root.pointer_mut(&format!("/components/{family}/{name}"))
            .ok_or_else(|| format!("missing components.{family}.{name}"))?
            .clone_from(&target);
    }
    Ok(())
}

fn rewrite_external_refs(
    protocol_root: &Path,
    source_path: &Path,
    value: &mut Value,
    sources: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object
                .get("$ref")
                .and_then(Value::as_str)
                .map(str::to_string)
                && !reference.starts_with('#')
            {
                let (path, fragment) = resolve_local_path(protocol_root, source_path, &reference)?;
                let document = load_yaml(&path)?;
                if document.pointer(&fragment).is_none() {
                    return Err(format!("unresolved ref {reference}"));
                }
                sources
                    .entry(relative(protocol_root, &path)?)
                    .or_insert(file_hash(&path)?);
                let local = fragment
                    .strip_prefix("/components/")
                    .ok_or_else(|| format!("external ref must target a component: {reference}"))?;
                object.insert(
                    "$ref".to_string(),
                    Value::String(format!("#/components/{local}")),
                );
            }
            for child in object.values_mut() {
                rewrite_external_refs(protocol_root, source_path, child, sources)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                rewrite_external_refs(protocol_root, source_path, child, sources)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reject_external_refs(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && !reference.starts_with("#/components/")
            {
                return Err(format!("bundle contains non-component ref {reference}"));
            }
            for child in object.values() {
                reject_external_refs(child)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                reject_external_refs(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_component_refs(root: &Value) -> Result<(), String> {
    fn visit(root: &Value, value: &Value) -> Result<(), String> {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    let pointer = reference
                        .strip_prefix('#')
                        .ok_or_else(|| format!("invalid ref {reference}"))?;
                    if root.pointer(pointer).is_none() {
                        return Err(format!("unresolved bundled ref {reference}"));
                    }
                }
                for child in object.values() {
                    visit(root, child)?;
                }
            }
            Value::Array(values) => {
                for child in values {
                    visit(root, child)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(root, root)
}

fn validate_vendored_schemas(protocol_root: &Path, document: &Value) -> Result<(), String> {
    let tooling = protocol_root
        .parent()
        .ok_or("protocol root has no parent")?
        .join("tooling");
    for name in [
        "openrpc-1.4.0-meta-schema.json",
        "starweaver-openrpc-profile.schema.json",
    ] {
        let path = tooling.join(name);
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("read vendored schema {}: {error}", path.display()))?;
        let mut schema: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse vendored schema {}: {error}", path.display()))?;
        // The draft is selected explicitly below. Removing the declaration keeps
        // validation fully offline even when an upstream schema uses an https
        // spelling for the draft-07 metaschema URI.
        schema
            .as_object_mut()
            .ok_or_else(|| format!("vendored schema {} is not an object", path.display()))?
            .remove("$schema");
        normalize_draft7_uri(&mut schema);
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .build(&schema)
            .map_err(|error| format!("compile vendored schema {}: {error}", path.display()))?;
        let mut validation_document = document.clone();
        if name == "openrpc-1.4.0-meta-schema.json" {
            strip_starweaver_extensions(&mut validation_document);
        }
        if let Err(error) = validator.validate(&validation_document) {
            return Err(format!("{name} rejected canonical OpenRPC source: {error}"));
        }
    }
    Ok(())
}

fn strip_starweaver_extensions(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| !key.starts_with("x-starweaver-"));
            for child in object.values_mut() {
                strip_starweaver_extensions(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_starweaver_extensions(child);
            }
        }
        _ => {}
    }
}

fn normalize_draft7_uri(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for child in object.values_mut() {
                normalize_draft7_uri(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                normalize_draft7_uri(child);
            }
        }
        Value::String(text)
            if text == "https://json-schema.org/draft-07/schema#"
                || text == "https://json-schema.org/draft-07/schema" =>
        {
            *text = "http://json-schema.org/draft-07/schema#".to_string();
        }
        _ => {}
    }
}

fn protocol_digest(root: &Value) -> Result<String, String> {
    let mut digest_input = root.clone();
    digest_input
        .pointer_mut("/x-starweaver-protocol")
        .and_then(Value::as_object_mut)
        .ok_or("protocol extension missing")?
        .remove("schemaDigest");
    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&sort_value(&digest_input))
            .map_err(|error| error.to_string())?
    );
    Ok(format!("sha256:{:x}", Sha256::digest(bytes.as_bytes())))
}

fn file_hash(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
fn relative(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map_err(|_| format!("{} is outside {}", path.display(), root.display()))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}
