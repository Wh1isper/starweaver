use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{CliService, render_json_lines};
use crate::{
    CliError, CliResult,
    args::{OutputMode, ResetCommand, SetupCommand},
    config::{
        CliConfig, ConfigScope, DEFAULT_GLOBAL_GITIGNORE_TEMPLATE, DEFAULT_MCP_TEMPLATE,
        DEFAULT_PROJECT_GITIGNORE_TEMPLATE, DEFAULT_TOOLS_TEMPLATE, init_config_file,
        write_default_subagent_presets,
    },
};

impl CliService {
    pub(super) fn setup(&self, command: &SetupCommand) -> CliResult<String> {
        let mut rows = Vec::new();
        if command.global || !command.project {
            rows.push(setup_config_file(
                &self.config,
                ConfigScope::Global,
                command.force,
            )?);
            setup_catalog_files(&self.config.global_dir, command.force, &mut rows)?;
            rows.push(write_template_if_missing(
                &self.config.global_dir.join(".gitignore"),
                DEFAULT_GLOBAL_GITIGNORE_TEMPLATE,
                command.force,
                "global-state-ignore",
            )?);
        }
        if command.project {
            rows.push(setup_config_file(
                &self.config,
                ConfigScope::Project,
                command.force,
            )?);
            setup_catalog_files(&self.config.project_dir, command.force, &mut rows)?;
            rows.push(write_template_if_missing(
                &self.config.project_dir.join(".gitignore"),
                DEFAULT_PROJECT_GITIGNORE_TEMPLATE,
                command.force,
                "state-ignore",
            )?);
        }
        render_json_lines(&rows)
    }

    pub(super) fn reset(&mut self, command: &ResetCommand) -> CliResult<String> {
        if !command.yes {
            return Err(CliError::Usage(
                "pass --yes to remove runtime session state".to_string(),
            ));
        }
        self.store = None;
        let removed_database = remove_file_if_exists(&self.config.database_path)?;
        let removed_state = remove_file_if_exists(&self.config.project_dir.join("state.json"))?;
        let removed_store = remove_dir_if_exists(&self.config.file_store_path)?;
        match command.output {
            OutputMode::Text => Ok(format!(
                "removed_database={removed_database}\nremoved_state={removed_state}\nremoved_store={removed_store}\nstatus=reset\n"
            )),
            OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => Ok(format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "removed_database": removed_database,
                    "removed_state": removed_state,
                    "removed_store": removed_store,
                    "status": "reset"
                }))?
            )),
            OutputMode::Silent => Ok("status=reset\n".to_string()),
        }
    }
}

pub(super) fn remove_file_if_exists(path: &Path) -> CliResult<bool> {
    if path.exists() {
        fs::remove_file(path).map_err(|error| crate::error::io_error(path, error))?;
        return Ok(true);
    }
    Ok(false)
}

fn remove_dir_if_exists(path: &Path) -> CliResult<bool> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|error| crate::error::io_error(path, error))?;
        return Ok(true);
    }
    Ok(false)
}

fn setup_catalog_files(root: &Path, force: bool, rows: &mut Vec<Value>) -> CliResult<()> {
    rows.push(write_template_if_missing(
        &root.join("tools.toml"),
        DEFAULT_TOOLS_TEMPLATE,
        force,
        "tools",
    )?);
    rows.push(write_template_if_missing(
        &root.join("mcp.json"),
        DEFAULT_MCP_TEMPLATE,
        force,
        "mcp",
    )?);
    for name in ["skills", "subagents"] {
        let path = root.join(name);
        fs::create_dir_all(&path).map_err(|error| crate::error::io_error(&path, error))?;
        rows.push(json!({"kind": "directory", "path": path, "status": "ready"}));
    }
    for path in write_default_subagent_presets(root, force)? {
        rows.push(json!({"kind": "subagent", "path": path, "status": "ready"}));
    }
    Ok(())
}

fn setup_config_file(config: &CliConfig, scope: ConfigScope, force: bool) -> CliResult<Value> {
    let root = match scope {
        ConfigScope::Global => &config.global_dir,
        ConfigScope::Project => &config.project_dir,
    };
    let path = root.join("config.toml");
    if path.exists() && !force {
        return Ok(
            json!({"kind": "config", "scope": scope_name(scope), "path": path, "status": "exists"}),
        );
    }
    let path = init_config_file(config, scope, force)?;
    Ok(json!({"kind": "config", "scope": scope_name(scope), "path": path, "status": "ready"}))
}

const fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "global",
        ConfigScope::Project => "project",
    }
}

fn write_template_if_missing(
    path: &Path,
    content: &str,
    force: bool,
    kind: &str,
) -> CliResult<Value> {
    if path.exists() && !force {
        return Ok(json!({"kind": kind, "path": path, "status": "exists"}));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| crate::error::io_error(parent, error))?;
    }
    fs::write(path, content).map_err(|error| crate::error::io_error(path, error))?;
    Ok(json!({"kind": kind, "path": path, "status": "ready"}))
}
