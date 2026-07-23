use std::{
    collections::BTreeMap,
    io::Write as _,
    path::PathBuf,
    process::{Command, Stdio},
};

use super::model::ProtocolIr;

mod client;
mod protocol;
mod types;
mod validation;

pub fn generate(ir: &ProtocolIr) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let rendered = [
        ("mod.rs", generated_mod()),
        ("identity.rs", protocol::identity(ir)),
        ("types.rs", types::render(ir)?),
        ("errors.rs", protocol::errors(ir)),
        ("metadata.rs", protocol::metadata(ir)),
        ("validation.rs", validation::render(ir)?),
        ("envelope.rs", protocol::envelope(ir)),
        ("client.rs", client::render(ir)),
        ("server.rs", protocol::server(ir)),
        ("dispatcher.rs", protocol::dispatcher(ir)),
    ];
    rendered
        .into_iter()
        .map(|(name, source)| {
            Ok((
                PathBuf::from(format!("crates/starweaver-rpc-core/src/generated/{name}")),
                rustfmt(&source)?,
            ))
        })
        .collect()
}

fn rustfmt(source: &str) -> Result<Vec<u8>, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start rustfmt: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("rustfmt stdin unavailable")?
        .write_all(source.as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "generated Rust failed rustfmt: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn generated_mod() -> String {
    "//! Generated from protocol/host/openrpc.yaml. Do not edit.\n#![allow(\n    missing_docs,\n    clippy::derive_partial_eq_without_eq,\n    clippy::expect_used,\n    clippy::missing_errors_doc,\n    clippy::missing_panics_doc,\n    clippy::too_many_lines,\n    clippy::wildcard_imports,\n)]\n\nmod client;\nmod dispatcher;\nmod envelope;\nmod errors;\nmod identity;\nmod metadata;\nmod server;\nmod types;\nmod validation;\n\npub use client::*;\npub use dispatcher::*;\npub use envelope::*;\npub use errors::*;\npub use identity::*;\npub use metadata::*;\npub use server::*;\npub use types::*;\n".to_string()
}

pub(super) fn pascal(value: &str) -> String {
    let mut result = String::new();
    let mut upper = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if upper {
                result.extend(character.to_uppercase());
                upper = false;
            } else {
                result.push(character);
            }
        } else {
            upper = true;
        }
    }
    if result
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
    {
        result.insert(0, 'V');
    }
    result
}

pub(super) fn snake(value: &str) -> String {
    let mut result = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase() && index > 0 && !result.ends_with('_') {
                result.push('_');
            }
            result.push(character.to_ascii_lowercase());
        } else if !result.ends_with('_') {
            result.push('_');
        }
    }
    match result.as_str() {
        "type" | "ref" | "match" | "self" | "crate" => format!("r#{result}"),
        _ => result,
    }
}
