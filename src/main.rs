// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only

// Prevent console window in addition to Slint window in Windows release builds when,
// e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::time::{Duration, Instant};

mod demo;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    // use the same renderer as on the rp2040, so the desktop shows what the display will show
    slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()?;

    // all six instruments at once, in the standard layout
    let ui = SixPack::new()?;

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

/// Shows the values on the instruments
fn show(ui: &SixPack, values: demo::Values) {
    ui.set_roll(values.roll);
    ui.set_pitch(values.pitch);
    ui.set_altitude(values.altitude);
    ui.set_airspeed(values.airspeed);
    ui.set_heading(values.heading);
    ui.set_vertical_speed(values.vertical_speed);
    ui.set_turn_rate(values.turn_rate);
    ui.set_slip(values.slip);
}
