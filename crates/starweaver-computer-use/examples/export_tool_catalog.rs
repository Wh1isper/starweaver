//! Print the canonical Computer Use V1 tool catalog fixture.

use starweaver_computer_use::ComputerToolCatalog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&ComputerToolCatalog::canonical_fixture())?
    );
    Ok(())
}
