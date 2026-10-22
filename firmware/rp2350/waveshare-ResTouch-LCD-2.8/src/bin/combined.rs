// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Raspberry Pi Pico 2 (RP2350) with a Waveshare Pico-ResTouch-LCD-2.8: the
// combined display of aircraft-gauges (attitude, vertical speed, altitude, heading and airspeed)
// on the whole display in landscape, with the demo values. `cargo run --release --bin combined`
// The same hardware setup as the firmware with one instrument and the menu (src/main.rs); no
// menu, a tap on the display shows or hides the frame rate and the heap peak. Not for the
// RP2040: the combined display needs about 85 KB of heap.

#![no_std]
#![no_main]

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_rp::clocks::ClockConfig;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1};
use embassy_rp::spi::{self, Spi};
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use firmware_common::display::{self, ChipSelect};
use firmware_common::{Statistics, crash, demo};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use slint::platform::software_renderer::Rgb565BigEndianPixel;

slint::include_modules!();

// the display in landscape: the 240x320 panel turned by 90 degree
// if the picture is upside down, use Rotation::Deg270
const ROTATION: Rotation = Rotation::Deg90;
const DISPLAY_WIDTH: usize = 320;
const DISPLAY_HEIGHT: usize = 240;
// the clocks of the firmware with one instrument, see src/main.rs
const SYSTEM_CLOCK: u32 = 150_000_000;
const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 2;
// the XPT2046 touch controller shares SPI1, it is read at 2 MHz (its maximum clock)
const TOUCH_SPI_FREQUENCY: u32 = 2_000_000;
// shortest time between two taps: the panel can bounce while it is pressed
const TAP_TIME: Duration = Duration::from_millis(300);
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// 150 KB frame buffer and 256 KB heap of the 520 KB RAM, like the firmware with one instrument;
// the combined display needs about 85 KB
const HEAP_SIZE: usize = 256 * 1024;

// a full frame buffer is needed, because the software renderer
// skips Path elements when rendering line by line
// the pixels are stored in the byte order of the ST7789, so it is sent unchanged by DMA
static mut FRAME_BUFFER: [Rgb565BigEndianPixel; DISPLAY_WIDTH * DISPLAY_HEIGHT] =
    [Rgb565BigEndianPixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];

bind_interrupts!(struct Irqs {
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>;
});

/// On a crash the backlight shows what happened:
/// - blinking: out of heap memory
/// - dark: any other panic
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // the backlight is on GP13
    crash::signal(&crash::RP2350, 13, SYSTEM_CLOCK)
}

/// Time since the start, for Slint and the demo
fn uptime() -> core::time::Duration {
    core::time::Duration::from_micros(Instant::now().as_micros())
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // the heap must be ready before slint is used
    firmware_common::init_heap!(HEAP_SIZE);

    let clocks = ClockConfig::system_freq(SYSTEM_CLOCK).unwrap();
    let p = embassy_rp::init(embassy_rp::config::Config::new(clocks));

    // LCD on SPI1: CLK GP10, MOSI GP11, CS GP9, DC GP8, RST GP15, backlight GP13
    // touch controller XPT2046 on the same bus: MISO GP12, CS GP16, interrupt GP17
    let mut spi_config = spi::Config::default();
    spi_config.frequency = DISPLAY_SPI_FREQUENCY;
    let mut spi = Spi::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_12, p.DMA_CH0, p.DMA_CH1, Irqs, spi_config,
    );
    // the touch is only a tap, from its interrupt (open drain, low while touched): one
    // conversion with power down between conversions turns that interrupt on, the position is
    // not read
    let mut touch_cs = Output::new(p.PIN_16, Level::High);
    let touch_irq = Input::new(p.PIN_17, Pull::Up);
    spi.set_frequency(TOUCH_SPI_FREQUENCY);
    touch_cs.set_low();
    let _ = spi.blocking_write(&[0xb0, 0, 0]);
    touch_cs.set_high();
    spi.set_frequency(DISPLAY_SPI_FREQUENCY);
    // the microSD slot of the board is on SPI1 too (CS GP22): kept deselected
    let _sd_cs = Output::new(p.PIN_22, Level::High);
    // shared between mipidsi and the DMA transfers
    let lcd_cs = RefCell::new(Output::new(p.PIN_9, Level::High));
    let lcd_dc = Output::new(p.PIN_8, Level::Low);
    let lcd_rst = Output::new(p.PIN_15, Level::Low);
    // full brightness, there is no slider
    let _backlight = Output::new(p.PIN_13, Level::High);

    let spi_device = ExclusiveDevice::new(spi, ChipSelect(&lcd_cs), Delay).unwrap();
    let mut interface_buffer = [0u8; 512];
    let interface = SpiInterface::new(spi_device, lcd_dc, &mut interface_buffer);

    // mipidsi is only used to initialize the display, its rotation stays set in the controller,
    // which then takes the rows of the frame buffer as rows of the landscape picture
    let display = mipidsi::Builder::new(ST7789, interface)
        .display_size(240, 320)
        .orientation(Orientation::new().rotate(ROTATION))
        .invert_colors(ColorInversion::Inverted)
        .reset_pin(lcd_rst)
        .init(&mut Delay)
        .unwrap();
    // take the bus back for the DMA transfers, the reset pin must stay alive
    let (interface, _model, _lcd_rst) = display.release();
    let (mut spi_device, mut lcd_dc) = interface.release();
    let spi = spi_device.bus_mut();

    let window = firmware_common::init_window(DISPLAY_WIDTH, DISPLAY_HEIGHT, uptime);

    let ui = CombinedPanel::new().unwrap();
    ui.show().unwrap();

    // SAFETY: the only reference to the frame buffer, created once
    let frame_buffer: &mut [Rgb565BigEndianPixel] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // frame statistics, shown once per second while the status is on
    let mut statistics = Statistics::new(uptime());
    let mut show_status = false;
    // whether the display was touched in the last frame, and when the last tap started
    let mut touched = false;
    let mut last_tap = Instant::from_ticks(0);

    loop {
        let frame_start = Instant::now();

        // demo movement until real sensor values are available
        let values = demo::values(uptime());
        ui.set_roll(values.roll);
        ui.set_pitch(values.pitch);
        ui.set_altitude(values.altitude);
        ui.set_airspeed(values.airspeed);
        ui.set_heading(values.heading);
        ui.set_vertical_speed(values.vertical_speed);

        // a tap turns the status on or off
        let touching = touch_irq.is_low();
        if touching && !touched && frame_start - last_tap >= TAP_TIME {
            last_tap = frame_start;
            show_status = !show_status;
            if !show_status {
                ui.set_status(slint::SharedString::new());
            }
        }
        touched = touching;

        slint::platform::update_timers_and_animations();

        if let Some((first_row, last_row, rows)) =
            firmware_common::render(&window, frame_buffer, DISPLAY_WIDTH)
        {
            display::send_rows(
                spi,
                &mut lcd_dc,
                &lcd_cs,
                DISPLAY_WIDTH,
                (first_row, last_row),
                rows,
            )
            .await;
            statistics.frame();
        }

        if let Some(status) = statistics.status(uptime())
            && show_status
        {
            ui.set_status(status);
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
