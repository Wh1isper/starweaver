use starweaver_core::sdk_name;

use super::{auth::oauth_cli_error, render_json_lines, CliService};
use crate::{
    args::{CatalogCommand, ConfigCommand, ProfileCommand, ToolsCommand, UpdateCommand},
    config::{
        get_config_value, init_config_file, mcp_servers, tool_need_approval, ConfigScope,
        ProviderConfig,
    },
    profiles::{
        doctor_mcp_servers, list_default_tools, list_mcp_servers, list_profiles, list_skills,
        list_subagents, show_mcp_server, show_profile, show_skill, show_subagent,
    },
    CliError, CliResult,
};

impl CliService {
    pub(super) fn profile(&self, command: ProfileCommand) -> CliResult<String> {
        match command {
            ProfileCommand::List => list_profiles(&self.config)
                .iter()
                .map(|profile| serde_json::to_string(profile).map(|line| format!("{line}\n")))
                .collect::<Result<String, _>>()
                .map_err(CliError::from),
            ProfileCommand::Show { name } => show_profile(&self.config, &name),
        }
    }

    pub(super) fn update(command: &UpdateCommand) -> CliResult<String> {
        crate::launcher::update_component_with_env_options(
            &command.target,
            crate::launcher::UpdateOptions {
                dry_run: command.dry_run,
                force: command.force,
            },
        )
    }

    pub(super) fn skills(&self, command: CatalogCommand) -> CliResult<String> {
        match command {
            CatalogCommand::List => render_json_lines(&list_skills(&self.config)),
            CatalogCommand::Show { name } => show_skill(&self.config, &name),
            CatalogCommand::Doctor => Ok(format!(
                "skill_dirs={}\nskills={}\nstatus=ok\n",
                self.config
                    .skill_dirs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(":"),
                list_skills(&self.config).len()
            )),
        }
    }

    pub(super) fn subagents(&self, command: CatalogCommand) -> CliResult<String> {
        match command {
            CatalogCommand::List => render_json_lines(&list_subagents(&self.config)),
            CatalogCommand::Show { name } => show_subagent(&self.config, &name),
            CatalogCommand::Doctor => Ok(format!(
                "subagent_dirs={}\nsubagents={}\ndisabled={}\nstatus=ok\n",
                self.config
                    .subagent_dirs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(":"),
                list_subagents(&self.config).len(),
                self.config.disabled_subagents.join(",")
            )),
        }
    }

    pub(super) fn mcp(&self, command: CatalogCommand) -> CliResult<String> {
        match command {
            CatalogCommand::List => render_json_lines(&list_mcp_servers(&self.config)),
            CatalogCommand::Show { name } => show_mcp_server(&self.config, &name),
            CatalogCommand::Doctor => {
                let findings = doctor_mcp_servers(&self.config);
                let output = render_json_lines(&findings)?;
                if findings.iter().any(|finding| finding.status == "error") {
                    Err(CliError::Config(output))
                } else {
                    Ok(output)
                }
            }
        }
    }

    pub(super) fn tools(&self, command: &ToolsCommand) -> CliResult<String> {
        match command {
            ToolsCommand::List => render_json_lines(&list_default_tools(&self.config)),
            ToolsCommand::Doctor => Ok(format!(
                "tools={}\nneed_approval={}\nstatus=ok\n",
                list_default_tools(&self.config).len(),
                tool_need_approval(&self.config).join(",")
            )),
        }
    }

    pub(super) fn config(&self, command: ConfigCommand) -> CliResult<String> {
        match command {
            ConfigCommand::Init {
                global,
                project: _,
                force,
            } => {
                let scope = if global {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project
                };
                let path = init_config_file(&self.config, scope, force)?;
                Ok(format!(
                    "config_path={}\nstatus=initialized\n",
                    path.display()
                ))
            }
            ConfigCommand::Get { key } => get_config_value(&self.config, &key),
            ConfigCommand::Set {
                global,
                project: _,
                key,
                value,
            } => {
                let scope = if global {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project
                };
                crate::config::set_config_value(&self.config, scope, &key, &value)?;
                Ok(format!("{key}={value}\n"))
            }
        }
    }

    pub(super) fn diagnostics(&self) -> CliResult<String> {
        Ok(format!(
            "sdk={}\nworkspace_version={}\ndatabase_path={}\nfile_store_path={}\nprofile={}\ndefault_model={}\nmodel_profiles={}\noauth_refresh.enabled={}\noauth_refresh.interval_seconds={}\noauth_refresh.failure_retry_seconds={}\noauth_refresh.refresh_on_startup={}\nworkspace_root={}\nenvironment_provider={}\nfiles_policy={}\nshell_enabled={}\nskills={}\nsubagents={}\nmcp_servers={}\ntools={}\ntools.need_approval={}\nprovider.openai.ready={}\nprovider.openai.api_key_env={}\nprovider.openai.base_url={}\nprovider.codex.logged_in={}\nprovider.codex.base_url={}\nprovider.anthropic.ready={}\nprovider.anthropic.api_key_env={}\nprovider.anthropic.base_url={}\nprovider.gemini.ready={}\nprovider.gemini.api_key_env={}\nprovider.gemini.base_url={}\nwal=true\n",
            sdk_name(),
            env!("CARGO_PKG_VERSION"),
            self.config.database_path.display(),
            self.config.file_store_path.display(),
            self.config.default_profile,
            self.config
                .default_model
                .as_ref()
                .map(|profile| profile.model_id.as_str())
                .unwrap_or_default(),
            self.config.model_profiles.len(),
            self.config.oauth_refresh.enabled,
            self.config.oauth_refresh.interval_seconds,
            self.config.oauth_refresh.failure_retry_seconds,
            self.config.oauth_refresh.refresh_on_startup,
            self.config.workspace_root.display(),
            self.config.environment_provider,
            self.config.files_policy,
            self.config.shell_enabled,
            list_skills(&self.config).len(),
            list_subagents(&self.config).len(),
            mcp_servers(&self.config).len(),
            list_default_tools(&self.config).len(),
            tool_need_approval(&self.config).join(","),
            provider_ready(&self.config.providers.openai),
            self.config.providers.openai.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.openai.base_url.as_deref().unwrap_or_default(),
            crate::oauth::OAuthStore::new(crate::oauth::OAuthStore::default_path())
                .get_provider("codex")
                .map_err(oauth_cli_error)?
                .is_some(),
            self.config.providers.codex.base_url.as_deref().unwrap_or_default(),
            provider_ready(&self.config.providers.anthropic),
            self.config.providers.anthropic.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.anthropic.base_url.as_deref().unwrap_or_default(),
            provider_ready(&self.config.providers.gemini),
            self.config.providers.gemini.api_key_env.as_deref().unwrap_or_default(),
            self.config.providers.gemini.base_url.as_deref().unwrap_or_default()
        ))
    }
}

fn provider_ready(provider: &ProviderConfig) -> bool {
    provider.enabled
        && provider.api_key_env.as_deref().is_some_and(|name| {
            let name = name.trim();
            !name.is_empty() && std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
        })
}
