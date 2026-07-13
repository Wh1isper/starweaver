use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use serde::{Deserialize, Serialize};
use syn::{Item, ItemMod, ItemUse, UseTree, Visibility};

use crate::common::root as workspace_root;

const AGENT_LIB: &str = "crates/starweaver-agent/src/lib.rs";
const SNAPSHOT: &str = "crates/starweaver-agent/tests/fixtures/public-api-v1.json";

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AgentApiSnapshot {
    root: BTreeSet<String>,
    prelude: BTreeSet<String>,
    advanced: BTreeMap<String, String>,
}

pub fn check_agent_api(args: &[String]) -> Result<(), String> {
    let bless = match args {
        [] => false,
        [arg] if arg == "--bless" => true,
        _ => return Err("usage: check-agent-api [--bless]".to_string()),
    };
    let root = workspace_root()?;
    let source = fs::read_to_string(root.join(AGENT_LIB))
        .map_err(|error| format!("failed to read {AGENT_LIB}: {error}"))?;
    let file = syn::parse_file(&source)
        .map_err(|error| format!("failed to parse {AGENT_LIB}: {error}"))?;
    let actual = collect_snapshot(&file.items)?;
    let snapshot_path = root.join(SNAPSHOT);
    if bless {
        let json = serde_json::to_string_pretty(&actual)
            .map_err(|error| format!("failed to encode API snapshot: {error}"))?;
        if let Some(parent) = snapshot_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&snapshot_path, format!("{json}\n"))
            .map_err(|error| format!("failed to write {}: {error}", snapshot_path.display()))?;
        println!("updated {SNAPSHOT}");
        return Ok(());
    }
    let expected: AgentApiSnapshot = serde_json::from_str(
        &fs::read_to_string(&snapshot_path)
            .map_err(|error| format!("failed to read {SNAPSHOT}: {error}"))?,
    )
    .map_err(|error| format!("failed to parse {SNAPSHOT}: {error}"))?;
    if actual != expected {
        return Err(format!(
            "starweaver-agent public API snapshot changed; review the diff and run \
             `cargo run -p xtask -- check-agent-api --bless` to accept it\n{}",
            snapshot_diff(&expected, &actual)
        ));
    }
    println!(
        "starweaver-agent API snapshot passed: {} root, {} prelude, {} advanced namespaces",
        actual.root.len(),
        actual.prelude.len(),
        actual.advanced.len()
    );
    Ok(())
}

fn collect_snapshot(items: &[Item]) -> Result<AgentApiSnapshot, String> {
    let mut root = BTreeSet::new();
    let mut prelude = None;
    let mut advanced = None;
    for item in items {
        if !is_public(item) {
            continue;
        }
        match item {
            Item::Mod(module) if module.ident == "prelude" => {
                root.insert(module.ident.to_string());
                prelude = Some(collect_module_exports(module)?);
            }
            Item::Mod(module) if module.ident == "advanced" => {
                root.insert(module.ident.to_string());
                advanced = Some(collect_advanced_exports(module)?);
            }
            Item::Mod(module) => {
                root.insert(module.ident.to_string());
            }
            Item::Use(item_use) => collect_use_names(&item_use.tree, &mut root)?,
            Item::Const(item) => {
                root.insert(item.ident.to_string());
            }
            Item::Enum(item) => {
                root.insert(item.ident.to_string());
            }
            Item::ExternCrate(item) => {
                root.insert(
                    item.rename
                        .as_ref()
                        .map_or(&item.ident, |(_, name)| name)
                        .to_string(),
                );
            }
            Item::Fn(item) => {
                root.insert(item.sig.ident.to_string());
            }
            Item::Static(item) => {
                root.insert(item.ident.to_string());
            }
            Item::Struct(item) => {
                root.insert(item.ident.to_string());
            }
            Item::Trait(item) => {
                root.insert(item.ident.to_string());
            }
            Item::TraitAlias(item) => {
                root.insert(item.ident.to_string());
            }
            Item::Type(item) => {
                root.insert(item.ident.to_string());
            }
            Item::Union(item) => {
                root.insert(item.ident.to_string());
            }
            _ => {}
        }
    }
    Ok(AgentApiSnapshot {
        root,
        prelude: prelude.ok_or_else(|| "missing public prelude module".to_string())?,
        advanced: advanced.ok_or_else(|| "missing public advanced module".to_string())?,
    })
}

