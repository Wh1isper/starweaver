#![allow(clippy::unwrap_used)]

use super::*;

fn resolver_with_current_dir(root: &Path, current_dir: &Path) -> ConfigResolver {
    ConfigResolver {
        global_dir: Some(root.join("global")),
        project_dir: None,
        shared_agents_dir: Some(root.join("shared-agents")),
        current_dir: Some(current_dir.to_path_buf()),
    }
}

#[test]
fn default_workspace_root_uses_invocation_cwd_without_project_config() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let cli =
        crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();

    let config = resolver_with_current_dir(temp.path(), &workspace)
        .resolve(&cli)
        .unwrap();

    assert_eq!(config.project_dir, temp.path().join("global"));
    assert_eq!(config.workspace_root, workspace);
}

#[test]
fn default_workspace_root_uses_discovered_project_parent() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let nested = workspace.join("crates/example");
    fs::create_dir_all(workspace.join(".starweaver")).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(workspace.join(".starweaver/config.toml"), "").unwrap();
    let cli =
        crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();

    let config = resolver_with_current_dir(temp.path(), &nested)
        .resolve(&cli)
        .unwrap();

    assert_eq!(config.project_dir, workspace.join(".starweaver"));
    assert_eq!(config.workspace_root, workspace);
}

#[test]
fn default_project_discovery_ignores_home_global_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = home.join("workspace");
    fs::create_dir_all(home.join(".starweaver")).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(home.join(".starweaver/config.toml"), "").unwrap();
    let cli =
        crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();
    let resolver = ConfigResolver {
        global_dir: Some(home.join(".starweaver")),
        project_dir: None,
        shared_agents_dir: Some(temp.path().join("shared-agents")),
        current_dir: Some(workspace.clone()),
    };

    let config = resolver.resolve(&cli).unwrap();

    assert_eq!(config.project_dir, home.join(".starweaver"));
    assert_eq!(config.workspace_root, workspace);
}

#[test]
fn project_setup_targets_invocation_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let cli = crate::args::parse([
        "starweaver-cli".to_string(),
        "setup".to_string(),
        "--project".to_string(),
    ])
    .unwrap();

    let config = resolver_with_current_dir(temp.path(), &workspace)
        .resolve(&cli)
        .unwrap();

    assert_eq!(config.project_dir, workspace.join(".starweaver"));
    assert_eq!(config.workspace_root, workspace);
}

#[test]
fn shell_review_config_parses_security_table_and_getters() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let project_config = workspace.join(".starweaver");
    fs::create_dir_all(&project_config).unwrap();
    fs::write(
        project_config.join("config.toml"),
        r#"
[security.shell_review]
enabled = true
model = "local_echo"
model_settings = "openai_responses_medium"
on_needs_approval = "deny"
risk_threshold = "extra-high"
system_prompt = "Review safely."
"#,
    )
    .unwrap();
    let cli =
        crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();
    let resolver = ConfigResolver {
        global_dir: Some(temp.path().join("global")),
        project_dir: Some(project_config),
        shared_agents_dir: Some(temp.path().join("shared-agents")),
        current_dir: Some(workspace),
    };

    let config = resolver.resolve(&cli).unwrap();

    assert!(config.shell_review.enabled);
    assert_eq!(config.shell_review.model.as_deref(), Some("local_echo"));
    assert_eq!(
        config.shell_review.model_settings.as_deref(),
        Some("openai_responses_medium")
    );
    assert_eq!(config.shell_review.on_needs_approval, "deny");
    assert_eq!(config.shell_review.risk_threshold, "extra_high");
    assert_eq!(
        config.shell_review.system_prompt.as_deref(),
        Some("Review safely.")
    );
    assert_eq!(
        get_config_value(&config, "security.shell_review.risk_threshold").unwrap(),
        "extra_high\n"
    );
}

#[test]
fn shell_review_validation_requires_model_when_enabled() {
    let missing_model = CliShellReviewConfig {
        enabled: true,
        model: None,
        ..CliShellReviewConfig::default()
    };
    assert!(matches!(missing_model.validate(), Err(CliError::Config(_))));
    assert!(validate_shell_review_action("approve").is_err());
    assert!(validate_shell_review_risk("critical").is_err());
}

#[test]
fn set_config_value_writes_nested_shell_review_table() {
    let temp = tempfile::tempdir().unwrap();
    let cli =
        crate::args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();
    let config = ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .unwrap();

    set_config_value(
        &config,
        ConfigScope::Project,
        "security.shell_review.enabled",
        "true",
    )
    .unwrap();
    set_config_value(
        &config,
        ConfigScope::Project,
        "security.shell_review.risk_threshold",
        "extra-high",
    )
    .unwrap();

    let content = fs::read_to_string(config.project_dir.join("config.toml")).unwrap();
    let parsed = content.parse::<Value>().unwrap();
    assert_eq!(
        parsed["security"]["shell_review"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(
        parsed["security"]["shell_review"]["risk_threshold"].as_str(),
        Some("extra_high")
    );
}
