use std::fmt::Write as _;

use super::{pascal, snake};
use crate::rpc_idl::model::{ObjectSchema, ProtocolIr, SchemaKind, StringSchema};

pub fn render(ir: &ProtocolIr) -> Result<String, String> {
    let mut out = String::from(
        "//! Generated closed wire types.\n\nuse serde::{Deserialize, Deserializer, Serialize};\nuse serde_json::Value;\nuse std::{fmt, str::FromStr};\n\n",
    );
    out.push_str(SUPPORT);
    for schema in ir.schemas.values() {
        match &schema.kind {
            SchemaKind::String(value) => named_string(&mut out, &schema.name, value)?,
            SchemaKind::Object(value) => object(&mut out, ir, &schema.name, value)?,
            SchemaKind::JsonObject => writeln!(
                out,
                "/// Explicit arbitrary JSON object `{}`.\npub type {} = Value;\n",
                schema.name, schema.name
            )
            .map_err(|error| error.to_string())?,
            SchemaKind::OneOf {
                variants,
                nullable: false,
                ..
            } => union(&mut out, &schema.name, variants)?,
            other => writeln!(
                out,
                "/// Generated schema `{}`.\npub type {} = {};\n",
                schema.name,
                schema.name,
                rust_type(other)?
            )
            .map_err(|error| error.to_string())?,
        }
    }
    Ok(out)
}

const SUPPORT: &str = r#"fn deserialize_json_object<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.is_object() { Ok(value) } else { Err(serde::de::Error::custom("expected JSON object")) }
}

fn validate_string(value: &str, minimum: usize, maximum: usize, kind: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length < minimum || length > maximum { return Err(format!("{kind} length must be in {minimum}..={maximum}")); }
    if minimum > 0 && value.trim().is_empty() { return Err(format!("{kind} must not be blank")); }
    match kind {
        "SchemaDigest" if value.len() != 71 || !value.starts_with("sha256:") || !value[7..].bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) => Err("SchemaDigest must be canonical sha256 hex".to_string()),
        "HostEventCursor" if !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') => Err("HostEventCursor must be unpadded base64url".to_string()),
        "Timestamp" if !value.ends_with('Z') => Err("Timestamp must be RFC 3339 UTC".to_string()),
        _ => Ok(()),
    }
}

"#;

