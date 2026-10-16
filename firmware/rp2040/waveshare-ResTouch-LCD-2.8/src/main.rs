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
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, SPI1};
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

// display in its native portrait orientation (rotation 0 degree)
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 320;
// System clock of the RP2040: its default and the highest officially specified one
// It has been sucessfully tested with 200 MHz and a SPI frequency of 100 MHz
const SYSTEM_CLOCK: u32 = 135_000_000;
// SPI clock for the display, at most half of the system clock, with even dividers only:
// 100 MHz or 50 MHz at 200 MHz system clock (the ST7789 datasheet specifies 62.5 MHz)
// if the picture shows speckles or wrong colors, use 31_250_000
const DISPLAY_SPI_FREQUENCY: u32 = 62_500_000;
// the XPT2046 touch controller shares SPI1, it is read at 2 MHz (its maximum clock)
const TOUCH_SPI_FREQUENCY: u32 = 2_000_000;
// conversions per axis: the first ones are dropped while the voltages on the panel settle after
// its layers were switched, the middle half of the rest is averaged against noise
const TOUCH_SETTLE: usize = 2;
const TOUCH_SAMPLES: usize = 8;
// a light touch gives readings pulled towards the middle of the panel, touches with a lower
// pressure value (Z1 + 4095 - Z2, the threshold common XPT2046 drivers use) are ignored
const TOUCH_MIN_PRESSURE: i32 = 350;
// raw touch values (12 bit) to display position, affine, so it can also correct a rotated or
// skewed panel: x = A * raw_x + B * raw_y + C, y = D * raw_x + E * raw_y + F.
// Fitted with tools/touch-calibration.py to five targets touched on the device (all within 2 px);
// the panel is mirrored on both axes, not rotated. To calibrate again, see the README.
const TOUCH_CALIBRATION: [f32; 6] = [-0.06518, 0.00011, 253.63, 0.00043, -0.08898, 349.34];
// shows the last raw and mapped touch position in the menu area, for the calibration
const SHOW_TOUCH: bool = false;
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// instruments of the selector in the menu area, and how long each is shown in the automatic
// sequence
const INSTRUMENTS: i32 = 6;
const INSTRUMENT_TIME: Duration = Duration::from_secs(20);
// RP2040 has 264 KB RAM: 150 KB frame buffer, 92 KB heap, the rest for stack and statics
// path rendering temporarily allocates one contiguous block of one byte per pixel
// of a Path element's box (largest is the disc of heading: 240x240 = 57.6 KB)
// The instruments use about 61 KB of memory in total.
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
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>;
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

/// One conversion of the XPT2046: the command selects the channel (12 bit, differential, power
/// down between conversions, so the touch interrupt stays active), the result follows in the next
/// two bytes, left aligned
fn touch_channel(spi: &mut Spi<'_, SPI1, Async>, command: u8) -> u16 {
    let mut answer = [0u8; 3];
    // errors are ignored, a wrong reading is replaced by the next one
    let _ = spi.blocking_transfer(&mut answer, &[command, 0, 0]);
    (u16::from(answer[1]) << 8 | u16::from(answer[2])) >> 3
}

/// One channel, converted repeatedly: the first conversions are dropped while the panel settles,
/// the middle half of the sorted rest is averaged, outliers from a bouncing touch are dropped
fn touch_axis(spi: &mut Spi<'_, SPI1, Async>, command: u8) -> i32 {
    for _ in 0..TOUCH_SETTLE {
        touch_channel(spi, command);
    }
    let mut samples = [0u16; TOUCH_SAMPLES];
    for sample in samples.iter_mut() {
        *sample = touch_channel(spi, command);
    }
    samples.sort_unstable();
    let middle = &samples[TOUCH_SAMPLES / 4..TOUCH_SAMPLES * 3 / 4];
    middle.iter().map(|&v| i32::from(v)).sum::<i32>() / middle.len() as i32
}

/// Reads the raw touch position and pressure, None when the display is not touched or only
/// lightly
/// The touch interrupt pin (GP17) is low while touched, so the SPI bus is only used then.
fn read_touch(
    spi: &mut Spi<'_, SPI1, Async>,
    touch_cs: &mut Output<'_>,
    touch_irq: &Input<'_>,
) -> Option<(i32, i32, i32)> {
    if touch_irq.is_high() {
        return None;
    }
    spi.set_frequency(TOUCH_SPI_FREQUENCY);
    touch_cs.set_low();
    // pressure from the resistance between the layers, then each axis on its own
    let z1 = touch_axis(spi, 0xb0);
    let z2 = touch_axis(spi, 0xc0);
    let x = touch_axis(spi, 0xd0);
    let y = touch_axis(spi, 0x90);
    touch_cs.set_high();
    spi.set_frequency(DISPLAY_SPI_FREQUENCY);
    // released while sampling, or too light: the readings are not valid
    let pressure = z1 + 4095 - z2;
    if touch_irq.is_high() || pressure < TOUCH_MIN_PRESSURE {
        return None;
    }
    Some((x, y, pressure))
}

