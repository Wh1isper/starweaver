//! Environment provider abstractions for filesystem, shell, and resource access.

mod composite_provider;
mod context_xml;
mod envd_provider;
mod error;
mod factory;
mod file_tree;
mod local_io;
mod local_provider;
mod path;
mod policy;
mod provider;
mod search;
mod shell;
mod switchable_provider;
mod types;
mod virtual_provider;

pub(crate) use context_xml::{FileTreeBlock, local_shell_metadata, render_environment_context_xml};
pub(crate) use file_tree::{
    DEFAULT_FILE_TREE_MAX_DEPTH, file_tree_directory_depth_increment,
    file_tree_directory_is_visible, render_local_file_tree_listing,
    render_virtual_file_tree_listing,
};
pub(crate) use local_io::{
    copy_local_dir, create_local_tmp_dir, map_io_error, prepare_local_destination,
};
pub(crate) use path::{
    DEFAULT_TMP_DIR, LOCAL_TMP_DIR_PREFIX, PathGlob, display_local_path, include_path,
    is_absolute_request_path, is_provider_visible_absolute_path, is_tmp_path, join_logical_path,
    logical_ancestors, normalize_absolute_request_path, normalize_local_config_path,
    normalize_path, normalize_requested_path, normalize_str_path, normalize_tmp_filename,
    normalize_tmp_namespace, parent_path, path_contains, provider_visible_path_allowed_by_context,
    push_shell_review_context_path_candidates, push_unique_candidate, push_unique_path,
    replace_logical_prefix, strip_path_prefix,
};
pub use path::{matches_path_pattern, normalize_match_path, path_match_candidates};
pub(crate) use search::{
    LocalGrepSink, local_grep_file_match_limit, local_search_walk_builder, search_text,
};
pub(crate) use shell::{
    local_shell_command, read_child_pipe, refresh_local_shell_process, run_local_shell_command,
    shell_process_metadata,
};

pub use composite_provider::{
    CompositeEnvironmentProvider, EnvironmentMount, EnvironmentMountMode,
};
pub use envd_provider::{
    ENVD_ENVIRONMENT_ID_KEY, ENVD_KIND_KEY, ENVD_STATE_VERSION_KEY, ENVD_STORE_KEY,
    EnvdEnvironmentProvider,
};
pub use error::{EnvironmentError, EnvironmentResult};
pub use factory::{
    DynEnvironmentProviderFactory, DynResourceRestoreFactory, ENVIRONMENT_PROVIDER_KIND_KEY,
    EnvironmentProviderFactory, EnvironmentProviderFactoryRegistry, RESOURCE_REF_KIND_KEY,
    ResourceRestoreFactory, ResourceRestoreFactoryRegistry, TrustedLocalEnvironmentProviderFactory,
    VirtualEnvironmentProviderFactory, environment_provider_kind, resource_ref_kind,
};
pub use local_provider::LocalEnvironmentProvider;
pub use policy::{EnvironmentPolicy, FilePolicy, ShellPolicy};
pub use provider::{
    DynEnvironmentProvider, DynProcessShellProvider, EnvironmentProvider, ProcessShellProvider,
};
pub use switchable_provider::{SwitchableEnvironmentProvider, SwitchableEnvironmentTarget};
pub use types::{
    EnvironmentState, FileGlobMatch, FileGlobOptions, FileGrepMatch, FileGrepOptions,
    FileListOptions, FileListResult, FileStat, ResourceRef, ShellCommand, ShellOutput,
    ShellProcessSnapshot, ShellProcessStatus, ShellReviewEnvironmentContext,
};
pub use virtual_provider::VirtualEnvironmentProvider;

#[cfg(test)]
mod tests;

pub(crate) fn list_ignore_match(patterns: &[String], entry: &str) -> bool {
    patterns.iter().any(|pattern| {
        entry == pattern
            || entry.ends_with(pattern)
            || entry.contains(pattern)
            || pattern
                .strip_suffix('/')
                .is_some_and(|prefix| entry.starts_with(prefix))
    })
}
