#![allow(clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use starweaver_core::Metadata;

use super::*;

#[tokio::test]
async fn virtual_provider_reads_lists_shells_and_exports_state() {
    let output = ShellOutput {
        status: 0,
        stdout: "ok".to_string(),
        stderr: String::new(),
        metadata: Metadata::default(),
    };
    let process = ShellProcessSnapshot {
        process_id: "process_1".to_string(),
        command: "sleep 1".to_string(),
        status: ShellProcessStatus::Completed,
        stdout: "done".to_string(),
        stderr: String::new(),
        return_code: Some(0),
        metadata: Metadata::default(),
    };
    let provider = VirtualEnvironmentProvider::new("test")
        .with_file("src/lib.rs", "content")
        .with_shell_output("echo ok", output.clone())
        .with_process(process.clone());

    assert_eq!(provider.read_text("src/lib.rs").await.unwrap(), "content");
    provider
        .write_text("src/main.rs", "fn main() {}")
        .await
        .unwrap();
    assert_eq!(
        provider.read_text("src/main.rs").await.unwrap(),
        "fn main() {}"
    );
    assert_eq!(
        provider.list("src").await.unwrap(),
        vec!["src/lib.rs", "src/main.rs"]
    );
    assert_eq!(
        provider.list("./src").await.unwrap(),
        vec!["src/lib.rs", "src/main.rs"]
    );
    assert_eq!(
        provider.list(".").await.unwrap(),
        vec!["src/lib.rs", "src/main.rs"]
    );
    assert_eq!(
        provider
            .list_with_options(
                ".",
                FileListOptions {
                    ignore_patterns: Vec::new(),
                    max_entries: 1,
                },
            )
            .await
            .unwrap(),
        FileListResult {
            entries: vec!["src/lib.rs".to_string()],
            truncated: true,
            total_entries: 2,
        }
    );
    assert_eq!(
        provider
            .list_with_options(
                ".",
                FileListOptions {
                    ignore_patterns: vec!["main".to_string()],
                    max_entries: 1,
                },
            )
            .await
            .unwrap(),
        FileListResult {
            entries: vec!["src/lib.rs".to_string()],
            truncated: false,
            total_entries: 1,
        }
    );
    assert_eq!(
        provider
            .run_shell(ShellCommand {
                command: "echo ok".to_string(),
                ..ShellCommand::default()
            })
            .await
            .unwrap(),
        output
    );
    let state = provider.export_state().await.unwrap();
    assert_eq!(state.provider_id, "test");
    assert_eq!(environment_provider_kind(&state), Some("virtual"));
    assert_eq!(state.files["src/main.rs"], "fn main() {}");
    assert_eq!(state.processes, vec![process.clone()]);

    let restored = VirtualEnvironmentProvider::from_state(state).unwrap();
    assert_eq!(restored.read_text("src/lib.rs").await.unwrap(), "content");
    assert_eq!(
        restored.read_text("src/main.rs").await.unwrap(),
        "fn main() {}"
    );
    assert_eq!(restored.list_processes().await.unwrap(), vec![process]);
}

#[tokio::test]
async fn environment_factory_registry_restores_virtual_provider_state() {
    let provider = VirtualEnvironmentProvider::new("factory")
        .with_file("README.md", "factory restore")
        .with_resource(ResourceRef {
            id: "resource-1".to_string(),
            uri: "resource://factory/artifact-1".to_string(),
            metadata: Metadata::from_iter([(
                "resource_kind".to_string(),
                serde_json::json!("media"),
            )]),
        });
    let state = provider.export_state().await.unwrap();
    assert_eq!(state.resources.len(), 1);
    let registry = EnvironmentProviderFactoryRegistry::portable_defaults();

    let restored = registry.restore(&state).unwrap();

    assert_eq!(restored.id(), "factory");
    assert_eq!(
        restored.read_text("README.md").await.unwrap(),
        "factory restore"
    );
    let restored_state = restored.export_state().await.unwrap();
    assert_eq!(restored_state.resources, state.resources);
}

#[derive(Debug)]
struct TestResourceRestoreFactory;

#[async_trait::async_trait]
impl ResourceRestoreFactory for TestResourceRestoreFactory {
    fn kind(&self) -> &'static str {
        "media"
    }

    async fn restore(&self, resource: &ResourceRef) -> EnvironmentResult<ResourceRef> {
        let mut restored = resource.clone();
        restored.uri = restored.uri.replace("s3://bucket/", "resource://restored/");
        restored
            .metadata
            .insert("restored".to_string(), serde_json::json!(true));
        Ok(restored)
    }
}

#[tokio::test]
async fn resource_restore_registry_restores_typed_resources_and_preserves_provider_refs() {
    let external = ResourceRef {
        id: "external-media".to_string(),
        uri: "s3://bucket/media.png".to_string(),
        metadata: Metadata::from_iter([(
            RESOURCE_REF_KIND_KEY.to_string(),
            serde_json::json!("media"),
        )]),
    };
    let provider_scoped = ResourceRef {
        id: "provider-scoped".to_string(),
        uri: "resource://provider-scoped/item".to_string(),
        metadata: Metadata::default(),
    };
    let registry =
        ResourceRestoreFactoryRegistry::new().with_factory(Arc::new(TestResourceRestoreFactory));

    let restored = registry
        .restore_all(&[external.clone(), provider_scoped.clone()])
        .await
        .unwrap();

    assert_eq!(resource_ref_kind(&external), Some("media"));
    assert_eq!(restored[0].uri, "resource://restored/media.png");
    assert_eq!(restored[0].metadata["restored"], serde_json::json!(true));
    assert_eq!(restored[1], provider_scoped);
    assert_eq!(
        registry.restore_required(&external).await.unwrap().uri,
        "resource://restored/media.png"
    );
    assert!(registry.restore_required(&provider_scoped).await.is_err());
}

