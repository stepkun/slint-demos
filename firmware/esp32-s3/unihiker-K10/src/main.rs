// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a DFRobot UNIHIKER K10 (ESP32-S3)
// The instruments and the demo of the Pico firmwares in ../../rp2040 and ../../rp2350. The K10
// has no touch screen: its buttons A and B select the instrument instead.

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use critical_section::Mutex;

use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::timer::OneShotTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{Blocking, dma_rx_buffer, dma_tx_buffer, handler, main};
use firmware_common::display::{self, ChipSelect};
use firmware_common::{Sequence, Statistics, demo};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation};
use slint::platform::software_renderer::Rgb565BigEndianPixel;

slint::include_modules!();

// application descriptor, the bootloader of ESP-IDF that espflash writes checks it
esp_bootloader_esp_idf::esp_app_desc!();

// display in its native portrait orientation, turned by 180 degree like DFRobot's own firmware
// (the panel is mounted upside down) and mirrored horizontally: its columns run the other way
// round (seen on the device)
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 320;
// SPI clock for the display: the pins of the K10 (SCK GPIO12, MOSI GPIO21) go through the GPIO
// matrix, which allows 40 MHz; DFRobot's firmware uses the same
const DISPLAY_SPI_FREQUENCY: Rate = Rate::from_mhz(40);
// the I2C bus with the IO expander (and the sensors of the board)
const I2C_FREQUENCY: Rate = Rate::from_khz(400);
// IO expander XL9535 (registers like a PCA9555: input 0/1, output 2/3, configuration 6/7, a 1
// configures an input). Its pins carry the backlight and the buttons, see DFRobot's MicroPython
// firmware (K10_Micropython, modules mpython.py and _k10_base.py).
const EXPANDER: u8 = 0x20;
const EXPANDER_INPUT: u8 = 0x00;
const EXPANDER_OUTPUT: u8 = 0x02;
const EXPANDER_CONFIG: u8 = 0x06;
// port 0 bit 0: backlight, on/off only: it is dimmed by switching it on and off from a timer
// interrupt, BACKLIGHT_PERIOD_US long (200 Hz), each switch is one I2C write of about 70 us
const BACKLIGHT: (usize, u8) = (0, 1 << 0);
const BACKLIGHT_PERIOD_US: u32 = 5000;
// buttons, low while pressed: A on port 1 bit 4, B on port 0 bit 2
const BUTTON_A: (usize, u8) = (1, 1 << 4);
const BUTTON_B: (usize, u8) = (0, 1 << 2);
// a button held this long changes the brightness instead of the instrument: A brighter, B
// dimmer, by BRIGHTNESS_STEP percent every BRIGHTNESS_REPEAT while held (5 to 100 % in 2 s)
const LONG_PRESS: Duration = Duration::from_millis(500);
const BRIGHTNESS_REPEAT: Duration = Duration::from_millis(100);
const BRIGHTNESS_STEP: i32 = 5;
// shortest time per frame, limits the frame rate to 60 per second
const FRAME_TIME: Duration = Duration::from_millis(17);
// The ESP32-S3 has 512 KB internal SRAM, of which about 334 KB are left for the static data and
// the stack of the application (esp-hal's memory.x, shared with the code in IRAM): 150 KB frame
// buffer, 112 KB heap, the DMA buffer, the other statics and the stack. The instruments need
// about 61 KB of heap, which has to make do with 92 KB. The 8 MB PSRAM of the K10 isn't used.
const HEAP_SIZE: usize = 112 * 1024;
// copy buffer of the SPI DMA, the frame buffer rows are sent through it in chunks of this size
const DMA_BUFFER_SIZE: usize = 8 * 1024;

