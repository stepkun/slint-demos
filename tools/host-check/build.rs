// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // The font sizes of the instruments scale with their size, so they are no literals the
    // compiler could collect: embed the glyphs of the font sizes of Theme.
    // SAFETY: the build script is single-threaded, nothing reads the environment concurrently.
    unsafe { std::env::set_var("SLINT_FONT_SIZES", aircraft_gauges::FONT_SIZES) };

    // build the slint artefacts from the shared ui, with resources embedded for the software
    // renderer, like the firmware does
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        .with_library_paths(aircraft_gauges::library_paths());
    slint_build::compile_with_config("../../ui/basic-panel-device.slint", config)
        .expect("Slint build failed");
}
