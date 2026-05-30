#![allow(missing_docs, clippy::unwrap_used)]

use std::process::Command;

#[test]
fn cli_prints_sdk_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"))
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "starweaver-agent-sdk\n"
    );
}
