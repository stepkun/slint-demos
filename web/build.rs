// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // the same ui as the desktop app: all six instruments in the standard layout and the
    // combined display, both windows in ui/desktop.slint
    let config = slint_build::CompilerConfiguration::new()
        .with_library_paths(aircraft_gauges::library_paths());
    slint_build::compile_with_config("../ui/desktop.slint", config).expect("Slint build failed");
}