/// Display position of a raw touch position, with the affine calibration
fn touch_position((x, y, _): (i32, i32, i32)) -> slint::LogicalPosition {
    let [a, b, c, d, e, f] = TOUCH_CALIBRATION;
    let (x, y) = (x as f32, y as f32);
    slint::LogicalPosition::new(
        (a * x + b * y + c).clamp(0.0, DISPLAY_WIDTH as f32 - 1.0),
        (d * x + e * y + f).clamp(0.0, DISPLAY_HEIGHT as f32 - 1.0),
    )
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
    // touch controller XPT2046 on the same bus: MISO GP12, CS GP16, interrupt GP17
    let mut spi_config = spi::Config::default();
    spi_config.frequency = DISPLAY_SPI_FREQUENCY;
    // DMA for the frame buffer (channel 0), the receive channel is needed by the driver,
    // the touch controller is read without DMA
    let spi = Spi::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_12, p.DMA_CH0, p.DMA_CH1, Irqs, spi_config,
    );
    // touch controller deselected, the interrupt is open drain and low while touched
    let mut touch_cs = Output::new(p.PIN_16, Level::High);
    let touch_irq = Input::new(p.PIN_17, Pull::Up);
    // the microSD slot of the board is on SPI1 too (CS GP22): kept deselected, so a card can't
    // drive MISO while the touch controller is read
    let _sd_cs = Output::new(p.PIN_22, Level::High);
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
    // full brightness until the ui is running, then the slider in the menu area sets it
    let _ = backlight.set_duty_cycle_percent(100);

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

    // brightness set on the backlight, it changes with the slider in the menu area
    let mut brightness = 100;
    // automatic sequence of instruments: when it was last switched on or changed the instrument
    let mut auto_select = false;
    let mut instrument_shown_since = Instant::now();
    // position of the touch while the display is touched, for the release event
    let mut touch: Option<slint::LogicalPosition> = None;
    // last raw and mapped touch position, shown with SHOW_TOUCH
    let mut last_touch = None;

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
        // automatic sequence (switch in the menu area): the next instrument every 20 seconds,
        // counted from when the switch was turned on
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

        // touch input, as pointer events of the left button: pressed when the touch starts,
        // moved while it lasts, released where it ended (a click on the buttons)
        let button = slint::platform::PointerEventButton::Left;
        match (read_touch(spi, &mut touch_cs, &touch_irq), touch) {
            (Some(raw), previous) => {
                let position = touch_position(raw);
                let event = match previous {
                    None => slint::platform::WindowEvent::PointerPressed { position, button },
                    Some(_) => slint::platform::WindowEvent::PointerMoved { position },
                };
                window.dispatch_event(event);
                touch = Some(position);
                last_touch = Some((raw, position));
            }
            (None, Some(position)) => {
                window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position,
                    button,
                });
                touch = None;
            }
            (None, None) => {}
        }

        // backlight brightness from the slider in the menu area (5 to 100 %)
        let selected_brightness = ui.get_brightness().clamp(5, 100) as u8;
        if selected_brightness != brightness {
            brightness = selected_brightness;
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
        }

        let elapsed = Instant::now() - statistics_start;
        if elapsed >= Duration::from_secs(1) && frames > 0 {
            let fps = frames as f32 * 1000.0 / elapsed.as_millis() as f32;
            let heap_peak = HEAP_PEAK.load(Ordering::Relaxed) / 1024;
            let status = match last_touch {
                Some(((raw_x, raw_y, pressure), position)) if SHOW_TOUCH => slint::format!(
                    "{:.1} fps, heap peak {} of {} KB\ntouch raw {}, {} z {} at {:.0}, {:.0}",
                    fps,
                    heap_peak,
                    HEAP_SIZE / 1024,
                    raw_x,
                    raw_y,
                    pressure,
                    position.x,
                    position.y,
                ),
                _ => slint::format!(
                    "{:.1} fps, heap peak {} of {} KB",
                    fps,
                    heap_peak,
                    HEAP_SIZE / 1024
                ),
            };
            ui.set_status(status);
            statistics_start = Instant::now();
            frames = 0;
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
