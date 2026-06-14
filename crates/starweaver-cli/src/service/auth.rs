use serde_json::json;
use starweaver_oauth::{redact_record, CodexOAuthClient, OAuthStore};

use super::{render_json_lines, CliService};
use crate::{args::AuthCommand, CliError, CliResult};

impl CliService {
    pub(super) fn auth(command: AuthCommand) -> CliResult<String> {
        match command {
            AuthCommand::Login(command) => auth_login(command),
            AuthCommand::Status(command) => auth_status(command),
            AuthCommand::Refresh(command) => auth_refresh(command),
            AuthCommand::Logout(command) => auth_logout(command),
            AuthCommand::Doctor(command) => auth_doctor(command),
        }
    }
}

fn oauth_store(auth_file: Option<String>) -> OAuthStore {
    auth_file.map_or_else(OAuthStore::default_store, OAuthStore::new)
}

pub(super) fn oauth_cli_error(error: impl std::fmt::Display) -> CliError {
    CliError::Config(error.to_string())
}

fn auth_login(command: crate::args::AuthProviderCommand) -> CliResult<String> {
    let store = oauth_store(command.auth_file);
    let provider = command.provider;
    if provider != "codex" {
        return Err(CliError::Config(format!(
            "unknown OAuth provider: {provider}"
        )));
    }
    let client = CodexOAuthClient::with_store(store.clone()).map_err(oauth_cli_error)?;
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let (device_code, record) = runtime
        .block_on(async {
            let device_code = client.request_device_code().await?;
            eprintln!("Open this URL in your browser and sign in to ChatGPT:");
            eprintln!("{}", device_code.verification_url);
            eprintln!();
            eprintln!("Enter this one-time code:");
            eprintln!("{}", device_code.user_code);
            eprintln!();
            eprintln!("Waiting for browser authorization...");
            let token_code = client
                .poll_device_token(&device_code, command.timeout_seconds)
                .await?;
            let record = client.exchange_device_code(&token_code).await?;
            Ok::<_, starweaver_oauth::OAuthError>((device_code, record))
        })
        .map_err(oauth_cli_error)?;
    store
        .set_provider("codex", record.clone())
        .map_err(oauth_cli_error)?;
    let identity = record
        .account
        .email
        .clone()
        .or_else(|| record.account.chatgpt_user_id.clone())
        .unwrap_or_else(|| "unknown account".to_string());
    let value = json!({
        "provider": "codex",
        "logged_in": true,
        "identity": identity,
        "auth_path": store.path(),
        "verification_url": device_code.verification_url,
    });
    Ok(format!("{}\n", serde_json::to_string(&value)?))
}

fn auth_status(command: crate::args::AuthStatusCommand) -> CliResult<String> {
    let store = oauth_store(command.auth_file);
    let auth = store.load().map_err(oauth_cli_error)?;
    let provider_names = command.provider.map_or_else(
        || auth.providers.keys().cloned().collect::<Vec<_>>(),
        |provider| vec![provider],
    );
    if provider_names.is_empty() {
        let value = json!({
            "auth_path": store.path(),
            "providers": [],
        });
        return Ok(format!("{}\n", serde_json::to_string(&value)?));
    }
    let rows = provider_names
        .into_iter()
        .map(|provider| {
            let record = auth.providers.get(&provider).cloned();
            json!({
                "provider": provider,
                "logged_in": record.is_some(),
                "auth_path": store.path(),
                "record": record.map(|record| record.status_value()),
            })
        })
        .collect::<Vec<_>>();
    render_json_lines(&rows)
}

fn auth_refresh(command: crate::args::AuthProviderCommand) -> CliResult<String> {
    let store = oauth_store(command.auth_file);
    let provider = command.provider;
    if provider != "codex" {
        return Err(CliError::Config(format!(
            "unknown OAuth provider: {provider}"
        )));
    }
    let client = CodexOAuthClient::with_store(store.clone()).map_err(oauth_cli_error)?;
    let record = store
        .get_provider("codex")
        .map_err(oauth_cli_error)?
        .ok_or_else(|| CliError::Config("OAuth provider is not logged in: codex".to_string()))?;
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let refreshed = runtime
        .block_on(client.refresh_record(&record))
        .map_err(oauth_cli_error)?;
    store
        .set_provider("codex", refreshed.clone())
        .map_err(oauth_cli_error)?;
    let value = json!({
        "provider": "codex",
        "refreshed": true,
        "auth_path": store.path(),
        "record": refreshed.status_value(),
    });
    Ok(format!("{}\n", serde_json::to_string(&value)?))
}

fn auth_logout(command: crate::args::AuthLogoutCommand) -> CliResult<String> {
    let store = oauth_store(command.auth_file);
    let provider = command.provider;
    if provider != "codex" {
        return Err(CliError::Config(format!(
            "unknown OAuth provider: {provider}"
        )));
    }
    let record = store.get_provider("codex").map_err(oauth_cli_error)?;
    if let Some(record) = record.as_ref().filter(|_| command.revoke) {
        let client = CodexOAuthClient::with_store(store.clone()).map_err(oauth_cli_error)?;
        let runtime =
            tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
        runtime
            .block_on(client.revoke_record(record))
            .map_err(oauth_cli_error)?;
    }
    let removed = store.remove_provider("codex").map_err(oauth_cli_error)?;
    Ok(format!(
        "provider=codex\nremoved={removed}\nrevoked={}\nauth_path={}\n",
        command.revoke && record.is_some(),
        store.path().display()
    ))
}

fn auth_doctor(command: crate::args::AuthDoctorCommand) -> CliResult<String> {
    let store = oauth_store(command.auth_file);
    let auth = store.load().map_err(oauth_cli_error)?;
    let rows = auth
        .providers
        .iter()
        .map(|(provider, record)| {
            json!({
                "provider": provider,
                "auth_path": store.path(),
                "record": redact_record(record),
            })
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        let value = json!({
            "auth_path": store.path(),
            "providers": [],
        });
        Ok(format!("{}\n", serde_json::to_string(&value)?))
    } else {
        render_json_lines(&rows)
    }
}
