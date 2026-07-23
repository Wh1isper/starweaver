#![allow(clippy::format_push_string)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::json;

use super::{
    bundle, lint,
    model::{MethodIr, ObjectSchema, ProtocolIr, SchemaKind, StringSchema},
};

const GENERATED_MARKER: &str = ".starweaver-host-typescript-generated";

pub fn generate(ir: &ProtocolIr) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    insert(&mut files, "index.ts", index());
    insert(&mut files, "fetch.ts", fetch_transport());
    insert(&mut files, "node.ts", node_transport());
    insert(&mut files, "generated/identity.ts", identity(ir));
    insert(&mut files, "generated/types.ts", types(ir));
    insert(&mut files, "generated/metadata.ts", metadata(ir)?);
    insert(&mut files, "generated/validators.ts", validators(ir)?);
    insert(&mut files, "generated/codecs.ts", codecs(ir));
    insert(&mut files, "generated/client.ts", client(ir));
    insert(
        &mut files,
        GENERATED_MARKER,
        format!("{}\n", ir.identity.schema_digest),
    );
    Ok(files)
}

pub fn generate_to(args: &[String]) -> Result<(), String> {
    let output_value = match args {
        [flag, value] if flag == "--output" => value,
        _ => {
            return Err(
                "generate-rpc-typescript requires exactly --output <directory>".to_string(),
            );
        }
    };
    let repository = crate::common::root()?;
    let output = resolve_output(&repository, output_value)?;
    let bundled = bundle::build(&repository.join("protocol/host"))?;
    let ir = ProtocolIr::from_bundle(&bundled.value)?;
    lint::check(&ir)?;
    promote(&output, generate(&ir)?)
}

fn resolve_output(repository: &Path, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err("TypeScript output directory must not be empty".to_string());
    }
    let requested = Path::new(value);
    if requested
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("TypeScript output directory must not contain `..`".to_string());
    }
    let output = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        repository.join(requested)
    };
    let forbidden = [
        repository.to_path_buf(),
        repository.join("protocol/host"),
        repository.join("crates/starweaver-rpc-core/src/generated"),
        repository.join("apps/starweaver-desktop/src/generated/host"),
        repository.join("apps/starweaver-desktop/src-tauri/src/generated/host"),
    ];
    if forbidden
        .iter()
        .any(|path| output == *path || path != repository && output.starts_with(path))
    {
        return Err(format!(
            "refusing to generate complete TypeScript bindings into managed path {}",
            output.display()
        ));
    }
    Ok(output)
}

fn promote(output: &Path, files: BTreeMap<PathBuf, Vec<u8>>) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(output) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "output {} must be a real directory",
                output.display()
            ));
        }
        let mut entries = fs::read_dir(output)
            .map_err(|error| format!("read output {}: {error}", output.display()))?;
        let is_empty = entries.next().is_none();
        if !is_empty && !output.join(GENERATED_MARKER).is_file() {
            return Err(format!(
                "output {} is non-empty and was not created by this generator",
                output.display()
            ));
        }
    }
    let parent = output
        .parent()
        .ok_or_else(|| format!("output {} has no parent", output.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create output parent {}: {error}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".starweaver-host-typescript-")
        .tempdir_in(parent)
        .map_err(|error| format!("create TypeScript staging directory: {error}"))?;
    for (relative, bytes) in files {
        let path = staging.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
        fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    let staged = staging.keep();
    let backup = output.with_extension(format!("starweaver-backup-{}", std::process::id()));
    if backup.exists() {
        return Err(format!("stale backup path exists at {}", backup.display()));
    }
    if output.exists() {
        fs::rename(output, &backup)
            .map_err(|error| format!("backup {}: {error}", output.display()))?;
    }
    if let Err(error) = fs::rename(&staged, output) {
        if backup.exists() {
            let _ = fs::rename(&backup, output);
        }
        let _ = fs::remove_dir_all(&staged);
        return Err(format!(
            "promote TypeScript output {}: {error}",
            output.display()
        ));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("remove backup {}: {error}", backup.display()))?;
    }
    println!("generated TypeScript host bindings at {}", output.display());
    Ok(())
}

fn insert(files: &mut BTreeMap<PathBuf, Vec<u8>>, path: &str, text: String) {
    files.insert(PathBuf::from(path), normalize(text).into_bytes());
}

