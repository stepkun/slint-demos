### Title

Software renderer: `Path` is redrawn outside the dirty region with `RepaintBufferType::ReusedBuffer`, so anti-aliased edges accumulate

### Bug description

With the software renderer, partial rendering (`RepaintBufferType::ReusedBuffer`) and the `path` feature, a static `Path` is drawn differently from a full repaint whenever something nearby changes. Its anti-aliased pixels get blended onto themselves again on every partial frame. Strokes become thicker and brighter, and fine details look doubled or smeared.

The cause seems to be that `RenderToBuffer` doesn't restrict path drawing to the dirty region:

- Rectangles, textures and gradients go through `self.dirty_region.intersection(&clip)` or `foreach_ranges()`, so they only touch pixels inside the dirty region.
- `RenderToBuffer::process_filled_path` / `process_stroked_path` (in `internal/renderers/software/lib.rs`) pass `clip_geometry` straight to `path::render_filled_path` / `render_stroked_path`. `clip_geometry` is the path geometry intersected with the item clip only.
- So when a path intersects the dirty region, the **whole** path is blended into the buffer again. Pixels outside the dirty region weren't cleared first, so the alpha blending adds up.

Observed with Slint 1.18.1 and with current master (`0a4e236344`, 2026-10-13). The same thing happens on hardware: an RP2040 (Pico W) with an ST7789 display, where it showed up as artifacts around the tick marks of an attitude indicator.

### Reproducible code

`Cargo.toml`:

```toml
[package]
name = "repro"
version = "0.1.0"
edition = "2024"

[dependencies]
slint = { version = "1.18.1", default-features = false, features = ["compat-1-2", "std", "renderer-software"] }
```

`src/main.rs`:

```rust
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    export component Repro inherits Window {
        in property <length> marker-x: 10px;
        background: black;
        // a static stroked path covering the whole window
        Path {
            width: 100%;
            height: 100%;
            viewbox-width: 100;
            viewbox-height: 100;
            stroke: white;
            stroke-width: 3px;
            commands: "M 10 90 L 90 10";
        }
        // a small rectangle moving over the path, causing partial redraws
        Rectangle {
            x: root.marker-x;
            y: 45px;
            width: 10px;
            height: 10px;
            background: red;
        }
    }
}

const W: usize = 100;
const H: usize = 100;

thread_local! {
    static NEXT_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = RefCell::new(None);
}

struct Platform;
impl slint::platform::Platform for Platform {
    fn create_window_adapter(&self) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(NEXT_WINDOW.with(|w| w.borrow().clone()).unwrap())
    }
}

fn render(repaint: RepaintBufferType, steps: &[f32]) -> Vec<Rgb565Pixel> {
    let window = MinimalSoftwareWindow::new(repaint);
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32));
    NEXT_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
    let ui = Repro::new().unwrap();
    ui.show().unwrap();
    let mut buffer = vec![Rgb565Pixel(0); W * H];
    for &x in steps {
        ui.set_marker_x(x);
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, W);
        });
    }
    buffer
}

fn main() {
    slint::platform::set_platform(Box::new(Platform)).unwrap();
    let steps: Vec<f32> = (0..80).map(|i| i as f32).collect();
    // 80 partial frames vs. one full repaint of the final state
    let partial = render(RepaintBufferType::ReusedBuffer, &steps);
    let full = render(RepaintBufferType::NewBuffer, &steps[steps.len() - 1..]);
    let differing = partial.iter().zip(&full).filter(|(a, b)| a != b).count();
    println!("pixels differing: {differing}");
    assert_eq!(differing, 0, "partial rendering differs from a full repaint");
}
```

Output on master:

```
pixels differing: 162
```

The differing pixels are along the anti-aliased edges of the whole line (rows 10 to 89), not just near the moving rectangle. Rendered side by side (left: after partial frames, right: full repaint), the line on the left is visibly thicker and brighter.

### Expected behavior

After any sequence of partial frames, the buffer should match a full repaint of the same state. In practice, path drawing should be limited to `dirty_region ∩ clip`, as it is for the other primitives. One option is to apply the mask through `foreach_ranges()`, or to intersect `clip_geometry` with each dirty-region rectangle before calling `render_filled_path` / `render_stroked_path`.

### Workaround

Use `RepaintBufferType::NewBuffer` (a full repaint every frame).

### Slint version

1.18.1, and master at `0a4e236344`

### Platform/OS

Reproduced on Linux x86_64 (host, `std`). Originally found on an RP2040 (`no_std`, `software-renderer-path`).

### Programming language

Rust

### Backend/renderer

Software renderer (`MinimalSoftwareWindow`, `SoftwareRenderer::render`)
