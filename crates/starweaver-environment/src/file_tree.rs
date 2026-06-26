use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use crate::{
    join_logical_path, normalize_path, normalize_str_path, parent_path, path_contains,
    strip_path_prefix, EnvironmentError, EnvironmentPolicy, EnvironmentResult,
};

const DEFAULT_FILE_TREE_SKIPPED_DIR_NAMES: &[&str] =
    &["node_modules", ".git", ".venv", "__pycache__"];
const DEFAULT_FILE_TREE_VISIBLE_DOT_DIR_NAMES: &[&str] = &[".agents"];
pub const DEFAULT_FILE_TREE_MAX_DEPTH: usize = 3;

pub fn render_virtual_file_tree_listing(
    files: &BTreeMap<String, String>,
    root_path: &str,
    max_depth: usize,
) -> String {
    let root = normalize_file_tree_root(root_path);
    let gitignore = files
        .get(&join_logical_path(&root, ".gitignore"))
        .and_then(|content| build_gitignore(&root, content).ok());
    let entries = collect_virtual_file_tree_entries(files, &root);
    render_flat_file_tree_entries(entries, gitignore.as_ref(), max_depth)
}

pub fn render_local_file_tree_listing(
    root: &Path,
    visible_root: &str,
    policy: &EnvironmentPolicy,
    max_depth: usize,
) -> EnvironmentResult<String> {
    if !root.exists() || !root.is_dir() {
        return Ok(format!("Directory not found: {}", root.display()));
    }
    let gitignore = std::fs::read_to_string(root.join(".gitignore"))
        .ok()
        .and_then(|content| build_gitignore(".", &content).ok());
    let mut output = Vec::new();
    append_local_file_tree_lines(
        root,
        root,
        "",
        visible_root,
        policy,
        gitignore.as_ref(),
        1,
        max_depth,
        &mut output,
    )?;
    Ok(output.join("\n"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileTreeEntry {
    path: String,
    is_dir: bool,
}

fn collect_virtual_file_tree_entries(
    files: &BTreeMap<String, String>,
    root: &str,
) -> Vec<FileTreeEntry> {
    let prefix = if root == "." {
        ""
    } else {
        root.trim_matches('/')
    };
    let mut dirs = BTreeSet::<String>::new();
    let mut entries = Vec::new();
    for path in files.keys() {
        if !path_contains(prefix, path) {
            continue;
        }
        let rel = strip_path_prefix(prefix, path);
        if rel.is_empty() {
            continue;
        }
        let normalized = normalize_str_path(rel);
        let mut current = String::new();
        for segment in normalized
            .split('/')
            .collect::<Vec<_>>()
            .iter()
            .take(normalized.split('/').count().saturating_sub(1))
        {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            dirs.insert(current.clone());
        }
        entries.push(FileTreeEntry {
            path: normalized,
            is_dir: false,
        });
    }
    entries.extend(
        dirs.into_iter()
            .map(|path| FileTreeEntry { path, is_dir: true }),
    );
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

#[allow(clippy::too_many_arguments)]
fn append_local_file_tree_lines(
    root: &Path,
    current: &Path,
    current_logical: &str,
    visible_root: &str,
    policy: &EnvironmentPolicy,
    gitignore: Option<&ignore::gitignore::Gitignore>,
    depth: usize,
    max_depth: usize,
    output: &mut Vec<String>,
) -> EnvironmentResult<()> {
    let children = match std::fs::read_dir(current) {
        Ok(children) => children,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            output.push(permission_denied_directory_line(current_logical));
            return Ok(());
        }
        Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
    };

    let mut directories = Vec::new();
    let mut files = Vec::new();
    for child in children {
        let child = match child {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => continue,
            Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
        };
        let path = child.path();
        let relative = normalize_path(
            path.strip_prefix(root)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?,
        );
        let policy_path = if visible_root.is_empty() {
            relative.clone()
        } else {
            join_logical_path(visible_root, &relative)
        };
        if !policy.files.permits(&policy_path, false) {
            continue;
        }
        let file_type = match child.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                output.push(format!("{relative} (permission denied)"));
                continue;
            }
            Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
        };
        if file_type.is_dir() {
            directories.push((relative, path));
        } else if file_type.is_file() {
            files.push(relative);
        }
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    files.sort();

    for (logical, path) in directories {
        let Some(name) = logical.rsplit('/').next() else {
            continue;
        };
        let (should_skip, should_mark) = classify_file_tree_entry_visibility(name, true);
        if should_skip {
            if should_mark {
                output.push(format!("{logical}/ (skipped)"));
            }
            continue;
        }
        if is_gitignored(gitignore, &logical, true) {
            output.push(format!("{logical}/ (gitignored)"));
            continue;
        }
        let next_depth = depth + file_tree_directory_depth_increment(name);
        if next_depth <= max_depth {
            append_local_file_tree_lines(
                root,
                &path,
                &logical,
                visible_root,
                policy,
                gitignore,
                next_depth,
                max_depth,
                output,
            )?;
        }
    }

    for logical in files {
        let Some(name) = logical.rsplit('/').next() else {
            continue;
        };
        let (should_skip, _) = classify_file_tree_entry_visibility(name, false);
        if should_skip {
            continue;
        }
        if is_gitignored(gitignore, &logical, false) {
            output.push(format!("{logical} (gitignored)"));
        } else {
            output.push(logical);
        }
    }

    Ok(())
}

fn permission_denied_directory_line(logical: &str) -> String {
    if logical.is_empty() {
        "(permission denied)".to_string()
    } else {
        format!("{logical}/ (permission denied)")
    }
}

fn render_flat_file_tree_entries(
    entries: Vec<FileTreeEntry>,
    gitignore: Option<&ignore::gitignore::Gitignore>,
    max_depth: usize,
) -> String {
    let mut by_parent = BTreeMap::<String, Vec<FileTreeEntry>>::new();
    for entry in entries {
        let parent = parent_path(&entry.path).unwrap_or_default();
        by_parent.entry(parent).or_default().push(entry);
    }
    for children in by_parent.values_mut() {
        children.sort_by(|left, right| match (left.is_dir, right.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left.path.cmp(&right.path),
        });
    }
    let mut output = Vec::new();
    append_virtual_file_tree_lines("", 1, max_depth, &by_parent, gitignore, &mut output);
    output.join("\n")
}

fn append_virtual_file_tree_lines(
    parent: &str,
    depth: usize,
    max_depth: usize,
    by_parent: &BTreeMap<String, Vec<FileTreeEntry>>,
    gitignore: Option<&ignore::gitignore::Gitignore>,
    output: &mut Vec<String>,
) {
    let Some(children) = by_parent.get(parent) else {
        return;
    };
    for entry in children {
        let Some(name) = entry.path.rsplit('/').next() else {
            continue;
        };
        let (should_skip, should_mark) = classify_file_tree_entry_visibility(name, entry.is_dir);
        if should_skip {
            if should_mark {
                output.push(format!("{}/ (skipped)", entry.path));
            }
            continue;
        }
        if is_gitignored(gitignore, &entry.path, entry.is_dir) {
            if entry.is_dir {
                output.push(format!("{}/ (gitignored)", entry.path));
            } else {
                output.push(format!("{} (gitignored)", entry.path));
            }
            continue;
        }
        if entry.is_dir {
            let next_depth = depth + file_tree_directory_depth_increment(name);
            if next_depth <= max_depth {
                append_virtual_file_tree_lines(
                    &entry.path,
                    next_depth,
                    max_depth,
                    by_parent,
                    gitignore,
                    output,
                );
            }
        } else {
            output.push(entry.path.clone());
        }
    }
}

fn classify_file_tree_entry_visibility(name: &str, is_dir: bool) -> (bool, bool) {
    if is_dir && DEFAULT_FILE_TREE_SKIPPED_DIR_NAMES.contains(&name) {
        return (true, true);
    }
    if !name.starts_with('.') {
        return (false, false);
    }
    if is_dir && DEFAULT_FILE_TREE_VISIBLE_DOT_DIR_NAMES.contains(&name) {
        return (false, false);
    }
    if !is_dir && name == ".env" {
        return (false, false);
    }
    (true, false)
}

pub fn file_tree_directory_is_visible(name: &str) -> bool {
    !classify_file_tree_entry_visibility(name, true).0
}

pub fn file_tree_directory_depth_increment(name: &str) -> usize {
    usize::from(!DEFAULT_FILE_TREE_VISIBLE_DOT_DIR_NAMES.contains(&name))
}

fn is_gitignored(
    gitignore: Option<&ignore::gitignore::Gitignore>,
    path: &str,
    is_dir: bool,
) -> bool {
    gitignore.is_some_and(|matcher| matcher.matched(path, is_dir).is_ignore())
}

fn build_gitignore(
    root: &str,
    content: &str,
) -> Result<ignore::gitignore::Gitignore, ignore::Error> {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for line in content.lines() {
        builder.add_line(None, line)?;
    }
    builder.build()
}

fn normalize_file_tree_root(root: &str) -> String {
    let normalized = normalize_str_path(root);
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}
