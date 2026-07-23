use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{
    Deserialize as _,
    de::{Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

pub fn load_yaml(path: &Path) -> Result<Value, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    reject_yaml_features(path, &text)?;
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let mut deserializer = serde_json::Deserializer::from_str(&text);
        let value = StrictJsonValue::deserialize(&mut deserializer)
            .map_err(|error| format!("parse {}: {error}", path.display()))?
            .0;
        deserializer
            .end()
            .map_err(|error| format!("parse {}: {error}", path.display()))?;
        return Ok(value);
    }
    let mut documents = yaml_serde::Deserializer::from_str(&text);
    let document = documents
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?;
    let value = Value::deserialize(document)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if documents.next().is_some() {
        return Err(format!(
            "{} contains more than one YAML document",
            path.display()
        ));
    }
    reject_non_string_keys(&value)?;
    Ok(value)
}

pub fn resolve_local_path(
    protocol_root: &Path,
    source: &Path,
    reference: &str,
) -> Result<(PathBuf, String), String> {
    let (file, fragment) = reference
        .split_once('#')
        .ok_or_else(|| format!("reference has no fragment: {reference}"))?;
    if file.is_empty() || file.contains('\\') || file.starts_with('/') || file.contains("://") {
        return Err(format!(
            "reference is not a repository-local relative path: {reference}"
        ));
    }
    let base = source
        .parent()
        .ok_or_else(|| format!("{} has no parent", source.display()))?;
    let candidate = normalize(&base.join(file))?;
    let root = normalize(protocol_root)?;
    if !candidate.starts_with(&root) {
        return Err(format!("reference escapes protocol root: {reference}"));
    }
    Ok((candidate, fragment.to_string()))
}

fn normalize(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!("path escapes root: {}", path.display()));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn reject_yaml_features(path: &Path, text: &str) -> Result<(), String> {
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("---") || trimmed.starts_with("...") || trimmed.starts_with("<<:") {
            return Err(format!(
                "{}:{} uses a forbidden YAML document or merge construct",
                path.display(),
                index + 1
            ));
        }
        if trimmed.starts_with('&')
            || trimmed.starts_with('*')
            || trimmed.contains(" <<:")
            || trimmed.contains(": &")
            || trimmed.contains(": *")
            || trimmed.starts_with("- &")
            || trimmed.starts_with("- *")
        {
            return Err(format!(
                "{}:{} uses a forbidden YAML anchor or alias",
                path.display(),
                index + 1
            ));
        }
    }
    Ok(())
}

struct StrictJsonValue(Value);

impl<'de> serde::Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(StrictJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value::<StrictJsonValue>()?.0;
            if values.insert(key.clone(), value).is_some() {
                return Err(A::Error::custom(format!("duplicate object key {key}")));
            }
        }
        Ok(StrictJsonValue(Value::Object(values)))
    }
}

fn reject_non_string_keys(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            for value in object.values() {
                reject_non_string_keys(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_non_string_keys(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
