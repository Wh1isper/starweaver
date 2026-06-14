//! Filesystem tool bundle.

mod context;
mod editing;
mod instructions;
mod media;
mod output;
mod paths;
mod reading;
mod search;
mod text;

use starweaver_tools::DynToolset;

use super::args::{
    default_view_line_limit, default_view_max_line_length, DeleteArgs, EditArgs, FilePathArgs,
    GlobArgs, GrepArgs, ListArgs, MkdirArgs, MultiEditArgs, PathPairsArgs, ViewArgs, WriteArgs,
};
use crate::bundles::helpers::tool_execution_error;

use context::tool_config_from_context;
use editing::{edit_text, multi_edit_text, write_text};
use paths::{copy_paths, delete_paths, list_files, mkdir_paths, move_paths, resource_ref};
use reading::read_text;
use search::{glob_files, grep_files};
use text::{format_size, read_text_file, truncate_chars};

/// Create filesystem tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
pub fn filesystem_tools() -> DynToolset {
    instructions::filesystem_tools()
}
