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
    DeleteArgs, EditArgs, FilePathArgs, GlobArgs, GrepArgs, ListArgs, MkdirArgs, MultiEditArgs,
    PathPairsArgs, ViewArgs, WriteArgs, default_view_line_limit, default_view_max_line_length,
};
use crate::bundles::helpers::tool_execution_error;

use context::tool_config_from_context;
use editing::{edit_text, multi_edit_text, write_text};
use paths::{copy_paths, delete_paths, list_files, mkdir_paths, move_paths, resource_ref};
use reading::read_text;
use search::{glob_files, grep_files};
use text::{format_size, read_text_file, truncate_chars};

const SKILL_DOCUMENT_NAME: &str = "SKILL.md";
const SKILL_DOCUMENT_REMINDER: &str = "Skill documents were found in the results. Before applying a skill, read each relevant SKILL.md in full with `view`; paths and grep snippets are not sufficient to assess applicability, and discovery alone does not activate a skill.";

fn is_skill_document(path: &str) -> bool {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .is_some_and(|name| name == SKILL_DOCUMENT_NAME)
}

fn add_skill_document_reminder(result: &mut serde_json::Value) {
    result["system-reminder"] = serde_json::Value::String(SKILL_DOCUMENT_REMINDER.to_string());
}

/// Create filesystem tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
pub fn filesystem_tools() -> DynToolset {
    instructions::filesystem_tools()
}
