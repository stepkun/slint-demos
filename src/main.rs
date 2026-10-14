// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only

// Prevent console window in addition to Slint window in Windows release builds when,
// e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::time::{Duration, Instant};

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    // use the same renderer as on the rp2040, so the desktop shows what the display will show
    slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()?;

    let ui = AppWindow::new()?;

    // demo movement until real sensor values are available, the same as in the firmware
    let start = Instant::now();
    let ui_handle = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(20),
        move || {
            let Some(ui) = ui_handle.upgrade() else {
                return;
            };
            let time = start.elapsed();
            ui.set_roll(triangle(time, Duration::from_secs(8), 30.0));
            ui.set_pitch(triangle(time, Duration::from_secs(5), 10.0));
            ui.set_altitude(5000.0 + triangle(time, Duration::from_secs(60), 5000.0));
            ui.set_airspeed(105.0 + triangle(time, Duration::from_secs(30), 75.0));
            // a steady turn, once around per minute
            ui.set_heading((time.as_millis() % 60_000) as f32 * 0.006);
            // a little beyond the scale, so the needle reaches its stop
            ui.set_vertical_speed(triangle(time, Duration::from_secs(40), 2200.0));
            // turns beyond the standard rate (3 degree per second), the ball swinging a little
            ui.set_turn_rate(triangle(time, Duration::from_secs(30), 5.0));
            ui.set_slip(triangle(time, Duration::from_secs(12), 0.6));
        },
    );

    ui.run()?;

    Ok(())
}

/// Triangle wave between -amplitude and +amplitude with the given period
fn triangle(time: Duration, period: Duration, amplitude: f32) -> f32 {
    let period = period.as_millis();
    let phase = (time.as_millis() % period) as f32 / period as f32;
    let value = if phase < 0.5 {
        phase * 4.0 - 1.0
    } else {
        3.0 - phase * 4.0
    };
    value * amplitude
}