fn collect_module_exports(module: &ItemMod) -> Result<BTreeSet<String>, String> {
    let (_, items) = module
        .content
        .as_ref()
        .ok_or_else(|| format!("{} must be an inline module", module.ident))?;
    let mut exports = BTreeSet::new();
    for item in items {
        if let Item::Use(item_use) = item
            && matches!(item_use.vis, Visibility::Public(_))
        {
            collect_use_names(&item_use.tree, &mut exports)?;
        }
    }
    Ok(exports)
}

fn collect_advanced_exports(module: &ItemMod) -> Result<BTreeMap<String, String>, String> {
    let (_, items) = module
        .content
        .as_ref()
        .ok_or_else(|| "advanced must be an inline module".to_string())?;
    let mut exports = BTreeMap::new();
    for item in items {
        let Item::Use(ItemUse { vis, tree, .. }) = item else {
            continue;
        };
        if !matches!(vis, Visibility::Public(_)) {
            continue;
        }
        let (name, target) = advanced_export(tree)?;
        if exports.insert(name.clone(), target).is_some() {
            return Err(format!("duplicate advanced namespace {name}"));
        }
    }
    Ok(exports)
}

fn advanced_export(tree: &UseTree) -> Result<(String, String), String> {
    match tree {
        UseTree::Name(name) => Ok((name.ident.to_string(), name.ident.to_string())),
        UseTree::Rename(rename) => Ok((rename.rename.to_string(), rename.ident.to_string())),
        _ => Err("advanced exports must be direct named crate re-exports".to_string()),
    }
}

fn collect_use_names(tree: &UseTree, names: &mut BTreeSet<String>) -> Result<(), String> {
    match tree {
        UseTree::Path(path) => collect_use_names(&path.tree, names),
        UseTree::Name(name) => {
            names.insert(name.ident.to_string());
            Ok(())
        }
        UseTree::Rename(rename) => {
            names.insert(rename.rename.to_string());
            Ok(())
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_names(item, names)?;
            }
            Ok(())
        }
        UseTree::Glob(_) => Err("public glob re-exports are forbidden by the API gate".to_string()),
    }
}

const fn is_public(item: &Item) -> bool {
    let visibility = match item {
        Item::Const(item) => &item.vis,
        Item::Enum(item) => &item.vis,
        Item::ExternCrate(item) => &item.vis,
        Item::Fn(item) => &item.vis,
        Item::Mod(item) => &item.vis,
        Item::Static(item) => &item.vis,
        Item::Struct(item) => &item.vis,
        Item::Trait(item) => &item.vis,
        Item::TraitAlias(item) => &item.vis,
        Item::Type(item) => &item.vis,
        Item::Union(item) => &item.vis,
        Item::Use(item) => &item.vis,
        _ => return false,
    };
    matches!(visibility, Visibility::Public(_))
}

fn snapshot_diff(expected: &AgentApiSnapshot, actual: &AgentApiSnapshot) -> String {
    let mut lines = Vec::new();
    set_diff("root", &expected.root, &actual.root, &mut lines);
    set_diff("prelude", &expected.prelude, &actual.prelude, &mut lines);
    if expected.advanced != actual.advanced {
        lines.push(format!(
            "advanced: expected {:?}, actual {:?}",
            expected.advanced, actual.advanced
        ));
    }
    lines.join("\n")
}

fn set_diff(
    label: &str,
    expected: &BTreeSet<String>,
    actual: &BTreeSet<String>,
    lines: &mut Vec<String>,
) {
    let added = actual.difference(expected).cloned().collect::<Vec<_>>();
    let removed = expected.difference(actual).cloned().collect::<Vec<_>>();
    if !added.is_empty() || !removed.is_empty() {
        lines.push(format!("{label}: added={added:?}, removed={removed:?}"));
    }
}