// The IO expander, used by the main loop (buttons) and by the backlight interrupt, and the timer
// of that interrupt. The backlight is on for BACKLIGHT_ON_US of each period.
static EXPANDER_DEVICE: Mutex<RefCell<Option<Expander<'static>>>> = Mutex::new(RefCell::new(None));
static BACKLIGHT_TIMER: Mutex<RefCell<Option<OneShotTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));
static BACKLIGHT_ON_US: AtomicU32 = AtomicU32::new(BACKLIGHT_PERIOD_US);
static BACKLIGHT_LIT: AtomicBool = AtomicBool::new(false);

/// Timer interrupt of the backlight: switches it on at the start of each period and off after
/// BACKLIGHT_ON_US, and schedules itself for the next switch. At full brightness it stays on and
/// the expander isn't written.
#[handler]
fn backlight_interrupt() {
    critical_section::with(|cs| {
        let mut timer = BACKLIGHT_TIMER.borrow_ref_mut(cs);
        let mut expander = EXPANDER_DEVICE.borrow_ref_mut(cs);
        let (Some(timer), Some(expander)) = (timer.as_mut(), expander.as_mut()) else {
            return;
        };
        timer.clear_interrupt();
        let on_us = BACKLIGHT_ON_US.load(Ordering::Relaxed);
        let lit = BACKLIGHT_LIT.load(Ordering::Relaxed);
        let (light, next_us) = if on_us >= BACKLIGHT_PERIOD_US {
            (true, BACKLIGHT_PERIOD_US)
        } else if lit {
            (false, BACKLIGHT_PERIOD_US - on_us)
        } else {
            (true, on_us)
        };
        if light != lit {
            expander.set_backlight(light);
            BACKLIGHT_LIT.store(light, Ordering::Relaxed);
        }
        let _ = timer.schedule(Duration::from_micros(u64::from(next_us)));
    });
}

// a full frame buffer is needed, because the software renderer
// skips Path elements when rendering line by line
// the pixels are stored in the byte order of the ILI9341, so they are sent unchanged
static mut FRAME_BUFFER: [Rgb565BigEndianPixel; DISPLAY_WIDTH * DISPLAY_HEIGHT] =
    [Rgb565BigEndianPixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];

/// Time since the start, for Slint and the demo
fn uptime() -> core::time::Duration {
    core::time::Duration::from_micros(Instant::now().duration_since_epoch().as_micros())
}

/// The IO expander with the backlight and the buttons
struct Expander<'d> {
    i2c: I2c<'d, Blocking>,
    // value of the output register of port 0, so the backlight is switched with a single write
    output_0: u8,
}

impl Expander<'_> {
    /// Sets or clears bits of a register of a port, the other bits keep their value (the
    /// expander also drives the camera reset and the amplifier of the speaker)
    fn modify(&mut self, register: u8, (port, bits): (usize, u8), set: bool) {
        let register = register + port as u8;
        let mut value = [0u8];
        // errors are ignored, the next frame tries again
        if self
            .i2c
            .write_read(EXPANDER, &[register], &mut value)
            .is_ok()
        {
            let value = if set {
                value[0] | bits
            } else {
                value[0] & !bits
            };
            let _ = self.i2c.write(EXPANDER, &[register, value]);
        }
    }

    /// Reads the output register of port 0, configures the backlight pin as an output
    fn init_backlight(&mut self) {
        let mut value = [0u8];
        let _ = self
            .i2c
            .write_read(EXPANDER, &[EXPANDER_OUTPUT], &mut value);
        self.output_0 = value[0];
        self.modify(EXPANDER_CONFIG, BACKLIGHT, false);
    }

    /// Switches the backlight, with one write of the cached output register of port 0
    fn set_backlight(&mut self, on: bool) {
        let (_, bits) = BACKLIGHT;
        self.output_0 = if on {
            self.output_0 | bits
        } else {
            self.output_0 & !bits
        };
        // errors are ignored, the next period switches it again
        let _ = self.i2c.write(EXPANDER, &[EXPANDER_OUTPUT, self.output_0]);
    }

    /// Configures the pins of the buttons as inputs
    fn buttons_as_inputs(&mut self) {
        self.modify(EXPANDER_CONFIG, BUTTON_A, true);
        self.modify(EXPANDER_CONFIG, BUTTON_B, true);
    }

    /// Buttons A and B, true while pressed
    fn buttons(&mut self) -> (bool, bool) {
        let mut input = [0xffu8; 2];
        // a failed read counts as not pressed
        let _ = self.i2c.write_read(EXPANDER, &[EXPANDER_INPUT], &mut input);
        let pressed = |(port, bits): (usize, u8)| input[port] & bits == 0;
        (pressed(BUTTON_A), pressed(BUTTON_B))
    }
}

