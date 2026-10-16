// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Host check: renders the instrument UI like the RP2040 firmware and checks heap use and redraws

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565BigEndianPixel,
};

slint::include_modules!();

// the same display as in the firmware: 240x320 portrait
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 320;
// limits of the firmware: heap size, and the largest block that may be allocated after the
// start: the mask of a path band (MAX_MASK_BYTES of the patched renderer). Larger blocks may not
// be found any more once the heap is fragmented. Only the renderer's path buffers (mask and
// rasterizer cells) are larger, they are allocated once at the start, with the window.
const HEAP_LIMIT: usize = 92 * 1024;
const ALLOCATION_LIMIT: usize = 8 * 1024;
// largest allowed difference per colour channel between partial and full redraw, in RGB565 steps
// (the banded path masks shift anti-aliased edges by one step, see the firmware README)
const COLOUR_TOLERANCE: u8 = 1;

// the instruments of the selector in ui/test-app.slint, in the order of their `instrument` value
const INSTRUMENTS: [&str; 6] = [
    "attitude",
    "altimeter",
    "airspeed",
    "heading",
    "vertical-speed",
    "turn-coordinator",
];
// like the firmware demo, the instrument changes every 20 seconds
const INSTRUMENT_MS: u64 = 20_000;
// with --random the instrument changes after 1 to 10 seconds, like tapping the buttons
const RANDOM_MIN_MS: u64 = 1_000;
const RANDOM_MAX_MS: u64 = 10_000;

// defaults of the simulated run: the firmware reaches about 15 fps,
// 2100 frames are 141 seconds: each instrument for 20 seconds, then back to the first one,
// more than a full period of every demo value (the longest are altitude and heading, 60 s)
const DEFAULT_FRAMES: u64 = 2100;
const DEFAULT_FRAME_MS: u64 = 67;

// heap bytes in use, the highest value, the largest single allocation and the number of
// allocations, counted like the allocator of the firmware does (requested sizes only)
static HEAP_USED: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST_ALLOCATION: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// on 32-bit builds the heap of the firmware: its TLSF allocator in a block of HEAP_LIMIT bytes,
// so the overhead per block and the fragmentation are the same as on the device. Allocations
// made before it is switched on, by the Rust runtime, stay with the system allocator.
static TLSF: embedded_alloc::TlsfHeap = embedded_alloc::TlsfHeap::empty();
static USE_TLSF: AtomicBool = AtomicBool::new(false);
#[repr(align(8))]
#[allow(dead_code)] // only used through a pointer
struct Arena([u8; HEAP_LIMIT]);
static mut ARENA: Arena = Arena([0; HEAP_LIMIT]);
// frame being rendered and heap in use before the slint setup, for the out of memory report
static FRAME: AtomicU64 = AtomicU64::new(0);
static HEAP_BASE: AtomicUsize = AtomicUsize::new(0);
// size of the TLSF heap of this run, HEAP_LIMIT or less with --heap-kb
static HEAP_SIZE: AtomicUsize = AtomicUsize::new(HEAP_LIMIT);

fn in_arena(ptr: *mut u8) -> bool {
    let start = (&raw const ARENA) as usize;
    (start..start + HEAP_LIMIT).contains(&(ptr as usize))
}

/// Global allocator that counts like the one of the firmware
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = if USE_TLSF.load(Relaxed) {
            let ptr = unsafe { TLSF.alloc(layout) };
            // the use including the TLSF overhead isn't measured: `used()` and `free()` of
            // embedded-alloc 0.7 walk all blocks, `used()` panics (it borrows the heap twice),
            // and `free()` never returns when built with Rust 1.99 for i686
            if ptr.is_null() {
                // like the firmware this ends the run: report without allocating, then abort.
                // Returning null would let Rust's allocation error handler print a backtrace,
                // which allocates from the exhausted heap again and then deadlocks.
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr(),
                    "FAIL: out of heap in frame {}: allocation of {} bytes, {} bytes requested in use, TLSF heap of {} KB",
                    FRAME.load(Relaxed),
                    layout.size(),
                    HEAP_USED.load(Relaxed) - HEAP_BASE.load(Relaxed),
                    HEAP_SIZE.load(Relaxed) / 1024
                );
                std::process::abort();
            }
            ptr
        } else {
            unsafe { System.alloc(layout) }
        };
        if !ptr.is_null() {
            let used = HEAP_USED.fetch_add(layout.size(), Relaxed) + layout.size();
            HEAP_PEAK.fetch_max(used, Relaxed);
            LARGEST_ALLOCATION.fetch_max(layout.size(), Relaxed);
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP_USED.fetch_sub(layout.size(), Relaxed);
        if in_arena(ptr) {
            unsafe { TLSF.dealloc(ptr, layout) }
        } else {
            unsafe { System.dealloc(ptr, layout) }
        }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// simulated time since start, advanced by one frame time per frame
static SIMULATED_TIME_MS: AtomicU64 = AtomicU64::new(0);

/// Slint platform with the single window of the simulated display
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
        core::time::Duration::from_millis(SIMULATED_TIME_MS.load(Relaxed))
    }
}

