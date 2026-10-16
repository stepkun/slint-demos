// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Firmware for a DFRobot UNIHIKER K10 (ESP32-S3)
// The instruments and the demo of the Pico firmwares in ../../rp2040 and ../../rp2350. The K10
// has no touch screen: its buttons A and B select the instrument instead.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use critical_section::Mutex;

use embedded_alloc::TlsfHeap as Heap;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi, SpiDma};
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::timer::OneShotTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{Blocking, dma_rx_buffer, dma_tx_buffer, handler, main};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565BigEndianPixel,
};

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
// instruments of the selector in the menu area, and how long each is shown in the automatic
// sequence
const INSTRUMENTS: i32 = 6;
const INSTRUMENT_TIME: Duration = Duration::from_secs(20);
// The ESP32-S3 has 512 KB internal SRAM, of which about 334 KB are left for the static data and
// the stack of the application (esp-hal's memory.x, shared with the code in IRAM): 150 KB frame
// buffer, 112 KB heap, the DMA buffer, the other statics and the stack. The instruments need
// about 61 KB of heap, which has to make do with 92 KB. The 8 MB PSRAM of the K10 isn't used.
const HEAP_SIZE: usize = 112 * 1024;
// copy buffer of the SPI DMA, the frame buffer rows are sent through it in chunks of this size
const DMA_BUFFER_SIZE: usize = 8 * 1024;

// TLSF keeps fragmentation low, the same allocator as on the Picos
static HEAP: Heap = Heap::empty();

// heap bytes in use and the highest value so far, shown in the status display
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

/// Sends a command with its parameters to the ILI9341
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
    // page (row) address set
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
    // the backlight goes on with the first frame, so the uninitialized panel isn't seen
    let mut backlight = false;

    // automatic sequence of instruments: when it was last switched on or changed the instrument
    let mut auto_select = false;
    let mut instrument_shown_since = Instant::now();
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
                let step = if released.0 { INSTRUMENTS - 1 } else { 1 };
                ui.set_instrument((ui.get_instrument() + step) % INSTRUMENTS);
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

        let elapsed = Instant::now() - statistics_start;
        if elapsed >= Duration::from_secs(1) && frames > 0 {
            let fps = frames as f32 * 1000.0 / elapsed.as_millis() as f32;
            let heap_peak = HEAP_PEAK.load(Ordering::Relaxed) / 1024;
            ui.set_status(slint::format!(
                "{:.1} fps, heap peak {} of {} KB",
                fps,
                heap_peak,
                HEAP_SIZE / 1024
            ));
            statistics_start = Instant::now();
            frames = 0;
        }

        // wait only for the rest of the frame time
        while Instant::now() - frame_start < FRAME_TIME {}
    }
}
