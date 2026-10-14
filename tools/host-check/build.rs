// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // build the slint artefacts from the shared ui, with resources embedded for the software
    // renderer, like the firmware does
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("../../ui/test-app.slint", config)
        .expect("Slint build failed");
}
