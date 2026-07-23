use std::{collections::BTreeMap, fs, path::PathBuf};

use serde_json::json;

use super::{
    bundle,
    check::GeneratedOutputs,
    generate_desktop, generate_rust, generate_typescript, lint,
    model::{ProtocolIr, SchemaKind},
    source,
};

#[test]
fn source_rejects_duplicate_keys_forbidden_yaml_and_path_escape() {
    let Ok(directory) = tempfile::tempdir() else {
        panic!("temporary directory must be available");
    };
    let duplicate = directory.path().join("duplicate.yaml");
    assert!(fs::write(&duplicate, "{\"key\": 1, \"key\": 2}\n").is_ok());
    assert!(source::load_yaml(&duplicate).is_err());

    for (name, source_text) in [
        ("multi.yaml", "---\nvalue: 1\n---\nvalue: 2\n"),
        ("anchor.yaml", "value: &shared 1\nother: *shared\n"),
        ("merge.yaml", "value:\n  <<: {nested: true}\n"),
    ] {
        let path = directory.path().join(name);
        assert!(fs::write(&path, source_text).is_ok());
        assert!(source::load_yaml(&path).is_err());
    }

    let protocol_root = directory.path().join("protocol");
    let source_path = protocol_root.join("schemas/source.yaml");
    assert!(
        source::resolve_local_path(&protocol_root, &source_path, "../../secret.json#/x").is_err()
    );
    assert!(
        source::resolve_local_path(&protocol_root, &source_path, "https://example.test/x#/x")
            .is_err()
    );
}

#[test]
fn normalized_ir_rejects_unknown_schema_keywords() {
    let mut bundle = minimal_bundle();
    bundle["components"]["schemas"]["EmptyParams"]["unevaluatedProperties"] = json!(false);
    let Err(error) = ProtocolIr::from_bundle(&bundle) else {
        panic!("unknown schema keyword must fail");
    };
    assert!(error.contains("unsupported schema keyword"));
}

#[test]
fn arbitrary_json_object_requires_the_explicit_narrow_marker() {
    let mut bundle = minimal_bundle();
    bundle["components"]["schemas"]["JsonObject"] = json!({
        "type": "object",
        "additionalProperties": true,
        "x-starweaver-json-value": "object"
    });
    let Ok(ir) = ProtocolIr::from_bundle(&bundle) else {
        panic!("explicit arbitrary JSON object must lower");
    };
    assert!(matches!(
        ir.schemas["JsonObject"].kind,
        SchemaKind::JsonObject
    ));

    bundle["components"]["schemas"]["JsonObject"] = json!({
        "type": "object",
        "additionalProperties": true
    });
    assert!(matches!(
        ProtocolIr::from_bundle(&bundle),
        Err(error) if error.contains("objects must set additionalProperties: false")
    ));
}

#[test]
fn canonical_bundle_is_deterministic_and_has_only_internal_refs() {
    let Ok(repository) = crate::common::root() else {
        panic!("repository root must resolve");
    };
    let protocol_root = repository.join("protocol/host");
    let Ok(first) = bundle::build(&protocol_root) else {
        panic!("canonical protocol must bundle");
    };
    let Ok(second) = bundle::build(&protocol_root) else {
        panic!("canonical protocol must bundle twice");
    };
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.digest, second.digest);
    assert!(first.digest.starts_with("sha256:"));
    assert!(!String::from_utf8_lossy(&first.bytes).contains("./schemas/"));
}

