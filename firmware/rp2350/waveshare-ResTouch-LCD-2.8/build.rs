// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // put the memory layout where the linker finds it
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // The font sizes of the instruments scale with their size, so they are no literals the
    // compiler could collect: embed the glyphs of the font sizes of Theme.
    // SAFETY: the build script is single-threaded, nothing reads the environment concurrently.
    unsafe { std::env::set_var("SLINT_FONT_SIZES", aircraft_gauges::FONT_SIZES) };

    // build the slint artefacts from the shared ui, with resources embedded for the software
    // renderer: the window with one instrument and the menu, and the combined display of the
    // binary `combined`, both in ui/device.slint
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        .with_library_paths(aircraft_gauges::library_paths());
    slint_build::compile_with_config("../../../ui/device.slint", config)
        .expect("Slint build failed");
}