#[tokio::test]
async fn virtual_provider_globs_and_greps_with_native_matchers() {
    let provider = VirtualEnvironmentProvider::new("test")
        .with_file("src/lib.rs", "pub fn library() {}\n")
        .with_file("src/main.rs", "fn main() { library(); }\n")
        .with_file("README.md", "library docs\n");

    let glob_matches = provider
        .glob("", "*.rs", FileGlobOptions::default())
        .await
        .unwrap();
    assert_eq!(
        glob_matches
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["src/lib.rs", "src/main.rs"]
    );

    let grep_matches = provider
        .grep(
            "",
            "library",
            FileGrepOptions {
                include: Some("**/*.rs".to_string()),
                context_lines: 0,
                max_results: 10,
                max_matches_per_file: 10,
                max_files: 50,
                include_hidden: false,
                include_ignored: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(grep_matches.len(), 2);
    assert_eq!(grep_matches[0].path, "src/lib.rs");
    assert_eq!(grep_matches[0].line_number, 1);
}

#[test]
fn path_glob_matches_ripgrep_style_patterns() {
    let bare = PathGlob::new("*.rs").unwrap();
    assert!(bare.is_match("lib.rs"));
    assert!(bare.is_match("src/lib.rs"));
    assert!(!bare.is_match("src/lib.py"));

    let recursive = PathGlob::new("**/*.rs").unwrap();
    assert!(recursive.is_match("lib.rs"));
    assert!(recursive.is_match("src/lib.rs"));

    let anchored_file = PathGlob::new("/*.rs").unwrap();
    assert!(anchored_file.is_match("lib.rs"));
    assert!(!anchored_file.is_match("src/lib.rs"));

    let scoped_dir = PathGlob::new("src/*.rs").unwrap();
    assert!(scoped_dir.is_match("src/lib.rs"));
    assert!(!scoped_dir.is_match("src/nested/mod.rs"));

    let anchored_dir = PathGlob::new("/src/*.rs").unwrap();
    assert!(anchored_dir.is_match("src/lib.rs"));
    assert!(!anchored_dir.is_match("src/nested/mod.rs"));
    assert!(!anchored_dir.is_match("nested/src/lib.rs"));
}

#[tokio::test]
async fn virtual_provider_search_respects_root_hidden_limits_and_invalid_patterns() {
    let provider = VirtualEnvironmentProvider::new("test")
        .with_file("src/lib.rs", "alpha\nbeta\nalpha again\n")
        .with_file("src/nested/mod.rs", "alpha nested\n")
        .with_file("tests/lib.rs", "alpha test\n")
        .with_file("src/.hidden.rs", "alpha hidden\n")
        .with_file("README.md", "alpha docs\n");

    let src_matches = provider
        .glob("src", "*.rs", FileGlobOptions::default())
        .await
        .unwrap();
    assert_eq!(
        src_matches
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["src/lib.rs", "src/nested/mod.rs"]
    );

    let hidden_default = provider
        .glob("src", ".*.rs", FileGlobOptions::default())
        .await
        .unwrap();
    assert!(hidden_default.is_empty());

    let hidden_included = provider
        .glob(
            "src",
            ".*.rs",
            FileGlobOptions {
                include_hidden: true,
                ..FileGlobOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(hidden_included[0].path, "src/.hidden.rs");

    let limited = provider
        .glob(
            "",
            "*.rs",
            FileGlobOptions {
                max_results: 1,
                ..FileGlobOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(limited.len(), 1);

    let grep_matches = provider
        .grep(
            "src",
            "alpha",
            FileGrepOptions {
                include: Some("**/*.rs".to_string()),
                context_lines: 1,
                max_results: 2,
                max_matches_per_file: 1,
                max_files: 50,
                include_hidden: false,
                include_ignored: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(grep_matches.len(), 2);
    assert_eq!(grep_matches[0].path, "src/lib.rs");
    assert_eq!(grep_matches[0].line_number, 1);
    assert_eq!(grep_matches[0].context_start_line, 1);
    assert!(grep_matches[0].context.contains("beta"));
    assert_eq!(grep_matches[1].path, "src/nested/mod.rs");

    assert!(matches!(
        provider.grep("", "(", FileGrepOptions::default()).await,
        Err(EnvironmentError::InvalidRequest(_))
    ));
    assert!(matches!(
        provider.glob("", "[", FileGlobOptions::default()).await,
        Err(EnvironmentError::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn local_provider_search_respects_gitignore_hidden_and_policy() {
    let root = unique_test_dir();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "needle\n").unwrap();
    std::fs::write(root.join("src/ignored.log"), "needle ignored\n").unwrap();
    std::fs::write(root.join(".hidden.rs"), "needle hidden\n").unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });

    let visible = provider
        .glob("", "**/*", FileGlobOptions::default())
        .await
        .unwrap();
    let visible_paths = visible
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert!(visible_paths.contains(&"src/lib.rs"));
    assert!(!visible_paths.contains(&"src/ignored.log"));
    assert!(!visible_paths.contains(&".hidden.rs"));

    let all_files = provider
        .glob(
            "",
            "**/*",
            FileGlobOptions {
                include_hidden: true,
                include_ignored: true,
                max_results: 0,
            },
        )
        .await
        .unwrap();
    let all_paths = all_files
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert!(all_paths.contains(&"src/ignored.log"));
    assert!(all_paths.contains(&".hidden.rs"));

    let grep_matches = provider
        .grep(
            "",
            "needle",
            FileGrepOptions {
                include: Some("**/*".to_string()),
                include_hidden: true,
                include_ignored: true,
                max_results: 0,
                max_matches_per_file: 0,
                max_files: 0,
                context_lines: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(grep_matches.len(), 3);

    let restricted = provider.with_policy(EnvironmentPolicy {
        files: FilePolicy {
            allow_read: true,
            allow_write: false,
            allowed_prefixes: vec!["src".to_string()],
        },
        shell: ShellPolicy::default(),
    });
    assert!(matches!(
        restricted
            .glob("README.md", "**/*", FileGlobOptions::default())
            .await,
        Err(EnvironmentError::AccessDenied(_))
    ));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_grep_streams_context_limits_and_binary_detection() {
    let root = unique_test_dir();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.txt"), "before\nneedle one\nafter\n").unwrap();
    std::fs::write(root.join("src/b.txt"), "needle two\nneedle three\n").unwrap();
    std::fs::write(root.join("src/binary.bin"), b"needle\0binary\n").unwrap();

    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });

    let context_matches = provider
        .grep(
            "",
            "needle one",
            FileGrepOptions {
                include: Some("**/*.txt".to_string()),
                include_hidden: false,
                include_ignored: false,
                max_results: 10,
                max_matches_per_file: 10,
                max_files: 10,
                context_lines: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(context_matches.len(), 1);
    assert_eq!(context_matches[0].path, "src/a.txt");
    assert_eq!(context_matches[0].line_number, 2);
    assert_eq!(context_matches[0].matching_line, "needle one");
    assert_eq!(context_matches[0].context_start_line, 1);
    assert_eq!(context_matches[0].context, "before\nneedle one\nafter\n");

    let limited_matches = provider
        .grep(
            "",
            "needle",
            FileGrepOptions {
                include: Some("**/*.txt".to_string()),
                include_hidden: false,
                include_ignored: false,
                max_results: 10,
                max_matches_per_file: 1,
                max_files: 10,
                context_lines: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        limited_matches
            .iter()
            .filter(|entry| entry.path == "src/b.txt")
            .count(),
        1
    );

    let binary_skipped = provider
        .grep(
            "",
            "binary",
            FileGrepOptions {
                include: Some("**/*".to_string()),
                include_hidden: true,
                include_ignored: true,
                max_results: 10,
                max_matches_per_file: 10,
                max_files: 10,
                context_lines: 0,
            },
        )
        .await
        .unwrap();
    assert!(binary_skipped.is_empty());

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_runs_background_shell_processes() {
    let root = unique_test_dir();
    std::fs::create_dir_all(&root).unwrap();
    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_write(),
        shell: ShellPolicy::allow_all(),
    });

    let started = provider
        .start_process(ShellCommand {
            command: "printf ready".to_string(),
            timeout_seconds: Some(5),
            ..ShellCommand::default()
        })
        .await
        .unwrap();
    assert_eq!(started.status, ShellProcessStatus::Running);
    assert_eq!(started.command, "printf ready");
    assert_eq!(started.metadata["timeout_seconds"], serde_json::json!(5));

    let completed = provider.wait_process(&started.process_id, 5).await.unwrap();
    assert_eq!(completed.status, ShellProcessStatus::Completed);
    assert_eq!(completed.stdout, "ready");
    assert_eq!(completed.return_code, Some(0));

    let listed = provider.list_processes().await.unwrap();
    assert!(listed.iter().any(|process| {
        process.process_id == started.process_id && process.status == ShellProcessStatus::Completed
    }));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_manages_tmp_files_as_allowed_absolute_paths() {
    let root = unique_test_dir();
    let external = unique_test_dir();
    let unrelated_tmp = std::env::temp_dir().join(format!(
        "starweaver-unrelated-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&unrelated_tmp, "secret").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([external.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

    let tmp_path = provider
        .write_tmp_file("stdout.log", b"full shell output")
        .await
        .unwrap();
    let tmp_path_buf = normalize_local_config_path(PathBuf::from(&tmp_path));
    assert!(tmp_path_buf.is_absolute());
    assert!(provider.path_is_managed_tmp(&tmp_path_buf));
    assert!(provider
        .allowed_paths()
        .iter()
        .any(|path| tmp_path_buf.starts_with(path)));
    assert_eq!(
        provider.read_text(&tmp_path).await.unwrap(),
        "full shell output"
    );
    assert!(!root.join(".starweaver/tmp/stdout.log").exists());
    assert_eq!(
        provider
            .read_text(".starweaver/tmp/stdout.log")
            .await
            .unwrap(),
        "full shell output"
    );
    assert!(matches!(
        provider
            .read_text(&unrelated_tmp.display().to_string())
            .await,
        Err(EnvironmentError::AccessDenied(_))
    ));

    let _ = std::fs::remove_file(unrelated_tmp);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(external).unwrap();
}

#[cfg(windows)]
#[test]
fn display_local_path_strips_windows_verbatim_prefixes() {
    assert_eq!(
        display_local_path(Path::new(
            r"\\?\C:\Users\runneradmin\AppData\Local\Temp\stdout.log"
        )),
        "C:/Users/runneradmin/AppData/Local/Temp/stdout.log"
    );
    assert_eq!(
        display_local_path(Path::new(r"\\?\UNC\server\share\stdout.log")),
        "//server/share/stdout.log"
    );
}

#[cfg(windows)]
fn windows_msys_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let (drive, rest) = normalized.split_once(":/").unwrap();
    format!("/{}/{}", drive.to_ascii_lowercase(), rest)
}

#[cfg(windows)]
#[tokio::test]
async fn local_provider_accepts_windows_verbatim_tmp_paths() {
    let root = unique_test_dir();
    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });

    let tmp_path = provider
        .write_tmp_file("stdout.log", b"full shell output")
        .await
        .unwrap();
    assert!(!tmp_path.starts_with("//?/"));
    assert_eq!(
        provider.read_text(&tmp_path).await.unwrap(),
        "full shell output"
    );

    let leaked_verbatim_path = format!("//?/{tmp_path}");
    assert_eq!(
        provider.read_text(&leaked_verbatim_path).await.unwrap(),
        "full shell output"
    );

    let msys_path = windows_msys_path(&tmp_path);
    assert_eq!(
        provider.read_text(&msys_path).await.unwrap(),
        "full shell output"
    );

    let temp_dir = display_local_path(&std::env::temp_dir());
    if let Some(relative) = tmp_path.strip_prefix(&format!("{}/", temp_dir.trim_end_matches('/'))) {
        let msys_tmp_path = format!("/tmp/{relative}");
        assert_eq!(
            provider.read_text(&msys_tmp_path).await.unwrap(),
            "full shell output"
        );
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_restores_from_trusted_state_with_explicit_policy() {
    let root = unique_test_dir();
    let external = unique_test_dir();
    std::fs::write(root.join("README.md"), "root").unwrap();
    std::fs::write(external.join("extra.txt"), "extra").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_id("local-test")
        .with_allowed_paths([external.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let state = provider.export_state().await.unwrap();

    let restored = LocalEnvironmentProvider::from_trusted_state(
        &state,
        EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        },
    )
    .unwrap();

    assert_eq!(restored.id(), "local-test");
    assert_eq!(restored.read_text("README.md").await.unwrap(), "root");
    assert_eq!(
        restored
            .read_text(external.join("extra.txt").to_str().unwrap())
            .await
            .unwrap(),
        "extra"
    );

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(external).unwrap();
}

#[tokio::test]
async fn local_provider_tmp_namespace_isolates_managed_tmp_files() {
    let root = unique_test_dir();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_tmp_namespace("session_123")
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

    let tmp_path = provider.write_tmp_file("grep.json", b"[]").await.unwrap();
    let tmp_path_buf = PathBuf::from(&tmp_path);
    assert!(tmp_path_buf.ends_with("session_123/grep.json"));
    assert_eq!(provider.read_text(&tmp_path).await.unwrap(), "[]");
    assert_eq!(
        provider
            .read_text(".starweaver/tmp/session_123/grep.json")
            .await
            .unwrap(),
        "[]"
    );
    assert!(tmp_path_buf
        .parent()
        .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "session_123")));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn virtual_provider_tmp_namespace_isolates_tmp_files() {
    let provider = VirtualEnvironmentProvider::new("virtual").with_tmp_namespace("session_123");

    let tmp_path = provider.write_tmp_file("grep.json", b"[]").await.unwrap();
    assert_eq!(tmp_path, ".starweaver/tmp/session_123/grep.json");
    assert_eq!(provider.read_text(&tmp_path).await.unwrap(), "[]");
    assert!(matches!(
        provider.read_text(".starweaver/tmp/grep.json").await,
        Err(EnvironmentError::NotFound(_))
    ));
}

#[tokio::test]
async fn local_provider_tmp_base_dir_places_managed_tmp_under_base() {
    let root = unique_test_dir();
    let tmp_base = unique_test_dir();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_tmp_base_dir(tmp_base.clone())
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

    let normalized_tmp_base = normalize_local_config_path(tmp_base.clone());
    let tmp_dir = normalize_local_config_path(provider.tmp_dir_path().unwrap().to_path_buf());
    assert!(tmp_dir.starts_with(&normalized_tmp_base));
    assert!(tmp_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(LOCAL_TMP_DIR_PREFIX));
    let tmp_path = provider.write_tmp_file("grep.json", b"[]").await.unwrap();
    assert!(normalize_local_config_path(PathBuf::from(&tmp_path)).starts_with(&tmp_dir));
    assert_eq!(provider.read_text(&tmp_path).await.unwrap(), "[]");

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(tmp_base).unwrap();
}

#[tokio::test]
async fn local_provider_search_preserves_gitignore_negations() {
    let root = unique_test_dir();
    std::fs::create_dir_all(root.join("ignored")).unwrap();
    std::fs::create_dir_all(root.join("other_ignored")).unwrap();
    std::fs::write(root.join("ignored/keep.txt"), "needle keep\n").unwrap();
    std::fs::write(root.join("ignored/drop.txt"), "needle drop\n").unwrap();
    std::fs::write(root.join("other_ignored/drop.txt"), "needle other\n").unwrap();
    std::fs::write(
        root.join(".gitignore"),
        "ignored/*\nother_ignored/\n!ignored/keep.txt\n",
    )
    .unwrap();

    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });

    let glob_matches = provider
        .glob(
            "",
            "**/*.txt",
            FileGlobOptions {
                max_results: 0,
                ..FileGlobOptions::default()
            },
        )
        .await
        .unwrap();
    let glob_paths = glob_matches
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert!(glob_paths.contains(&"ignored/keep.txt"));
    assert!(!glob_paths.contains(&"ignored/drop.txt"));
    assert!(!glob_paths.contains(&"other_ignored/drop.txt"));

    let grep_matches = provider
        .grep(
            "",
            "needle",
            FileGrepOptions {
                include: Some("**/*.txt".to_string()),
                include_hidden: false,
                include_ignored: false,
                max_results: 0,
                max_matches_per_file: 0,
                max_files: 0,
                context_lines: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(grep_matches.len(), 1);
    assert_eq!(grep_matches[0].path, "ignored/keep.txt");

    let include_ignored = provider
        .glob(
            "",
            "**/*.txt",
            FileGlobOptions {
                include_hidden: false,
                include_ignored: true,
                max_results: 0,
            },
        )
        .await
        .unwrap();
    let include_ignored_paths = include_ignored
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert!(include_ignored_paths.contains(&"ignored/drop.txt"));
    assert!(include_ignored_paths.contains(&"other_ignored/drop.txt"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn virtual_context_file_tree_matches_starweaver_sdk_semantics() {
    let provider = VirtualEnvironmentProvider::new("test")
        .with_file(".agents/skills/research/SKILL.md", "skill")
        .with_file(".git/config", "git")
        .with_file(".gitignore", "*.log\nbuild/\n")
        .with_file(".hidden/secret.txt", "secret")
        .with_file(".env", "ENV=value")
        .with_file("README.md", "readme")
        .with_file("build/output.js", "built")
        .with_file("error.log", "log")
        .with_file("level1/level2/level3/file.txt", "too deep")
        .with_file("node_modules/package.json", "{}")
        .with_file("src/main.rs", "fn main() {}")
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert!(instructions.contains("<environment-context>"));
    assert!(instructions.contains("<file-system>"));
    assert!(instructions.contains("<default-directory>.</default-directory>"));
    assert!(!instructions.contains("<tmp-directory>"));
    assert!(instructions.contains("<file-trees>"));
    assert!(instructions.contains("<directory path=\".\">"));
    assert!(!instructions.contains("<file>"));
    assert!(instructions.contains(".git/ (skipped)"));
    assert!(instructions.contains("node_modules/ (skipped)"));
    assert!(instructions.contains("build/ (gitignored)"));
    assert!(instructions.contains("error.log (gitignored)"));
    assert!(instructions.contains(".agents/skills/research/SKILL.md"));
    assert!(instructions.contains(".env"));
    assert!(instructions.contains("README.md"));
    assert!(instructions.contains("src/main.rs"));
    assert!(!instructions.contains(".hidden"));
    assert!(!instructions.contains(".gitignore"));
    assert!(!instructions.contains("package.json"));
    assert!(!instructions.contains("build/output.js"));
    assert!(!instructions.contains("level1/level2/level3/file.txt"));
}

#[tokio::test]
async fn local_context_file_tree_matches_starweaver_sdk_semantics() {
    let root = unique_test_dir();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join(".agents/skills/research")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::create_dir_all(root.join("build")).unwrap();
    std::fs::create_dir_all(root.join("level1/level2/level3")).unwrap();
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".git/config"), "git").unwrap();
    std::fs::write(root.join(".agents/skills/research/SKILL.md"), "skill").unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
    std::fs::write(root.join(".hidden/secret.txt"), "secret").unwrap();
    std::fs::write(root.join(".env"), "ENV=value").unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(root.join("build/output.js"), "built").unwrap();
    std::fs::write(root.join("error.log"), "log").unwrap();
    std::fs::write(root.join("level1/level2/level3/file.txt"), "too deep").unwrap();
    std::fs::write(root.join("node_modules/package.json"), "{}").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    let tmp_dir = display_local_path(provider.tmp_dir_path().unwrap());
    assert!(instructions.contains(&format!("<tmp-directory>{tmp_dir}</tmp-directory>")));
    assert!(instructions.contains(
        "<tmp-directory-note>This is an agent-only temporary directory for intermediate files."
    ));
    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&root)
    )));
    assert!(!instructions.contains("<file>"));
    assert!(instructions.contains(".git/ (skipped)"));
    assert!(instructions.contains("node_modules/ (skipped)"));
    assert!(instructions.contains("build/ (gitignored)"));
    assert!(instructions.contains("error.log (gitignored)"));
    assert!(instructions.contains(".agents/skills/research/SKILL.md"));
    assert!(instructions.contains(".env"));
    assert!(instructions.contains("README.md"));
    assert!(instructions.contains("src/main.rs"));
    assert!(!instructions.contains(".hidden"));
    assert!(!instructions.contains(".gitignore"));
    assert!(!instructions.contains("package.json"));
    assert!(!instructions.contains("build/output.js"));
    assert!(!instructions.contains("level1/level2/level3/file.txt"));

    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn local_context_file_tree_marks_permission_denied_directories() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_test_dir();
    let restricted = root.join("Documents");
    std::fs::create_dir_all(&restricted).unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o000)).unwrap();

    if std::fs::read_dir(&restricted).is_ok() {
        std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        return;
    }

    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert!(instructions.contains("Documents/ (permission denied)"));
    assert!(instructions.contains("README.md"));

    std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_accepts_allowed_absolute_paths_and_rejects_unsafe_paths() {
    let root = unique_test_dir();
    let external = unique_test_dir();
    std::fs::create_dir_all(external.join("research")).unwrap();
    std::fs::write(root.join("safe..name.txt"), "ok").unwrap();
    std::fs::write(external.join("research/SKILL.md"), "skill").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([external.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

    assert_eq!(
        provider.read_text("safe..name.txt").await.unwrap(),
        "ok".to_string()
    );
    assert_eq!(
        provider
            .read_text(&external.join("research/SKILL.md").display().to_string())
            .await
            .unwrap(),
        "skill".to_string()
    );
    assert_eq!(
        provider
            .list(&external.display().to_string())
            .await
            .unwrap(),
        vec!["research"]
    );
    let matches = provider
        .glob(
            &external.display().to_string(),
            "*/SKILL.md",
            FileGlobOptions {
                include_hidden: true,
                include_ignored: true,
                max_results: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        matches,
        vec![FileGlobMatch {
            path: display_local_path(&external.join("research/SKILL.md")),
        }]
    );
    #[cfg(not(windows))]
    assert!(matches!(
        provider.read_text("/etc/passwd").await,
        Err(EnvironmentError::AccessDenied(_))
    ));
    #[cfg(windows)]
    assert!(matches!(
        provider.read_text("/etc/passwd").await,
        Err(EnvironmentError::InvalidRequest(_))
    ));
    assert!(matches!(
        provider.read_text("../outside.txt").await,
        Err(EnvironmentError::InvalidRequest(_))
    ));
    assert!(matches!(
        provider
            .read_text(&format!("{}/../outside.txt", external.display()))
            .await,
        Err(EnvironmentError::InvalidRequest(_))
    ));

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(external).unwrap();
}

#[tokio::test]
async fn local_provider_list_with_options_filters_and_limits_sorted_entries() {
    let root = unique_test_dir();
    std::fs::write(root.join("zeta.txt"), "z").unwrap();
    std::fs::write(root.join("alpha.log"), "a").unwrap();
    std::fs::write(root.join("beta.txt"), "b").unwrap();
    std::fs::write(root.join("gamma.txt"), "g").unwrap();
    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });

    let listing = provider
        .list_with_options(
            ".",
            FileListOptions {
                ignore_patterns: vec![".log".to_string()],
                max_entries: 2,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        listing,
        FileListResult {
            entries: vec!["beta.txt".to_string(), "gamma.txt".to_string()],
            truncated: true,
            total_entries: 3,
        }
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_context_file_tree_includes_allowed_external_roots() {
    let root = unique_test_dir();
    let external = unique_test_dir();
    std::fs::create_dir_all(external.join("skills/research")).unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(external.join("skills/research/SKILL.md"), "skill").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([external.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert!(instructions.contains(&format!(
        "<default-directory>{}</default-directory>",
        display_local_path(&root)
    )));
    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&root)
    )));
    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&external)
    )));
    assert!(instructions.contains("README.md"));
    assert!(instructions.contains("skills/research/SKILL.md"));

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(external).unwrap();
}

#[tokio::test]
async fn local_context_file_tree_deduplicates_visible_nested_allowed_roots() {
    let root = unique_test_dir();
    std::fs::create_dir_all(root.join("skills/research")).unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(root.join("skills/research/SKILL.md"), "skill").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([root.join("skills")])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        instructions
            .matches(&format!(
                "<directory path=\"{}\">",
                display_local_path(&root)
            ))
            .count(),
        1
    );
    assert!(!instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&root.join("skills"))
    )));
    assert!(instructions.contains("skills/research/SKILL.md"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_context_file_tree_deduplicates_visible_agents_skill_roots() {
    let root = unique_test_dir();
    let allowed_root = root.join(".agents/skills");
    std::fs::create_dir_all(allowed_root.join("research")).unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(allowed_root.join("research/SKILL.md"), "skill").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([allowed_root.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        instructions
            .matches(&format!(
                "<directory path=\"{}\">",
                display_local_path(&root)
            ))
            .count(),
        1
    );
    assert!(!instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&allowed_root)
    )));
    assert!(instructions.contains(".agents/skills/research/SKILL.md"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_context_file_tree_keeps_hidden_nested_allowed_roots() {
    let root = unique_test_dir();
    let hidden = root.join(".starweaver/skills/research");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(hidden.join("SKILL.md"), "skill").unwrap();
    let allowed_root = root.join(".starweaver/skills");
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([allowed_root.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&root)
    )));
    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&allowed_root)
    )));
    assert!(instructions.contains("research/SKILL.md"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_context_file_tree_keeps_gitignored_nested_allowed_roots() {
    let root = unique_test_dir();
    let allowed_root = root.join("ignored/skills");
    std::fs::create_dir_all(&allowed_root).unwrap();
    std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(allowed_root.join("SKILL.md"), "skill").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([allowed_root.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&root)
    )));
    assert!(instructions.contains("ignored/ (gitignored)"));
    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&allowed_root)
    )));
    assert!(instructions.contains("SKILL.md"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_context_file_tree_keeps_deep_nested_allowed_roots() {
    let root = unique_test_dir();
    let allowed_root = root.join("level1/level2/level3");
    std::fs::create_dir_all(&allowed_root).unwrap();
    std::fs::write(root.join("README.md"), "readme").unwrap();
    std::fs::write(allowed_root.join("SKILL.md"), "skill").unwrap();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_allowed_paths([allowed_root.clone()])
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
    let instructions = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();

    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&root)
    )));
    assert!(instructions.contains(&format!(
        "<directory path=\"{}\">",
        display_local_path(&allowed_root)
    )));
    assert!(instructions.contains("SKILL.md"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_runs_shell_with_cwd_environment_and_policy() {
    let root = unique_test_dir();
    std::fs::create_dir_all(root.join("work")).unwrap();
    std::fs::write(root.join("work/input.txt"), "content").unwrap();
    let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::allow_all(),
    });

    let output = provider
        .run_shell(ShellCommand {
            command: "printf '%s:%s' \"$STARWEAVER_TEST\" \"$(pwd | sed 's#.*/##')\"".to_string(),
            cwd: Some("work".to_string()),
            environment: BTreeMap::from([("STARWEAVER_TEST".to_string(), "ok".to_string())]),
            ..ShellCommand::default()
        })
        .await
        .unwrap();
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout, "ok:work");

    let denied = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
        files: FilePolicy::read_only(),
        shell: ShellPolicy::default(),
    });
    assert!(matches!(
        denied
            .run_shell(ShellCommand {
                command: "echo denied".to_string(),
                ..ShellCommand::default()
            })
            .await,
        Err(EnvironmentError::AccessDenied(_))
    ));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_shell_tmpdir_uses_managed_namespace() {
    let root = unique_test_dir();
    let provider = LocalEnvironmentProvider::new(&root)
        .with_tmp_namespace("session_123")
        .with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::allow_all(),
        });

    let output = provider
            .run_shell(ShellCommand {
                command: "printf managed > \"$TMPDIR/clippy-sdk-filter.txt\"; printf '%s' \"$TMPDIR/clippy-sdk-filter.txt\"".to_string(),
                ..ShellCommand::default()
            })
            .await
            .unwrap();
    assert_eq!(output.status, 0);
    let path = output.stdout;
    assert!(path.contains("session_123"));
    let normalized_path = normalize_absolute_request_path(Path::new(&path)).unwrap();
    assert!(provider.path_is_managed_tmp(&normalized_path));
    assert_eq!(provider.read_text(&path).await.unwrap(), "managed");

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_provider_background_shell_tmpdir_uses_managed_namespace() {
    let root = unique_test_dir();
    let provider = Arc::new(
        LocalEnvironmentProvider::new(&root)
            .with_tmp_namespace("session_123")
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::allow_all(),
            }),
    );
    let process_provider = provider.clone().process_shell_provider().unwrap();

    let started = process_provider
            .start_process(ShellCommand {
                command: "printf managed > \"$TMPDIR/background.txt\"; printf '%s' \"$TMPDIR/background.txt\"".to_string(),
                ..ShellCommand::default()
            })
            .await
            .unwrap();
    let completed = process_provider
        .wait_process(&started.process_id, 5)
        .await
        .unwrap();
    assert_eq!(completed.status, ShellProcessStatus::Completed);
    let path = completed.stdout;
    assert!(path.contains("session_123"));
    let normalized_path = normalize_absolute_request_path(Path::new(&path)).unwrap();
    assert!(provider.path_is_managed_tmp(&normalized_path));
    assert_eq!(provider.read_text(&path).await.unwrap(), "managed");

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn policy_denies_disallowed_file_access() {
    let provider = VirtualEnvironmentProvider::new("test").with_policy(EnvironmentPolicy {
        files: FilePolicy::default(),
        shell: ShellPolicy::default(),
    });
    assert!(matches!(
        provider.read_text("secret").await,
        Err(EnvironmentError::AccessDenied(_))
    ));
}

