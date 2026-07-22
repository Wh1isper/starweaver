use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct ProtocolIr {
    pub identity: ProtocolIdentityIr,
    pub schemas: BTreeMap<String, SchemaIr>,
    pub methods: BTreeMap<String, MethodIr>,
    pub notifications: BTreeMap<String, NotificationIr>,
    pub errors: BTreeMap<String, ErrorIr>,
    pub features: BTreeSet<String>,
    pub event_classes: BTreeMap<String, EventClassIr>,
    pub event_profiles: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct ProtocolIdentityIr {
    pub name: String,
    pub major: u32,
    pub revision: String,
    pub schema_digest: String,
}

#[derive(Clone, Debug)]
pub struct MethodIr {
    pub name: String,
    pub params_type: String,
    pub result_type: String,
    pub errors: Vec<String>,
    pub features: Vec<String>,
    pub transports: Vec<String>,
    pub scopes: Vec<String>,
    pub idempotency: String,
    pub stability: String,
    pub spec: String,
}

#[derive(Clone, Debug)]
pub struct NotificationIr {
    pub name: String,
    pub params_type: String,
    pub features: Vec<String>,
    pub transports: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct EventClassIr {
    pub name: String,
    pub schema_type: String,
    pub feature: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ErrorIr {
    pub name: String,
    pub code: i64,
    pub message: String,
    pub data_type: String,
}

#[derive(Clone, Debug)]
pub struct SchemaIr {
    pub name: String,
    pub kind: SchemaKind,
}

#[derive(Clone, Debug)]
pub enum SchemaKind {
    Ref(String),
    String(StringSchema),
    Integer {
        minimum: i64,
        maximum: i64,
    },
    Boolean,
    Null,
    /// Explicitly marked arbitrary JSON object. This is the only open object shape.
    JsonObject,
    Object(ObjectSchema),
    Array {
        items: Box<Self>,
        min_items: usize,
        max_items: usize,
        unique: bool,
        canonical_utf8_byte_order: bool,
    },
    OneOf {
        variants: Vec<Self>,
        discriminator: Option<String>,
        nullable: bool,
    },
}

#[derive(Clone, Debug)]
pub struct StringSchema {
    pub enum_values: Vec<String>,
    pub const_value: Option<String>,
    pub pattern: Option<String>,
    pub format: Option<String>,
    pub min_length: usize,
    pub max_length: usize,
    pub decimal: Option<DecimalSchema>,
}

#[derive(Clone, Debug)]
pub struct DecimalSchema {
    pub kind: String,
    pub minimum: String,
    pub maximum: String,
}

#[derive(Clone, Debug)]
pub struct ObjectSchema {
    pub properties: BTreeMap<String, SchemaKind>,
    pub required: BTreeSet<String>,
}

impl ProtocolIr {
    pub fn from_bundle(bundle: &Value) -> Result<Self, String> {
        let protocol = object_member(bundle, "x-starweaver-protocol")?;
        let identity = ProtocolIdentityIr {
            name: string_member(protocol, "name")?.to_string(),
            major: u32::try_from(integer_member(protocol, "major")?)
                .map_err(|_| "protocol major is outside u32".to_string())?,
            revision: string_member(protocol, "revision")?.to_string(),
            schema_digest: string_member(protocol, "schemaDigest")?.to_string(),
        };
        let components = object_member(bundle, "components")?;
        let mut schemas = BTreeMap::new();
        for (name, schema) in components
            .get("schemas")
            .and_then(Value::as_object)
            .ok_or("components.schemas must be an object")?
        {
            schemas.insert(
                name.clone(),
                SchemaIr {
                    name: name.clone(),
                    kind: parse_schema(schema)?,
                },
            );
        }
        let error_data = object_member(bundle, "x-starweaver-error-data")?;
        let mut errors = BTreeMap::new();
        for (name, error) in components
            .get("errors")
            .and_then(Value::as_object)
            .ok_or("components.errors must be an object")?
        {
            let object = error
                .as_object()
                .ok_or_else(|| format!("error {name} must be an object"))?;
            let data_type = error_data
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("error {name} has no root data registry entry"))?;
            errors.insert(
                name.clone(),
                ErrorIr {
                    name: name.clone(),
                    code: object
                        .get("code")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| format!("error {name} has no integer code"))?,
                    message: object
                        .get("message")
                        .and_then(Value::as_str)
                        .ok_or_else(|| format!("error {name} has no message"))?
                        .to_string(),
                    data_type: data_type.to_string(),
                },
            );
        }
        let methods_value = bundle
            .get("methods")
            .and_then(Value::as_array)
            .ok_or("methods must be an array")?;
        let mut methods = BTreeMap::new();
        for method in methods_value {
            let object = method.as_object().ok_or("method must be an object")?;
            let name = required_string(object, "name")?.to_string();
            let errors = object
                .get("errors")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("method {name} errors must be an array"))?
                .iter()
                .map(|entry| component_ref_name(entry, "errors"))
                .collect::<Result<Vec<_>, _>>()?;
            let method_ir = MethodIr {
                name: name.clone(),
                params_type: required_string(object, "x-starweaver-params-type")?.to_string(),
                result_type: required_string(object, "x-starweaver-result-type")?.to_string(),
                errors,
                features: string_array(object, "x-starweaver-features")?,
                transports: string_array(object, "x-starweaver-transports")?,
                scopes: string_array(object, "x-starweaver-scopes")?,
                idempotency: required_string(object, "x-starweaver-idempotency")?.to_string(),
                stability: required_string(object, "x-starweaver-stability")?.to_string(),
                spec: required_string(object, "x-starweaver-spec")?.to_string(),
            };
            if methods.insert(name.clone(), method_ir).is_some() {
                return Err(format!("duplicate method {name}"));
            }
        }
        let mut notifications = BTreeMap::new();
        for notification in bundle
            .get("x-starweaver-notifications")
            .and_then(Value::as_array)
            .ok_or("notifications must be an array")?
        {
            let object = notification
                .as_object()
                .ok_or("notification must be an object")?;
            let name = required_string(object, "name")?.to_string();
            let params_type = component_ref_name(
                object.get("params").ok_or("notification params missing")?,
                "schemas",
            )?;
            notifications.insert(
                name.clone(),
                NotificationIr {
                    name,
                    params_type,
                    features: string_array(object, "features")?,
                    transports: string_array(object, "transports")?,
                    scopes: string_array(object, "scopes")?,
                },
            );
        }
        let features = bundle
            .get("x-starweaver-features")
            .and_then(Value::as_array)
            .ok_or("features must be an array")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "feature must be a string".to_string())
            })
            .collect::<Result<_, _>>()?;
        let event_classes = object_member(bundle, "x-starweaver-event-classes")?
            .iter()
            .map(|(name, value)| {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("event class {name} must be an object"))?;
                reject_unknown_schema_keys(object, &["schema", "feature", "scopes"])?;
                let feature = object
                    .get("feature")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value.as_str().map(str::to_string).ok_or_else(|| {
                            format!("event class {name} feature must be string or null")
                        })
                    })
                    .transpose()?;
                Ok((
                    name.clone(),
                    EventClassIr {
                        name: name.clone(),
                        schema_type: component_ref_name(
                            object
                                .get("schema")
                                .ok_or_else(|| format!("event class {name} schema missing"))?,
                            "schemas",
                        )?,
                        feature,
                        scopes: string_array(object, "scopes")?,
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        let event_profiles = object_member(bundle, "x-starweaver-event-profiles")?
            .iter()
            .map(|(name, value)| {
                let variants = value
                    .as_array()
                    .ok_or_else(|| format!("event profile {name} must be an array"))?
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("event profile {name} entry must be a string"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((name.clone(), variants))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            identity,
            schemas,
            methods,
            notifications,
            errors,
            features,
            event_classes,
            event_profiles,
        })
    }
}

fn parse_schema(value: &Value) -> Result<SchemaKind, String> {
    let object = value.as_object().ok_or("schema must be an object")?;
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        reject_unknown_schema_keys(object, &["$ref"])?;
        return Ok(SchemaKind::Ref(
            reference
                .strip_prefix("#/components/schemas/")
                .ok_or_else(|| format!("non-component ref in bundle: {reference}"))?
                .to_string(),
        ));
    }
    if let Some(variants) = object.get("oneOf").and_then(Value::as_array) {
        reject_unknown_schema_keys(object, &["oneOf", "discriminator", "x-starweaver-nullable"])?;
        let variants = variants
            .iter()
            .map(parse_schema)
            .collect::<Result<Vec<_>, _>>()?;
        let nullable = object
            .get("x-starweaver-nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let discriminator = object
            .get("discriminator")
            .and_then(Value::as_object)
            .and_then(|value| value.get("propertyName"))
            .and_then(Value::as_str)
            .map(str::to_string);
        return Ok(SchemaKind::OneOf {
            variants,
            discriminator,
            nullable,
        });
    }
    match object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("schema type is required")?
    {
        "string" => {
            reject_unknown_schema_keys(
                object,
                &[
                    "type",
                    "enum",
                    "const",
                    "pattern",
                    "format",
                    "minLength",
                    "maxLength",
                    "x-starweaver-decimal",
                ],
            )?;
            let enum_values =
                object
                    .get("enum")
                    .and_then(Value::as_array)
                    .map_or_else(Vec::new, |items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    });
            let decimal = object
                .get("x-starweaver-decimal")
                .map(|value| {
                    let decimal = value
                        .as_object()
                        .ok_or("x-starweaver-decimal must be an object")?;
                    reject_unknown_schema_keys(decimal, &["kind", "minimum", "maximum"])?;
                    Ok::<DecimalSchema, String>(DecimalSchema {
                        kind: string_member(decimal, "kind")?.to_string(),
                        minimum: string_member(decimal, "minimum")?.to_string(),
                        maximum: string_member(decimal, "maximum")?.to_string(),
                    })
                })
                .transpose()?;
            Ok(SchemaKind::String(StringSchema {
                enum_values,
                const_value: object
                    .get("const")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                pattern: object
                    .get("pattern")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                format: object
                    .get("format")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                min_length: optional_usize(object, "minLength", 0)?,
                max_length: optional_usize(object, "maxLength", usize::MAX)?,
                decimal,
            }))
        }
        "integer" => {
            reject_unknown_schema_keys(object, &["type", "const", "minimum", "maximum"])?;
            let constant = object.get("const").and_then(Value::as_i64);
            Ok(SchemaKind::Integer {
                minimum: constant
                    .or_else(|| object.get("minimum").and_then(Value::as_i64))
                    .ok_or("integer minimum is required")?,
                maximum: constant
                    .or_else(|| object.get("maximum").and_then(Value::as_i64))
                    .ok_or("integer maximum is required")?,
            })
        }
        "boolean" => {
            reject_unknown_schema_keys(object, &["type"])?;
            Ok(SchemaKind::Boolean)
        }
        "null" => {
            reject_unknown_schema_keys(object, &["type"])?;
            Ok(SchemaKind::Null)
        }
        "object" => {
            if object.get("x-starweaver-json-value") == Some(&Value::String("object".to_string())) {
                reject_unknown_schema_keys(
                    object,
                    &["type", "additionalProperties", "x-starweaver-json-value"],
                )?;
                if object.get("additionalProperties") != Some(&Value::Bool(true)) {
                    return Err(
                        "arbitrary JSON objects must set additionalProperties: true".to_string()
                    );
                }
                return Ok(SchemaKind::JsonObject);
            }
            reject_unknown_schema_keys(
                object,
                &["type", "additionalProperties", "required", "properties"],
            )?;
            if object.get("additionalProperties") != Some(&Value::Bool(false)) {
                return Err("objects must set additionalProperties: false".to_string());
            }
            let required = object
                .get("required")
                .and_then(Value::as_array)
                .ok_or("object required must be an array")?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "required entry must be string".to_string())
                })
                .collect::<Result<_, _>>()?;
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or("object properties must be an object")?
                .iter()
                .map(|(name, schema)| Ok((name.clone(), parse_schema(schema)?)))
                .collect::<Result<_, String>>()?;
            Ok(SchemaKind::Object(ObjectSchema {
                properties,
                required,
            }))
        }
        "array" => {
            reject_unknown_schema_keys(
                object,
                &[
                    "type",
                    "items",
                    "minItems",
                    "maxItems",
                    "uniqueItems",
                    "x-starweaver-canonical-order",
                ],
            )?;
            let canonical_utf8_byte_order = match object.get("x-starweaver-canonical-order") {
                None => false,
                Some(Value::String(value)) if value == "utf8-byte-ascending" => true,
                Some(_) => {
                    return Err(
                        "x-starweaver-canonical-order must be utf8-byte-ascending".to_string()
                    );
                }
            };
            Ok(SchemaKind::Array {
                items: Box::new(parse_schema(
                    object.get("items").ok_or("array items missing")?,
                )?),
                min_items: optional_usize(object, "minItems", 0)?,
                max_items: required_usize(object, "maxItems")?,
                unique: object
                    .get("uniqueItems")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                canonical_utf8_byte_order,
            })
        }
        other => Err(format!("unsupported schema type {other}")),
    }
}

