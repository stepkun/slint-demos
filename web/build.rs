// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Build file or necessary steps before the src compile runs

fn main() {
    // the same ui as the desktop app: all six instruments in the standard layout
    slint_build::compile("../ui/six-pack.slint").expect("Slint build failed");
}
