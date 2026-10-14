// Copyright © 2026 Stephan Kunz
// SPDX-License-Identifier: GPL-3.0-only
// Host check: renders the instrument UI like the RP2040 firmware and checks heap use and redraws

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565BigEndianPixel,
};

slint::include_modules!();

// the same display as in the firmware: 240x320 portrait
const DISPLAY_WIDTH: usize = 240;
const DISPLAY_HEIGHT: usize = 320;
// limits of the firmware: heap size, and the largest block the patched renderer
// allocates for a path mask band (MAX_MASK_BYTES)
const HEAP_LIMIT: usize = 92 * 1024;
const ALLOCATION_LIMIT: usize = 24 * 1024;
// largest allowed difference per colour channel between partial and full redraw, in RGB565 steps
// (the banded path masks shift anti-aliased edges by one step, see the firmware README)
const COLOUR_TOLERANCE: u8 = 1;

// defaults of the simulated run: the firmware reaches about 15 fps,
// 600 frames are 40 seconds, more than a full roll (8 s) and pitch (5 s) period
const DEFAULT_FRAMES: u64 = 600;
const DEFAULT_FRAME_MS: u64 = 67;

// heap bytes in use, the highest value, the largest single allocation and the number of
// allocations, counted like the allocator of the firmware does (requested sizes only)
static HEAP_USED: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST_ALLOCATION: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Global allocator that counts like the one of the firmware
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
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
        unsafe { System.dealloc(ptr, layout) }
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
    png: Option<String>,
}

const USAGE: &str = "\
usage: host-check [--frames N] [--frame-ms MS] [--png FILE]

  --frames N     number of animated frames (default 600)
  --frame-ms MS  simulated time per frame in ms (default 67, about 15 fps like the firmware)
  --png FILE     save the last partially redrawn frame as PNG";

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        frames: DEFAULT_FRAMES,
        frame_ms: DEFAULT_FRAME_MS,
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
            "--png" => options.png = Some(value()?),
            "-h" | "--help" => return Err(String::new()),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if options.frames == 0 || options.frame_ms == 0 {
        return Err("--frames and --frame-ms must be greater than 0".into());
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
    // the first output allocates the stdout buffer, so it is done before counting starts too
    println!(
        "host check: {DISPLAY_WIDTH}x{DISPLAY_HEIGHT}, {} frames of {} ms",
        options.frames, options.frame_ms
    );

    // from here on the heap is counted, like in the firmware from the start
    let heap_base = HEAP_USED.load(Relaxed);
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

    // status line like in the firmware, updated once per simulated second
    let mut statistics_start = 0;
    let mut statistics_frames: u32 = 0;
    let mut render_time = Duration::ZERO;
    let mut redraw_rows = 0usize;

    for frame in 0..options.frames {
        let time_ms = frame * options.frame_ms;
        SIMULATED_TIME_MS.store(time_ms, Relaxed);
        let allocations_before = ALLOCATIONS.load(Relaxed);

        // demo movement of the firmware
        ui.set_roll(triangle(time_ms, 8000, 30.0));
        ui.set_pitch(triangle(time_ms, 5000, 10.0));

        slint::platform::update_timers_and_animations();

        let render_start = Instant::now();
        let rendered = Cell::new(false);
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut frame_buffer, DISPLAY_WIDTH);
            redraw_rows += region.bounding_box_size().height as usize;
            rendered.set(true);
        });
        if rendered.get() {
            statistics_frames += 1;
            render_time += render_start.elapsed();
        }

        let elapsed_ms = time_ms - statistics_start;
        if elapsed_ms >= 1000 && statistics_frames > 0 {
            let fps = statistics_frames as f32 * 1000.0 / elapsed_ms as f32;
            ui.set_status(slint::format!(
                "{:.1} fps, render {} ms, send {} ms\nheap peak {} of {} KB",
                fps,
                render_time.as_millis() / statistics_frames as u128,
                0,
                (HEAP_PEAK.load(Relaxed) - heap_base) / 1024,
                HEAP_LIMIT / 1024,
            ));
            statistics_start = time_ms;
            statistics_frames = 0;
            render_time = Duration::ZERO;
        }

        frame_allocations.push(ALLOCATIONS.load(Relaxed) - allocations_before);
    }

    // results of the animated run, before the comparison allocates anything
    let heap_peak = HEAP_PEAK.load(Relaxed) - heap_base;
    let largest_allocation = LARGEST_ALLOCATION.load(Relaxed);
    let heap_end = HEAP_USED.load(Relaxed) - heap_base;

    // fresh full render of the final state, NewBuffer clears the renderer caches
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        renderer.render(&mut full_buffer, DISPLAY_WIDTH);
        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
    });

    let mut differing_pixels = 0usize;
    let mut failing_pixels = 0usize;
    let mut largest_difference = 0u8;
    let mut first_failing = None;
    for (index, (partial, full)) in frame_buffer.iter().zip(&full_buffer).enumerate() {
        let difference = channels(*partial)
            .iter()
            .zip(channels(*full))
            .map(|(a, b)| a.abs_diff(b))
            .max()
            .unwrap();
        if difference > 0 {
            differing_pixels += 1;
            largest_difference = largest_difference.max(difference);
        }
        if difference > COLOUR_TOLERANCE {
            failing_pixels += 1;
            first_failing.get_or_insert((index % DISPLAY_WIDTH, index / DISPLAY_WIDTH));
        }
    }

    // report
    let kb = |bytes: usize| bytes as f32 / 1024.0;
    let first_frame = frame_allocations[0];
    let following = &frame_allocations[1..];
    println!("heap after ui setup:        {:6.1} KB", kb(setup_heap));
    println!("heap after last frame:      {:6.1} KB", kb(heap_end));
    println!(
        "heap peak:                  {:6.1} KB (limit {} KB)",
        kb(heap_peak),
        HEAP_LIMIT / 1024
    );
    println!(
        "largest single allocation:  {:6.1} KB (limit {} KB)",
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
        "partial vs. full redraw:    {differing_pixels} pixels differ, largest difference {largest_difference} colour steps (limit {COLOUR_TOLERANCE})"
    );

    let mut failed = false;
    if heap_peak > HEAP_LIMIT {
        println!(
            "FAIL: heap peak {:.1} KB exceeds {} KB",
            kb(heap_peak),
            HEAP_LIMIT / 1024
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
    if let Some((x, y)) = first_failing {
        println!(
            "FAIL: {failing_pixels} pixels differ by more than {COLOUR_TOLERANCE} colour step between partial and full redraw, first at x {x}, y {y}"
        );
        failed = true;
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