/// Command line options
struct Options {
    frames: u64,
    frame_ms: u64,
    // only this instrument, instead of all one after the other
    instrument: Option<usize>,
    // allocate from the TLSF heap of the firmware instead of the system allocator
    // (default on 32-bit builds, a 64-bit build doesn't fit into it)
    tlsf: bool,
    // size of the TLSF heap in bytes, smaller than the firmware's to find the room left
    heap_size: usize,
    // seed of a random sequence of instruments instead of the demo sequence
    random: Option<u64>,
    png: Option<String>,
}

const USAGE: &str = "\
usage: host-check [--frames N] [--frame-ms MS] [--instrument NAME] [--random SEED]
                  [--heap-kb KB] [--system-alloc] [--png FILE]

  --frames N         number of animated frames (default 2100)
  --frame-ms MS      simulated time per frame in ms (default 67, about 15 fps like the firmware)
  --instrument NAME  only attitude, altimeter, airspeed, heading, vertical-speed or
                     turn-coordinator (default: each for 20 s, like the firmware demo)
  --random SEED      switch to a random other instrument every 1 to 10 s, like tapping the
                     buttons on the device; the same SEED gives the same sequence
  --heap-kb KB       TLSF heap of KB instead of 92 KB: the smallest size that still passes
                     shows how much room is left for the differences to the device
  --system-alloc     only count the allocations, instead of allocating from the TLSF heap of
                     the firmware (92 KB, the default on 32-bit builds like i686, which
                     tools/host-check/.cargo/config.toml selects)
  --png FILE         save the last partially redrawn frame as PNG";

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        frames: DEFAULT_FRAMES,
        frame_ms: DEFAULT_FRAME_MS,
        instrument: None,
        tlsf: cfg!(target_pointer_width = "32"),
        heap_size: HEAP_LIMIT,
        random: None,
        png: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--frames" => {
                options.frames = value()?.parse().map_err(|e| format!("--frames: {e}"))?
            }
            "--frame-ms" => {
                options.frame_ms = value()?.parse().map_err(|e| format!("--frame-ms: {e}"))?
            }
            "--instrument" => {
                let name = value()?;
                let index = INSTRUMENTS.iter().position(|i| *i == name);
                options.instrument =
                    Some(index.ok_or(format!("--instrument: unknown instrument {name}"))?);
            }
            "--random" => {
                options.random = Some(value()?.parse().map_err(|e| format!("--random: {e}"))?)
            }
            "--heap-kb" => {
                let kb: usize = value()?.parse().map_err(|e| format!("--heap-kb: {e}"))?;
                if kb == 0 || kb * 1024 > HEAP_LIMIT {
                    return Err(format!("--heap-kb must be 1 to {}", HEAP_LIMIT / 1024));
                }
                options.heap_size = kb * 1024;
            }
            "--system-alloc" => options.tlsf = false,
            "--png" => options.png = Some(value()?),
            "-h" | "--help" => return Err(String::new()),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if options.frames == 0 || options.frame_ms == 0 {
        return Err("--frames and --frame-ms must be greater than 0".into());
    }
    if options.instrument.is_some() && options.random.is_some() {
        return Err("--instrument and --random exclude each other".into());
    }
    Ok(options)
}

/// Triangle wave between -amplitude and +amplitude with the given period, as in the firmware
fn triangle(time_ms: u64, period_ms: u64, amplitude: f32) -> f32 {
    let phase = (time_ms % period_ms) as f32 / period_ms as f32;
    let value = if phase < 0.5 {
        phase * 4.0 - 1.0
    } else {
        3.0 - phase * 4.0
    };
    value * amplitude
}

