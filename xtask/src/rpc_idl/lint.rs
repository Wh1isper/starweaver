use std::collections::{BTreeMap, BTreeSet};

use super::model::{ProtocolIr, SchemaKind};

pub fn check(ir: &ProtocolIr) -> Result<(), String> {
    if ir.identity.name != "starweaver.host" || ir.identity.major != 1 {
        return Err("protocol identity must be starweaver.host major 1".to_string());
    }
    if !ir.identity.schema_digest.starts_with("sha256:") || ir.identity.schema_digest.len() != 71 {
        return Err("protocol schema digest is not canonical SHA-256".to_string());
    }
    let mut codes = BTreeSet::new();
    let mut data_types = BTreeSet::new();
    for error in ir.errors.values() {
        if !codes.insert(error.code) {
            return Err(format!("duplicate public error code {}", error.code));
        }
        if !data_types.insert(&error.data_type) {
            return Err(format!(
                "duplicate public error data type {}",
                error.data_type
            ));
        }
        if !ir.schemas.contains_key(&error.data_type) {
            return Err(format!(
                "error {} references missing data schema {}",
                error.name, error.data_type
            ));
        }
    }
    for method in ir.methods.values() {
        if !ir.schemas.contains_key(&method.params_type)
            || !ir.schemas.contains_key(&method.result_type)
        {
            return Err(format!(
                "method {} references missing params/result schema",
                method.name
            ));
        }
        if method.transports.is_empty() || method.scopes.is_empty() {
            return Err(format!(
                "method {} has incomplete transport/scope metadata",
                method.name
            ));
        }
        if method.spec.is_empty() || method.stability.is_empty() {
            return Err(format!(
                "method {} has incomplete review metadata",
                method.name
            ));
        }
        if !method.errors.iter().any(|error| error == "InternalError") {
            return Err(format!(
                "method {} must declare InternalError for safe generated error conversion",
                method.name
            ));
        }
        for error in &method.errors {
            if !ir.errors.contains_key(error) {
                return Err(format!(
                    "method {} references missing error {error}",
                    method.name
                ));
            }
        }
        for feature in &method.features {
            if !ir.features.contains(feature) {
                return Err(format!(
                    "method {} references undeclared feature {feature}",
                    method.name
                ));
            }
        }
    }
    for notification in ir.notifications.values() {
        if !ir.schemas.contains_key(&notification.params_type) {
            return Err(format!(
                "notification {} references missing params schema",
                notification.name
            ));
        }
    }
    check_event_classes(ir)?;
    for schema in ir.schemas.values() {
        check_schema(ir, &schema.kind, &schema.name)?;
    }
    Ok(())
}

fn check_event_classes(ir: &ProtocolIr) -> Result<(), String> {
    let host_event = resolve_schema(ir, "HostEvent")?;
    let SchemaKind::OneOf { variants, .. } = host_event else {
        return Err("HostEvent must be a discriminated union".to_string());
    };
    let mut union_schemas = BTreeSet::new();
    for variant in variants {
        let SchemaKind::Ref(name) = variant else {
            return Err("HostEvent union variants must be component schema references".to_string());
        };
        if !union_schemas.insert(name.clone()) {
            return Err(format!("HostEvent union contains duplicate variant {name}"));
        }
    }

    let allowed_scopes = BTreeSet::from(["public", "read", "run", "approval", "admin", "shutdown"]);
    let mut registry_schemas = BTreeSet::new();
    let mut class_by_kind = BTreeMap::new();
    for class in ir.event_classes.values() {
        if !ir.schemas.contains_key(&class.schema_type) {
            return Err(format!(
                "event class {} references missing schema {}",
                class.name, class.schema_type
            ));
        }
        if !registry_schemas.insert(class.schema_type.clone()) {
            return Err(format!(
                "event schema {} is registered by more than one event class",
                class.schema_type
            ));
        }
        let kind = event_kind(ir, &class.schema_type)?;
        if kind != class.name {
            return Err(format!(
                "event class {} schema {} declares kind {kind}",
                class.name, class.schema_type
            ));
        }
        if class_by_kind.insert(kind, class.name.clone()).is_some() {
            return Err(format!("duplicate event class kind {}", class.name));
        }
        if let Some(feature) = &class.feature
            && !ir.features.contains(feature)
        {
            return Err(format!(
                "event class {} references undeclared feature {feature}",
                class.name
            ));
        }
        if class.scopes.is_empty() {
            return Err(format!(
                "event class {} has no authorization scopes",
                class.name
            ));
        }
        let mut scopes = BTreeSet::new();
        for scope in &class.scopes {
            if !allowed_scopes.contains(scope.as_str()) {
                return Err(format!(
                    "event class {} references unknown authorization scope {scope}",
                    class.name
                ));
            }
            if !scopes.insert(scope) {
                return Err(format!(
                    "event class {} repeats authorization scope {scope}",
                    class.name
                ));
            }
        }
    }
    if registry_schemas != union_schemas {
        let missing = union_schemas
            .difference(&registry_schemas)
            .cloned()
            .collect::<Vec<_>>();
        let unknown = registry_schemas
            .difference(&union_schemas)
            .cloned()
            .collect::<Vec<_>>();
        return Err(format!(
            "event class registry and HostEvent union differ (missing: {missing:?}, unknown: {unknown:?})"
        ));
    }

    let registry_classes = ir.event_classes.keys().cloned().collect::<BTreeSet<_>>();
    let mut profiled_classes = BTreeSet::new();
    for (profile, classes) in &ir.event_profiles {
        if classes.is_empty() {
            return Err(format!("event profile {profile} must not be empty"));
        }
        let mut unique = BTreeSet::new();
        for class in classes {
            if !registry_classes.contains(class) {
                return Err(format!(
                    "event profile {profile} references unknown event class {class}"
                ));
            }
            if !unique.insert(class) {
                return Err(format!(
                    "event profile {profile} repeats event class {class}"
                ));
            }
            profiled_classes.insert(class.clone());
        }
    }
    if profiled_classes != registry_classes {
        let missing = registry_classes
            .difference(&profiled_classes)
            .cloned()
            .collect::<Vec<_>>();
        return Err(format!(
            "event profiles omit registered event classes {missing:?}"
        ));
    }
    Ok(())
}

