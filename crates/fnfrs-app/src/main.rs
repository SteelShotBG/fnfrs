use anyhow::Result;
use fnfrs_audio::AudioEngine;
use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;

fn main() -> Result<()> {
    let assets = AssetLocator::discover_or_default();
    let audio_status = match AudioEngine::try_new() {
        Ok(engine) => format!("cpal active at {} Hz", engine.sample_rate()),
        Err(err) => format!("audio unavailable ({err})"),
    };
    println!(
        "fnfrs-app initialized ({}x{}, assets root: {}, {audio_status})",
        VIRTUAL_WIDTH,
        VIRTUAL_HEIGHT,
        assets.root().display()
    );
    Ok(())
}
