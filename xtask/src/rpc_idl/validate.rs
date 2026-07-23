use std::collections::BTreeSet;

use serde_json::Value;

use super::model::{ProtocolIr, SchemaKind, StringSchema};

pub fn value(
    ir: &ProtocolIr,
    schema: &SchemaKind,
    input: &Value,
    path: &str,
) -> Result<(), String> {
    match schema {
        SchemaKind::Ref(name) => value(
            ir,
            &ir.schemas
                .get(name)
                .ok_or_else(|| format!("unknown schema {name}"))?
                .kind,
            input,
            path,
        ),
        SchemaKind::String(schema) => validate_string(schema, input, path),
        SchemaKind::Integer { minimum, maximum } => {
            let actual = input
                .as_i64()
                .ok_or_else(|| format!("{path}: expected integer"))?;
            if actual < *minimum || actual > *maximum {
                Err(format!("{path}: integer outside {minimum}..={maximum}"))
            } else {
                Ok(())
            }
        }
        SchemaKind::Boolean if input.is_boolean() => Ok(()),
        SchemaKind::Boolean => Err(format!("{path}: expected boolean")),
        SchemaKind::Null if input.is_null() => Ok(()),
        SchemaKind::Null => Err(format!("{path}: expected null")),
        SchemaKind::JsonObject if input.is_object() => Ok(()),
        SchemaKind::JsonObject => Err(format!("{path}: expected object")),
        SchemaKind::Object(schema) => {
            let object = input
                .as_object()
                .ok_or_else(|| format!("{path}: expected object"))?;
            let actual = object.keys().cloned().collect::<BTreeSet<_>>();
            let declared = schema.properties.keys().cloned().collect::<BTreeSet<_>>();
            if !actual.is_subset(&declared) {
                return Err(format!(
                    "{path}: unknown fields {:?}",
                    actual.difference(&declared).collect::<Vec<_>>()
                ));
            }
            for required in &schema.required {
                if !object.contains_key(required) {
                    return Err(format!("{path}: missing {required}"));
                }
            }
            for (name, field) in &schema.properties {
                if let Some(input) = object.get(name) {
                    value(ir, field, input, &format!("{path}.{name}"))?;
                }
            }
            Ok(())
        }
        SchemaKind::Array {
            items,
            min_items,
            max_items,
            unique,
            canonical_utf8_byte_order,
        } => {
            let array = input
                .as_array()
                .ok_or_else(|| format!("{path}: expected array"))?;
            if array.len() < *min_items || array.len() > *max_items {
                return Err(format!(
                    "{path}: array length outside {min_items}..={max_items}"
                ));
            }
            if *unique {
                let distinct = array.iter().map(Value::to_string).collect::<BTreeSet<_>>();
                if distinct.len() != array.len() {
                    return Err(format!("{path}: array items must be unique"));
                }
            }
            if *canonical_utf8_byte_order
                && array.windows(2).any(|pair| {
                    let Some(left) = pair[0].as_str() else {
                        return true;
                    };
                    let Some(right) = pair[1].as_str() else {
                        return true;
                    };
                    left.as_bytes() >= right.as_bytes()
                })
            {
                return Err(format!(
                    "{path}: array items must be in strict ascending UTF-8 byte order"
                ));
            }
            for (index, input) in array.iter().enumerate() {
                value(ir, items, input, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        SchemaKind::OneOf { variants, .. } => {
            let accepted = variants
                .iter()
                .filter(|variant| value(ir, variant, input, path).is_ok())
                .count();
            if accepted == 1 {
                Ok(())
            } else {
                Err(format!(
                    "{path}: expected exactly one union variant, accepted {accepted}"
                ))
            }
        }
    }
}

fn validate_string(schema: &StringSchema, input: &Value, path: &str) -> Result<(), String> {
    let actual = input
        .as_str()
        .ok_or_else(|| format!("{path}: expected string"))?;
    let length = actual.chars().count();
    if length < schema.min_length || length > schema.max_length {
        return Err(format!(
            "{path}: string length outside {}..={}",
            schema.min_length, schema.max_length
        ));
    }
    if let Some(constant) = &schema.const_value
        && actual != constant
    {
        return Err(format!("{path}: expected constant {constant}"));
    }
    if !schema.enum_values.is_empty() && !schema.enum_values.iter().any(|value| value == actual) {
        return Err(format!("{path}: unknown enum value"));
    }
    if schema.decimal.is_some()
        && (actual.is_empty()
            || (actual.len() > 1 && actual.starts_with('0'))
            || !actual.bytes().all(|byte| byte.is_ascii_digit())
            || actual.parse::<u64>().is_err())
    {
        return Err(format!("{path}: invalid canonical decimal u64"));
    }
    if schema.format.as_deref() == Some("date-time") && !actual.ends_with('Z') {
        return Err(format!("{path}: timestamp must be UTC"));
    }
    if let Some(pattern) = &schema.pattern {
        let expression = regex::Regex::new(pattern)
            .map_err(|error| format!("{path}: invalid schema regex {pattern}: {error}"))?;
        if !expression.is_match(actual) {
            return Err(format!("{path}: string does not match {pattern}"));
        }
    }
    Ok(())
}