fn normalize(mut text: String) -> String {
    text = text.replace("\r\n", "\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn types(ir: &ProtocolIr) -> String {
    let mut out = generated_header();
    for schema in ir.schemas.values() {
        out.push_str(&render_named_schema(&schema.name, &schema.kind));
        out.push('\n');
    }
    out.push_str("export interface HostMethodMap {\n");
    for method in ir.methods.values() {
        out.push_str(&format!(
            "  readonly {}: {{ readonly params: {}; readonly result: {} }};\n",
            ts_string(&method.name),
            method.params_type,
            method.result_type
        ));
    }
    out.push_str("}\nexport type HostMethodName = keyof HostMethodMap;\n");
    out.push_str("export interface HostNotificationMap {\n");
    for notification in ir.notifications.values() {
        out.push_str(&format!(
            "  readonly {}: {};\n",
            ts_string(&notification.name),
            notification.params_type
        ));
    }
    out.push_str("}\nexport type HostNotificationName = keyof HostNotificationMap;\n");
    out.push_str("export type HostErrorData =\n");
    for (index, error) in ir.errors.values().enumerate() {
        out.push_str(&format!(
            "  {} {}\n",
            if index == 0 { "" } else { "|" },
            error.data_type
        ));
    }
    out.push_str(";\nexport interface HostErrorEnvelope { readonly code: number; readonly message: string; readonly data: HostErrorData }\n");
    out.push_str("export interface HostRequestEnvelope<M extends HostMethodName = HostMethodName> { readonly jsonrpc: \"2.0\"; readonly id: RequestId; readonly method: M; readonly params: HostMethodMap[M][\"params\"] }\n");
    out.push_str("export interface HostSuccessEnvelope<R = unknown> { readonly jsonrpc: \"2.0\"; readonly id: RequestId; readonly result: R }\n");
    out.push_str("export interface HostErrorResponseEnvelope { readonly jsonrpc: \"2.0\"; readonly id: RequestId | null; readonly error: HostErrorEnvelope }\n");
    out.push_str("export interface HostNotificationEnvelope<N extends HostNotificationName = HostNotificationName> { readonly jsonrpc: \"2.0\"; readonly method: N; readonly params: HostNotificationMap[N] }\n");
    out
}

fn render_named_schema(name: &str, kind: &SchemaKind) -> String {
    match kind {
        SchemaKind::String(value) if !value.enum_values.is_empty() => format!(
            "export type {name} = {};\n",
            value
                .enum_values
                .iter()
                .map(|value| ts_string(value))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        SchemaKind::String(value) if value.const_value.is_some() => format!(
            "export type {name} = {};\n",
            ts_string(value.const_value.as_deref().unwrap_or_default())
        ),
        SchemaKind::String(_) => format!(
            "declare const {name}Brand: unique symbol;\nexport type {name} = string & {{ readonly [{name}Brand]: true }};\n"
        ),
        SchemaKind::Integer { .. } => format!("export type {name} = number;\n"),
        SchemaKind::Boolean => format!("export type {name} = boolean;\n"),
        SchemaKind::Null => format!("export type {name} = null;\n"),
        SchemaKind::JsonObject => {
            format!("export type {name} = Readonly<Record<string, unknown>>;\n")
        }
        SchemaKind::Object(object) => render_interface(name, object),
        SchemaKind::Array { .. } | SchemaKind::OneOf { .. } | SchemaKind::Ref(_) => {
            format!("export type {name} = {};\n", ts_type(kind))
        }
    }
}

fn render_interface(name: &str, object: &ObjectSchema) -> String {
    if object.properties.is_empty() {
        return format!("export type {name} = Readonly<Record<string, never>>;\n");
    }
    let mut out = format!("export interface {name} {{\n");
    for (field, kind) in &object.properties {
        let optional = if object.required.contains(field) {
            ""
        } else {
            "?"
        };
        out.push_str(&format!(
            "  readonly {}{}: {};\n",
            ts_property(field),
            optional,
            ts_type(kind)
        ));
    }
    out.push_str("}\n");
    out
}

fn ts_type(kind: &SchemaKind) -> String {
    match kind {
        SchemaKind::Ref(name) => name.clone(),
        SchemaKind::String(value) if !value.enum_values.is_empty() => value
            .enum_values
            .iter()
            .map(|value| ts_string(value))
            .collect::<Vec<_>>()
            .join(" | "),
        SchemaKind::String(value) if value.const_value.is_some() => {
            ts_string(value.const_value.as_deref().unwrap_or_default())
        }
        SchemaKind::String(_) => "string".to_string(),
        SchemaKind::Integer { .. } => "number".to_string(),
        SchemaKind::Boolean => "boolean".to_string(),
        SchemaKind::Null => "null".to_string(),
        SchemaKind::JsonObject | SchemaKind::Object(_) => {
            "Readonly<Record<string, unknown>>".to_string()
        }
        SchemaKind::Array { items, .. } => {
            format!("readonly {}[]", parenthesize(ts_type(items)))
        }
        SchemaKind::OneOf { variants, .. } => {
            variants.iter().map(ts_type).collect::<Vec<_>>().join(" | ")
        }
    }
}

fn parenthesize(value: String) -> String {
    if value.contains(" | ") {
        format!("({value})")
    } else {
        value
    }
}
fn ts_property(value: &str) -> String {
    if value.chars().enumerate().all(|(index, ch)| {
        ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
    }) {
        value.to_string()
    } else {
        ts_string(value)
    }
}
fn ts_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}
fn generated_header() -> String {
    "// @generated by `cargo run -p xtask -- generate-rpc-typescript --output <directory>`; do not edit.\n\n".to_string()
}

fn validators(ir: &ProtocolIr) -> Result<String, String> {
    let descriptors = ir
        .schemas
        .iter()
        .map(|(name, schema)| Ok((name.clone(), schema_descriptor(&schema.kind)?)))
        .collect::<Result<serde_json::Map<_, _>, String>>()?;
    let descriptor_json =
        serde_json::to_string_pretty(&descriptors).map_err(|error| error.to_string())?;
    let mut out = generated_header();
    out.push_str("import type * as T from \"./types.js\";\n\n");
    out.push_str("type Descriptor =\n  | { readonly kind: \"ref\"; readonly name: string }\n  | { readonly kind: \"string\"; readonly enumValues: readonly string[]; readonly constValue: string | null; readonly pattern: string | null; readonly format: string | null; readonly minLength: number; readonly maxLength: number; readonly decimal: { readonly minimum: string; readonly maximum: string } | null }\n  | { readonly kind: \"integer\"; readonly minimum: number; readonly maximum: number }\n  | { readonly kind: \"boolean\" }\n  | { readonly kind: \"null\" }\n  | { readonly kind: \"jsonObject\" }\n  | { readonly kind: \"object\"; readonly properties: Readonly<Record<string, Descriptor>>; readonly required: readonly string[] }\n  | { readonly kind: \"array\"; readonly items: Descriptor; readonly minItems: number; readonly maxItems: number; readonly unique: boolean; readonly canonicalUtf8ByteOrder: boolean }\n  | { readonly kind: \"oneOf\"; readonly variants: readonly Descriptor[]; readonly discriminator: string | null; readonly nullable: boolean };\n\n");
    out.push_str("const schemas: Readonly<Record<string, Descriptor>> = ");
    out.push_str(&descriptor_json);
    out.push_str(" as Readonly<Record<string, Descriptor>>;\n\n");
    out.push_str(VALIDATOR_RUNTIME);
    out.push('\n');
    for name in ir.schemas.keys() {
        out.push_str(&format!("export function parse{name}(value: unknown): T.{name} {{ return parseSchema<T.{name}>({},{}) }}\n", ts_string(name), "value"));
    }
    out.push_str("\nexport function decimalU64FromBigInt(value: bigint): T.DecimalU64 { if (value < 0n || value > 18446744073709551615n) throw new HostValidationError(\"$\", \"decimal_range\", \"value is outside u64\"); return parseDecimalU64(value.toString()) }\n");
    out.push_str("export function decimalU64ToBigInt(value: T.DecimalU64): bigint { return BigInt(parseDecimalU64(value)) }\n");
    out.push_str("export function incrementDecimalU64(value: T.DecimalU64): T.DecimalU64 { const next = decimalU64ToBigInt(value) + 1n; return decimalU64FromBigInt(next) }\n");
    Ok(out)
}

const VALIDATOR_RUNTIME: &str = r#"export class HostValidationError extends Error {
  readonly path: string;
  readonly category: string;
  constructor(path: string, category: string, message: string) {
    super(`${path}: ${message}`);
    this.name = "HostValidationError";
    this.path = path;
    this.category = category;
  }
}

