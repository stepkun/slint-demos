// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Waveshare ESP32-S3-LCD-1.28 (round 240x240 GC9A01 display, no touch)
// The esp-hal setup of the UNIHIKER K10 in ../unihiker-K10, the display of the round Pico
// boards (../../rp2040/waveshare-LCD-1.28-B). The BOOT button selects the instrument.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::RefCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use embedded_alloc::TlsfHeap as Heap;
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
use esp_hal::spi::master::{Config as SpiConfig, Spi, SpiDma};
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::{Blocking, dma_rx_buffer, dma_tx_buffer};
use esp_println::println;
use mipidsi::interface::SpiInterface;
use mipidsi::models::GC9A01;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565BigEndianPixel,
};

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
// instruments of ui/test-app.slint, and how long each is shown in the automatic sequence
const INSTRUMENTS: i32 = 6;
const INSTRUMENT_TIME: Duration = Duration::from_secs(20);
// The ESP32-S3 has 512 KB internal SRAM, of which about 334 KB are left for the static data and
// the stack of the application (esp-hal's memory.x, shared with the code in IRAM): 112.5 KB
// frame buffer, 128 KB heap, the DMA buffer, the other statics and the stack. The instruments
// request at most 58 KB at this size (tools/host-check --display 240x240). The 2 MB PSRAM of the
// ESP32-S3R2 isn't used.
const HEAP_SIZE: usize = 128 * 1024;
// copy buffer of the SPI DMA, the frame buffer rows are sent through it in chunks of this size
const DMA_BUFFER_SIZE: usize = 8 * 1024;

// TLSF keeps fragmentation low, the same allocator as on the Picos
static HEAP: Heap = Heap::empty();

// heap bytes in use and the highest value so far, printed once per second
// (load/store only like in the Pico firmwares; the firmware is single threaded)
static HEAP_USED: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

/// Global allocator that counts the heap use like the Pico firmwares
struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { HEAP.alloc(layout) };
        // a failed allocation panics, the panic is printed on the USB serial port
        if !ptr.is_null() {
            let used = HEAP_USED.load(Ordering::Relaxed) + layout.size();
            HEAP_USED.store(used, Ordering::Relaxed);
            if used > HEAP_PEAK.load(Ordering::Relaxed) {
                HEAP_PEAK.store(used, Ordering::Relaxed);
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP_USED.store(
            HEAP_USED.load(Ordering::Relaxed) - layout.size(),
            Ordering::Relaxed,
        );
        unsafe { HEAP.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

// a full frame buffer is needed, because the software renderer
// skips Path elements when rendering line by line
// the pixels are stored in the byte order of the GC9A01, so they are sent unchanged
static mut FRAME_BUFFER: [Rgb565BigEndianPixel; DISPLAY_WIDTH * DISPLAY_HEIGHT] =
    [Rgb565BigEndianPixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];

/// Slint platform with the single window of the display
struct Platform {
    window: Rc<MinimalSoftwareWindow>,
}

impl slint::platform::Platform for Platform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::from_micros(Instant::now().duration_since_epoch().as_micros())
    }
}

/// Chip select shared between the display init by mipidsi and the frame transfers
struct ChipSelect<'a>(&'a RefCell<Output<'static>>);

impl embedded_hal::digital::ErrorType for ChipSelect<'_> {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for ChipSelect<'_> {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.0.borrow_mut().set_low();
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.0.borrow_mut().set_high();
        Ok(())
    }
}

/// Sends a command with its parameters to the GC9A01
fn command(spi: &mut SpiDma<'_, Blocking>, dc: &mut Output<'_>, command: u8, parameters: &[u8]) {
    use embedded_hal::spi::SpiBus;
    // errors are ignored, a broken frame is overwritten with the next one
    dc.set_low();
    let _ = SpiBus::write(spi, &[command]);
    dc.set_high();
    if !parameters.is_empty() {
        let _ = SpiBus::write(spi, parameters);
    }
}

/// Sends full rows of the frame buffer to the display,
/// `rows` must contain exactly the pixel data of the rows `first_row..=last_row`
fn send_rows(
    spi: &mut SpiDma<'_, Blocking>,
    dc: &mut Output<'_>,
    cs: &RefCell<Output<'static>>,
    first_row: u16,
    last_row: u16,
    rows: &[u8],
) {
    use embedded_hal::spi::SpiBus;
    cs.borrow_mut().set_low();
    let last_column = (DISPLAY_WIDTH - 1) as u16;
    // column address set
    command(
        spi,
        dc,
        0x2a,
        &[0, 0, (last_column >> 8) as u8, last_column as u8],
    );
    // row address set
    command(
        spi,
        dc,
        0x2b,
        &[
            (first_row >> 8) as u8,
            first_row as u8,
            (last_row >> 8) as u8,
            last_row as u8,
        ],
    );
    // memory write, followed by the pixel data, by DMA through the copy buffer
    command(spi, dc, 0x2c, &[]);
    let _ = SpiBus::write(spi, rows);
    cs.borrow_mut().set_high();
}

/// Triangle wave between -amplitude and +amplitude with the given period
fn triangle(time: Instant, period: Duration, amplitude: f32) -> f32 {
    let period = period.as_millis();
    let phase = (time.duration_since_epoch().as_millis() % period) as f32 / period as f32;
    let value = if phase < 0.5 {
        phase * 4.0 - 1.0
    } else {
        3.0 - phase * 4.0
    };
    value * amplitude
}

