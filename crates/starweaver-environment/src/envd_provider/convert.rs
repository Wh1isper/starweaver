//! Conversion helpers for envd-backed environment providers.

use starweaver_envd_core::{EnvdError, EnvdErrorCode, ProcessStatus, ShellReviewContextResult};

use crate::{
    EnvironmentError, FileGlobOptions, FileGrepOptions, FileListOptions, FileListResult, FileStat,
    ResourceRef, ShellProcessSnapshot, ShellProcessStatus, ShellReviewEnvironmentContext,
};

pub(super) fn envd_error_to_environment(error: EnvdError) -> EnvironmentError {
    match error.code {
        EnvdErrorCode::InvalidRequest => EnvironmentError::InvalidRequest(error.message),
        EnvdErrorCode::AccessDenied => EnvironmentError::AccessDenied(error.message),
        EnvdErrorCode::NotFound => EnvironmentError::NotFound(error.message),
        EnvdErrorCode::Unsupported | EnvdErrorCode::Provider => {
            EnvironmentError::Provider(error.message)
        }
    }
}

pub(super) const fn file_stat_from_envd(stat: &starweaver_envd_core::FileStat) -> FileStat {
    FileStat {
        size: stat.size,
        is_file: stat.is_file,
        is_dir: stat.is_dir,
        modified_unix_seconds: stat.modified_unix_seconds,
    }
}

pub(super) fn file_list_options_to_envd(
    options: FileListOptions,
) -> starweaver_envd_core::FileListOptions {
    starweaver_envd_core::FileListOptions {
        ignore_patterns: options.ignore_patterns,
        max_entries: options.max_entries,
    }
}

pub(super) fn file_list_result_from_envd(
    result: starweaver_envd_core::FileListResult,
) -> FileListResult {
    FileListResult {
        entries: result.entries,
        truncated: result.truncated,
        total_entries: result.total_entries,
    }
}

pub(super) const fn file_glob_options_to_envd(
    options: &FileGlobOptions,
) -> starweaver_envd_core::FileGlobOptions {
    starweaver_envd_core::FileGlobOptions {
        include_hidden: options.include_hidden,
        include_ignored: options.include_ignored,
        max_results: options.max_results,
    }
}

pub(super) fn file_grep_options_to_envd(
    options: FileGrepOptions,
) -> starweaver_envd_core::FileGrepOptions {
    starweaver_envd_core::FileGrepOptions {
        include: options.include,
        context_lines: options.context_lines,
        max_results: options.max_results,
        max_matches_per_file: options.max_matches_per_file,
        max_files: options.max_files,
        include_hidden: options.include_hidden,
        include_ignored: options.include_ignored,
    }
}

pub(super) fn resource_from_envd(resource: starweaver_envd_core::ResourceRef) -> ResourceRef {
    ResourceRef {
        id: resource.id,
        uri: resource.uri,
        metadata: resource.metadata,
    }
}

pub(super) fn process_from_envd(
    process: starweaver_envd_core::ProcessSnapshot,
) -> ShellProcessSnapshot {
    ShellProcessSnapshot {
        process_id: process.process_id,
        command: process.command,
        status: match process.status {
            ProcessStatus::Running => ShellProcessStatus::Running,
            ProcessStatus::Completed => ShellProcessStatus::Completed,
            ProcessStatus::Failed => ShellProcessStatus::Failed,
            ProcessStatus::Killed => ShellProcessStatus::Killed,
        },
        stdout: process.stdout,
        stderr: process.stderr,
        return_code: process.return_code,
        metadata: process.metadata,
    }
}

impl From<ShellReviewContextResult> for ShellReviewEnvironmentContext {
    fn from(context: ShellReviewContextResult) -> Self {
        Self {
            default_cwd: context.default_cwd,
            allowed_paths: context.allowed_paths,
            shell_platform: context.shell_platform,
            shell_executable: context.shell_executable,
        }
    }
}
