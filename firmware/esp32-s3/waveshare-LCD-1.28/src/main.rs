// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Waveshare ESP32-S3-LCD-1.28 (round 240x240 GC9A01 display, no touch)
// The esp-hal setup of the UNIHIKER K10 in ../unihiker-K10, the display of the round Pico
// boards (../../rp2040/waveshare-LCD-1.28-B). The BOOT button selects the instrument.

#![no_std]
#![no_main]

use core::cell::RefCell;

use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{DriveMode, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::ledc::channel::{self, ChannelIFace};
use esp_hal::ledc::timer::{self, TimerIFace};
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed};
use esp_hal::main;
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::{dma_rx_buffer, dma_tx_buffer};
use esp_println::println;
use firmware_common::display::{self, ChipSelect};
use firmware_common::{Sequence, Statistics, demo};
use mipidsi::interface::SpiInterface;
use mipidsi::models::GC9A01;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use slint::platform::software_renderer::Rgb565BigEndianPixel;

slint::include_modules!();

// application descriptor, the bootloader of ESP-IDF that espflash writes checks it
esp_bootloader_esp_idf::esp_app_desc!();

// the round display is a 240x240 square with the corners cut off by the glass, the instruments
// are round and fill the circle; with a square screen the ui leaves out the menu area
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 240;
// SPI clock for the display, like in the Waveshare examples (TFT_eSPI); the GC9A01 datasheet
// specifies a write cycle of at least 15 ns (66 MHz), the display is only written
// if the picture shows speckles or wrong colors, use 40 MHz
const DISPLAY_SPI_FREQUENCY: Rate = Rate::from_mhz(80);
// backlight brightness in percent, there is no slider without the menu area
const BRIGHTNESS: u8 = 100;
// the BOOT button held this long starts the automatic sequence again, shorter it shows the next
// instrument
const LONG_PRESS: Duration = Duration::from_millis(1000);
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// The ESP32-S3 has 512 KB internal SRAM, of which about 334 KB are left for the static data and
// the stack of the application (esp-hal's memory.x, shared with the code in IRAM): 112.5 KB
// frame buffer, 128 KB heap, the DMA buffer, the other statics and the stack. The instruments
// request at most 58 KB at this size (tools/host-check --display 240x240). The 2 MB PSRAM of the
// ESP32-S3R2 isn't used.
const HEAP_SIZE: usize = 128 * 1024;
// copy buffer of the SPI DMA, the frame buffer rows are sent through it in chunks of this size
const DMA_BUFFER_SIZE: usize = 8 * 1024;

// a full frame buffer is needed, because the software renderer
// skips Path elements when rendering line by line
// the pixels are stored in the byte order of the GC9A01, so they are sent unchanged
static mut FRAME_BUFFER: [Rgb565BigEndianPixel; DISPLAY_WIDTH * DISPLAY_HEIGHT] =
    [Rgb565BigEndianPixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];

/// Time since the start, for Slint and the demo
fn uptime() -> core::time::Duration {
    core::time::Duration::from_micros(Instant::now().duration_since_epoch().as_micros())
}

#[main]
fn main() -> ! {
    // the heap must be ready before slint is used
    // a failed allocation panics, the panic is printed on the USB serial port
    firmware_common::init_heap!(HEAP_SIZE);

    // 240 MHz, the highest clock of the ESP32-S3
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let delay = Delay::new();

    // dimmable backlight on GPIO40 by LEDC, 20 kHz does not flicker
    let mut ledc = Ledc::new(p.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);
    let mut backlight_timer = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    backlight_timer
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty10Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_khz(20),
        })
        .unwrap();
    let mut backlight = ledc.channel(channel::Number::Channel0, p.GPIO40);
    // dark until the first frame is sent, so the uninitialized panel isn't seen
    backlight
        .configure(channel::config::Config {
            timer: &backlight_timer,
            duty_pct: 0,
            drive_mode: DriveMode::PushPull,
        })
        .unwrap();

    // BOOT button on GPIO0, low while pressed (a strapping pin, only read as such at reset)
    let boot_button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));

    // LCD on SPI2: SCK GPIO10, MOSI GPIO11, CS GPIO9, DC GPIO8, reset GPIO12
    // The display is only written, there is no MISO.
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(DISPLAY_SPI_FREQUENCY)
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(p.GPIO10)
    .with_mosi(p.GPIO11)
    .with_dma(p.DMA_CH0)
    .with_buffers(
        dma_rx_buffer!(4).unwrap(),
        dma_tx_buffer!(DMA_BUFFER_SIZE).unwrap(),
    );
    // shared between mipidsi and the frame transfers
    let lcd_cs = RefCell::new(Output::new(p.GPIO9, Level::High, OutputConfig::default()));
    let lcd_dc = Output::new(p.GPIO8, Level::Low, OutputConfig::default());
    let lcd_rst = Output::new(p.GPIO12, Level::Low, OutputConfig::default());

    let spi_device = ExclusiveDevice::new(spi, ChipSelect(&lcd_cs), delay).unwrap();
    let mut interface_buffer = [0u8; 512];
    let interface = SpiInterface::new(spi_device, lcd_dc, &mut interface_buffer);

    // the panel needs inverted colours and BGR order, rotation 0 of the Waveshare examples
    // (MADCTL 0x08), the same as on the round Pico boards
    // if colors look wrong, try ColorInversion::Normal or ColorOrder::Rgb
    // mipidsi is only used to initialize the display
    let display = mipidsi::Builder::new(GC9A01, interface)
        .display_size(240, 240)
        .orientation(Orientation::new().rotate(Rotation::Deg0))
        .invert_colors(ColorInversion::Inverted)
        .color_order(ColorOrder::Bgr)
        .reset_pin(lcd_rst)
        .init(&mut delay.clone())
        .unwrap();
    // take the bus back for the frame transfers, the reset pin must stay alive
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

    // frame statistics, printed once per second on the serial port (there is no status line)
    let mut statistics = Statistics::new(uptime());
    let mut backlight_on = false;

    // automatic sequence of instruments, a long press starts it
    let mut sequence = Sequence::default();
    // when the BOOT button was pressed, while it is held
    let mut pressed_since: Option<Instant> = None;

    loop {
        let frame_start = Instant::now();
        let time = uptime();

        // demo movement until real sensor values are available
        firmware_common::show!(ui, demo::values(time));

        // BOOT button, when released: a short press shows the next instrument and stops the
        // automatic sequence, like a tap on an instrument button; a long press starts it again
        match (boot_button.is_low(), pressed_since) {
            (true, None) => pressed_since = Some(frame_start),
            (false, Some(since)) => {
                pressed_since = None;
                if frame_start - since >= LONG_PRESS {
                    ui.set_auto_select(true);
                } else {
                    ui.set_instrument(firmware_common::next_instrument(ui.get_instrument()));
                    ui.set_auto_select(false);
                }
            }
            _ => {}
        }

        if sequence.next(ui.get_auto_select(), time) {
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
            );
            statistics.frame();
            if !backlight_on {
                let _ = backlight.set_duty(BRIGHTNESS);
                backlight_on = true;
            }
        }

        if let Some(status) = statistics.status(uptime()) {
            println!("{}", status);
        }

        // wait only for the rest of the frame time
        while Instant::now() - frame_start < FRAME_TIME {}
    }
}
