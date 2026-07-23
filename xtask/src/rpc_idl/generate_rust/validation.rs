use serde_json::{Value, json};

use crate::rpc_idl::model::{ProtocolIr, SchemaKind};

pub fn render(ir: &ProtocolIr) -> Result<String, String> {
    let schemas = ir
        .schemas
        .iter()
        .map(|(name, schema)| Ok((name.clone(), descriptor(&schema.kind)?)))
        .collect::<Result<serde_json::Map<_, _>, String>>()?;
    let request_roots = ir
        .methods
        .values()
        .map(|method| {
            (
                method.name.clone(),
                Value::String(method.params_type.clone()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let result_roots = ir
        .methods
        .values()
        .map(|method| {
            (
                method.name.clone(),
                Value::String(method.result_type.clone()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let notification_roots = ir
        .notifications
        .values()
        .map(|notification| {
            (
                notification.name.clone(),
                Value::String(notification.params_type.clone()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let descriptor_json = serde_json::to_string(&json!({
        "requestRoots": request_roots,
        "resultRoots": result_roots,
        "notificationRoots": notification_roots,
        "schemas": schemas,
    }))
    .map_err(|error| error.to_string())?;

    Ok(format!(
        r#"//! Generated exhaustive runtime validation for request parameters.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::metadata::{{Method, Notification}};

const DESCRIPTORS_JSON: &str = {descriptor_json:?};
static DESCRIPTORS: OnceLock<Value> = OnceLock::new();
const MAX_VALIDATION_DEPTH: usize = 128;

pub(super) fn validate_method_params(method: Method, input: &Value) -> Result<(), ()> {{
    let descriptors = DESCRIPTORS.get_or_init(|| {{
        serde_json::from_str(DESCRIPTORS_JSON)
            .expect("generated runtime validation descriptors are valid JSON")
    }});
    validate_root(descriptors, "requestRoots", method.metadata().name, input)
}}

pub(super) fn validate_method_result(method: Method, input: &Value) -> Result<(), ()> {{
    let descriptors = DESCRIPTORS.get_or_init(|| {{
        serde_json::from_str(DESCRIPTORS_JSON)
            .expect("generated runtime validation descriptors are valid JSON")
    }});
    validate_root(descriptors, "resultRoots", method.metadata().name, input)
}}

pub(super) fn validate_notification_params(
    notification: Notification,
    input: &Value,
) -> Result<(), ()> {{
    let descriptors = DESCRIPTORS.get_or_init(|| {{
        serde_json::from_str(DESCRIPTORS_JSON)
            .expect("generated runtime validation descriptors are valid JSON")
    }});
    validate_root(
        descriptors,
        "notificationRoots",
        notification.metadata().name,
        input,
    )
}}

pub(super) fn validate_launch_envelope(input: &Value) -> Result<(), ()> {{
    let descriptors = DESCRIPTORS.get_or_init(|| {{
        serde_json::from_str(DESCRIPTORS_JSON)
            .expect("generated runtime validation descriptors are valid JSON")
    }});
    validate_named(descriptors, "LaunchEnvelope", input, 0)
}}

fn validate_root(
    descriptors: &Value,
    registry: &str,
    name: &str,
    input: &Value,
) -> Result<(), ()> {{
    let root = descriptors
        .get(registry)
        .and_then(|roots| roots.get(name))
        .and_then(Value::as_str)
        .ok_or(())?;
    validate_named(descriptors, root, input, 0)
}}

fn validate_named(
    descriptors: &Value,
    name: &str,
    input: &Value,
    depth: usize,
) -> Result<(), ()> {{
    let schema = descriptors
        .get("schemas")
        .and_then(|schemas| schemas.get(name))
        .ok_or(())?;
    validate(descriptors, schema, input, depth)
}}

fn validate(
    descriptors: &Value,
    schema: &Value,
    input: &Value,
    depth: usize,
) -> Result<(), ()> {{
    if depth > MAX_VALIDATION_DEPTH {{
        return Err(());
    }}
    match schema.get("kind").and_then(Value::as_str).ok_or(())? {{
        "ref" => validate_named(
            descriptors,
            schema.get("name").and_then(Value::as_str).ok_or(())?,
            input,
            depth + 1,
        ),
        "string" => validate_string(schema, input),
        "integer" => {{
            let value = input.as_i64().ok_or(())?;
            let minimum = schema.get("minimum").and_then(Value::as_i64).ok_or(())?;
            let maximum = schema.get("maximum").and_then(Value::as_i64).ok_or(())?;
            (value >= minimum && value <= maximum).then_some(()).ok_or(())
        }}
        "boolean" => input.is_boolean().then_some(()).ok_or(()),
        "null" => input.is_null().then_some(()).ok_or(()),
        "jsonObject" => input.is_object().then_some(()).ok_or(()),
        "object" => validate_object(descriptors, schema, input, depth + 1),
        "array" => validate_array(descriptors, schema, input, depth + 1),
        "oneOf" => {{
            let variants = schema.get("variants").and_then(Value::as_array).ok_or(())?;
            (variants
                .iter()
                .filter(|variant| validate(descriptors, variant, input, depth + 1).is_ok())
                .count()
                == 1)
                .then_some(())
                .ok_or(())
        }}
        _ => Err(()),
    }}
}}

fn validate_string(schema: &Value, input: &Value) -> Result<(), ()> {{
    let value = input.as_str().ok_or(())?;
    let length = value.chars().count();
    let minimum = usize::try_from(
        schema.get("minLength").and_then(Value::as_u64).ok_or(())?,
    )
    .map_err(|_| ())?;
    let maximum = usize::try_from(
        schema.get("maxLength").and_then(Value::as_u64).ok_or(())?,
    )
    .map_err(|_| ())?;
    if length < minimum || length > maximum {{
        return Err(());
    }}
    if let Some(constant) = schema.get("constValue").and_then(Value::as_str)
        && value != constant
    {{
        return Err(());
    }}
    let enum_values = schema.get("enumValues").and_then(Value::as_array).ok_or(())?;
    if !enum_values.is_empty()
        && !enum_values.iter().any(|candidate| candidate.as_str() == Some(value))
    {{
        return Err(());
    }}
    if let Some(decimal) = schema.get("decimal").filter(|value| !value.is_null()) {{
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {{
            return Err(());
        }}
        let parsed = value.parse::<u64>().map_err(|_| ())?;
        let minimum = decimal
            .get("minimum")
            .and_then(Value::as_str)
            .ok_or(())?
            .parse::<u64>()
            .map_err(|_| ())?;
        let maximum = decimal
            .get("maximum")
            .and_then(Value::as_str)
            .ok_or(())?
            .parse::<u64>()
            .map_err(|_| ())?;
        if parsed < minimum || parsed > maximum {{
            return Err(());
        }}
    }}
    if schema.get("format").and_then(Value::as_str) == Some("date-time")
        && (!value.ends_with('Z')
            || chrono::DateTime::parse_from_rfc3339(value)
                .is_err())
    {{
        return Err(());
    }}
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
        && !Regex::new(pattern).map_err(|_| ())?.is_match(value)
    {{
        return Err(());
    }}
    Ok(())
}}

fn validate_object(
    descriptors: &Value,
    schema: &Value,
    input: &Value,
    depth: usize,
) -> Result<(), ()> {{
    let object = input.as_object().ok_or(())?;
    let properties = schema.get("properties").and_then(Value::as_object).ok_or(())?;
    if object.keys().any(|name| !properties.contains_key(name)) {{
        return Err(());
    }}
    let required = schema.get("required").and_then(Value::as_array).ok_or(())?;
    if required
        .iter()
        .any(|name| name.as_str().is_none_or(|name| !object.contains_key(name)))
    {{
        return Err(());
    }}
    for (name, value) in object {{
        validate(descriptors, properties.get(name).ok_or(())?, value, depth)?;
    }}
    Ok(())
}}

fn validate_array(
    descriptors: &Value,
    schema: &Value,
    input: &Value,
    depth: usize,
) -> Result<(), ()> {{
    let array = input.as_array().ok_or(())?;
    let minimum = usize::try_from(
        schema.get("minItems").and_then(Value::as_u64).ok_or(())?,
    )
    .map_err(|_| ())?;
    let maximum = usize::try_from(
        schema.get("maxItems").and_then(Value::as_u64).ok_or(())?,
    )
    .map_err(|_| ())?;
    if array.len() < minimum || array.len() > maximum {{
        return Err(());
    }}
    if schema.get("unique").and_then(Value::as_bool) == Some(true)
        && array
            .iter()
            .enumerate()
            .any(|(index, value)| array[..index].contains(value))
    {{
        return Err(());
    }}
    if schema
        .get("canonicalUtf8ByteOrder")
        .and_then(Value::as_bool)
        == Some(true)
        && array.windows(2).any(|pair| {{
            let Some(left) = pair[0].as_str() else {{
                return true;
            }};
            let Some(right) = pair[1].as_str() else {{
                return true;
            }};
            left.as_bytes() >= right.as_bytes()
        }})
    {{
        return Err(());
    }}
    let items = schema.get("items").ok_or(())?;
    for value in array {{
        validate(descriptors, items, value, depth)?;
    }}
    Ok(())
}}
"#
    ))
}

fn descriptor(schema: &SchemaKind) -> Result<Value, String> {
    Ok(match schema {
        SchemaKind::Ref(name) => json!({"kind": "ref", "name": name}),
        SchemaKind::String(value) => json!({
            "kind": "string",
            "enumValues": value.enum_values,
            "constValue": value.const_value,
            "pattern": value.pattern,
            "format": value.format,
            "minLength": value.min_length,
            "maxLength": value.max_length,
            "decimal": value.decimal.as_ref().map(|decimal| json!({
                "minimum": decimal.minimum,
                "maximum": decimal.maximum,
            })),
        }),
        SchemaKind::Integer { minimum, maximum } => {
            json!({"kind": "integer", "minimum": minimum, "maximum": maximum})
        }
        SchemaKind::Boolean => json!({"kind": "boolean"}),
        SchemaKind::Null => json!({"kind": "null"}),
        SchemaKind::JsonObject => json!({"kind": "jsonObject"}),
        SchemaKind::Object(value) => {
            let properties = value
                .properties
                .iter()
                .map(|(name, schema)| Ok((name.clone(), descriptor(schema)?)))
                .collect::<Result<serde_json::Map<_, _>, String>>()?;
            json!({
                "kind": "object",
                "properties": properties,
                "required": value.required,
            })
        }
        SchemaKind::Array {
            items,
            min_items,
            max_items,
            unique,
            canonical_utf8_byte_order,
        } => json!({
            "kind": "array",
            "items": descriptor(items)?,
            "minItems": min_items,
            "maxItems": max_items,
            "unique": unique,
            "canonicalUtf8ByteOrder": canonical_utf8_byte_order,
        }),
        SchemaKind::OneOf {
            variants,
            discriminator,
            nullable,
        } => json!({
            "kind": "oneOf",
            "variants": variants
                .iter()
                .map(descriptor)
                .collect::<Result<Vec<_>, _>>()?,
            "discriminator": discriminator,
            "nullable": nullable,
        }),
    })
}
