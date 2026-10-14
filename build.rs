// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // build the slint artefacts
    slint_build::compile("ui/test-app.slint").expect("Slint build failed");
}
