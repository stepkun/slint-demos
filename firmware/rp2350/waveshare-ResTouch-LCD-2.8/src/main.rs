// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a Raspberry Pi Pico 2 (RP2350) with a Waveshare Pico-ResTouch-LCD-2.8
// The same as the RP2040 firmware in ../../rp2040, apart from the chip specific parts.

#![no_std]
#![no_main]

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_rp::clocks::ClockConfig;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, SPI1};
use embassy_rp::pwm::{self, Pwm, SetDutyCycle};
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use firmware_common::display::{self, ChipSelect};
use firmware_common::{Sequence, Statistics, crash, demo};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use slint::platform::software_renderer::Rgb565BigEndianPixel;

slint::include_modules!();

// display in its native portrait orientation (rotation 0 degree)
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 320;
// System clock of the RP2350: its default and the highest officially specified one
// It has been sucessfully tested with 200 MHz and a SPI frequency of 100 MHz
// A higher clock would be the largest possible speed-up, but is outside the specification: the
// code runs from the flash through a cache, whose clock follows the system clock. On the RP2040
// (../../rp2040/waveshare-ResTouch-LCD-2.8) about half of the time of a frame is spent waiting
// for the flash; on the RP2350 a higher clock can make the instruments up to 30 % faster.
const SYSTEM_CLOCK: u32 = 150_000_000;
// SPI clock for the display: the system clock divided by an even number, the SPI can't divide
// by anything else, a frequency in between is rounded down to the next one. 75 MHz at 150 MHz
// (the ST7789 datasheet specifies 62.5 MHz, the RP2040 firmware runs it at 67.5 MHz)
// if the picture shows speckles or wrong colors, use SYSTEM_CLOCK / 4
const DISPLAY_SPI_FREQUENCY: u32 = SYSTEM_CLOCK / 2;
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
// RP2350 has 520 KB RAM, of which 512 KB are used: 150 KB frame buffer, 256 KB heap, the rest
// for stack and statics. The instruments need about 61 KB of heap on the RP2040, which has to
// make do with 92 KB; here the heap has plenty of room.
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // the heap must be ready before slint is used
    firmware_common::init_heap!(HEAP_SIZE);

    // the default clocks of the RP2350 (150 MHz from the 12 MHz crystal), at the default core
    // voltage; the flash and the SPI peripherals are clocked from the system clock as well
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
    // 20 kHz (150 MHz / 10 / 750) is neither visible nor audible
    let mut backlight_config = pwm::Config::default();
    backlight_config.divider = 10.into();
    backlight_config.top = 749;
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

    let window = firmware_common::init_window(DISPLAY_WIDTH, DISPLAY_HEIGHT, uptime);

    let ui = BasicPanelDevice::new().unwrap();
    // the ui arranges itself for portrait or landscape from the screen size
    ui.set_screen_width(DISPLAY_WIDTH as f32);
    ui.set_screen_height(DISPLAY_HEIGHT as f32);
    ui.show().unwrap();

    // SAFETY: the only reference to the frame buffer, created once
    let frame_buffer: &mut [Rgb565BigEndianPixel] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // frame statistics, shown once per second in the menu area
    let mut statistics = Statistics::new(uptime());

    // brightness set on the backlight, it changes with the slider in the menu area
    let mut brightness = 100;
    // automatic sequence of instruments, switched in the menu area
    let mut sequence = Sequence::default();
    // position of the touch while the display is touched, for the release event
    let mut touch: Option<slint::LogicalPosition> = None;
    // last raw and mapped touch position, shown with SHOW_TOUCH
    let mut last_touch = None;

    loop {
        let frame_start = Instant::now();
        let time = uptime();

        // demo movement until real sensor values are available
        firmware_common::show!(ui, demo::values(time));
        if sequence.next(ui.get_auto_select(), time) {
            ui.set_instrument(firmware_common::next_instrument(ui.get_instrument()));
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

        if let Some(mut status) = statistics.status(uptime()) {
            if let (Some(((raw_x, raw_y, pressure), position)), true) = (last_touch, SHOW_TOUCH) {
                status.push_str(&slint::format!(
                    "\ntouch raw {}, {} z {} at {:.0}, {:.0}",
                    raw_x,
                    raw_y,
                    pressure,
                    position.x,
                    position.y,
                ));
            }
            ui.set_status(status);
        }

        // wait only for the rest of the frame time, returns at once if the frame took longer
        Timer::at(frame_start + FRAME_TIME).await;
    }
}