fn named_string(out: &mut String, name: &str, schema: &StringSchema) -> Result<(), String> {
    if !schema.enum_values.is_empty() {
        writeln!(out, "/// Generated string enum `{name}`.\n#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]\npub enum {name} {{").map_err(|error| error.to_string())?;
        for value in &schema.enum_values {
            writeln!(
                out,
                "    #[serde(rename = {:?})]\n    {},",
                value,
                pascal(value)
            )
            .map_err(|error| error.to_string())?;
        }
        out.push_str("}\n\n");
    } else if schema.decimal.is_some() {
        writeln!(out, "/// Canonical decimal-string unsigned 64-bit value.\n#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]\npub struct {name}(u64);\nimpl {name} {{ #[must_use] pub const fn new(value: u64) -> Self {{ Self(value) }} #[must_use] pub const fn get(self) -> u64 {{ self.0 }} pub fn checked_increment(self) -> Option<Self> {{ self.0.checked_add(1).map(Self) }} }}\nimpl fmt::Display for {name} {{ fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{ self.0.fmt(f) }} }}\nimpl FromStr for {name} {{ type Err = String; fn from_str(value: &str) -> Result<Self, Self::Err> {{ if value.is_empty() || (value.len() > 1 && value.starts_with('0')) || !value.bytes().all(|byte| byte.is_ascii_digit()) {{ return Err(\"non-canonical decimal u64\".to_string()); }} value.parse::<u64>().map(Self).map_err(|error| error.to_string()) }} }}\nimpl Serialize for {name} {{ fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{ serializer.serialize_str(&self.to_string()) }} }}\nimpl<'de> Deserialize<'de> for {name} {{ fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{ String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom) }} }}\n").map_err(|error| error.to_string())?;
    } else if let Some(constant) = &schema.const_value {
        writeln!(out, "/// Constant string `{constant}`.\n#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]\npub enum {name} {{ #[serde(rename = {constant:?})] Value }}\n").map_err(|error| error.to_string())?;
    } else {
        writeln!(out, "/// Validated generated string `{name}`.\n#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]\n#[serde(transparent)]\npub struct {name}(String);\nimpl {name} {{ pub fn new(value: impl Into<String>) -> Result<Self, String> {{ let value = value.into(); validate_string(&value, {}, {}, {:?})?; Ok(Self(value)) }} #[must_use] pub fn as_str(&self) -> &str {{ &self.0 }} #[must_use] pub fn into_string(self) -> String {{ self.0 }} }}\nimpl<'de> Deserialize<'de> for {name} {{ fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{ Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom) }} }}\nimpl fmt::Display for {name} {{ fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{ self.0.fmt(f) }} }}\n", schema.min_length, schema.max_length, name).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn object(
    out: &mut String,
    ir: &ProtocolIr,
    name: &str,
    value: &ObjectSchema,
) -> Result<(), String> {
    for (field, schema) in &value.properties {
        if let SchemaKind::String(StringSchema {
            const_value: Some(constant),
            ..
        }) = schema
        {
            writeln!(out, "/// Discriminator `{constant}`.\n#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]\npub enum {name}{} {{ #[serde(rename = {:?})] Value }}\n", pascal(field), constant).map_err(|error| error.to_string())?;
        }
    }
    writeln!(out, "/// Generated closed object `{name}`.\n#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]\n#[serde(rename_all = \"camelCase\", deny_unknown_fields)]\npub struct {name} {{").map_err(|error| error.to_string())?;
    for (field, schema) in &value.properties {
        if matches!(schema, SchemaKind::Ref(target) if matches!(ir.schemas.get(target).map(|schema| &schema.kind), Some(SchemaKind::JsonObject)))
        {
            out.push_str("    #[serde(deserialize_with = \"deserialize_json_object\")]\n");
        }
        let mut ty = if matches!(
            schema,
            SchemaKind::String(StringSchema {
                const_value: Some(_),
                ..
            })
        ) {
            format!("{name}{}", pascal(field))
        } else {
            rust_type(schema)?
        };
        if !value.required.contains(field) && !ty.starts_with("Option<") {
            ty = format!("Option<{ty}>");
        }
        if !value.required.contains(field) {
            out.push_str("    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n");
        }
        writeln!(
            out,
            "    /// Wire field `{field}`.\n    pub {}: {ty},",
            snake(field)
        )
        .map_err(|error| error.to_string())?;
    }
    out.push_str("}\n\n");
    Ok(())
}

fn union(out: &mut String, name: &str, variants: &[SchemaKind]) -> Result<(), String> {
    writeln!(out, "/// Generated closed discriminated union `{name}`.\n#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]\n#[serde(untagged)]\npub enum {name} {{").map_err(|error| error.to_string())?;
    for variant in variants {
        let SchemaKind::Ref(target) = variant else {
            return Err(format!("union {name} variants must be refs"));
        };
        writeln!(out, "    /// `{target}`.\n    {target}({target}),")
            .map_err(|error| error.to_string())?;
    }
    out.push_str("}\n\n");
    Ok(())
}

fn rust_type(schema: &SchemaKind) -> Result<String, String> {
    match schema {
        SchemaKind::Ref(name) => Ok(name.clone()),
        SchemaKind::String(_) => Ok("String".to_string()),
        SchemaKind::Integer { minimum, maximum }
            if *minimum >= 0 && *maximum <= i64::from(u32::MAX) =>
        {
            Ok("u32".to_string())
        }
        SchemaKind::Integer { .. } => Ok("i64".to_string()),
        SchemaKind::Boolean => Ok("bool".to_string()),
        SchemaKind::Null => Ok("()".to_string()),
        SchemaKind::JsonObject => Ok("Value".to_string()),
        SchemaKind::Array { items, .. } => Ok(format!("Vec<{}>", rust_type(items)?)),
        SchemaKind::OneOf {
            variants,
            nullable: true,
            ..
        } => Ok(format!(
            "Option<{}>",
            rust_type(
                variants
                    .iter()
                    .find(|variant| !matches!(variant, SchemaKind::Null))
                    .ok_or("nullable union has no value")?
            )?
        )),
        SchemaKind::Object(_) | SchemaKind::OneOf { .. } => {
            Err("inline complex schemas require a name".to_string())
        }
    }
}
