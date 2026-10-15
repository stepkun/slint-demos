// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // the linker scripts (linkall.x) come with esp-hal, there is no memory.x

    // The font sizes of the instruments scale with their size, so they are no literals the
    // compiler could collect: embed the glyphs of Theme.menu_font_size, Theme.font_size and
    // Theme.large_font_size (keep in sync with ui/theme.slint and the other build.rs files).
    // SAFETY: the build script is single-threaded, nothing reads the environment concurrently.
    unsafe { std::env::set_var("SLINT_FONT_SIZES", "12,14,20") };

    // build the slint artefacts from the shared ui, with resources embedded for the software renderer
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("../../../ui/test-app.slint", config)
        .expect("Slint build failed");
}