#[main]
fn main() -> ! {
    // the heap must be ready before slint is used
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

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

    // slint setup
    // ReusedBuffer only redraws what changed, the frame buffer keeps the rest
    // (needs the patched renderer, the original blends Paths outside of the redrawn region,
    // see issues/rp2040)
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    window.set_size(slint::PhysicalSize::new(
        DISPLAY_WIDTH as u32,
        DISPLAY_HEIGHT as u32,
    ));
    slint::platform::set_platform(Box::new(Platform {
        window: window.clone(),
    }))
    .unwrap();

    let ui = AppWindow::new().unwrap();
    // a square screen: only the instrument, without the menu area
    ui.set_screen_width(DISPLAY_WIDTH as f32);
    ui.set_screen_height(DISPLAY_HEIGHT as f32);
    ui.show().unwrap();

    // SAFETY: the only reference to the frame buffer, created once
    let frame_buffer: &mut [Rgb565BigEndianPixel] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // frame statistics, printed once per second on the serial port (there is no status line)
    let mut statistics_start = Instant::now();
    let mut frames: u32 = 0;
    let mut backlight_on = false;

    // automatic sequence of instruments: when it was last switched on or changed the instrument
    let mut auto_select = false;
    let mut instrument_shown_since = Instant::now();
    // when the BOOT button was pressed, while it is held
    let mut pressed_since: Option<Instant> = None;

    loop {
        let frame_start = Instant::now();

        // demo movement until real sensor values are available
        ui.set_roll(triangle(frame_start, Duration::from_secs(8), 30.0));
        ui.set_pitch(triangle(frame_start, Duration::from_secs(5), 10.0));
        ui.set_altitude(5000.0 + triangle(frame_start, Duration::from_secs(60), 5000.0));
        ui.set_airspeed(105.0 + triangle(frame_start, Duration::from_secs(30), 75.0));
        // a steady turn, once around per minute
        ui.set_heading((frame_start.duration_since_epoch().as_millis() % 60_000) as f32 * 0.006);
        // a little beyond the scale, so the needle reaches its stop
        ui.set_vertical_speed(triangle(frame_start, Duration::from_secs(40), 2200.0));
        // turns beyond the standard rate (3 degree per second), the ball swinging a little
        ui.set_turn_rate(triangle(frame_start, Duration::from_secs(30), 5.0));
        ui.set_slip(triangle(frame_start, Duration::from_secs(12), 0.6));

        // BOOT button, when released: a short press shows the next instrument and stops the
        // automatic sequence, like a tap on an instrument button; a long press starts it again
        match (boot_button.is_low(), pressed_since) {
            (true, None) => pressed_since = Some(frame_start),
            (false, Some(since)) => {
                pressed_since = None;
                if frame_start - since >= LONG_PRESS {
                    ui.set_auto_select(true);
                } else {
                    ui.set_instrument((ui.get_instrument() + 1) % INSTRUMENTS);
                    ui.set_auto_select(false);
                }
            }
            _ => {}
        }

        // automatic sequence: the next instrument every 20 seconds, counted from when it was
        // switched on
        if ui.get_auto_select() {
            if !auto_select {
                auto_select = true;
                instrument_shown_since = frame_start;
            } else if frame_start - instrument_shown_since >= INSTRUMENT_TIME {
                ui.set_instrument((ui.get_instrument() + 1) % INSTRUMENTS);
                instrument_shown_since = frame_start;
            }
        } else {
            auto_select = false;
        }

        slint::platform::update_timers_and_animations();

        let mut changed_rows = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(frame_buffer, DISPLAY_WIDTH);
            let (origin, size) = (region.bounding_box_origin(), region.bounding_box_size());
            if size.width > 0 && size.height > 0 {
                changed_rows = Some((origin.y as usize, origin.y as usize + size.height as usize));
            }
        });
        if let Some((first_row, end_row)) = changed_rows {
            // full rows are contiguous in the frame buffer, so they are sent with one transfer
            let rows = &frame_buffer[first_row * DISPLAY_WIDTH..end_row * DISPLAY_WIDTH];
            // SAFETY: Rgb565BigEndianPixel is a plain u16 in display byte order
            let rows = unsafe {
                core::slice::from_raw_parts(
                    rows.as_ptr() as *const u8,
                    core::mem::size_of_val(rows),
                )
            };
            send_rows(
                spi,
                &mut lcd_dc,
                &lcd_cs,
                first_row as u16,
                (end_row - 1) as u16,
                rows,
            );
            frames += 1;
            if !backlight_on {
                let _ = backlight.set_duty(BRIGHTNESS);
                backlight_on = true;
            }
        }

        let elapsed = Instant::now() - statistics_start;
        if elapsed >= Duration::from_secs(1) && frames > 0 {
            let fps = frames as f32 * 1000.0 / elapsed.as_millis() as f32;
            println!(
                "{:.1} fps, heap peak {} of {} KB",
                fps,
                HEAP_PEAK.load(Ordering::Relaxed) / 1024,
                HEAP_SIZE / 1024
            );
            statistics_start = Instant::now();
            frames = 0;
        }

        // wait only for the rest of the frame time
        while Instant::now() - frame_start < FRAME_TIME {}
    }
}