#[test]
fn event_class_registry_lint_is_closed_across_union_and_profiles() {
    let Ok(repository) = crate::common::root() else {
        panic!("repository root must resolve");
    };
    let Ok(bundled) = bundle::build(&repository.join("protocol/host")) else {
        panic!("canonical protocol must bundle");
    };
    let Ok(ir) = ProtocolIr::from_bundle(&bundled.value) else {
        panic!("canonical protocol must lower to IR");
    };
    assert!(lint::check(&ir).is_ok());

    let mut missing_registry_entry = ir.clone();
    missing_registry_entry.event_classes.remove("diagnostic");
    assert!(matches!(
        lint::check(&missing_registry_entry),
        Err(error) if error.contains("registry and HostEvent union differ")
    ));

    let mut unknown_profile_entry = ir.clone();
    let Some(profile) = unknown_profile_entry
        .event_profiles
        .get_mut("operations.v1")
    else {
        panic!("canonical profile must exist");
    };
    profile.push("unknown_event".to_string());
    assert!(matches!(
        lint::check(&unknown_profile_entry),
        Err(error) if error.contains("unknown event class unknown_event")
    ));

    let mut omitted_from_profiles = ir.clone();
    let Some(profile) = omitted_from_profiles
        .event_profiles
        .get_mut("operations.v1")
    else {
        panic!("canonical profile must exist");
    };
    profile.retain(|class| class != "diagnostic");
    assert!(matches!(
        lint::check(&omitted_from_profiles),
        Err(error) if error.contains("event profiles omit registered event classes")
    ));

    let mut incomplete_union = ir;
    let Some(host_event) = incomplete_union.schemas.get_mut("HostEvent") else {
        panic!("HostEvent schema must exist");
    };
    let SchemaKind::OneOf { variants, .. } = &mut host_event.kind else {
        panic!("HostEvent must lower to a oneOf union");
    };
    variants.pop();
    assert!(matches!(
        lint::check(&incomplete_union),
        Err(error) if error.contains("registry and HostEvent union differ")
    ));
}

#[test]
fn rust_generator_owns_decode_errors_notifications_and_error_eligibility() {
    let Ok(repository) = crate::common::root() else {
        panic!("repository root must resolve");
    };
    let Ok(bundled) = bundle::build(&repository.join("protocol/host")) else {
        panic!("canonical protocol must bundle");
    };
    let Ok(ir) = ProtocolIr::from_bundle(&bundled.value) else {
        panic!("canonical protocol must lower to IR");
    };
    let Ok(files) = generate_rust::generate(&ir) else {
        panic!("canonical Rust protocol must generate");
    };
    let envelope_path = PathBuf::from("crates/starweaver-rpc-core/src/generated/envelope.rs");
    let metadata_path = PathBuf::from("crates/starweaver-rpc-core/src/generated/metadata.rs");
    let errors_path = PathBuf::from("crates/starweaver-rpc-core/src/generated/errors.rs");
    let types_path = PathBuf::from("crates/starweaver-rpc-core/src/generated/types.rs");
    let Some(envelope) = files
        .get(&envelope_path)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        panic!("generated envelope must be UTF-8");
    };
    assert!(envelope.contains("pub struct DecodeRequestError"));
    assert!(envelope.contains("pub enum HostNotificationParams"));
    assert!(envelope.contains("pub fn encode_notification_frame"));
    assert!(envelope.contains("HostNotificationParams::HostEvent"));
    assert!(envelope.contains("HostNotificationParams::SubscriptionClosed"));

    let client_path = PathBuf::from("crates/starweaver-rpc-core/src/generated/client.rs");
    let Some(client) = files
        .get(&client_path)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        panic!("generated client must be UTF-8");
    };
    assert!(client.contains("pub fn encode_request_frame"));
    assert!(client.contains("pub fn decode_server_frame"));
    assert!(client.contains("pub enum HostResult"));
    assert!(client.contains("HostResult::Initialize"));
    assert!(client.contains("HostResult::RunStart"));
    assert!(client.contains("pub fn decode_launch_envelope"));
    assert!(client.contains("InvalidRemoteError"));

    let Some(metadata) = files
        .get(&metadata_path)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        panic!("generated metadata must be UTF-8");
    };
    assert!(metadata.contains("name: \"host.event\""));
    assert!(metadata.contains("name: \"subscription.closed\""));
    assert!(metadata.contains("pub enum EventClass"));
    assert!(metadata.contains("pub const EVENT_CLASSES"));
    assert!(metadata.contains("schema_type: \"ApprovalChangedEvent\""));
    assert!(metadata.contains("feature: Some(\"hitl\")"));
    assert!(metadata.contains("pub const EVENT_PROFILES"));
    assert!(metadata.contains("event_classes: &'static [EventClass]"));
    assert!(metadata.contains("EventClass::RunChanged"));
    assert!(metadata.contains("profile: EventProfile::ConversationV1"));
    assert!(metadata.contains("profile: EventProfile::DesktopConversationV1"));
    assert!(metadata.contains("profile: EventProfile::OperationsV1"));
    assert!(metadata.contains("pub fn allows_event_class"));
    assert!(metadata.contains("pub fn is_admitted"));

    let Some(errors) = files
        .get(&errors_path)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        panic!("generated errors must be UTF-8");
    };
    assert!(errors.contains("impl From<HostError> for InitializeError"));
    assert!(errors.contains("message: \"internal error\".to_string()"));
    assert!(errors.contains("reconciliation_required: true"));

    let Some(types) = files
        .get(&types_path)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        panic!("generated types must be UTF-8");
    };
    assert!(types.contains("pub type JsonObject = Value"));
    assert!(types.contains("#[serde(deserialize_with = \"deserialize_json_object\")]"));
    assert!(types.contains("pub input_schema: JsonObject"));

    let mut invalid_ir = ir;
    let Some(method) = invalid_ir.methods.values_mut().next() else {
        panic!("canonical protocol must contain methods");
    };
    method.errors.retain(|error| error != "InternalError");
    assert!(matches!(
        lint::check(&invalid_ir),
        Err(error) if error.contains("must declare InternalError")
    ));
}

