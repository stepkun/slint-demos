// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // the desktop app shows all six instruments at once (the device shows one at a time,
    // with ui/basic-panel-device.slint), the binary `combined` the combined display: both windows in
    // ui/desktop.slint
    // the instruments come from the aircraft-gauges library
    let config = slint_build::CompilerConfiguration::new()
        .with_library_paths(aircraft_gauges::library_paths());
    slint_build::compile_with_config("ui/desktop.slint", config).expect("Slint build failed");
}
