// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // The font sizes of the instruments scale with their size, so they are no literals the
    // compiler could collect: embed the glyphs of Theme.font_size and Theme.large_font_size
    // (keep in sync with ui/theme.slint).
    // SAFETY: the build script is single-threaded, nothing reads the environment concurrently.
    unsafe { std::env::set_var("SLINT_FONT_SIZES", "12,14") };

    // build the slint artefacts from the shared ui, with resources embedded for the software
    // renderer, like the firmware does
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("../../ui/test-app.slint", config)
        .expect("Slint build failed");
}
