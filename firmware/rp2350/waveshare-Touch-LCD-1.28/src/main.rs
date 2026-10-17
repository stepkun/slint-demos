// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Waveshare RP2350-Touch-LCD-1.28 (round 240x240 GC9A01 display, CST816S touch)
// The display part is the same as in ../../rp2040/waveshare-LCD-1.28-B, the chip specific parts
// as in ../waveshare-ResTouch-LCD-2.8.

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
use embassy_rp::i2c::{self, Blocking, I2c};
use embassy_rp::peripherals::{DMA_CH0, I2C1, SPI1};
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
// System clock of the RP2350: its default and the highest officially specified one
const SYSTEM_CLOCK: u32 = 150_000_000;
// SPI clock for the display, at most half of the system clock, with even dividers only:
// 75 MHz or 37.5 MHz at 150 MHz system clock. The GC9A01 datasheet specifies a write cycle of
// at least 15 ns (66 MHz), the Waveshare examples of this board run it at 75 MHz as well.
// if the picture shows speckles or wrong colors, use 37_500_000
const DISPLAY_SPI_FREQUENCY: u32 = 75_000_000;
// the CST816S touch controller and the QMI8658 IMU share I2C1, at 400 kHz like in the
// Waveshare examples
const TOUCH_I2C_FREQUENCY: u32 = 400_000;
const TOUCH_ADDRESS: u8 = 0x15;
// Gestures, evaluated by the firmware when the finger is lifted: the ui has no menu area on a
// square screen. A touch that moves less than TOUCH_SWIPE pixels is a tap, held at least
// TOUCH_LONG_PRESS a long press.
const TOUCH_SWIPE: i32 = 40;
const TOUCH_LONG_PRESS: Duration = Duration::from_millis(1000);
// brightness change per swipe up or down, in percent, and the lowest brightness (a dark display
// would look like a crash)
const BRIGHTNESS_STEP: i32 = 20;
const BRIGHTNESS_MIN: i32 = 5;
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// instruments of ui/test-app.slint, and how long each is shown in the automatic sequence
const INSTRUMENTS: i32 = 6;
const INSTRUMENT_TIME: Duration = Duration::from_secs(20);
// RP2350 has 520 KB RAM, of which 512 KB are used: 112.5 KB frame buffer, 256 KB heap, the rest
// for stack and statics. The instruments request at most 58 KB at this size
// (tools/host-check --display 240x240), so the heap has plenty of room.
const HEAP_SIZE: usize = 256 * 1024;

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
    // with direct register writes, because the HAL peripherals are owned by main. The addresses
    // are the RP2350's: IO_BANK0 GPIO25_CTRL (0x4002_80cc) function 5 = SIO, SIO GPIO_OE_SET
    // (0xd000_0038), then GPIO_OUT_CLR (0xd000_0020) and GPIO_OUT_XOR (0xd000_0028) switch it.
    // The pad isolation of the RP2350 was already removed by the PWM setup.
    unsafe {
        core::ptr::write_volatile(0xd000_0020 as *mut u32, 1 << 25);
        core::ptr::write_volatile(0xd000_0038 as *mut u32, 1 << 25);
        core::ptr::write_volatile(0x4002_80cc as *mut u32, 5);
    }
    if OUT_OF_MEMORY.load(Ordering::Relaxed) {
        loop {
            // about 4 times per second
            cortex_m::asm::delay(SYSTEM_CLOCK / 8);
            unsafe { core::ptr::write_volatile(0xd000_0028 as *mut u32, 1 << 25) };
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
    // a full frame takes about 12 ms at 75 MHz, a much longer time means the DMA hangs
    if with_timeout(Duration::from_millis(500), spi.write(rows))
        .await
        .is_err()
    {
        panic!("DMA transfer to the display timed out");
    }
    cs.borrow_mut().set_high();
}

/// Reads the touch position from the CST816S, None while not touched (or on a bus error)
/// Its registers from 0x02: number of fingers, then x and y with 12 bit each, high byte first.
/// The coordinates are those of the display in the orientation used here.
fn read_touch(i2c: &mut I2c<'_, I2C1, Blocking>) -> Option<(i32, i32)> {
    let mut data = [0u8; 5];
    i2c.blocking_write_read(TOUCH_ADDRESS, &[0x02], &mut data)
        .ok()?;
    if data[0] == 0 {
        return None;
    }
    let x = i32::from(data[1] & 0x0f) << 8 | i32::from(data[2]);
    let y = i32::from(data[3] & 0x0f) << 8 | i32::from(data[4]);
    Some((x, y))
}

/// What a finished touch means
enum Gesture {
    Tap,
    LongPress,
    Left,
    Right,
    Up,
    Down,
}

/// A touch while the display is touched
#[derive(Clone, Copy)]
struct Touch {
    // where and when it started
    start: (i32, i32),
    since: Instant,
    // where it is now
    end: (i32, i32),
}

impl Touch {
    /// The gesture of the touch, lifted at `now`: from where it started and ended and how long
    /// it lasted
    fn gesture(&self, now: Instant) -> Gesture {
        let (dx, dy) = (self.end.0 - self.start.0, self.end.1 - self.start.1);
        if dx.abs() < TOUCH_SWIPE && dy.abs() < TOUCH_SWIPE {
            if now - self.since >= TOUCH_LONG_PRESS {
                Gesture::LongPress
            } else {
                Gesture::Tap
            }
        } else if dx.abs() > dy.abs() {
            if dx < 0 {
                Gesture::Left
            } else {
                Gesture::Right
            }
        } else if dy < 0 {
            Gesture::Up
        } else {
            Gesture::Down
        }
    }
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

    // the default clocks of the RP2350 (150 MHz from the 12 MHz crystal), at the default core
    // voltage; the flash and the SPI peripherals are clocked from the system clock as well
    let clocks = ClockConfig::system_freq(SYSTEM_CLOCK).unwrap();
    let p = embassy_rp::init(embassy_rp::config::Config::new(clocks));

    // LCD on SPI1: CLK GP10, DIN GP11, CS GP9, DC GP8, RST GP13, backlight GP25 (PWM)
    // The display is only written, so the SPI has no MISO and needs no receive DMA channel.
    let mut spi_config = spi::Config::default();
    spi_config.frequency = DISPLAY_SPI_FREQUENCY;
    let spi = Spi::new_txonly(p.SPI1, p.PIN_10, p.PIN_11, p.DMA_CH0, Irqs, spi_config);
    // shared between mipidsi and the DMA transfers
    let lcd_cs = RefCell::new(Output::new(p.PIN_9, Level::High));
    let lcd_dc = Output::new(p.PIN_8, Level::Low);
    let lcd_rst = Output::new(p.PIN_13, Level::Low);
    // dimmable backlight: GP25 is channel B of PWM slice 4,
    // 20 kHz (150 MHz / 10 / 750) does not flicker
    let mut backlight_config = pwm::Config::default();
    backlight_config.divider = 10.into();
    backlight_config.top = 749;
    let mut backlight = Pwm::new_output_b(p.PWM_SLICE4, p.PIN_25, backlight_config);
    // full brightness until the ui is running, then swipes up and down change it
    let _ = backlight.set_duty_cycle_percent(100);

    // touch controller CST816S on I2C1: SDA GP6, SCL GP7, reset GP22 (its interrupt on GP21 is
    // not needed, the touch is polled every frame); the QMI8658 IMU on the same bus is not used
    let mut i2c_config = i2c::Config::default();
    i2c_config.frequency = TOUCH_I2C_FREQUENCY;
    let mut i2c = I2c::new_blocking(p.I2C1, p.PIN_7, p.PIN_6, i2c_config);
    let mut touch_rst = Output::new(p.PIN_22, Level::Low);
    Timer::after_millis(10).await;
    touch_rst.set_high();
    Timer::after_millis(50).await;
    // no automatic sleep: a sleeping CST816S doesn't answer until it is touched again, so it
    // couldn't be polled; errors are ignored, then there is just no touch input
    let _ = i2c.blocking_write(TOUCH_ADDRESS, &[0xfe, 0x01]);

    let spi_device = ExclusiveDevice::new(spi, ChipSelect(&lcd_cs), Delay).unwrap();
    let mut interface_buffer = [0u8; 512];
    let interface = SpiInterface::new(spi_device, lcd_dc, &mut interface_buffer);

    // the panel needs inverted colours and BGR order, like in the Waveshare examples, which also
    // leave the scan direction unchanged (MADCTL 0x08), so the touch coordinates match
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

    // brightness set on the backlight, it changes with swipes up and down
    let mut brightness = 100;
    // automatic sequence of instruments: when it was last switched on or changed the instrument
    let mut auto_select = false;
    let mut instrument_shown_since = Instant::now();
    // the touch while the display is touched
    let mut touch: Option<Touch> = None;

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

        // touch gestures, when the finger is lifted: a tap or a swipe to the left shows the next
        // instrument, a swipe to the right the previous one, both stop the automatic sequence;
        // a long press starts it again; swipes up and down change the brightness
        match (read_touch(&mut i2c), touch) {
            (Some(position), None) => {
                touch = Some(Touch {
                    start: position,
                    since: frame_start,
                    end: position,
                })
            }
            (Some(position), Some(started)) => {
                touch = Some(Touch {
                    end: position,
                    ..started
                })
            }
            (None, Some(finished)) => {
                touch = None;
                let instrument = ui.get_instrument();
                match finished.gesture(frame_start) {
                    Gesture::Tap | Gesture::Left => {
                        ui.set_instrument((instrument + 1) % INSTRUMENTS);
                        ui.set_auto_select(false);
                    }
                    Gesture::Right => {
                        ui.set_instrument((instrument + INSTRUMENTS - 1) % INSTRUMENTS);
                        ui.set_auto_select(false);
                    }
                    Gesture::LongPress => ui.set_auto_select(true),
                    Gesture::Up => ui.set_brightness(
                        (ui.get_brightness() + BRIGHTNESS_STEP).clamp(BRIGHTNESS_MIN, 100),
                    ),
                    Gesture::Down => ui.set_brightness(
                        (ui.get_brightness() - BRIGHTNESS_STEP).clamp(BRIGHTNESS_MIN, 100),
                    ),
                }
            }
            (None, None) => {}
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

        // backlight brightness from the brightness property of the ui (5 to 100 %)
        let selected_brightness = ui.get_brightness().clamp(BRIGHTNESS_MIN, 100) as u8;
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
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
