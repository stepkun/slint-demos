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

    // build the slint artefacts from the shared ui, with resources embedded for the software renderer
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("../../../ui/test-app.slint", config)
        .expect("Slint build failed");
}