fn resolve_schema<'a>(ir: &'a ProtocolIr, name: &str) -> Result<&'a SchemaKind, String> {
    let mut current = name;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return Err(format!("schema reference cycle while resolving {name}"));
        }
        let schema = &ir
            .schemas
            .get(current)
            .ok_or_else(|| format!("missing schema {current}"))?
            .kind;
        match schema {
            SchemaKind::Ref(next) => current = next,
            other => return Ok(other),
        }
    }
}

fn event_kind(ir: &ProtocolIr, schema_name: &str) -> Result<String, String> {
    let SchemaKind::Object(object) = resolve_schema(ir, schema_name)? else {
        return Err(format!("event schema {schema_name} must be an object"));
    };
    let Some(kind_schema) = object.properties.get("kind") else {
        return Err(format!("event schema {schema_name} has no kind field"));
    };
    let SchemaKind::String(kind) = kind_schema else {
        return Err(format!(
            "event schema {schema_name} kind must be a string const"
        ));
    };
    kind.const_value
        .clone()
        .ok_or_else(|| format!("event schema {schema_name} kind must be a string const"))
}

fn check_schema(ir: &ProtocolIr, schema: &SchemaKind, context: &str) -> Result<(), String> {
    match schema {
        SchemaKind::Ref(name) => {
            if !ir.schemas.contains_key(name) {
                return Err(format!("schema {context} references missing schema {name}"));
            }
        }
        SchemaKind::String(value) => {
            if value.max_length == usize::MAX
                && value.enum_values.is_empty()
                && value.const_value.is_none()
            {
                return Err(format!("schema {context} contains an unbounded string"));
            }
            if let Some(decimal) = &value.decimal
                && (decimal.kind != "u64"
                    || decimal.minimum != "0"
                    || decimal.maximum != "18446744073709551615")
            {
                return Err(format!(
                    "schema {context} has an unsupported decimal domain"
                ));
            }
        }
        SchemaKind::Integer { minimum, maximum } if minimum > maximum => {
            return Err(format!("schema {context} has an invalid integer range"));
        }
        SchemaKind::Object(object) => {
            for required in &object.required {
                if !object.properties.contains_key(required) {
                    return Err(format!(
                        "schema {context} requires unknown field {required}"
                    ));
                }
            }
            for (name, field) in &object.properties {
                check_schema(ir, field, &format!("{context}.{name}"))?;
            }
        }
        SchemaKind::Array {
            items,
            min_items,
            max_items,
            unique,
            canonical_utf8_byte_order,
        } => {
            if min_items > max_items {
                return Err(format!("schema {context} has invalid array bounds"));
            }
            if *canonical_utf8_byte_order {
                if !unique {
                    return Err(format!(
                        "schema {context} canonical ordered array must also require unique items"
                    ));
                }
                if !schema_resolves_to_string(ir, items, &mut BTreeSet::new()) {
                    return Err(format!(
                        "schema {context} canonical ordered array items must resolve to strings"
                    ));
                }
            }
            check_schema(ir, items, context)?;
        }
        SchemaKind::OneOf {
            variants,
            discriminator,
            nullable,
        } => {
            if !nullable && discriminator.as_deref() != Some("kind") {
                return Err(format!(
                    "schema {context} union must use the kind discriminator"
                ));
            }
            for variant in variants {
                check_schema(ir, variant, context)?;
            }
        }
        SchemaKind::Integer { .. }
        | SchemaKind::Boolean
        | SchemaKind::Null
        | SchemaKind::JsonObject => {}
    }
    Ok(())
}

fn schema_resolves_to_string(
    ir: &ProtocolIr,
    schema: &SchemaKind,
    visiting: &mut BTreeSet<String>,
) -> bool {
    match schema {
        SchemaKind::String(_) => true,
        SchemaKind::Ref(name) if visiting.insert(name.clone()) => {
            let resolved = ir
                .schemas
                .get(name)
                .is_some_and(|schema| schema_resolves_to_string(ir, &schema.kind, visiting));
            visiting.remove(name);
            resolved
        }
        _ => false,
    }
}
