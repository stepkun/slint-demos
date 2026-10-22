// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Waveshare RP2040-LCD-1.28-B (round 240x240 GC9A01 display, no touch)

#![no_std]
#![no_main]

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_rp::clocks::ClockConfig;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::DMA_CH0;
use embassy_rp::pwm::{self, Pwm, SetDutyCycle};
use embassy_rp::spi::{self, Spi};
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use firmware_common::display::{self, ChipSelect};
use firmware_common::{Sequence, crash, demo};
use mipidsi::interface::SpiInterface;
use mipidsi::models::GC9A01;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use slint::platform::software_renderer::Rgb565BigEndianPixel;

slint::include_modules!();

// the round display is a 240x240 square with the corners cut off by the glass, the instruments
// are round and fill the circle; with a square screen the ui leaves out the menu area
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 240;
// System clock of the RP2040: its default (the highest officially qualified is 200 MHz).
// A higher clock is the largest possible speed-up: the code runs from the flash through a 16 KB
// cache, the flash at half the system clock, and about half of the time of a frame is spent
// waiting for it (measured on the Pico-ResTouch-LCD-2.8, ../waveshare-ResTouch-LCD-2.8). A clock
// up to 200 MHz (system_freq raises the core voltage to 1.15 V) can make the instruments up to
// 45 % faster.
const SYSTEM_CLOCK: u32 = 135_000_000;
// SPI clock for the display: the system clock divided by an even number, the SPI can't divide
// by anything else, a frequency in between is rounded down to the next one. 62.5 MHz at 125 MHz;
// the GC9A01 datasheet specifies a write cycle of at least 15 ns (66 MHz).
// if the picture shows speckles or wrong colors, use SYSTEM_CLOCK / 4
const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 2;
// backlight brightness in percent, there is no touch for the slider of the menu area
const BRIGHTNESS: u8 = 100;
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// RP2040 has 264 KB RAM: 112.5 KB frame buffer, 128 KB heap, the rest for stack and statics
// The instruments request at most 58 KB at this size (tools/host-check --display 240x240),
// the larger heap of this board leaves more room than the 92 KB of the Pico-ResTouch-LCD-2.8.
const HEAP_SIZE: usize = 128 * 1024;

// a full frame buffer is needed, because the software renderer
// skips Path elements when rendering line by line
// the pixels are stored in the byte order of the GC9A01, so it is sent unchanged by DMA
static mut FRAME_BUFFER: [Rgb565BigEndianPixel; DISPLAY_WIDTH * DISPLAY_HEIGHT] =
    [Rgb565BigEndianPixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];

bind_interrupts!(struct Irqs {
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

/// On a crash the backlight shows what happened:
/// - blinking: out of heap memory
/// - dark: any other panic
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // the backlight is on GP25
    crash::signal(&crash::RP2040, 25, SYSTEM_CLOCK)
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

    // LCD on SPI1: CLK GP10, DIN GP11, CS GP9, DC GP8, RST GP12, backlight GP25 (PWM)
    // The display is only written, so the SPI has no MISO and needs no receive DMA channel.
    // The QMI8658 IMU (I2C1 on GP6 and GP7) is not used yet.
    let mut spi_config = spi::Config::default();
    spi_config.frequency = DISPLAY_SPI_FREQUENCY;
    let spi = Spi::new_txonly(p.SPI1, p.PIN_10, p.PIN_11, p.DMA_CH0, Irqs, spi_config);
    // shared between mipidsi and the DMA transfers
    let lcd_cs = RefCell::new(Output::new(p.PIN_9, Level::High));
    let lcd_dc = Output::new(p.PIN_8, Level::Low);
    let lcd_rst = Output::new(p.PIN_12, Level::Low);
    // dimmable backlight: GP25 is channel B of PWM slice 4,
    // 12.5 kHz (125 MHz / 10 / 1000) does not flicker
    let mut backlight_config = pwm::Config::default();
    backlight_config.divider = 10.into();
    backlight_config.top = 999;
    let mut backlight = Pwm::new_output_b(p.PWM_SLICE4, p.PIN_25, backlight_config);
    let _ = backlight.set_duty_cycle_percent(BRIGHTNESS);

    let spi_device = ExclusiveDevice::new(spi, ChipSelect(&lcd_cs), Delay).unwrap();
    let mut interface_buffer = [0u8; 512];
    let interface = SpiInterface::new(spi_device, lcd_dc, &mut interface_buffer);

    // the panel needs inverted colours and BGR order, like in the Waveshare examples
    // if colors look wrong, try ColorInversion::Normal or ColorOrder::Rgb
    // mipidsi is only used to initialize the display
    let display = mipidsi::Builder::new(GC9A01, interface)
        .display_size(240, 240)
        .orientation(Orientation::new().rotate(Rotation::Deg0))
        .invert_colors(ColorInversion::Inverted)
        .color_order(ColorOrder::Bgr)
        .reset_pin(lcd_rst)
        .init(&mut Delay)
        .unwrap();
    // take the bus back for the DMA transfers, the reset pin must stay alive
    let (interface, _model, _lcd_rst) = display.release();
    let (mut spi_device, mut lcd_dc) = interface.release();
    let spi = spi_device.bus_mut();

    let window = firmware_common::init_window(DISPLAY_WIDTH, DISPLAY_HEIGHT, uptime);

    let ui = BasicPanelDevice::new().unwrap();
    // a square screen: only the instrument, without the menu area
    ui.set_screen_width(DISPLAY_WIDTH as f32);
    ui.set_screen_height(DISPLAY_HEIGHT as f32);
    ui.show().unwrap();

    // SAFETY: the only reference to the frame buffer, created once
    let frame_buffer: &mut [Rgb565BigEndianPixel] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // the instruments one after the other: without touch or buttons the automatic sequence is
    // the only way to see them all
    let mut sequence = Sequence::default();

    loop {
        let frame_start = Instant::now();
        let time = uptime();

        // demo movement until real sensor values are available
        firmware_common::show!(ui, demo::values(time));
        if sequence.next(true, time) {
            ui.set_instrument(firmware_common::next_instrument(ui.get_instrument()));
        }

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
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
