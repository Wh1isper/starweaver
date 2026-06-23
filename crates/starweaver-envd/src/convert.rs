//! Conversion between envd DTOs and environment provider types.

use starweaver_envd_core::{
    EnvdError, FileListResult, ProcessSnapshot, ProcessStatus, ResourceRef,
};
use starweaver_environment::{EnvironmentError, FileGlobOptions, FileGrepOptions, FileListOptions};

pub fn env_error_to_envd(error: EnvironmentError) -> EnvdError {
    match error {
        EnvironmentError::AccessDenied(message) => EnvdError::access_denied(message),
        EnvironmentError::NotFound(message) => EnvdError::not_found(message),
        EnvironmentError::InvalidRequest(message) => EnvdError::invalid_request(message),
        EnvironmentError::Provider(message) => EnvdError::provider(message),
    }
}

pub const fn file_stat_to_envd(
    stat: &starweaver_environment::FileStat,
) -> starweaver_envd_core::FileStat {
    starweaver_envd_core::FileStat {
        size: stat.size,
        is_file: stat.is_file,
        is_dir: stat.is_dir,
        modified_unix_seconds: stat.modified_unix_seconds,
    }
}

pub fn list_options_from_envd(options: starweaver_envd_core::FileListOptions) -> FileListOptions {
    FileListOptions {
        ignore_patterns: options.ignore_patterns,
        max_entries: options.max_entries,
    }
}

pub fn list_result_to_envd(result: starweaver_environment::FileListResult) -> FileListResult {
    FileListResult {
        entries: result.entries,
        truncated: result.truncated,
        total_entries: result.total_entries,
    }
}

pub const fn glob_options_from_envd(
    options: &starweaver_envd_core::FileGlobOptions,
) -> FileGlobOptions {
    FileGlobOptions {
        include_hidden: options.include_hidden,
        include_ignored: options.include_ignored,
        max_results: options.max_results,
    }
}

pub fn grep_options_from_envd(options: starweaver_envd_core::FileGrepOptions) -> FileGrepOptions {
    FileGrepOptions {
        include: options.include,
        context_lines: options.context_lines,
        max_results: options.max_results,
        max_matches_per_file: options.max_matches_per_file,
        max_files: options.max_files,
        include_hidden: options.include_hidden,
        include_ignored: options.include_ignored,
    }
}

pub fn resource_to_envd(resource: starweaver_environment::ResourceRef) -> ResourceRef {
    ResourceRef {
        id: resource.id,
        uri: resource.uri,
        metadata: resource.metadata,
    }
}

pub fn process_to_envd(process: starweaver_environment::ShellProcessSnapshot) -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: process.process_id,
        command: process.command,
        status: match process.status {
            starweaver_environment::ShellProcessStatus::Running => ProcessStatus::Running,
            starweaver_environment::ShellProcessStatus::Completed => ProcessStatus::Completed,
            starweaver_environment::ShellProcessStatus::Failed => ProcessStatus::Failed,
            starweaver_environment::ShellProcessStatus::Killed => ProcessStatus::Killed,
        },
        stdout: process.stdout,
        stderr: process.stderr,
        return_code: process.return_code,
        metadata: process.metadata,
    }
}