#[tokio::test]
async fn composite_provider_routes_default_and_environment_namespace_paths() {
    let workspace = Arc::new(
        VirtualEnvironmentProvider::new("workspace")
            .with_file("README.md", "workspace")
            .with_file("environment/README.md", "default environment dir")
            .with_file("src/lib.rs", "lib"),
    );
    let data = Arc::new(
        VirtualEnvironmentProvider::new("data")
            .with_file("README.md", "data")
            .with_file("table.csv", "x,y\n1,2\n"),
    );
    let provider = CompositeEnvironmentProvider::new(vec![
        EnvironmentMount::new("workspace", workspace)
            .unwrap()
            .with_default(true)
            .with_default_for_shell(true),
        EnvironmentMount::new("data", data)
            .unwrap()
            .with_mode(EnvironmentMountMode::ReadOnly),
    ])
    .unwrap();

    assert_eq!(provider.read_text("README.md").await.unwrap(), "workspace");
    assert_eq!(
        provider.read_text("environment/README.md").await.unwrap(),
        "default environment dir"
    );
    assert_eq!(
        provider
            .read_text("/environment/data/README.md")
            .await
            .unwrap(),
        "data"
    );
    assert_eq!(
        provider.list("/environment").await.unwrap(),
        vec![
            "/environment/workspace".to_string(),
            "/environment/data".to_string()
        ]
    );
    assert_eq!(
        provider.list("/environment/data").await.unwrap(),
        vec![
            "/environment/data/README.md".to_string(),
            "/environment/data/table.csv".to_string()
        ]
    );
    assert!(matches!(
        provider.write_text("/environment/data/new.txt", "no").await,
        Err(EnvironmentError::AccessDenied(_))
    ));
    provider.write_text("new.txt", "ok").await.unwrap();
    assert_eq!(provider.read_text("new.txt").await.unwrap(), "ok");
}

