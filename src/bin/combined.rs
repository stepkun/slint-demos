// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only

//! The combined display of aircraft-gauges (attitude, vertical speed, altitude, heading and
//! airspeed in 320x240) with the demo values, like the app with all six instruments in
//! `src/bin/basic-panel.rs`: `cargo run --bin combined`

// Prevent console window in addition to Slint window in Windows release builds when,
// e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::time::{Duration, Instant};

// the demo values of the app with all six instruments; turn rate and slip are not shown here
#[path = "../demo.rs"]
#[allow(dead_code)]
mod demo;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    // use the same renderer as on the microcontrollers, so the desktop shows what the display
    // of the RP2350 firmware will show
    slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()?;

    let ui = CombinedPanel::new()?;

    // demo movement until real sensor values are available, the same as in the firmware
    let start = Instant::now();
    let ui_handle = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(20),
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                show(&ui, demo::values(start.elapsed()));
            }
        },
    );

    ui.run()?;

    Ok(())
}

/// Shows the values on the combined display
fn show(ui: &CombinedPanel, values: demo::Values) {
    ui.set_roll(values.roll);
    ui.set_pitch(values.pitch);
    ui.set_altitude(values.altitude);
    ui.set_airspeed(values.airspeed);
    ui.set_heading(values.heading);
    ui.set_vertical_speed(values.vertical_speed);
}