#[test]
fn desktop_manifest_rejects_authority_confusion_and_unreviewed_surface() {
    let Ok(repository) = crate::common::root() else {
        panic!("repository root must resolve");
    };
    let Ok(bundled) = bundle::build(&repository.join("protocol/host")) else {
        panic!("canonical protocol must bundle");
    };
    let Ok(ir) = ProtocolIr::from_bundle(&bundled.value) else {
        panic!("canonical protocol must lower to IR");
    };
    let Ok(manifest) =
        source::load_yaml(&repository.join("apps/starweaver-desktop/host-bridge/manifest.yaml"))
    else {
        panic!("Desktop surface manifest must load");
    };
    assert!(generate_desktop::validate_manifest_value(&ir, manifest.clone()).is_ok());
    let Some(operations) = manifest["operations"].as_object() else {
        panic!("Desktop operations object");
    };
    for backend_owned in [
        "initialize",
        "shutdown",
        "events.replay",
        "events.subscribe",
        "events.unsubscribe",
        "environment.attach",
    ] {
        assert!(!operations.contains_key(backend_owned));
    }

    let mut duplicate_authority = manifest.clone();
    let Some(renderer_fields) = duplicate_authority
        .pointer_mut("/operations/run.start/input/rendererProvided")
        .and_then(serde_json::Value::as_array_mut)
    else {
        panic!("run.start renderer authority list must exist");
    };
    renderer_fields.push(json!("idempotencyKey"));
    assert!(generate_desktop::validate_manifest_value(&ir, duplicate_authority).is_err());

    let mut unknown_operation = manifest.clone();
    let Some(operations) = unknown_operation
        .get_mut("operations")
        .and_then(serde_json::Value::as_object_mut)
    else {
        panic!("Desktop operations must be an object");
    };
    let Some(catalog_operation) = operations.get("catalog.list").cloned() else {
        panic!("catalog.list surface must exist");
    };
    operations.insert("diagnostics.get".to_string(), catalog_operation);
    assert!(generate_desktop::validate_manifest_value(&ir, unknown_operation).is_err());

    let mut unauthorized_profile = manifest.clone();
    unauthorized_profile["notifications"]["host.event"]["profile"] =
        json!("internal.everything.v1");
    assert!(generate_desktop::validate_manifest_value(&ir, unauthorized_profile).is_err());

    let mut raw_resource_authority = manifest.clone();
    let Some(allowed) = raw_resource_authority
        .pointer_mut("/rendererInputVariants/InputPart")
        .and_then(serde_json::Value::as_array_mut)
    else {
        panic!("InputPart renderer variant allowlist must exist");
    };
    allowed.push(json!("ResourceInputPart"));
    assert!(generate_desktop::validate_manifest_value(&ir, raw_resource_authority).is_err());

    let Ok(outputs) = generate_desktop::generate(&repository, &ir) else {
        panic!("Desktop outputs must generate");
    };
    let Some(types) = outputs.get(std::path::Path::new(
        "apps/starweaver-desktop/src/generated/host/types.ts",
    )) else {
        panic!("generated Desktop TypeScript types must exist");
    };
    let types = String::from_utf8_lossy(types);
    assert!(!types.contains("readonly uri:"));
    assert!(!types.contains("readonly kind: \"resource\""));
    let Some(rust) = outputs.get(std::path::Path::new(
        "apps/starweaver-desktop/src-tauri/src/generated/host/mod.rs",
    )) else {
        panic!("generated Desktop Rust bridge must exist");
    };
    let rust = String::from_utf8_lossy(rust);
    assert!(!rust.contains("ResourceInputPart"));
    assert!(!rust.contains("#[serde(rename = \"resource\")]"));

    let mut incomplete = manifest;
    let Some(input) = incomplete
        .pointer_mut("/operations/session.get/input")
        .and_then(serde_json::Value::as_object_mut)
    else {
        panic!("session.get input authority must exist");
    };
    input.remove("forbidden");
    assert!(generate_desktop::validate_manifest_value(&ir, incomplete).is_err());
}

