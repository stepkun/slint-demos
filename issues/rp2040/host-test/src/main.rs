use std::rc::Rc;
use slint::platform::software_renderer::*;
slint::include_modules!();

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static BIGGEST: AtomicUsize = AtomicUsize::new(0);
struct Count;
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let c = CUR.fetch_add(l.size(), Relaxed) + l.size();
        PEAK.fetch_max(c, Relaxed);
        BIGGEST.fetch_max(l.size(), Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { CUR.fetch_sub(l.size(), Relaxed); unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static A: Count = Count;
fn report(what: &str) {
    println!("{what}: current {} KB, peak {} KB, biggest block {} KB",
        CUR.load(Relaxed) / 1024, PEAK.load(Relaxed) / 1024, BIGGEST.load(Relaxed) / 1024);
}

const W: usize = 320;
const H: usize = 240;

struct P(Rc<MinimalSoftwareWindow>);
impl slint::platform::Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> { Ok(self.0.clone()) }
    fn duration_since_start(&self) -> core::time::Duration { core::time::Duration::from_millis(0) }
}

fn save(name: &str, buf: &[Rgb565Pixel]) {
    let mut out = format!("P6 {W} {H} 255\n").into_bytes();
    for p in buf { let v = p.0; out.extend([((v >> 11) << 3) as u8, (((v >> 5) & 0x3f) << 2) as u8, ((v & 0x1f) << 3) as u8]); }
    std::fs::write(name, out).unwrap();
}

struct Lines<'a>(&'a mut [Rgb565Pixel]);
impl LineBufferProvider for Lines<'_> {
    type TargetPixel = Rgb565Pixel;
    fn process_line(&mut self, line: usize, range: core::ops::Range<usize>, f: impl FnOnce(&mut [Rgb565Pixel])) {
        f(&mut self.0[line * W..][range]);
    }
}

fn main() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32));
    slint::platform::set_platform(Box::new(P(window.clone()))).unwrap();
    let mut fb = vec![Rgb565Pixel(0); W * H];
    let base = CUR.load(Relaxed);
    PEAK.store(base, Relaxed);
    BIGGEST.store(0, Relaxed);
    let ui = AppWindow::new().unwrap();
    ui.show().unwrap();
    report("after ui setup");
    let tri = |t: u64, period: u64, amp: f32| { let ph = (t % period) as f32 / period as f32; (if ph < 0.5 { ph * 4.0 - 1.0 } else { 3.0 - ph * 4.0 }) * amp };
    let mut worst_frame = 0usize;
    for frame in 0..2000u64 {
        let t = frame * 33;
        ui.set_roll(tri(t, 8000, 30.0));
        ui.set_pitch(tri(t, 5000, 10.0));
        slint::platform::update_timers_and_animations();
        let before = PEAK.load(Relaxed);
        window.draw_if_needed(|r| { r.render(&mut fb, W); });
        if PEAK.load(Relaxed) > before { worst_frame = frame as usize; }
        if frame == 0 || frame == 1 || frame == 100 || frame == 1999 { report(&format!("after frame {frame}")); }
    }
    save("partial.ppm", &fb);
    // fresh full render of the same state for comparison
    let fresh = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = fresh;
    window.request_redraw();
    let mut full = vec![Rgb565Pixel(0); W * H];
    // force a full repaint by resizing
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32 - 1));
    window.draw_if_needed(|r| { r.render(&mut full, W); });
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32));
    window.draw_if_needed(|r| { r.render(&mut full, W); });
    save("full.ppm", &full);
    let diff_rows: Vec<usize> = (0..H).filter(|y| fb[y * W..(y + 1) * W].iter().zip(&full[y * W..(y + 1) * W]).any(|(a, b)| a.0 != b.0)).collect();
    println!("rows differing between partial and full render: {} {:?}", diff_rows.len(), diff_rows.iter().take(40).collect::<Vec<_>>());
    println!("peak reached in frame {worst_frame}, peak minus frame buffer: {} KB", (PEAK.load(Relaxed) - base) / 1024);
}
