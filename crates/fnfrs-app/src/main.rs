use anyhow::Result;
use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;

fn main() -> Result<()> {
    let assets = AssetLocator::discover_or_default();
    println!(
        "fnfrs-app initialized ({}x{}, assets root: {})",
        VIRTUAL_WIDTH,
        VIRTUAL_HEIGHT,
        assets.root().display()
    );
    Ok(())
}