#[main]
fn main() -> ! {
    // the heap must be ready before slint is used
    // a failed allocation panics, the panic is printed on the USB serial port
    firmware_common::init_heap!(HEAP_SIZE);

    // 240 MHz, the highest clock of the ESP32-S3
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let delay = Delay::new();

    // The speaker amplifier (NS4168) is always on (CTRL tied to 3V3), and its I2S inputs pick up
    // the display transfers as noise while they float: the speaker chirps. Held low, it gets no
    // clock and stays silent. LRCK GPIO38, BCLK GPIO0, data GPIO45 (both strapping pins, only
    // read at reset). Bound to names, so they stay driven while main runs.
    let _speaker_lrck = Output::new(p.GPIO38, Level::Low, OutputConfig::default());
    let _speaker_bclk = Output::new(p.GPIO0, Level::Low, OutputConfig::default());
    let _speaker_data = Output::new(p.GPIO45, Level::Low, OutputConfig::default());

    // IO expander on I2C0: SDA GPIO47, SCL GPIO48
    let i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(I2C_FREQUENCY))
        .unwrap()
        .with_sda(p.GPIO47)
        .with_scl(p.GPIO48);
    let mut expander = Expander { i2c, output_0: 0 };
    expander.buttons_as_inputs();
    expander.init_backlight();
    critical_section::with(|cs| EXPANDER_DEVICE.borrow_ref_mut(cs).replace(expander));

    // timer of the backlight interrupt, it starts with the first frame
    let timer_group = TimerGroup::new(p.TIMG0);
    let mut backlight_timer = OneShotTimer::new(timer_group.timer0);
    backlight_timer.set_interrupt_handler(backlight_interrupt);
    backlight_timer.listen();
    critical_section::with(|cs| BACKLIGHT_TIMER.borrow_ref_mut(cs).replace(backlight_timer));

    // LCD on SPI2: SCK GPIO12, MOSI GPIO21, CS GPIO14, DC GPIO13, no reset pin (software reset)
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(DISPLAY_SPI_FREQUENCY)
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(p.GPIO12)
    .with_mosi(p.GPIO21)
    .with_dma(p.DMA_CH0)
    .with_buffers(
        dma_rx_buffer!(4).unwrap(),
        dma_tx_buffer!(DMA_BUFFER_SIZE).unwrap(),
    );
    // shared between mipidsi and the frame transfers
    let lcd_cs = RefCell::new(Output::new(p.GPIO14, Level::High, OutputConfig::default()));
    let lcd_dc = Output::new(p.GPIO13, Level::Low, OutputConfig::default());

    let spi_device = ExclusiveDevice::new(spi, ChipSelect(&lcd_cs), delay).unwrap();
    let mut interface_buffer = [0u8; 512];
    let interface = SpiInterface::new(spi_device, lcd_dc, &mut interface_buffer);

    // the ILI9341 is a 240x320 portrait panel, BGR like in DFRobot's firmware
    // mipidsi is only used to initialize the display
    let display = mipidsi::Builder::new(ILI9341Rgb565, interface)
        .display_size(240, 320)
        .orientation(Orientation::new().flip_vertical())
        .color_order(ColorOrder::Bgr)
        .invert_colors(ColorInversion::Normal)
        .init(&mut delay.clone())
        .unwrap();
    // take the bus back for the frame transfers
    let (interface, _model, _reset) = display.release();
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
    // the backlight goes on with the first frame, so the uninitialized panel isn't seen
    let mut backlight = false;

    // automatic sequence of instruments, switched by both buttons together
    let mut sequence = Sequence::default();
    // buttons pressed in the last frame, an action happens when a button is released
    let mut last_buttons = (false, false);
    // both buttons were pressed together since the last release of all buttons
    let mut both_pressed = false;
    // when the button that is held alone was pressed, and when the brightness was last changed
    let mut pressed_since = Instant::now();
    let mut brightness_changed = Instant::now();
    // the button held alone has changed the brightness, its release doesn't change the instrument
    let mut long_press = false;
    // brightness of the backlight (5 to 100 %), the slider in the menu area shows it
    let mut brightness = ui.get_brightness().clamp(5, 100);
    BACKLIGHT_ON_US.store(
        brightness as u32 * BACKLIGHT_PERIOD_US / 100,
        Ordering::Relaxed,
    );

    loop {
        let frame_start = Instant::now();
        let time = uptime();

        // demo movement until real sensor values are available
        firmware_common::show!(ui, demo::values(time));

        // Buttons instead of the touch buttons of the menu: A shows the previous, B the next
        // instrument, both together toggle the automatic sequence. Like a touch on an
        // instrument button, A or B switch it off. Held alone, A makes the display brighter and B
        // dimmer, like the brightness slider of the Picos (which shows the value).
        let buttons = critical_section::with(|cs| {
            EXPANDER_DEVICE
                .borrow_ref_mut(cs)
                .as_mut()
                .map_or((false, false), |expander| expander.buttons())
        });
        if buttons.0 && buttons.1 {
            both_pressed = true;
        }
        if (buttons.0 && !last_buttons.0) || (buttons.1 && !last_buttons.1) {
            pressed_since = frame_start;
        }
        if buttons.0 != buttons.1
            && !both_pressed
            && frame_start - pressed_since >= LONG_PRESS
            && (!long_press || frame_start - brightness_changed >= BRIGHTNESS_REPEAT)
        {
            let step = if buttons.0 {
                BRIGHTNESS_STEP
            } else {
                -BRIGHTNESS_STEP
            };
            ui.set_brightness((ui.get_brightness() + step).clamp(5, 100));
            brightness_changed = frame_start;
            long_press = true;
        }
        let released = (last_buttons.0 && !buttons.0, last_buttons.1 && !buttons.1);
        if !buttons.0 && !buttons.1 && (released.0 || released.1) {
            if both_pressed {
                ui.set_auto_select(!ui.get_auto_select());
            } else if !long_press {
                let instrument = ui.get_instrument();
                ui.set_instrument(if released.0 {
                    firmware_common::previous_instrument(instrument)
                } else {
                    firmware_common::next_instrument(instrument)
                });
                ui.set_auto_select(false);
            }
            both_pressed = false;
            long_press = false;
        }
        last_buttons = buttons;

        // brightness from the slider value in the menu area (5 to 100 %), the backlight
        // interrupt picks it up with its next switch
        let selected_brightness = ui.get_brightness().clamp(5, 100);
        if selected_brightness != brightness {
            brightness = selected_brightness;
            BACKLIGHT_ON_US.store(
                brightness as u32 * BACKLIGHT_PERIOD_US / 100,
                Ordering::Relaxed,
            );
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
            if !backlight {
                // the first interrupt switches the backlight on
                critical_section::with(|cs| {
                    if let Some(timer) = BACKLIGHT_TIMER.borrow_ref_mut(cs).as_mut() {
                        let _ = timer.schedule(Duration::from_micros(10));
                    }
                });
                backlight = true;
            }
        }

        if let Some(status) = statistics.status(uptime()) {
            ui.set_status(status);
        }

        // wait only for the rest of the frame time
        while Instant::now() - frame_start < FRAME_TIME {}
    }
}
