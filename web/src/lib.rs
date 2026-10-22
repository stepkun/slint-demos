// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// The desktop apps in the browser, as WebAssembly: the basic panel with all six instruments
// (basic-panel.html) and the combined display (combined.html)

use core::time::Duration;

use wasm_bindgen::prelude::*;
use web_time::Instant;

// the demo values of the desktop app
#[path = "../../src/demo.rs"]
mod demo;

slint::include_modules!();

/// Called by basic-panel.html when the module is loaded: shows all six instruments in the canvas with
/// the id "canvas" and animates them with the demo values
#[wasm_bindgen]
pub fn basic_panel() -> Result<(), JsValue> {
    let ui = BasicPanel::new().map_err(|e| JsValue::from_str(&e.to_string()))?;
    animate(&ui, |ui, values| {
        ui.set_roll(values.roll);
        ui.set_pitch(values.pitch);
        ui.set_altitude(values.altitude);
        ui.set_airspeed(values.airspeed);
        ui.set_heading(values.heading);
        ui.set_vertical_speed(values.vertical_speed);
        ui.set_turn_rate(values.turn_rate);
        ui.set_slip(values.slip);
    });
    ui.run().map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Called by combined.html when the module is loaded: the combined display (attitude, vertical
/// speed, altitude, heading and airspeed) in the canvas with the id "canvas", animated with the
/// demo values
#[wasm_bindgen]
pub fn combined() -> Result<(), JsValue> {
    let ui = CombinedPanel::new().map_err(|e| JsValue::from_str(&e.to_string()))?;
    animate(&ui, |ui, values| {
        ui.set_roll(values.roll);
        ui.set_pitch(values.pitch);
        ui.set_altitude(values.altitude);
        ui.set_airspeed(values.airspeed);
        ui.set_heading(values.heading);
        ui.set_vertical_speed(values.vertical_speed);
    });
    ui.run().map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Demo movement until real sensor values are available, the same as in the desktop app
fn animate<T: slint::ComponentHandle + 'static>(ui: &T, show: fn(&T, demo::Values)) {
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
    // the event loop of the browser keeps running after the start function returned, the timer
    // must stay alive with it
    core::mem::forget(timer);
}