/// Red, green and blue of a pixel in RGB565 steps (5, 6 and 5 bit)
fn channels(pixel: Rgb565BigEndianPixel) -> [u8; 3] {
    let v = u16::from_be(pixel.0);
    [(v >> 11) as u8, ((v >> 5) & 0x3f) as u8, (v & 0x1f) as u8]
}

/// Result of comparing the partially redrawn frame buffer with a fresh full render
#[derive(Default)]
struct Comparison {
    differing_pixels: usize,
    failing_pixels: usize,
    largest_difference: u8,
    first_failing: Option<(usize, usize)>,
}

/// Renders the current state again from scratch and compares it with the frame buffer,
/// NewBuffer clears the renderer caches, so the next frame is a full redraw again
fn compare_with_full_render(
    window: &MinimalSoftwareWindow,
    frame_buffer: &[Rgb565BigEndianPixel],
    full_buffer: &mut [Rgb565BigEndianPixel],
) -> Comparison {
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        renderer.render(full_buffer, DISPLAY_WIDTH);
        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
    });

    let mut result = Comparison::default();
    for (index, (partial, full)) in frame_buffer.iter().zip(full_buffer.iter()).enumerate() {
        let difference = channels(*partial)
            .iter()
            .zip(channels(*full))
            .map(|(a, b)| a.abs_diff(b))
            .max()
            .unwrap();
        if difference > 0 {
            result.differing_pixels += 1;
            result.largest_difference = result.largest_difference.max(difference);
        }
        if difference > COLOUR_TOLERANCE {
            result.failing_pixels += 1;
            result
                .first_failing
                .get_or_insert((index % DISPLAY_WIDTH, index / DISPLAY_WIDTH));
        }
    }
    result
}

fn save_png(path: &str, buffer: &[Rgb565BigEndianPixel]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        DISPLAY_WIDTH as u32,
        DISPLAY_HEIGHT as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = buffer
        .iter()
        .flat_map(|p| [p.red(), p.green(), p.blue()])
        .collect();
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&data))
        .map_err(|e| format!("{path}: {e}"))
}