function parseSchema<TValue>(name: string, value: unknown): TValue {
  const descriptor = schemas[name];
  if (descriptor === undefined) throw new HostValidationError("$", "schema_missing", `unknown schema ${name}`);
  validate(descriptor, value, "$", 0);
  return value as TValue;
}

function validate(descriptor: Descriptor, value: unknown, path: string, depth: number): void {
  if (depth > 128) throw new HostValidationError(path, "depth", "maximum validation depth exceeded");
  switch (descriptor.kind) {
    case "ref": {
      const target = schemas[descriptor.name];
      if (target === undefined) throw new HostValidationError(path, "schema_missing", `unknown schema ${descriptor.name}`);
      validate(target, value, path, depth + 1);
      return;
    }
    case "string": {
      if (typeof value !== "string") throw new HostValidationError(path, "type", "expected string");
      const length = Array.from(value).length;
      if (length < descriptor.minLength || length > descriptor.maxLength) throw new HostValidationError(path, "string_length", "string length is outside bounds");
      if (descriptor.constValue !== null && value !== descriptor.constValue) throw new HostValidationError(path, "const", "unexpected constant value");
      if (descriptor.enumValues.length > 0 && !descriptor.enumValues.includes(value)) throw new HostValidationError(path, "enum", "unknown enum value");
      if (descriptor.pattern !== null && !(new RegExp(descriptor.pattern, "u")).test(value)) throw new HostValidationError(path, "pattern", "string does not match pattern");
      if (descriptor.format === "date-time" && !isUtcRfc3339(value)) throw new HostValidationError(path, "format", "expected an RFC 3339 UTC timestamp");
      if (descriptor.decimal !== null) {
        let parsed: bigint;
        try { parsed = BigInt(value); } catch { throw new HostValidationError(path, "decimal", "invalid decimal string"); }
        if (parsed < BigInt(descriptor.decimal.minimum) || parsed > BigInt(descriptor.decimal.maximum)) throw new HostValidationError(path, "decimal_range", "decimal is outside bounds");
      }
      return;
    }
    case "integer":
      if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new HostValidationError(path, "type", "expected safe integer");
      if (value < descriptor.minimum || value > descriptor.maximum) throw new HostValidationError(path, "integer_range", "integer is outside bounds");
      return;
    case "boolean":
      if (typeof value !== "boolean") throw new HostValidationError(path, "type", "expected boolean");
      return;
    case "null":
      if (value !== null) throw new HostValidationError(path, "type", "expected null");
      return;
    case "jsonObject":
      if (!isRecord(value)) throw new HostValidationError(path, "type", "expected object");
      return;
    case "object": {
      if (!isRecord(value)) throw new HostValidationError(path, "type", "expected object");
      const allowed = new Set(Object.keys(descriptor.properties));
      for (const key of Object.keys(value)) if (!allowed.has(key)) throw new HostValidationError(`${path}.${key}`, "unknown_field", "unknown field");
      for (const key of descriptor.required) if (!(key in value)) throw new HostValidationError(`${path}.${key}`, "required", "missing required field");
      for (const [key, child] of Object.entries(descriptor.properties)) if (key in value) validate(child, value[key], `${path}.${key}`, depth + 1);
      return;
    }
    case "array": {
      if (!Array.isArray(value)) throw new HostValidationError(path, "type", "expected array");
      if (value.length < descriptor.minItems || value.length > descriptor.maxItems) throw new HostValidationError(path, "array_length", "array length is outside bounds");
      value.forEach((item, index) => { validate(descriptor.items, item, `${path}[${index}]`, depth + 1); });
      if (descriptor.unique) {
        const keys = new Set(value.map(canonicalKey));
        if (keys.size !== value.length) throw new HostValidationError(path, "unique", "array items must be unique");
      }
      if (descriptor.canonicalUtf8ByteOrder) {
        for (let index = 1; index < value.length; index += 1) {
          const left = value[index - 1];
          const right = value[index];
          if (typeof left !== "string" || typeof right !== "string" || compareUtf8Bytes(left, right) >= 0) {
            throw new HostValidationError(path, "canonical_order", "array items must be in strict ascending UTF-8 byte order");
          }
        }
      }
      return;
    }
    case "oneOf": {
      let accepted = 0;
      let lastError: HostValidationError | undefined;
      for (const variant of descriptor.variants) {
        try { validate(variant, value, path, depth + 1); accepted += 1; } catch (error) { if (error instanceof HostValidationError) lastError = error; else throw error; }
      }
      if (accepted !== 1) throw lastError ?? new HostValidationError(path, "union", `expected exactly one union variant, accepted ${accepted}`);
      return;
    }
  }
}