fn optional_usize(
    object: &serde_json::Map<String, Value>,
    name: &str,
    default: usize,
) -> Result<usize, String> {
    match object.get(name) {
        None => Ok(default),
        Some(value) => usize::try_from(
            value
                .as_u64()
                .ok_or_else(|| format!("{name} must be an unsigned integer"))?,
        )
        .map_err(|_| format!("{name} exceeds the generator platform limit")),
    }
}

fn required_usize(object: &serde_json::Map<String, Value>, name: &str) -> Result<usize, String> {
    let value = object
        .get(name)
        .ok_or_else(|| format!("{name} is required"))?;
    usize::try_from(
        value
            .as_u64()
            .ok_or_else(|| format!("{name} must be an unsigned integer"))?,
    )
    .map_err(|_| format!("{name} exceeds the generator platform limit"))
}

fn reject_unknown_schema_keys(
    object: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unsupported schema keyword {key}"));
        }
    }
    Ok(())
}

fn object_member<'a>(
    value: &'a Value,
    name: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{name} must be an object"))
}
fn string_member<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))
}
fn integer_member(object: &serde_json::Map<String, Value>, name: &str) -> Result<u64, String> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{name} must be an unsigned integer"))
}
fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    string_member(object, name)
}
fn string_array(
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Vec<String>, String> {
    object
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{name} must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{name} entries must be strings"))
        })
        .collect()
}
fn component_ref_name(value: &Value, family: &str) -> Result<String, String> {
    let reference = value
        .get("$ref")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("expected {family} component ref"))?;
    reference
        .strip_prefix(&format!("#/components/{family}/"))
        .map(str::to_string)
        .ok_or_else(|| format!("invalid {family} ref {reference}"))
}