#[test]
fn explicit_typescript_generation_is_isolated_and_repeatable() {
    let Ok(directory) = tempfile::tempdir() else {
        panic!("temporary directory must be available");
    };
    let output = directory.path().join("host-bindings");
    let args = vec![
        "--output".to_string(),
        output.to_string_lossy().into_owned(),
    ];
    assert!(generate_typescript::generate_to(&args).is_ok());
    assert!(output.join("index.ts").is_file());
    assert!(output.join("generated/client.ts").is_file());
    let Ok(types) = fs::read_to_string(output.join("generated/types.ts")) else {
        panic!("generated TypeScript types must be readable");
    };
    assert!(types.contains("export type JsonObject = Readonly<Record<string, unknown>>"));
    let Ok(metadata) = fs::read_to_string(output.join("generated/metadata.ts")) else {
        panic!("generated TypeScript metadata must be readable");
    };
    assert!(metadata.contains("\"eventClasses\""));
    assert!(metadata.contains("export type HostEventClass"));
    let Ok(validators) = fs::read_to_string(output.join("generated/validators.ts")) else {
        panic!("generated TypeScript validators must be readable");
    };
    assert!(validators.contains("canonicalUtf8ByteOrder"));
    assert!(validators.contains("compareUtf8Bytes"));
    assert!(validators.contains("strict ascending UTF-8 byte order"));
    assert!(
        output
            .join(".starweaver-host-typescript-generated")
            .is_file()
    );
    assert!(!output.join("package.json").exists());
    assert!(generate_typescript::generate_to(&args).is_ok());

    let unmanaged = directory.path().join("unmanaged");
    assert!(fs::create_dir(&unmanaged).is_ok());
    assert!(fs::write(unmanaged.join("keep.txt"), "keep").is_ok());
    let unmanaged_args = vec![
        "--output".to_string(),
        unmanaged.to_string_lossy().into_owned(),
    ];
    assert!(generate_typescript::generate_to(&unmanaged_args).is_err());
    assert!(unmanaged.join("keep.txt").is_file());
}

#[test]
fn generated_output_check_rejects_stale_files_and_promotion_removes_only_owned_files() {
    let Ok(repository) = tempfile::tempdir() else {
        panic!("temporary directory must be available");
    };
    let current = PathBuf::from("crates/starweaver-rpc-core/src/generated/current.rs");
    let stale = [
        "crates/starweaver-rpc-core/src/generated/obsolete.rs",
        "apps/starweaver-desktop/src/generated/host/obsolete.ts",
        "apps/starweaver-desktop/src-tauri/src/generated/host/obsolete.rs",
        "protocol/host/generated/obsolete.json",
    ];
    for relative in stale {
        let path = repository.path().join(relative);
        let Some(parent) = path.parent() else {
            panic!("stale output must have a parent");
        };
        assert!(fs::create_dir_all(parent).is_ok());
        assert!(fs::write(path, b"obsolete generated output").is_ok());
    }
    let permission = repository
        .path()
        .join("apps/starweaver-desktop/src-tauri/permissions/autogenerated/obsolete_host.toml");
    let Some(permission_parent) = permission.parent() else {
        panic!("permission must have a parent");
    };
    assert!(fs::create_dir_all(permission_parent).is_ok());
    assert!(
        fs::write(
            &permission,
            b"# @generated by `cargo run -p xtask -- generate-rpc-idl`; do not edit.\n",
        )
        .is_ok()
    );
    let capability = repository
        .path()
        .join("apps/starweaver-desktop/src-tauri/capabilities/obsolete-host.json");
    let Some(capability_parent) = capability.parent() else {
        panic!("capability must have a parent");
    };
    assert!(fs::create_dir_all(capability_parent).is_ok());
    assert!(
        fs::write(
            &capability,
            b"{\"description\":\"Generated least-authority host operation fragment\"}\n",
        )
        .is_ok()
    );

    let legitimate_permission = permission.with_file_name("get_desktop_status.toml");
    let legitimate_capability = capability.with_file_name("default.json");
    let legitimate_generated_parent = repository
        .path()
        .join("apps/starweaver-desktop/src-tauri/src/generated/mod.rs");
    assert!(fs::write(&legitimate_permission, b"# Tauri-generated permission\n").is_ok());
    assert!(fs::write(&legitimate_capability, b"{\"identifier\":\"main\"}\n").is_ok());
    let Some(generated_parent) = legitimate_generated_parent.parent() else {
        panic!("generated module must have a parent");
    };
    assert!(fs::create_dir_all(generated_parent).is_ok());
    assert!(fs::write(&legitimate_generated_parent, b"pub mod host;\n").is_ok());

    let Some(current_parent) = current.parent() else {
        panic!("current output must have a parent");
    };
    assert!(fs::create_dir_all(repository.path().join(current_parent)).is_ok());
    assert!(fs::write(repository.path().join(&current), b"current").is_ok());
    let outputs = GeneratedOutputs {
        files: BTreeMap::from([(current.clone(), b"current".to_vec())]),
    };
    let Err(error) = outputs.check(repository.path()) else {
        panic!("obsolete generated output must fail the drift check");
    };
    assert!(error.contains("obsolete generated RPC IDL output"));
    assert!(error.contains("obsolete.rs"));
    assert!(permission.is_file(), "the check must remain read-only");

    assert!(outputs.promote(repository.path()).is_ok());
    for relative in stale {
        assert!(!repository.path().join(relative).exists());
    }
    assert!(!permission.exists());
    assert!(!capability.exists());
    assert!(legitimate_permission.is_file());
    assert!(legitimate_capability.is_file());
    assert!(legitimate_generated_parent.is_file());
    assert!(matches!(
        fs::read(repository.path().join(current)),
        Ok(bytes) if bytes == b"current"
    ));
}

