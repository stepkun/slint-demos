// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Waveshare RP2040-LCD-1.28-B (round 240x240 GC9A01 display, no touch)

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_rp::clocks::ClockConfig;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, SPI1};
use embassy_rp::pwm::{self, Pwm, SetDutyCycle};
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration, Instant, Timer, with_timeout};
use embedded_alloc::TlsfHeap as Heap;
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::interface::SpiInterface;
use mipidsi::models::GC9A01;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565BigEndianPixel,
};

slint::include_modules!();

// the round display is a 240x240 square with the corners cut off by the glass, the instruments
// are round and fill the circle; with a square screen the ui leaves out the menu area
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 240;
// System clock of the RP2040: its default, so the SPI clock below is exactly half of it
const SYSTEM_CLOCK: u32 = 125_000_000;
// SPI clock for the display, at most half of the system clock, with even dividers only
// The GC9A01 datasheet specifies a write cycle of at least 15 ns (66 MHz).
// if the picture shows speckles or wrong colors, use 31_250_000
const DISPLAY_SPI_FREQUENCY: u32 = 62_500_000;
// backlight brightness in percent, there is no touch for the slider of the menu area
const BRIGHTNESS: u8 = 100;
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// instruments of ui/test-app.slint, shown one after the other: without touch or buttons the
// automatic sequence is the only way to see them all
const INSTRUMENTS: i32 = 6;
const INSTRUMENT_TIME: Duration = Duration::from_secs(20);
// RP2040 has 264 KB RAM: 112.5 KB frame buffer, 128 KB heap, the rest for stack and statics
// The instruments request at most 58 KB at this size (tools/host-check --display 240x240),
// the larger heap of this board leaves more room than the 92 KB of the Pico-ResTouch-LCD-2.8.
const HEAP_SIZE: usize = 128 * 1024;

// TLSF keeps fragmentation low, which matters for the large temporary path blocks
static HEAP: Heap = Heap::empty();

// set when an allocation fails, so the panic handler can signal it
static OUT_OF_MEMORY: AtomicBool = AtomicBool::new(false);

/// Global allocator that remembers a failed allocation
struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { HEAP.alloc(layout) };
        if ptr.is_null() {
            OUT_OF_MEMORY.store(true, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { HEAP.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

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
    // The backlight (GP25) is normally driven by PWM. It is switched back to a plain output
    // with direct register writes, because the HAL peripherals are owned by main:
    // IO_BANK0 GPIO25_CTRL (0x4001_40cc) function 5 = SIO, SIO GPIO_OE_SET (0xd000_0024),
    // then GPIO_OUT_CLR (0xd000_0018) and GPIO_OUT_XOR (0xd000_001c) switch it.
    unsafe {
        core::ptr::write_volatile(0xd000_0018 as *mut u32, 1 << 25);
        core::ptr::write_volatile(0xd000_0024 as *mut u32, 1 << 25);
        core::ptr::write_volatile(0x4001_40cc as *mut u32, 5);
    }
    if OUT_OF_MEMORY.load(Ordering::Relaxed) {
        loop {
            // about 4 times per second
            cortex_m::asm::delay(SYSTEM_CLOCK / 8);
            unsafe { core::ptr::write_volatile(0xd000_001c as *mut u32, 1 << 25) };
        }
    }
    loop {
        cortex_m::asm::wfi();
    }
}

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
        core::time::Duration::from_micros(Instant::now().as_micros())
    }
}

/// Chip select shared between the display init by mipidsi and the DMA transfers
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
fn command(spi: &mut Spi<'_, SPI1, Async>, dc: &mut Output<'_>, command: u8, parameters: &[u8]) {
    // errors are ignored, a broken frame is overwritten with the next one
    dc.set_low();
    let _ = spi.blocking_write(&[command]);
    dc.set_high();
    if !parameters.is_empty() {
        let _ = spi.blocking_write(parameters);
    }
}

/// Sends full rows of the frame buffer to the display using DMA,
/// `rows` must contain exactly the pixel data of the rows `first_row..=last_row`
async fn send_rows(
    spi: &mut Spi<'_, SPI1, Async>,
    dc: &mut Output<'_>,
    cs: &RefCell<Output<'static>>,
    first_row: u16,
    last_row: u16,
    rows: &[u8],
) {
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
    // memory write, followed by the pixel data
    command(spi, dc, 0x2c, &[]);
    // a full frame takes about 15 ms at 62.5 MHz, a much longer time means the DMA hangs
    if with_timeout(Duration::from_millis(500), spi.write(rows))
        .await
        .is_err()
    {
        panic!("DMA transfer to the display timed out");
    }
    cs.borrow_mut().set_high();
}

/// Triangle wave between -amplitude and +amplitude with the given period
fn triangle(time: Instant, period: Duration, amplitude: f32) -> f32 {
    let period = period.as_millis();
    let phase = (time.as_millis() % period) as f32 / period as f32;
    let value = if phase < 0.5 {
        phase * 4.0 - 1.0
    } else {
        3.0 - phase * 4.0
    };
    value * amplitude
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // the heap must be ready before slint is used
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

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

    let mut instrument_shown_since = Instant::now();

    loop {
        let frame_start = Instant::now();

        // demo movement until real sensor values are available
        ui.set_roll(triangle(frame_start, Duration::from_secs(8), 30.0));
        ui.set_pitch(triangle(frame_start, Duration::from_secs(5), 10.0));
        ui.set_altitude(5000.0 + triangle(frame_start, Duration::from_secs(60), 5000.0));
        ui.set_airspeed(105.0 + triangle(frame_start, Duration::from_secs(30), 75.0));
        // a steady turn, once around per minute
        ui.set_heading((frame_start.as_millis() % 60_000) as f32 * 0.006);
        // a little beyond the scale, so the needle reaches its stop
        ui.set_vertical_speed(triangle(frame_start, Duration::from_secs(40), 2200.0));
        // turns beyond the standard rate (3 degree per second), the ball swinging a little
        ui.set_turn_rate(triangle(frame_start, Duration::from_secs(30), 5.0));
        ui.set_slip(triangle(frame_start, Duration::from_secs(12), 0.6));
        // the next instrument every 20 seconds
        if frame_start - instrument_shown_since >= INSTRUMENT_TIME {
            ui.set_instrument((ui.get_instrument() + 1) % INSTRUMENTS);
            instrument_shown_since = frame_start;
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
            )
            .await;
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