function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value) }
function isUtcRfc3339(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?Z$/u.exec(value);
  if (match === null) return false;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const hour = Number(match[4]);
  const minute = Number(match[5]);
  const second = Number(match[6]);
  if (month < 1 || month > 12 || hour > 23 || minute > 59 || second > 60) return false;
  if (second === 60 && minute !== 59) return false;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return day >= 1 && day <= (days[month - 1] ?? 0);
}
function compareUtf8Bytes(left: string, right: string): number {
  const encoder = new TextEncoder();
  const leftBytes = encoder.encode(left);
  const rightBytes = encoder.encode(right);
  const length = Math.min(leftBytes.length, rightBytes.length);
  for (let index = 0; index < length; index += 1) {
    const difference = (leftBytes[index] ?? 0) - (rightBytes[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return leftBytes.length - rightBytes.length;
}
function canonicalKey(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalKey).join(",")}]`;
  if (isRecord(value)) return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalKey(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}
"#;

fn schema_descriptor(kind: &SchemaKind) -> Result<serde_json::Value, String> {
    Ok(match kind {
        SchemaKind::Ref(name) => json!({"kind": "ref", "name": name}),
        SchemaKind::String(StringSchema {
            enum_values,
            const_value,
            pattern,
            format,
            min_length,
            max_length,
            decimal,
        }) => {
            // JSON Schema omits maxLength for constants/enums whose finite values
            // already provide the effective bound. Use the largest exactly
            // representable JavaScript integer as the runtime no-bound sentinel;
            // this is stable across 32/64-bit generator hosts.
            let runtime_max_length = if *max_length == usize::MAX {
                9_007_199_254_740_991_u64
            } else {
                u64::try_from(*max_length)
                    .map_err(|_| "string maxLength exceeds u64".to_string())?
            };
            json!({
                "kind": "string", "enumValues": enum_values, "constValue": const_value, "pattern": pattern,
                "format": format, "minLength": min_length, "maxLength": runtime_max_length,
                "decimal": decimal.as_ref().map(|value| json!({"minimum": value.minimum, "maximum": value.maximum})),
            })
        }
        SchemaKind::Integer { minimum, maximum } => {
            json!({"kind": "integer", "minimum": minimum, "maximum": maximum})
        }
        SchemaKind::Boolean => json!({"kind": "boolean"}),
        SchemaKind::Null => json!({"kind": "null"}),
        SchemaKind::JsonObject => json!({"kind": "jsonObject"}),
        SchemaKind::Object(object) => {
            let properties = object
                .properties
                .iter()
                .map(|(name, value)| Ok((name.clone(), schema_descriptor(value)?)))
                .collect::<Result<serde_json::Map<_, _>, String>>()?;
            json!({"kind": "object", "properties": properties, "required": object.required})
        }
        SchemaKind::Array {
            items,
            min_items,
            max_items,
            unique,
            canonical_utf8_byte_order,
        } => {
            json!({"kind": "array", "items": schema_descriptor(items)?, "minItems": min_items, "maxItems": max_items, "unique": unique, "canonicalUtf8ByteOrder": canonical_utf8_byte_order})
        }
        SchemaKind::OneOf {
            variants,
            discriminator,
            nullable,
        } => json!({
            "kind": "oneOf", "variants": variants.iter().map(schema_descriptor).collect::<Result<Vec<_>, _>>()?,
            "discriminator": discriminator, "nullable": nullable,
        }),
    })
}

fn identity(ir: &ProtocolIr) -> String {
    format!(
        "{}export const hostProtocolIdentity = Object.freeze({{ name: {}, major: {}, revision: {}, schemaDigest: {} }} as const);\nexport const schemaDigest = hostProtocolIdentity.schemaDigest;\n",
        generated_header(),
        ts_string(&ir.identity.name),
        ir.identity.major,
        ts_string(&ir.identity.revision),
        ts_string(&ir.identity.schema_digest)
    )
}

fn metadata(ir: &ProtocolIr) -> Result<String, String> {
    let methods = ir.methods.values().map(|method| (method.name.clone(), json!({
        "paramsType": method.params_type, "resultType": method.result_type, "errors": method.errors,
        "features": method.features, "transports": method.transports, "scopes": method.scopes,
        "idempotency": method.idempotency, "stability": method.stability,
    }))).collect::<serde_json::Map<_, _>>();
    let notifications = ir
        .notifications
        .values()
        .map(|notification| {
            (
                notification.name.clone(),
                json!({
                    "paramsType": notification.params_type, "features": notification.features,
                    "transports": notification.transports, "scopes": notification.scopes,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let errors = ir
        .errors
        .values()
        .map(|error| {
            (
                error.name.clone(),
                json!({"code": error.code, "message": error.message, "dataType": error.data_type}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let event_classes = ir
        .event_classes
        .values()
        .map(|class| {
            (
                class.name.clone(),
                json!({
                    "schemaType": class.schema_type,
                    "feature": class.feature,
                    "scopes": class.scopes,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let value = json!({"methods": methods, "notifications": notifications, "errors": errors, "features": ir.features, "eventClasses": event_classes, "eventProfiles": ir.event_profiles});
    Ok(format!(
        "{}export const hostProtocolMetadata = {} as const;\nexport type HostMethodMetadata = (typeof hostProtocolMetadata.methods)[keyof typeof hostProtocolMetadata.methods];\nexport type HostEventClass = keyof typeof hostProtocolMetadata.eventClasses;\nexport type HostEventClassMetadata = (typeof hostProtocolMetadata.eventClasses)[HostEventClass];\nexport type HostEventProfile = keyof typeof hostProtocolMetadata.eventProfiles;\n",
        generated_header(),
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    ))
}

fn codecs(ir: &ProtocolIr) -> String {
    let mut out = generated_header();
    out.push_str("import type { HostErrorData, HostMethodMap, HostMethodName, HostNotificationEnvelope, HostNotificationName, HostRequestEnvelope, RequestId } from \"./types.js\";\n");
    out.push_str("import {\n");
    let parser_names = ir
        .methods
        .values()
        .flat_map(|method| [method.params_type.as_str(), method.result_type.as_str()])
        .chain(
            ir.notifications
                .values()
                .map(|notification| notification.params_type.as_str()),
        )
        .chain(ir.errors.values().map(|error| error.data_type.as_str()))
        .collect::<BTreeSet<_>>();
    let mut parsers = parser_names
        .into_iter()
        .map(|name| format!("  parse{name},"))
        .collect::<Vec<_>>();
    parsers.sort();
    out.push_str(&parsers.join("\n"));
    out.push_str("\n} from \"./validators.js\";\n\n");
    out.push_str("const textEncoder = new TextEncoder();\nconst textDecoder = new TextDecoder(\"utf-8\", { fatal: true });\n");
    out.push_str("type Parser = (value: unknown) => unknown;\nconst paramsParsers: Readonly<Record<HostMethodName, Parser>> = {\n");
    for method in ir.methods.values() {
        out.push_str(&format!(
            "  {}: parse{},\n",
            ts_string(&method.name),
            method.params_type
        ));
    }
    out.push_str("};\nconst resultParsers: Readonly<Record<HostMethodName, Parser>> = {\n");
    for method in ir.methods.values() {
        out.push_str(&format!(
            "  {}: parse{},\n",
            ts_string(&method.name),
            method.result_type
        ));
    }
    out.push_str(
        "};\nconst notificationParsers: Readonly<Record<HostNotificationName, Parser>> = {\n",
    );
    for notification in ir.notifications.values() {
        out.push_str(&format!(
            "  {}: parse{},\n",
            ts_string(&notification.name),
            notification.params_type
        ));
    }
    out.push_str("};\n\n");
    out.push_str("const errorDataParsers = new Map<number, Parser>([\n");
    for error in ir.errors.values() {
        out.push_str(&format!("  [{}, parse{}],\n", error.code, error.data_type));
    }
    out.push_str("]);\n\n");
    out.push_str(CODEC_RUNTIME);
    out
}

const CODEC_RUNTIME: &str = r#"export class HostCodecError extends Error { constructor(message: string) { super(message); this.name = "HostCodecError" } }
export class HostRpcRemoteError extends Error {
  readonly code: number;
  readonly data: HostErrorData;
  constructor(code: number, message: string, data: HostErrorData) { super(message); this.name = "HostRpcRemoteError"; this.code = code; this.data = data }
}

export function encodeHostRequest<M extends HostMethodName>(id: RequestId, method: M, params: HostMethodMap[M]["params"]): Uint8Array {
  const parsed = paramsParsers[method](params) as HostMethodMap[M]["params"];
  const envelope: HostRequestEnvelope<M> = { jsonrpc: "2.0", id, method, params: parsed };
  return textEncoder.encode(canonicalJson(envelope));
}

export function parseHostResponse<M extends HostMethodName>(method: M, expectedId: RequestId, bytes: Uint8Array): HostMethodMap[M]["result"] {
  const value = parseJson(bytes);
  if (!isRecord(value)) throw new HostCodecError("response must be an object");
  exactKeys(value, value.error === undefined ? ["jsonrpc", "id", "result"] : ["jsonrpc", "id", "error"]);
  if (value.jsonrpc !== "2.0" || value.id !== expectedId) throw new HostCodecError("response identity mismatch");
  if (value.error !== undefined) throw decodeRemoteError(value.error);
  return resultParsers[method](value.result) as HostMethodMap[M]["result"];
}

export function parseHostNotification(bytes: Uint8Array): HostNotificationEnvelope {
  const value = parseJson(bytes);
  if (!isRecord(value)) throw new HostCodecError("notification must be an object");
  exactKeys(value, ["jsonrpc", "method", "params"]);
  if (value.jsonrpc !== "2.0" || typeof value.method !== "string" || !(value.method in notificationParsers)) throw new HostCodecError("unknown notification");
  const method = value.method as HostNotificationName;
  const params = notificationParsers[method](value.params);
  return { jsonrpc: "2.0", method, params } as HostNotificationEnvelope;
}

export function canonicalJson(value: unknown): string { return JSON.stringify(canonicalize(value)) }
function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (isRecord(value)) { const output: Record<string, unknown> = {}; for (const key of Object.keys(value).sort()) output[key] = canonicalize(value[key]); return output; }
  return value;
}
function parseJson(bytes: Uint8Array): unknown { try { const value: unknown = JSON.parse(textDecoder.decode(bytes)); return value } catch (error) { throw new HostCodecError(`invalid JSON: ${String(error)}`) } }
function decodeRemoteError(value: unknown): HostRpcRemoteError {
  if (!isRecord(value)) throw new HostCodecError("error must be an object");
  exactKeys(value, ["code", "message", "data"]);
  if (typeof value.code !== "number" || !Number.isSafeInteger(value.code) || typeof value.message !== "string") throw new HostCodecError("malformed error");
  const parser = errorDataParsers.get(value.code);
  if (parser === undefined) throw new HostCodecError(`unknown error code ${value.code}`);
  return new HostRpcRemoteError(value.code, value.message, parser(value.data) as HostErrorData);
}
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value) }
function exactKeys(value: Record<string, unknown>, expected: readonly string[]): void {
  const actual = Object.keys(value).sort(); const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) throw new HostCodecError("unexpected envelope fields");
}
"#;

fn client(ir: &ProtocolIr) -> String {
    let mut out = generated_header();
    out.push_str("import { encodeHostRequest, HostCodecError, parseHostNotification, parseHostResponse } from \"./codecs.js\";\nimport { hostProtocolIdentity } from \"./identity.js\";\nimport { hostProtocolMetadata } from \"./metadata.js\";\nimport type { HostMethodMap, HostMethodName, HostNotificationEnvelope } from \"./types.js\";\nimport { parseRequestId } from \"./validators.js\";\n\n");
    out.push_str(CLIENT_RUNTIME_PREFIX);
    for method in ir.methods.values() {
        out.push_str(&client_method(method));
    }
    out.push_str(CLIENT_RUNTIME_SUFFIX);
    out
}

const CLIENT_RUNTIME_PREFIX: &str = r#"export interface HostRpcTransport {
  readonly profile: "stdio" | "http" | "memory";
  readonly stateful?: boolean;
  request(frame: Uint8Array, signal?: AbortSignal): Promise<Uint8Array>;
  notifications?(handler: (frame: Uint8Array) => void): () => void;
  close?(): Promise<void>;
}
export type RequestIdFactory = () => string;
export type HostNotificationHandler = (notification: HostNotificationEnvelope) => void;

export class HostRpcClient {
  readonly #transport: HostRpcTransport;
  readonly #idFactory: RequestIdFactory;
  readonly #notificationHandlers = new Set<HostNotificationHandler>();
  readonly #removeNotificationListener: (() => void) | undefined;
  #initialized = false;
  #negotiatedFeatures = new Set<string>();
  constructor(transport: HostRpcTransport, idFactory: RequestIdFactory = defaultRequestId) {
    this.#transport = transport;
    this.#idFactory = idFactory;
    this.#removeNotificationListener = transport.notifications?.((frame) => {
      const notification = parseHostNotification(frame);
      for (const handler of this.#notificationHandlers) handler(notification);
    });
  }
  onNotification(handler: HostNotificationHandler): () => void { this.#notificationHandlers.add(handler); return () => this.#notificationHandlers.delete(handler) }
  async call<M extends HostMethodName>(method: M, params: HostMethodMap[M]["params"], signal?: AbortSignal): Promise<HostMethodMap[M]["result"]> {
    if (this.#transport.stateful === true && method !== "initialize" && !this.#initialized) throw new HostCodecError("stateful transport must initialize first");
    if (this.#transport.stateful === true && method !== "initialize") {
      for (const feature of hostProtocolMetadata.methods[method].features) if (!this.#negotiatedFeatures.has(feature)) throw new HostCodecError(`method ${method} requires unnegotiated feature ${feature}`);
    }
    if (method === "initialize") {
      const requested = params as HostMethodMap["initialize"]["params"];
      assertCanonicalFeatureList(requested.supportedFeatures, "client supportedFeatures");
      assertCanonicalFeatureList(requested.requiredFeatures, "client requiredFeatures");
      if (requested.requiredFeatures.some((feature) => !requested.supportedFeatures.includes(feature))) throw new HostCodecError("required features must be included in supported features");
    }
    const id = parseRequestId(this.#idFactory());
    const response = await this.#transport.request(encodeHostRequest(id, method, params), signal);
    const result = parseHostResponse(method, id, response);
    if (method === "initialize") {
      const candidate = result as HostMethodMap["initialize"]["result"];
      if (candidate.protocol.name !== hostProtocolIdentity.name || candidate.protocol.major !== hostProtocolIdentity.major || candidate.protocol.revision !== hostProtocolIdentity.revision || candidate.protocol.schemaDigest !== hostProtocolIdentity.schemaDigest) throw new HostCodecError("host protocol identity or digest mismatch");
      const requested = params as HostMethodMap["initialize"]["params"];
      assertCanonicalFeatureList(candidate.supportedFeatures, "server supportedFeatures");
      assertCanonicalFeatureList(candidate.negotiatedFeatures, "negotiatedFeatures");
      const vocabulary = hostProtocolMetadata.features as readonly string[];
      if (candidate.supportedFeatures.some((feature) => !vocabulary.includes(feature))) throw new HostCodecError("server advertised an unknown protocol feature");
      const expected = requested.supportedFeatures.filter((feature) => candidate.supportedFeatures.includes(feature));
      if (!sameStrings(candidate.negotiatedFeatures, expected)) throw new HostCodecError("host negotiated feature intersection mismatch");
      if (requested.requiredFeatures.some((feature) => !candidate.negotiatedFeatures.includes(feature))) throw new HostCodecError("host omitted a required negotiated feature");
      this.#negotiatedFeatures = new Set(candidate.negotiatedFeatures);
      this.#initialized = true;
    }
    return result;
  }
"#;

const CLIENT_RUNTIME_SUFFIX: &str = r"  async close(): Promise<void> { this.#removeNotificationListener?.(); this.#notificationHandlers.clear(); await this.#transport.close?.() }
}
let requestSequence = 0n;
function defaultRequestId(): string { requestSequence += 1n; return `ts_${requestSequence.toString()}` }
function assertCanonicalFeatureList(values: readonly string[], label: string): void {
  const sorted = [...new Set(values)].sort();
  if (!sameStrings(values, sorted)) throw new HostCodecError(`${label} must be duplicate-free and canonically sorted`);
}
function sameStrings(left: readonly string[], right: readonly string[]): boolean { return left.length === right.length && left.every((value, index) => value === right[index]) }
";

fn client_method(method: &MethodIr) -> String {
    let name = ts_method_name(&method.name);
    format!(
        "  {name}(params: HostMethodMap[{}][\"params\"], signal?: AbortSignal): Promise<HostMethodMap[{}][\"result\"]> {{ return this.call({}, params, signal) }}\n",
        ts_string(&method.name),
        ts_string(&method.name),
        ts_string(&method.name)
    )
}

fn ts_method_name(method: &str) -> String {
    let mut parts = method.split('.');
    let mut output = parts.next().unwrap_or("call").to_string();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            output.push(first.to_ascii_uppercase());
            output.extend(chars);
        }
    }
    output
}

fn fetch_transport() -> String {
    r#"import type { HostRpcTransport } from "./generated/client.js";

export interface FetchHostTransportOptions {
  readonly endpoint: URL | string;
  readonly authorization?: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly maxResponseBytes?: number;
  readonly headers?: Readonly<Record<string, string>>;
}
export class FetchHostTransport implements HostRpcTransport {
  readonly profile = "http" as const;
  readonly stateful = false;
  readonly #options: FetchHostTransportOptions;
  constructor(options: FetchHostTransportOptions) { this.#options = options }
  async request(frame: Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
    const fetchImpl = this.#options.fetch ?? globalThis.fetch;
    const headers = new Headers(this.#options.headers);
    headers.set("content-type", "application/json");
    if (this.#options.authorization !== undefined) headers.set("authorization", this.#options.authorization);
    const response = await fetchImpl(this.#options.endpoint, { method: "POST", headers, body: Uint8Array.from(frame).buffer, signal: signal ?? null });
    if (!response.ok) throw new Error(`host HTTP transport returned ${response.status}`);
    const contentType = response.headers.get("content-type")?.split(";", 1)[0]?.trim();
    if (contentType !== "application/json") throw new Error("host HTTP transport returned a non-JSON content type");
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength > (this.#options.maxResponseBytes ?? 8 * 1024 * 1024)) throw new Error("host HTTP response exceeded the configured bound");
    return bytes;
  }
}
"#.to_string()
}

fn node_transport() -> String {
    r#"import type { Readable, Writable } from "node:stream";
import type { HostRpcTransport } from "./generated/client.js";

interface PendingRequest { readonly resolve: (frame: Uint8Array) => void; readonly reject: (error: Error) => void; readonly removeAbort: () => void }
export interface NodeNdjsonTransportOptions { readonly input: Readable; readonly output: Writable; readonly maxFrameBytes?: number }
export class NodeNdjsonTransport implements HostRpcTransport {
  readonly profile = "stdio" as const;
  readonly stateful = true;
  readonly #input: Readable;
  readonly #output: Writable;
  readonly #maxFrameBytes: number;
  readonly #pending = new Map<string, PendingRequest>();
  readonly #notificationHandlers = new Set<(frame: Uint8Array) => void>();
  #closed = false;
  #buffer = Buffer.alloc(0);
  constructor(options: NodeNdjsonTransportOptions) {
    this.#input = options.input; this.#output = options.output; this.#maxFrameBytes = options.maxFrameBytes ?? 8 * 1024 * 1024;
    this.#input.on("data", (chunk: Buffer | string) => this.#accept(typeof chunk === "string" ? Buffer.from(chunk) : chunk));
    this.#input.once("end", () => this.#failAll(new Error("host stream ended")));
    this.#input.once("error", (error) => this.#failAll(error));
    this.#output.once("error", (error) => this.#failAll(error));
  }
  notifications(handler: (frame: Uint8Array) => void): () => void { this.#notificationHandlers.add(handler); return () => this.#notificationHandlers.delete(handler) }
  request(frame: Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
    if (this.#closed) return Promise.reject(new Error("host stream is closed"));
    const id = requestId(frame);
    if (this.#pending.has(id)) return Promise.reject(new Error(`duplicate pending request id ${id}`));
    return new Promise((resolve, reject) => {
      const onAbort = (): void => { this.#pending.delete(id); reject(signal?.reason instanceof Error ? signal.reason : new Error("host request aborted")) };
      if (signal?.aborted === true) { onAbort(); return; }
      signal?.addEventListener("abort", onAbort, { once: true });
      const removeAbort = (): void => signal?.removeEventListener("abort", onAbort);
      this.#pending.set(id, { resolve, reject, removeAbort });
      const line = Buffer.concat([Buffer.from(frame), Buffer.from("\n")]);
      if (!this.#output.write(line)) this.#output.once("drain", () => undefined);
    });
  }
  async close(): Promise<void> { if (this.#closed) return; this.#closed = true; this.#output.end(); this.#failAll(new Error("host stream closed")) }
  #accept(chunk: Buffer): void {
    if (this.#closed) return;
    this.#buffer = Buffer.concat([this.#buffer, chunk]);
    if (this.#buffer.byteLength > this.#maxFrameBytes && !this.#buffer.includes(10)) { this.#failAll(new Error("host frame exceeded configured bound")); return; }
    for (;;) {
      const newline = this.#buffer.indexOf(10); if (newline < 0) return;
      const line = this.#buffer.subarray(0, newline); this.#buffer = this.#buffer.subarray(newline + 1);
      if (line.byteLength === 0) continue;
      if (line.byteLength > this.#maxFrameBytes) { this.#failAll(new Error("host frame exceeded configured bound")); return; }
      this.#route(new Uint8Array(line));
    }
  }
  #route(frame: Uint8Array): void {
    let value: unknown;
    try { value = JSON.parse(Buffer.from(frame).toString("utf8")) as unknown } catch { this.#failAll(new Error("host emitted malformed JSON")); return; }
    if (!isRecord(value)) { this.#failAll(new Error("host emitted a non-object frame")); return; }
    if (!("id" in value)) { for (const handler of this.#notificationHandlers) handler(frame); return; }
    if (typeof value.id !== "string") { this.#failAll(new Error("host response id is not a string")); return; }
    const pending = this.#pending.get(value.id);
    if (pending === undefined) { this.#failAll(new Error(`host returned unknown response id ${value.id}`)); return; }
    this.#pending.delete(value.id); pending.removeAbort(); pending.resolve(frame);
  }
  #failAll(error: Error): void { if (!this.#closed) this.#closed = true; for (const pending of this.#pending.values()) { pending.removeAbort(); pending.reject(error) } this.#pending.clear() }
}
function requestId(frame: Uint8Array): string {
  const value: unknown = JSON.parse(Buffer.from(frame).toString("utf8"));
  if (!isRecord(value) || typeof value.id !== "string" || value.id.length === 0) throw new Error("outgoing host request has no string id");
  return value.id;
}
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value) }
"#.to_string()
}

fn index() -> String {
    "export * from \"./generated/identity.js\";\nexport * from \"./generated/types.js\";\nexport * from \"./generated/metadata.js\";\nexport * from \"./generated/validators.js\";\nexport * from \"./generated/codecs.js\";\nexport * from \"./generated/client.js\";\n".to_string()
}