#[test]
fn promotion_failure_restores_existing_files_and_removes_new_files() {
    let Ok(repository) = tempfile::tempdir() else {
        panic!("temporary directory must be available");
    };
    let existing = PathBuf::from("a/existing.txt");
    let created = PathBuf::from("b/created.txt");
    assert!(fs::create_dir_all(repository.path().join("a")).is_ok());
    assert!(fs::write(repository.path().join(&existing), b"before").is_ok());
    let outputs = GeneratedOutputs {
        files: BTreeMap::from([
            (existing.clone(), b"after".to_vec()),
            (created.clone(), b"created".to_vec()),
        ]),
    };
    assert!(
        outputs
            .promote_with_failure_injection(repository.path(), Some(1))
            .is_err()
    );
    assert!(matches!(
        fs::read(repository.path().join(existing)),
        Ok(bytes) if bytes == b"before"
    ));
    assert!(!repository.path().join(created).exists());
}

#[test]
fn promotion_failure_restores_obsolete_generated_files() {
    let Ok(repository) = tempfile::tempdir() else {
        panic!("temporary directory must be available");
    };
    let root = PathBuf::from("crates/starweaver-rpc-core/src/generated");
    let obsolete = root.join("a_obsolete.rs");
    let current = root.join("z_current.rs");
    assert!(fs::create_dir_all(repository.path().join(&root)).is_ok());
    assert!(fs::write(repository.path().join(&obsolete), b"obsolete").is_ok());
    assert!(fs::write(repository.path().join(&current), b"before").is_ok());
    let outputs = GeneratedOutputs {
        files: BTreeMap::from([(current.clone(), b"after".to_vec())]),
    };
    assert!(
        outputs
            .promote_with_failure_injection(repository.path(), Some(1))
            .is_err()
    );
    assert!(matches!(
        fs::read(repository.path().join(obsolete)),
        Ok(bytes) if bytes == b"obsolete"
    ));
    assert!(matches!(
        fs::read(repository.path().join(current)),
        Ok(bytes) if bytes == b"before"
    ));
}

fn minimal_bundle() -> serde_json::Value {
    json!({
        "x-starweaver-protocol": {
            "name": "starweaver.host",
            "major": 1,
            "revision": "test",
            "schemaDigest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        },
        "components": {
            "schemas": {
                "EmptyParams": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {},
                    "required": []
                },
                "EmptyResult": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {},
                    "required": []
                }
            },
            "errors": {}
        },
        "x-starweaver-error-data": {},
        "methods": [],
        "x-starweaver-notifications": [],
        "x-starweaver-features": [],
        "x-starweaver-event-classes": {},
        "x-starweaver-event-profiles": {}
    })
}
