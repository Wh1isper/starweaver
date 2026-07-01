use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Utc;
use ring::digest;
use serde_json::json;
use starweaver_session::RunRecord;

use super::CliService;
use crate::{CliError, CliResult, args::RunCommand, slash_commands::ExpandedSlashCommand};

#[derive(Clone, Debug)]
pub(super) struct WorktreeResolution {
    pub(super) git_root: PathBuf,
    pub(super) path: PathBuf,
    pub(super) branch: String,
    pub(super) resumed: bool,
}

impl CliService {
    pub(super) fn resolve_worktree(
        &self,
        command: &RunCommand,
    ) -> CliResult<Option<WorktreeResolution>> {
        if command.worktree.is_none() && command.worktree_name.is_none() && command.branch.is_none()
        {
            return Ok(None);
        }
        let git_root = git_root(&self.config.workspace_root)?;
        let branch = command
            .branch
            .clone()
            .unwrap_or_else(default_worktree_branch);
        let worktree_name = command
            .worktree_name
            .clone()
            .or_else(|| command.worktree.as_ref().and_then(explicit_flag_value))
            .unwrap_or_else(|| branch.clone());
        let path = worktree_path(&self.config.global_dir, &git_root, &worktree_name);
        let resumed = path.exists();
        let group_dir = path.parent().unwrap_or(&self.config.global_dir);
        fs::create_dir_all(group_dir).map_err(|error| crate::error::io_error(group_dir, error))?;
        write_worktree_group_metadata(group_dir, &git_root)?;
        if !resumed {
            let status = Command::new("git")
                .arg("worktree")
                .arg("add")
                .arg("-b")
                .arg(&branch)
                .arg(&path)
                .current_dir(&git_root)
                .status()
                .map_err(|error| CliError::Run(error.to_string()))?;
            if !status.success() {
                return Err(CliError::Run(format!(
                    "git worktree add failed with status {status}"
                )));
            }
        }
        Ok(Some(WorktreeResolution {
            git_root,
            path,
            branch,
            resumed,
        }))
    }
}

fn git_root(workspace_root: &Path) -> CliResult<PathBuf> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(workspace_root)
        .output()
        .map_err(|error| CliError::Run(error.to_string()))?;
    if !output.status.success() {
        return Err(CliError::Usage(
            "--worktree/--branch requires a git repository workspace".to_string(),
        ));
    }
    let root =
        String::from_utf8(output.stdout).map_err(|error| CliError::Run(error.to_string()))?;
    Ok(PathBuf::from(root.trim()))
}

fn default_worktree_branch() -> String {
    format!("starweaver/{}", Utc::now().format("%Y%m%d-%H%M%S"))
}

fn worktree_path(global_dir: &Path, git_root: &Path, name: &str) -> PathBuf {
    global_dir
        .join("worktrees")
        .join(project_hash(git_root))
        .join(sanitize_worktree_name(name))
}

fn write_worktree_group_metadata(group_dir: &Path, git_root: &Path) -> CliResult<()> {
    let path = group_dir.join("metadata.json");
    if path.exists() {
        return Ok(());
    }
    let value = json!({
        "git_root": git_root.display().to_string(),
        "created_at": Utc::now(),
    });
    fs::write(&path, serde_json::to_vec_pretty(&value)?)
        .map_err(|error| crate::error::io_error(&path, error))?;
    Ok(())
}

fn project_hash(path: &Path) -> String {
    let digest = digest::digest(&digest::SHA256, path.display().to_string().as_bytes());
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn sanitize_worktree_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn explicit_flag_value(value: &String) -> Option<String> {
    (value != "true").then(|| value.clone())
}

pub(super) fn apply_starweaver_run_metadata(
    run: &mut RunRecord,
    command: &RunCommand,
    worktree: Option<&WorktreeResolution>,
    slash_expansion: Option<&ExpandedSlashCommand>,
) {
    if let Some(expanded) = slash_expansion {
        run.metadata.insert(
            "cli.slash_command.name".to_string(),
            json!(expanded.command_name),
        );
        run.metadata.insert(
            "cli.slash_command.invoked".to_string(),
            json!(expanded.invoked_name),
        );
        if !expanded.args.is_empty() {
            run.metadata
                .insert("cli.slash_command.args".to_string(), json!(expanded.args));
        }
    }
    if command.worker.is_some() || command.worker_label.is_some() {
        run.metadata
            .insert("cli.starweaver.worker_enabled".to_string(), json!(true));
    }
    let worker_label = command
        .worker_label
        .as_deref()
        .map(ToString::to_string)
        .or_else(|| command.worker.as_ref().and_then(explicit_flag_value));
    if let Some(worker) = worker_label {
        run.metadata
            .insert("cli.starweaver.worker".to_string(), json!(worker));
    }
    if let Some(worktree) = worktree {
        run.metadata.insert(
            "cli.starweaver.worktree".to_string(),
            json!(worktree.path.display().to_string()),
        );
        run.metadata.insert(
            "cli.starweaver.worktree_git_root".to_string(),
            json!(worktree.git_root.display().to_string()),
        );
        run.metadata.insert(
            "cli.starweaver.worktree_resumed".to_string(),
            json!(worktree.resumed),
        );
        run.metadata
            .insert("cli.starweaver.branch".to_string(), json!(worktree.branch));
    } else {
        let worktree_label = command
            .worktree_name
            .as_deref()
            .map(ToString::to_string)
            .or_else(|| command.worktree.as_ref().and_then(explicit_flag_value));
        if command.worktree.is_some() || command.worktree_name.is_some() {
            run.metadata
                .insert("cli.starweaver.worktree_enabled".to_string(), json!(true));
        }
        if let Some(worktree) = worktree_label {
            run.metadata
                .insert("cli.starweaver.worktree".to_string(), json!(worktree));
        }
        if let Some(branch) = command.branch.as_ref() {
            run.metadata
                .insert("cli.starweaver.branch".to_string(), json!(branch));
        }
    }
}