#[tokio::test]
async fn composite_provider_rebases_local_mount_list_entries_from_subdirs() {
    let workspace_root = unique_test_dir().join("workspace");
    let data_root = unique_test_dir().join("data");
    std::fs::create_dir_all(workspace_root.join("environment")).unwrap();
    std::fs::create_dir_all(data_root.join("src")).unwrap();
    std::fs::write(
        workspace_root.join("environment").join("note.txt"),
        "default",
    )
    .unwrap();
    std::fs::write(data_root.join("src").join("lib.rs"), "data").unwrap();
    std::fs::write(data_root.join("src").join("main.rs"), "data").unwrap();
    let workspace = Arc::new(LocalEnvironmentProvider::new(&workspace_root));
    let data = Arc::new(LocalEnvironmentProvider::new(&data_root));
    let provider = CompositeEnvironmentProvider::new(vec![
        EnvironmentMount::new("workspace", workspace)
            .unwrap()
            .with_default(true)
            .with_default_for_shell(true),
        EnvironmentMount::new("data", data).unwrap(),
    ])
    .unwrap();

    assert_eq!(
        provider.read_text("environment/note.txt").await.unwrap(),
        "default"
    );
    assert_eq!(
        provider.list("/environment/data/src").await.unwrap(),
        vec![
            "/environment/data/src/lib.rs".to_string(),
            "/environment/data/src/main.rs".to_string()
        ]
    );
    assert_eq!(
        provider
            .list_with_options(
                "/environment/data/src",
                FileListOptions {
                    ignore_patterns: Vec::new(),
                    max_entries: 1,
                },
            )
            .await
            .unwrap(),
        FileListResult {
            entries: vec!["/environment/data/src/lib.rs".to_string()],
            truncated: true,
            total_entries: 2,
        }
    );
}

