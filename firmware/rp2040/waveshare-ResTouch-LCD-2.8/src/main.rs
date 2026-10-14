// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Raspberry Pi Pico W with a Waveshare Pico-ResTouch-LCD-2.8

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565BigEndianPixel,
};

slint::include_modules!();

// system clock of the RP2040
const SYSTEM_CLOCK: u32 = 200_000_000;

// display in its native portrait orientation (rotation 0 degree)
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 320;
// shortest time per frame, limits the frame rate to 50 per second
const FRAME_TIME: Duration = Duration::from_millis(20);
// backlight brightness in percent, dimmed at night
const BRIGHTNESS_DAY: u8 = 100;
const BRIGHTNESS_NIGHT: u8 = 20;
// RP2040 has 264 KB RAM: 150 KB frame buffer, 92 KB heap, the rest for stack and statics
// path rendering temporarily allocates one contiguous block of one byte per pixel
// of a Path element's box (largest is the inner disc: 192x192 = 36.9 KB)
const HEAP_SIZE: usize = 92 * 1024;

// TLSF keeps fragmentation low, which matters for the large temporary path blocks
static HEAP: Heap = Heap::empty();

// set when an allocation fails, so the panic handler can signal it
static OUT_OF_MEMORY: AtomicBool = AtomicBool::new(false);
// heap bytes in use and the highest value so far, shown in the status display
// (load/store only, the RP2040 has no atomic read-modify-write, the firmware is single threaded)
static HEAP_USED: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

