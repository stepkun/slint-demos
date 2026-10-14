// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // the desktop app shows all six instruments at once (the device shows one at a time,
    // with ui/test-app.slint)
    slint_build::compile("ui/six-pack.slint").expect("Slint build failed");
}
