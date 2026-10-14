// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// The desktop app with all six instruments in the browser, as WebAssembly

use core::time::Duration;

use wasm_bindgen::prelude::*;
use web_time::Instant;

// the demo values of the desktop app
#[path = "../../src/demo.rs"]
mod demo;

slint::include_modules!();

/// Started by the page when the module is loaded: shows the instruments in the canvas with the
/// id "canvas" and animates them with the demo values
#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    let ui = SixPack::new().map_err(|e| JsValue::from_str(&e.to_string()))?;

    // demo movement until real sensor values are available, the same as in the desktop app
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
    // the event loop of the browser keeps running after this function returned, the timer
    // must stay alive with it
    core::mem::forget(timer);

    ui.run().map_err(|e| JsValue::from_str(&e.to_string()))
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
