// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Demo values of the instruments, shared by the desktop app, the web demo (web/) and the
// firmwares (firmware/common)

use core::time::Duration;

/// Values of all instruments at one point in time
pub struct Values {
    /// bank angle in degree, positive is a right bank
    pub roll: f32,
    /// pitch angle in degree, positive is nose up
    pub pitch: f32,
    /// altitude in feet
    pub altitude: f32,
    /// indicated airspeed in knots
    pub airspeed: f32,
    /// heading in degree
    pub heading: f32,
    /// vertical speed in feet per minute
    pub vertical_speed: f32,
    /// rate of turn in degree per second
    pub turn_rate: f32,
    /// position of the ball, -1 and 1 are the ends of the tube
    pub slip: f32,
}

/// Demo values `time` after the start, until real sensor values are available
pub fn values(time: Duration) -> Values {
    Values {
        roll: triangle(time, Duration::from_secs(8), 30.0),
        pitch: triangle(time, Duration::from_secs(5), 10.0),
        altitude: 5000.0 + triangle(time, Duration::from_secs(60), 5000.0),
        airspeed: 105.0 + triangle(time, Duration::from_secs(30), 75.0),
        // a steady turn, once around per minute
        heading: (time.as_millis() % 60_000) as f32 * 0.006,
        // a little beyond the scale, so the needle reaches its stop
        vertical_speed: triangle(time, Duration::from_secs(40), 2200.0),
        // turns beyond the standard rate (3 degree per second), the ball swinging a little
        turn_rate: triangle(time, Duration::from_secs(30), 5.0),
        slip: triangle(time, Duration::from_secs(12), 0.6),
    }
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
