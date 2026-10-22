// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Waveshare RP2350-Touch-LCD-1.28 (round 240x240 GC9A01 display, CST816S touch)
// The display part is the same as in ../../rp2040/waveshare-LCD-1.28-B, the chip specific parts
// as in ../waveshare-ResTouch-LCD-2.8.

#![no_std]
#![no_main]

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_rp::clocks::ClockConfig;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::i2c::{self, Blocking, I2c};
use embassy_rp::peripherals::{DMA_CH0, I2C1};
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
// System clock of the RP2350: its default and the highest officially specified one
// A higher clock would be the largest possible speed-up, but is outside the specification: the
// code runs from the flash through a cache, whose clock follows the system clock. On the RP2040
// (../../rp2040/waveshare-ResTouch-LCD-2.8) about half of the time of a frame is spent waiting
// for the flash; on the RP2350 a higher clock can make the instruments up to 30 % faster.
const SYSTEM_CLOCK: u32 = 150_000_000;
// SPI clock for the display: the system clock divided by an even number, the SPI can't divide
// by anything else, a frequency in between is rounded down to the next one. 75 MHz at 150 MHz;
// the GC9A01 datasheet specifies a write cycle of at least 15 ns (66 MHz), the Waveshare
// examples of this board run it at 75 MHz as well.
// if the picture shows speckles or wrong colors, use SYSTEM_CLOCK / 4
const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 2;
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
// RP2350 has 520 KB RAM, of which 512 KB are used: 112.5 KB frame buffer, 256 KB heap, the rest
// for stack and statics. The instruments request at most 58 KB at this size
// (tools/host-check --display 240x240), so the heap has plenty of room.
const HEAP_SIZE: usize = 256 * 1024;

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
    crash::signal(&crash::RP2350, 25, SYSTEM_CLOCK)
}

/// Time since the start, for Slint and the demo
fn uptime() -> core::time::Duration {
    core::time::Duration::from_micros(Instant::now().as_micros())
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // the heap must be ready before slint is used
    firmware_common::init_heap!(HEAP_SIZE);

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

    let window = firmware_common::init_window(DISPLAY_WIDTH, DISPLAY_HEIGHT, uptime);

    let ui = BasicPanelDevice::new().unwrap();
    // a square screen: only the instrument, without the menu area
    ui.set_screen_width(DISPLAY_WIDTH as f32);
    ui.set_screen_height(DISPLAY_HEIGHT as f32);
    ui.show().unwrap();

    // SAFETY: the only reference to the frame buffer, created once
    let frame_buffer: &mut [Rgb565BigEndianPixel] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // brightness set on the backlight, it changes with swipes up and down
    let mut brightness = 100;
    // automatic sequence of instruments, a long press starts it
    let mut sequence = Sequence::default();
    // the touch while the display is touched
    let mut touch: Option<Touch> = None;

    loop {
        let frame_start = Instant::now();
        let time = uptime();

        // demo movement until real sensor values are available
        firmware_common::show!(ui, demo::values(time));

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
                        ui.set_instrument(firmware_common::next_instrument(instrument));
                        ui.set_auto_select(false);
                    }
                    Gesture::Right => {
                        ui.set_instrument(firmware_common::previous_instrument(instrument));
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

        if sequence.next(ui.get_auto_select(), time) {
            ui.set_instrument(firmware_common::next_instrument(ui.get_instrument()));
        }

        // backlight brightness from the brightness property of the ui (5 to 100 %)
        let selected_brightness = ui.get_brightness().clamp(BRIGHTNESS_MIN, 100) as u8;
        if selected_brightness != brightness {
            brightness = selected_brightness;
            let _ = backlight.set_duty_cycle_percent(brightness);
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