#[tokio::test]
async fn composite_provider_routes_provider_visible_absolute_shell_cwd() {
    let workspace_root = unique_test_dir().join("workspace");
    std::fs::create_dir_all(workspace_root.join("nested")).unwrap();
    let workspace = Arc::new(LocalEnvironmentProvider::new(&workspace_root).with_policy(
        EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::allow_all(),
        },
    ));
    let provider =
        CompositeEnvironmentProvider::new(vec![EnvironmentMount::new("workspace", workspace)
            .unwrap()
            .with_default(true)
            .with_default_for_shell(true)])
        .unwrap();
    let absolute_cwd = display_local_path(&workspace_root.join("nested"));

    let output = provider
        .run_shell(ShellCommand {
            command: "pwd".to_string(),
            cwd: Some(absolute_cwd.clone()),
            ..ShellCommand::default()
        })
        .await
        .unwrap();

    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.trim(), absolute_cwd);

    std::fs::remove_dir_all(workspace_root.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn composite_provider_does_not_render_non_default_file_trees() {
    let workspace = Arc::new(
        VirtualEnvironmentProvider::new("workspace").with_file("default-only.txt", "visible"),
    );
    let data =
        Arc::new(VirtualEnvironmentProvider::new("data").with_file("hidden-data.csv", "secret"));
    let provider = CompositeEnvironmentProvider::new(vec![
        EnvironmentMount::new("workspace", workspace)
            .unwrap()
            .with_default(true)
            .with_default_for_shell(true),
        EnvironmentMount::new("data", data).unwrap(),
    ])
    .unwrap();

    let context = provider
        .render_environment_context()
        .await
        .unwrap()
        .unwrap();
    assert!(context.contains("/environment/data"));
    assert!(context.contains("default-only.txt"));
    assert!(!context.contains("hidden-data.csv"));
}

#[tokio::test]
async fn composite_provider_routes_shell_by_cwd_and_process_id() {
    let workspace = Arc::new(
        VirtualEnvironmentProvider::new("workspace").with_shell_output(
            "echo workspace",
            ShellOutput {
                status: 0,
                stdout: "workspace\n".to_string(),
                stderr: String::new(),
                metadata: Metadata::default(),
            },
        ),
    );
    let data = Arc::new(VirtualEnvironmentProvider::new("data").with_shell_output(
        "echo data",
        ShellOutput {
            status: 0,
            stdout: "data\n".to_string(),
            stderr: String::new(),
            metadata: Metadata::default(),
        },
    ));
    let provider = Arc::new(
        CompositeEnvironmentProvider::new(vec![
            EnvironmentMount::new("workspace", workspace)
                .unwrap()
                .with_default(true)
                .with_default_for_shell(true),
            EnvironmentMount::new("data", data).unwrap(),
        ])
        .unwrap(),
    );

    let workspace_output = provider
        .run_shell(ShellCommand {
            command: "echo workspace".to_string(),
            ..ShellCommand::default()
        })
        .await
        .unwrap();
    assert_eq!(workspace_output.stdout, "workspace\n");

    let data_output = provider
        .run_shell(ShellCommand {
            command: "echo data".to_string(),
            cwd: Some("/environment/data".to_string()),
            ..ShellCommand::default()
        })
        .await
        .unwrap();
    assert_eq!(data_output.stdout, "data\n");

    let process_provider = provider.clone().process_shell_provider().unwrap();
    let started = process_provider
        .start_process(ShellCommand {
            command: "sleep 1".to_string(),
            cwd: Some("/environment/data/jobs".to_string()),
            ..ShellCommand::default()
        })
        .await
        .unwrap();
    assert_eq!(started.process_id, "data:process_1");
    assert_eq!(
        started.metadata.get("cwd"),
        Some(&serde_json::json!("/environment/data/jobs"))
    );
    let waited = process_provider
        .wait_process(&started.process_id, 0)
        .await
        .unwrap();
    assert_eq!(waited.process_id, "data:process_1");
    let listed = process_provider.list_processes().await.unwrap();
    assert!(listed
        .iter()
        .any(|process| process.process_id == "data:process_1"));
}

#[tokio::test]
async fn composite_provider_allows_no_default_shell_mount() {
    let data = Arc::new(VirtualEnvironmentProvider::new("data"));
    let provider = CompositeEnvironmentProvider::new(vec![EnvironmentMount::new("data", data)
        .unwrap()
        .with_mode(EnvironmentMountMode::ReadOnly)
        .with_default(true)])
    .unwrap();

    assert!(matches!(
        provider
            .run_shell(ShellCommand {
                command: "echo no-default-shell".to_string(),
                ..ShellCommand::default()
            })
            .await,
        Err(EnvironmentError::InvalidRequest(_))
    ));
    let state = provider.export_state().await.unwrap();
    let mounts = state.metadata["mounts"].as_array().unwrap();
    assert_eq!(mounts[0]["default_for_shell"], false);
}

#[tokio::test]
async fn composite_provider_supports_text_cross_mount_copy_and_move() {
    let workspace =
        Arc::new(VirtualEnvironmentProvider::new("workspace").with_file("README.md", "workspace"));
    let data = Arc::new(VirtualEnvironmentProvider::new("data").with_file("source.txt", "data"));
    let provider = CompositeEnvironmentProvider::new(vec![
        EnvironmentMount::new("workspace", workspace)
            .unwrap()
            .with_default(true)
            .with_default_for_shell(true),
        EnvironmentMount::new("data", data).unwrap(),
    ])
    .unwrap();

    provider
        .copy_path("/environment/data/source.txt", "copied.txt", false)
        .await
        .unwrap();
    assert_eq!(provider.read_text("copied.txt").await.unwrap(), "data");

    provider
        .move_path("README.md", "/environment/data/moved.txt", false)
        .await
        .unwrap();
    assert_eq!(
        provider
            .read_text("/environment/data/moved.txt")
            .await
            .unwrap(),
        "workspace"
    );
    assert!(matches!(
        provider.read_text("README.md").await,
        Err(EnvironmentError::NotFound(_))
    ));
}

fn unique_test_dir() -> PathBuf {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "starweaver-env-test-{}-{:?}-{suffix}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap_or(path)
}