/// Global allocator that remembers a failed allocation
struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { HEAP.alloc(layout) };
        if ptr.is_null() {
            OUT_OF_MEMORY.store(true, Ordering::Relaxed);
        } else {
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
// the pixels are stored in the byte order of the ST7789, so it is sent unchanged by DMA
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
    // The backlight (GP13) is normally driven by PWM. It is switched back to a plain output
    // with direct register writes, because the HAL peripherals are owned by main:
    // IO_BANK0 GPIO13_CTRL (0x4001_406c) function 5 = SIO, SIO GPIO_OE_SET (0xd000_0024),
    // then GPIO_OUT_CLR (0xd000_0018) and GPIO_OUT_XOR (0xd000_001c) switch it.
    unsafe {
        core::ptr::write_volatile(0xd000_0018 as *mut u32, 1 << 13);
        core::ptr::write_volatile(0xd000_0024 as *mut u32, 1 << 13);
        core::ptr::write_volatile(0x4001_406c as *mut u32, 5);
    }
    if OUT_OF_MEMORY.load(Ordering::Relaxed) {
        loop {
            // about 4 times per second
            cortex_m::asm::delay(SYSTEM_CLOCK / 8);
            unsafe { core::ptr::write_volatile(0xd000_001c as *mut u32, 1 << 13) };
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

/// Sends a command with its parameters to the ST7789
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
    // a full frame takes about 12 ms at 100 MHz, a much longer time means the DMA hangs
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

    // 200 MHz is the highest officially qualified system clock of the RP2040,
    // system_freq raises the core voltage to 1.15 V for it
    // the flash and the SPI peripherals are clocked from the system clock as well
    let clocks = ClockConfig::system_freq(SYSTEM_CLOCK).unwrap();
    let p = embassy_rp::init(embassy_rp::config::Config::new(clocks));

    // LCD on SPI1: CLK GP10, MOSI GP11, CS GP9, DC GP8, RST GP15, backlight GP13 (PWM)
    // (MISO GP12 is not needed for the display)
    let mut spi_config = spi::Config::default();
    // SPI runs at most at half of the system clock, with even dividers only:
    // 100 MHz or 50 MHz at 200 MHz system clock (the ST7789 datasheet specifies 62.5 MHz)
    // if the picture shows speckles or wrong colors, use 50_000_000
    spi_config.frequency = 100_000_000;
    // transmit only, with DMA for the frame buffer
    let spi = Spi::new_txonly(p.SPI1, p.PIN_10, p.PIN_11, p.DMA_CH0, Irqs, spi_config);
    // shared between mipidsi and the DMA transfers
    let lcd_cs = RefCell::new(Output::new(p.PIN_9, Level::High));
    let lcd_dc = Output::new(p.PIN_8, Level::Low);
    let lcd_rst = Output::new(p.PIN_15, Level::Low);
    // dimmable backlight: GP13 is channel B of PWM slice 6,
    // 20 kHz (200 MHz / 10 / 1000) is neither visible nor audible
    let mut backlight_config = pwm::Config::default();
    backlight_config.divider = 10.into();
    backlight_config.top = 999;
    let mut backlight = Pwm::new_output_b(p.PWM_SLICE6, p.PIN_13, backlight_config);
    let _ = backlight.set_duty_cycle_percent(BRIGHTNESS_DAY);

    let spi_device = ExclusiveDevice::new(spi, ChipSelect(&lcd_cs), Delay).unwrap();
    let mut interface_buffer = [0u8; 512];
    let interface = SpiInterface::new(spi_device, lcd_dc, &mut interface_buffer);

    // the ST7789 is a 240x320 portrait panel, used without rotation
    // for landscape use Rotation::Deg90 or Deg270 and swap DISPLAY_WIDTH and DISPLAY_HEIGHT
    // if colors look wrong, try ColorInversion::Normal
    // mipidsi is only used to initialize the display
    let display = mipidsi::Builder::new(ST7789, interface)
        .display_size(240, 320)
        .orientation(Orientation::new().rotate(Rotation::Deg0))
        .invert_colors(ColorInversion::Inverted)
        .reset_pin(lcd_rst)
        .init(&mut Delay)
        .unwrap();
    // take the bus back for the DMA transfers, the reset pin must stay alive
    let (interface, _model, _lcd_rst) = display.release();
    let (mut spi_device, mut lcd_dc) = interface.release();
    let spi = spi_device.bus_mut();

    // slint setup
    // ReusedBuffer only redraws what changed, the frame buffer keeps the rest
    // (needs the patched renderer, Slint 1.18.1 blends Paths outside of the redrawn region,
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
    // the ui arranges itself for portrait or landscape from the screen size
    ui.set_screen_width(DISPLAY_WIDTH as f32);
    ui.set_screen_height(DISPLAY_HEIGHT as f32);
    ui.show().unwrap();

    // SAFETY: the only reference to the frame buffer, created once
    let frame_buffer: &mut [Rgb565BigEndianPixel] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // frame statistics, shown once per second in the menu area
    let mut statistics_start = Instant::now();
    let mut frames: u32 = 0;
    let mut render_time = Duration::from_ticks(0);
    let mut send_time = Duration::from_ticks(0);

    let mut backlight_night = false;
    let mut shown_instrument = 0;

    loop {
        let frame_start = Instant::now();

        // demo movement until real sensor values are available
        ui.set_roll(triangle(frame_start, Duration::from_secs(8), 30.0));
        ui.set_pitch(triangle(frame_start, Duration::from_secs(5), 10.0));
        ui.set_altitude(5000.0 + triangle(frame_start, Duration::from_secs(60), 5000.0));
        ui.set_airspeed(105.0 + triangle(frame_start, Duration::from_secs(30), 75.0));
        // a steady turn, once around per minute
        ui.set_heading((frame_start.as_millis() % 60_000) as f32 * 0.006);
        // demo: switch the instrument every 20 seconds, until the touch controller is used
        // and the buttons in the menu area can be pressed
        let instrument = (frame_start.as_secs() / 20 % 4) as i32;
        if instrument != shown_instrument {
            shown_instrument = instrument;
            ui.set_instrument(instrument);
        }
        // demo: night for 5 of every 15 seconds, until there is a switch or light sensor
        let night = frame_start.as_secs() % 15 >= 10;

        // at night the backlight is dimmed
        if night != backlight_night {
            backlight_night = night;
            let brightness = if night {
                BRIGHTNESS_NIGHT
            } else {
                BRIGHTNESS_DAY
            };
            let _ = backlight.set_duty_cycle_percent(brightness);
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
        let render_end = Instant::now();
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
            frames += 1;
            render_time += render_end - frame_start;
            send_time += Instant::now() - render_end;
        }

        let elapsed = Instant::now() - statistics_start;
        if elapsed >= Duration::from_secs(1) && frames > 0 {
            let fps = frames as f32 * 1000.0 / elapsed.as_millis() as f32;
            ui.set_status(slint::format!(
                "{:.1} fps, render {} ms, send {} ms\nheap peak {} of {} KB",
                fps,
                render_time.as_millis() / frames as u64,
                send_time.as_millis() / frames as u64,
                HEAP_PEAK.load(Ordering::Relaxed) / 1024,
                HEAP_SIZE / 1024,
            ));
            statistics_start = Instant::now();
            frames = 0;
            render_time = Duration::from_ticks(0);
            send_time = Duration::from_ticks(0);
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