/// xorshift64*, enough for a reproducible sequence of instruments without a dependency
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        // the state must not be 0
        Random(seed ^ 0x9e37_79b9_7f4a_7c15 | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// a number from 0 to n - 1
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The instrument shown in every frame: the demo sequence of the firmware, one instrument,
/// or a random sequence
fn instrument_sequence(options: &Options) -> Vec<usize> {
    if let Some(instrument) = options.instrument {
        return vec![instrument; options.frames as usize];
    }
    let Some(seed) = options.random else {
        return (0..options.frames)
            .map(|frame| (frame * options.frame_ms / INSTRUMENT_MS) as usize % INSTRUMENTS.len())
            .collect();
    };
    let mut random = Random::new(seed);
    let mut sequence = Vec::with_capacity(options.frames as usize);
    let mut shown = 0;
    let mut next_switch_ms = 0;
    for frame in 0..options.frames {
        let time_ms = frame * options.frame_ms;
        if time_ms >= next_switch_ms {
            if frame > 0 {
                // another instrument than the one shown, as a tap on another button
                shown = (shown + 1 + random.below(INSTRUMENTS.len() as u64 - 1) as usize)
                    % INSTRUMENTS.len();
            }
            next_switch_ms = time_ms + RANDOM_MIN_MS + random.below(RANDOM_MAX_MS - RANDOM_MIN_MS);
        }
        sequence.push(shown);
    }
    sequence
}


fn main() -> ExitCode {
    let options = match parse_options() {
        Ok(options) => options,
        Err(error) => {
            if !error.is_empty() {
                eprintln!("{error}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    // the frame buffers are static in the firmware, so they are allocated before counting starts
    let mut frame_buffer = vec![Rgb565BigEndianPixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];
    // a full redraw must cover every pixel, magenta shows the ones that are not
    let mut full_buffer =
        vec![Rgb565BigEndianPixel(0xf81fu16.to_be()); DISPLAY_WIDTH * DISPLAY_HEIGHT];
    let mut frame_allocations = Vec::with_capacity(options.frames as usize);
    let mut comparisons: Vec<(usize, Comparison)> = Vec::with_capacity(options.frames as usize);
    let sequence = instrument_sequence(&options);
    let instrument_at = |frame: u64| sequence[frame as usize];
    // the first output allocates the stdout buffer, so it is done before counting starts too
    println!(
        "host check: {DISPLAY_WIDTH}x{DISPLAY_HEIGHT}, {} frames of {} ms, {}{}",
        options.frames,
        options.frame_ms,
        if options.tlsf {
            format!("TLSF heap of {} KB", options.heap_size / 1024)
        } else {
            "system allocator".into()
        },
        match options.random {
            Some(seed) => format!(", random instruments (seed {seed})"),
            None => String::new(),
        }
    );
    if cfg!(target_pointer_width = "64") {
        println!(
            "note: 64-bit build, the heap numbers are larger than on the device and the TLSF heap \
             is not used, see README.md"
        );
    }

    if options.tlsf {
        // SAFETY: the arena is only used by the TLSF heap, which is initialized once
        unsafe { TLSF.init((&raw mut ARENA) as usize, options.heap_size) };
        HEAP_SIZE.store(options.heap_size, Relaxed);
        USE_TLSF.store(true, Relaxed);
    }

    // from here on the heap is counted, like in the firmware from the start
    let heap_base = HEAP_USED.load(Relaxed);
    HEAP_BASE.store(heap_base, Relaxed);
    HEAP_PEAK.store(heap_base, Relaxed);
    LARGEST_ALLOCATION.store(0, Relaxed);

    // slint setup, as in the firmware
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
    ui.set_screen_width(DISPLAY_WIDTH as f32);
    ui.set_screen_height(DISPLAY_HEIGHT as f32);
    ui.show().unwrap();
    let setup_heap = HEAP_USED.load(Relaxed) - heap_base;
    // the larger blocks of the start, while the heap is empty, are allowed
    let setup_largest_allocation = LARGEST_ALLOCATION.swap(0, Relaxed);

    // heap peak per instrument: the peak of each frame counts for the instrument that exists
    // in it (`shown` of the UI, which follows `instrument` through -1 for a frame, so the old
    // instrument is destroyed before the new one is created); the frame that creates an
    // instrument counts for it, the frame without one for the instrument before
    let mut instrument_peaks = [None; INSTRUMENTS.len()];
    let mut shown = instrument_at(0);
    ui.set_instrument(shown as i32);
    let mut existing = shown;
    let mut heap_peak = 0;

    // status line like in the firmware, updated once per simulated second
    let mut statistics_start = 0;
    let mut statistics_frames: u32 = 0;
    let mut redraw_rows = 0usize;

    for frame in 0..options.frames {
        let time_ms = frame * options.frame_ms;
        FRAME.store(frame, Relaxed);
        SIMULATED_TIME_MS.store(time_ms, Relaxed);
        let allocations_before = ALLOCATIONS.load(Relaxed);

        // the heap peak of this frame
        HEAP_PEAK.store(HEAP_USED.load(Relaxed), Relaxed);

        let instrument = instrument_at(frame);
        if instrument != shown {
            shown = instrument;
            ui.set_instrument(instrument as i32);
        }

        // demo movement of the firmware
        ui.set_roll(triangle(time_ms, 8000, 30.0));
        ui.set_pitch(triangle(time_ms, 5000, 10.0));
        ui.set_altitude(5000.0 + triangle(time_ms, 60_000, 5000.0));
        ui.set_airspeed(105.0 + triangle(time_ms, 30_000, 75.0));
        ui.set_heading((time_ms % 60_000) as f32 * 0.006);
        ui.set_vertical_speed(triangle(time_ms, 40_000, 2200.0));
        ui.set_turn_rate(triangle(time_ms, 30_000, 5.0));
        ui.set_slip(triangle(time_ms, 12_000, 0.6));

        slint::platform::update_timers_and_animations();

        let rendered = Cell::new(false);
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut frame_buffer, DISPLAY_WIDTH);
            redraw_rows += region.bounding_box_size().height as usize;
            rendered.set(true);
        });
        if rendered.get() {
            statistics_frames += 1;
        }

        // after the last frame of an instrument, while nothing has changed since rendering it,
        // compare it with a fresh full render (its allocations are not counted)
        if frame + 1 == options.frames || instrument_at(frame + 1) != shown {
            let peak = HEAP_PEAK.load(Relaxed);
            let allocations = ALLOCATIONS.load(Relaxed);
            comparisons.push((
                shown,
                compare_with_full_render(&window, &frame_buffer, &mut full_buffer),
            ));
            HEAP_PEAK.store(peak, Relaxed);
            ALLOCATIONS.store(allocations, Relaxed);
        }

        let elapsed_ms = time_ms - statistics_start;
        if elapsed_ms >= 1000 && statistics_frames > 0 {
            let fps = statistics_frames as f32 * 1000.0 / elapsed_ms as f32;
            ui.set_status(slint::format!(
                "{:.1} fps, heap peak {} of {} KB",
                fps,
                (HEAP_PEAK.load(Relaxed) - heap_base).max(heap_peak) / 1024,
                options.heap_size / 1024,
            ));
            statistics_start = time_ms;
            statistics_frames = 0;
        }

        if let Ok(index) = usize::try_from(ui.get_shown()) {
            existing = index;
        }
        let frame_peak = HEAP_PEAK.load(Relaxed) - heap_base;
        let peak = &mut instrument_peaks[existing];
        *peak = Some(peak.unwrap_or(0).max(frame_peak));
        heap_peak = heap_peak.max(frame_peak);

        frame_allocations.push(ALLOCATIONS.load(Relaxed) - allocations_before);
    }

    // results of the animated run
    // the report and the PNG are not part of the firmware, they don't use its heap
    USE_TLSF.store(false, Relaxed);
    let largest_allocation = LARGEST_ALLOCATION.load(Relaxed);
    let heap_end = HEAP_USED.load(Relaxed) - heap_base;

    // report
    let kb = |bytes: usize| bytes as f32 / 1024.0;
    let first_frame = frame_allocations[0];
    let following = &frame_allocations[1..];
    println!("heap after ui setup:        {:6.1} KB", kb(setup_heap));
    println!("heap after last frame:      {:6.1} KB", kb(heap_end));
    println!(
        "heap peak:                  {:6.1} KB (limit {} KB)",
        kb(heap_peak),
        options.heap_size / 1024
    );
    for (name, peak) in INSTRUMENTS.iter().zip(instrument_peaks) {
        if let Some(peak) = peak {
            println!("  {:<25} {:6.1} KB", format!("{name}:"), kb(peak));
        }
    }
    println!(
        "largest allocation, start:  {:6.1} KB (path buffers of the renderer)",
        kb(setup_largest_allocation)
    );
    println!(
        "largest allocation, frames: {:6.1} KB (limit {} KB)",
        kb(largest_allocation),
        ALLOCATION_LIMIT / 1024
    );
    println!("allocations, first frame:   {first_frame}");
    if !following.is_empty() {
        let average = following.iter().sum::<usize>() as f32 / following.len() as f32;
        let min = following.iter().min().unwrap();
        let max = following.iter().max().unwrap();
        println!("allocations, later frames:  {average:.1} on average, {min} to {max}");
    }
    println!(
        "redrawn rows per frame:     {:.1} of {DISPLAY_HEIGHT} on average",
        redraw_rows as f32 / options.frames as f32
    );
    println!(
        "partial vs. full redraw, at the end of each instrument (limit {COLOUR_TOLERANCE} colour step):"
    );
    for (instrument, comparison) in &comparisons {
        println!(
            "  {:<25} {} pixels differ, largest difference {} colour steps",
            format!("{}:", INSTRUMENTS[*instrument]),
            comparison.differing_pixels,
            comparison.largest_difference
        );
    }

    let mut failed = false;
    if heap_peak > options.heap_size {
        println!(
            "FAIL: heap peak {:.1} KB exceeds {} KB",
            kb(heap_peak),
            options.heap_size / 1024
        );
        failed = true;
    }
    if largest_allocation > ALLOCATION_LIMIT {
        println!(
            "FAIL: single allocation of {:.1} KB exceeds {} KB",
            kb(largest_allocation),
            ALLOCATION_LIMIT / 1024
        );
        failed = true;
    }
    for (instrument, comparison) in &comparisons {
        if let Some((x, y)) = comparison.first_failing {
            println!(
                "FAIL: {}: {} pixels differ by more than {COLOUR_TOLERANCE} colour step between partial and full redraw, first at x {x}, y {y}",
                INSTRUMENTS[*instrument], comparison.failing_pixels
            );
            failed = true;
        }
    }

    if let Some(path) = &options.png {
        match save_png(path, &frame_buffer) {
            Ok(()) => println!("last frame saved to {path}"),
            Err(error) => {
                eprintln!("could not save the last frame: {error}");
                failed = true;
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        println!("OK");
        ExitCode::SUCCESS
    }
}
