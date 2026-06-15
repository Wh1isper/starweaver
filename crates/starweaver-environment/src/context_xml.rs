use std::path::Path;

use starweaver_core::XmlWriter;

const TMP_DIRECTORY_CONTEXT_NOTE: &str = "This is an agent-only temporary directory for intermediate files. Never write deliverables or user-facing files here. Files the user needs to access must be written to the project directory. Never mention this path to the user.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTreeBlock {
    pub(super) path: String,
    pub(super) listing_text: String,
}

pub fn render_environment_context_xml(
    provider_id: &str,
    default_directory: &str,
    tmp_directory: Option<String>,
    file_trees: &[FileTreeBlock],
    shell_enabled: bool,
    shell_metadata: Option<ShellMetadata>,
) -> String {
    let mut xml = XmlWriter::new();
    xml.open("environment-context")
        .open("file-system")
        .text_element("provider-id", provider_id)
        .text_element("default-directory", default_directory)
        .open("file-trees");
    for file_tree in file_trees {
        if !file_tree.listing_text.is_empty() {
            xml.text_block_element_attrs(
                "directory",
                [("path", file_tree.path.as_str())],
                &file_tree.listing_text,
            );
        }
    }
    xml.close("file-trees");
    if let Some(tmp_directory) = tmp_directory {
        xml.text_element("tmp-directory", tmp_directory)
            .text_element("tmp-directory-note", TMP_DIRECTORY_CONTEXT_NOTE);
    }
    xml.close("file-system");

    if shell_enabled {
        xml.open("shell-execution");
        if let Some(metadata) = shell_metadata {
            xml.text_element("platform", metadata.platform)
                .text_element("shell-type", metadata.shell_type)
                .text_element("shell-executable", metadata.shell_executable);
        }
        xml.close("shell-execution");
    }

    xml.close("environment-context");
    xml.finish()
}

pub struct ShellMetadata {
    platform: &'static str,
    shell_type: String,
    shell_executable: String,
}

pub fn local_shell_metadata() -> ShellMetadata {
    let shell_executable = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let shell_type = Path::new(&shell_executable).file_name().map_or_else(
        || "sh".to_string(),
        |name| name.to_string_lossy().to_string(),
    );
    ShellMetadata {
        platform: std::env::consts::OS,
        shell_type,
        shell_executable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_trees_are_rendered_before_volatile_tmp_directory() {
        let xml = render_environment_context_xml(
            "local",
            "/workspace",
            Some("/tmp/starweaver-run-1".to_string()),
            &[FileTreeBlock {
                path: "/workspace".to_string(),
                listing_text: "Cargo.toml".to_string(),
            }],
            false,
            None,
        );

        let Some(file_trees_index) = xml.find("<file-trees>") else {
            panic!("environment context should include file trees: {xml}");
        };
        let Some(tmp_directory_index) = xml.find("<tmp-directory>") else {
            panic!("environment context should include tmp directory: {xml}");
        };
        assert!(file_trees_index < tmp_directory_index);
        assert!(xml.contains("Cargo.toml"));
        assert!(xml.contains("/tmp/starweaver-run-1"));
    }
}
