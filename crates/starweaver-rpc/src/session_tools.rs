use std::{collections::BTreeSet, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use starweaver_session::SessionRecord;
use starweaver_tools::{DeferredToolSpec, DeferredToolset, DynToolset};

use crate::{RpcHostError, RpcHostResult};

pub const RPC_DEFERRED_TOOLSET_METADATA_KEY: &str = "rpc.deferred_toolset";
const RPC_DEFERRED_TOOLSET_VERSION: u32 = 1;
const MAX_DEFERRED_TOOLS: usize = 64;
const MAX_DEFERRED_TOOLSET_BYTES: usize = 256 * 1024;
const MAX_TOOL_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_TOOL_INSTRUCTION_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredToolsetBindingSummary {
    pub binding_id: String,
    pub digest: String,
    pub tool_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableDeferredToolsetBinding {
    version: u32,
    binding_id: String,
    digest: String,
    tools: Vec<DeferredToolDefinition>,
}

pub fn bind_deferred_tools(
    session: &mut SessionRecord,
    definitions: Vec<DeferredToolDefinition>,
) -> RpcHostResult<Option<DeferredToolsetBindingSummary>> {
    if definitions.is_empty() {
        session.metadata.remove(RPC_DEFERRED_TOOLSET_METADATA_KEY);
        return Ok(None);
    }
    validate_deferred_tools(&definitions)?;
    let digest = deferred_tools_digest(&definitions)?;
    let binding_id = format!("rpc.deferred.{digest}");
    let binding = DurableDeferredToolsetBinding {
        version: RPC_DEFERRED_TOOLSET_VERSION,
        binding_id,
        digest,
        tools: definitions,
    };
    let value = serde_json::to_value(&binding)
        .map_err(|error| RpcHostError::Invalid(format!("invalid deferred toolset: {error}")))?;
    session
        .metadata
        .insert(RPC_DEFERRED_TOOLSET_METADATA_KEY.to_string(), value);
    Ok(Some(binding_summary(&binding)))
}

pub fn deferred_toolset_for_session(session: &SessionRecord) -> RpcHostResult<Option<DynToolset>> {
    let Some(value) = session.metadata.get(RPC_DEFERRED_TOOLSET_METADATA_KEY) else {
        return Ok(None);
    };
    let binding = serde_json::from_value::<DurableDeferredToolsetBinding>(value.clone()).map_err(
        |error| RpcHostError::Invalid(format!("invalid durable deferred toolset: {error}")),
    )?;
    validate_binding(&binding)?;
    let specs = binding
        .tools
        .into_iter()
        .map(|definition| DeferredToolSpec {
            name: definition.name,
            description: definition.description,
            parameters: definition.input_schema,
            instructions: definition.instructions,
        });
    Ok(Some(Arc::new(DeferredToolset::from_specs(
        binding.binding_id,
        specs,
    ))))
}

pub fn deferred_toolset_summary(
    session: &SessionRecord,
) -> RpcHostResult<Option<DeferredToolsetBindingSummary>> {
    let Some(value) = session.metadata.get(RPC_DEFERRED_TOOLSET_METADATA_KEY) else {
        return Ok(None);
    };
    let binding = serde_json::from_value::<DurableDeferredToolsetBinding>(value.clone()).map_err(
        |error| RpcHostError::Invalid(format!("invalid durable deferred toolset: {error}")),
    )?;
    validate_binding(&binding)?;
    Ok(Some(binding_summary(&binding)))
}

fn validate_binding(binding: &DurableDeferredToolsetBinding) -> RpcHostResult<()> {
    if binding.version != RPC_DEFERRED_TOOLSET_VERSION {
        return Err(RpcHostError::Invalid(format!(
            "unsupported deferred toolset version: {}",
            binding.version
        )));
    }
    validate_deferred_tools(&binding.tools)?;
    let digest = deferred_tools_digest(&binding.tools)?;
    if binding.digest != digest || binding.binding_id != format!("rpc.deferred.{digest}") {
        return Err(RpcHostError::Invalid(
            "deferred toolset binding digest does not match its definitions".to_string(),
        ));
    }
    Ok(())
}

fn validate_deferred_tools(definitions: &[DeferredToolDefinition]) -> RpcHostResult<()> {
    if definitions.len() > MAX_DEFERRED_TOOLS {
        return Err(RpcHostError::Invalid(format!(
            "session deferredTools exceeds the maximum of {MAX_DEFERRED_TOOLS}"
        )));
    }
    let encoded = serde_json::to_vec(definitions)
        .map_err(|error| RpcHostError::Invalid(format!("invalid deferredTools: {error}")))?;
    if encoded.len() > MAX_DEFERRED_TOOLSET_BYTES {
        return Err(RpcHostError::Invalid(format!(
            "session deferredTools exceeds {MAX_DEFERRED_TOOLSET_BYTES} bytes"
        )));
    }
    let mut names = BTreeSet::new();
    for definition in definitions {
        if !valid_tool_name(&definition.name) {
            return Err(RpcHostError::Invalid(format!(
                "invalid deferred tool name: {:?}",
                definition.name
            )));
        }
        if !names.insert(definition.name.as_str()) {
            return Err(RpcHostError::Invalid(format!(
                "duplicate deferred tool name: {}",
                definition.name
            )));
        }
        if definition.description.trim().is_empty()
            || definition.description.len() > MAX_TOOL_DESCRIPTION_BYTES
        {
            return Err(RpcHostError::Invalid(format!(
                "deferred tool {} requires a non-empty description no larger than {MAX_TOOL_DESCRIPTION_BYTES} bytes",
                definition.name
            )));
        }
        let Some(schema) = definition.input_schema.as_object() else {
            return Err(RpcHostError::Invalid(format!(
                "deferred tool {} inputSchema must be a JSON object schema",
                definition.name
            )));
        };
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(RpcHostError::Invalid(format!(
                "deferred tool {} inputSchema.type must be object",
                definition.name
            )));
        }
        for instruction in &definition.instructions {
            if instruction.trim().is_empty() || instruction.len() > MAX_TOOL_INSTRUCTION_BYTES {
                return Err(RpcHostError::Invalid(format!(
                    "deferred tool {} instructions must be non-empty and no larger than {MAX_TOOL_INSTRUCTION_BYTES} bytes each",
                    definition.name
                )));
            }
        }
    }
    Ok(())
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn deferred_tools_digest(definitions: &[DeferredToolDefinition]) -> RpcHostResult<String> {
    let encoded = serde_json::to_vec(&json!({
        "version": RPC_DEFERRED_TOOLSET_VERSION,
        "tools": definitions,
    }))
    .map_err(|error| RpcHostError::Invalid(format!("invalid deferredTools: {error}")))?;
    let mut hasher = Sha256::new();
    hasher.update(b"starweaver.rpc.deferred-toolset/v1");
    hasher.update([0]);
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn binding_summary(binding: &DurableDeferredToolsetBinding) -> DeferredToolsetBindingSummary {
    DeferredToolsetBindingSummary {
        binding_id: binding.binding_id.clone(),
        digest: binding.digest.clone(),
        tool_names: binding
            .tools
            .iter()
            .map(|definition| definition.name.clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use starweaver_core::SessionId;

    fn definition(name: &str) -> DeferredToolDefinition {
        DeferredToolDefinition {
            name: name.to_string(),
            description: format!("Execute {name} in the client"),
            input_schema: json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            instructions: vec![format!("Use {name} when external client work is required.")],
        }
    }

    #[test]
    fn durable_binding_round_trips_to_a_session_toolset() {
        let mut session = SessionRecord::new(SessionId::new());
        let summary = bind_deferred_tools(&mut session, vec![definition("client_lookup")])
            .unwrap()
            .unwrap();
        let restored = deferred_toolset_for_session(&session).unwrap().unwrap();

        assert_eq!(summary.tool_names, vec!["client_lookup"]);
        assert_eq!(restored.id(), Some(summary.binding_id.as_str()));
        assert_eq!(restored.get_tools()[0].name(), "client_lookup");
        assert!(restored.get_instructions()[0].dynamic);
    }

    #[test]
    fn rejects_duplicate_or_non_object_definitions() {
        let duplicate = vec![definition("same"), definition("same")];
        assert!(validate_deferred_tools(&duplicate).is_err());

        let mut invalid = definition("invalid");
        invalid.input_schema = json!({"type": "string"});
        assert!(validate_deferred_tools(&[invalid]).is_err());
    }

    #[test]
    fn rejects_tampered_durable_binding() {
        let mut session = SessionRecord::new(SessionId::new());
        bind_deferred_tools(&mut session, vec![definition("client_lookup")]).unwrap();
        session.metadata[RPC_DEFERRED_TOOLSET_METADATA_KEY]["tools"][0]["description"] =
            json!("tampered");

        assert!(deferred_toolset_for_session(&session).is_err());
    }
}
